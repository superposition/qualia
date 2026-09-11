//! `qualia-pose` estimates where the robot is and publishes that estimate into
//! the shared world model.
//!
//! The estimator is a 2-D lidar pose graph. Each accepted scan is registered
//! against the previous one with a small iterative closest-point (ICP) solve;
//! registered motion accumulates into a keyframe delta, and once that delta is
//! large enough the keyframe becomes a node in a pose graph joined by odometry
//! edges. Revisiting a keyframe closes a loop, which adds a higher-weight edge
//! and relaxes the graph so drift is shared across the trajectory.
//!
//! This is deliberately *not* a Kalman or particle filter: it carries no
//! covariance and no motion model, only the graph of registered scans. The
//! only state published externally is [`NavPose`] in shared memory.
//!
//! Inputs come from the lidar runner through the seqlocked scan slot; the
//! configured shared-memory region and poll cadence come from the environment:
//!
//! - `QUALIA_SHM_NAME` (default `/qualia_body`)
//! - `QUALIA_POSE_POLL_MS` (default `20`)
//! - `QUALIA_POSE_LOG_EVERY_UPDATES` (default `10`)

use qualia_shm::ShmRegion;
use qualia_types::{LidarScanSnapshot, NavPose, LIDAR_MAX_POINTS};
use std::f32::consts::PI;
use std::process;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_SHM_NAME: &str = "/qualia_body";
const DEFAULT_POLL_MS: u64 = 20;
const DEFAULT_LOG_EVERY_UPDATES: u64 = 10;

/// A usable scan needs at least this many returns; below it there is not enough
/// structure to register against.
const MIN_POINTS: usize = 48;
/// Scans are downsampled toward this many returns before registration.
const MAX_POINTS_USED: usize = 160;
/// ICP will not be allowed to report motion faster than this per scan.
const MAX_STEP_TRANSLATION_M: f32 = 0.35;
const MAX_STEP_ROTATION_RAD: f32 = 20.0 * PI / 180.0;
/// Registration is considered converged once an iteration moves less than this.
const ICP_CONVERGENCE_M: f32 = 0.002;
/// Correspondence search radius for a single ICP iteration.
const MATCH_RADIUS_M: f32 = 0.35;
/// A registration whose mean correspondence error exceeds this is rejected.
const MAX_MEAN_ERROR_M: f32 = 0.12;
/// Returns closer than this are treated as self-hits and dropped.
const MIN_RETURN_M: f32 = 0.05;
/// ICP iteration cap; registrations converge well before this on real scans.
const ICP_MAX_ITERS: usize = 8;
/// How much registered motion earns a new keyframe.
const KEYFRAME_TRANSLATION_M: f32 = 0.20;
const KEYFRAME_ROTATION_RAD: f32 = 8.0 * PI / 180.0;
/// Loop search: only keyframes at least this far apart in time are candidates,
/// and only those within this radius in the current estimate.
const LOOP_MIN_KEYFRAME_GAP: usize = 8;
const LOOP_SEARCH_RADIUS_M: f32 = 1.0;
/// A loop candidate is accepted only when the measurement and the graph agree
/// to within these bounds.
const LOOP_ACCEPT_TRANSLATION_M: f32 = 0.45;
const LOOP_ACCEPT_ROTATION_RAD: f32 = 18.0 * PI / 180.0;
const LOOP_MAX_MEAN_ERROR_M: f32 = 0.06;
/// Loop edges pull harder than odometry edges, but neither may move the origin.
const ODOM_EDGE_WEIGHT: f32 = 1.0;
const LOOP_EDGE_WEIGHT: f32 = 2.5;
const GRAPH_RELAX_ITERS: usize = 12;
const RELAX_BASE_ALPHA: f32 = 0.10;
const RELAX_MIN_ALPHA: f32 = 0.02;
const RELAX_MAX_ALPHA: f32 = 0.35;
/// Each relaxation step moves the far node fully and the near node half as far.
const RELAX_FROM_SHARE: f32 = 0.5;
/// Confidence published before any registration has succeeded.
const INITIAL_CONFIDENCE: f32 = 0.35;
/// Score-derived confidence floor and ceiling.
const CONFIDENCE_FLOOR: f32 = 0.35;
const CONFIDENCE_CEILING: f32 = 0.95;
const CONFIDENCE_MATCH_WEIGHT: f32 = 0.45;
const CONFIDENCE_ERROR_WEIGHT: f32 = 0.20;

/// A 2-D point in whatever frame it was produced in.
#[derive(Clone, Copy, Debug)]
struct Point2 {
    x: f32,
    y: f32,
}

/// A rigid motion: rotate by `yaw`, then translate by `(tx, ty)`.
#[derive(Clone, Copy, Debug)]
struct Transform2 {
    tx: f32,
    ty: f32,
    yaw: f32,
}

impl Transform2 {
    fn identity() -> Self {
        Self {
            tx: 0.0,
            ty: 0.0,
            yaw: 0.0,
        }
    }
}

/// A pose in the map frame.
#[derive(Clone, Copy, Debug)]
struct Pose2 {
    x: f32,
    y: f32,
    yaw: f32,
}

impl Pose2 {
    fn origin() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            yaw: 0.0,
        }
    }

    /// Apply a delta expressed in this pose's own frame.
    fn compose(self, delta: Transform2) -> Self {
        let (sin, cos) = self.yaw.sin_cos();
        Self {
            x: self.x + cos * delta.tx - sin * delta.ty,
            y: self.y + sin * delta.tx + cos * delta.ty,
            yaw: wrap_angle(self.yaw + delta.yaw),
        }
    }
}

/// Quality of one registration.
#[derive(Clone, Copy, Debug)]
struct MatchScore {
    matches: usize,
    mean_error_m: f32,
}

/// A registered scan retained as a graph node.
#[derive(Clone, Debug)]
struct Keyframe {
    pose: Pose2,
    points: Vec<Point2>,
}

/// A measured constraint between two keyframes.
#[derive(Clone, Copy, Debug)]
struct GraphEdge {
    from: usize,
    to: usize,
    measurement: Transform2,
    weight: f32,
}

/// A loop closure accepted this update, surfaced for logging.
#[derive(Clone, Copy, Debug)]
struct LoopClosure {
    from: usize,
    to: usize,
    tx: f32,
    ty: f32,
    yaw_rad: f32,
    weight: f32,
}

/// Everything the loop needs to publish and log after one update.
struct PoseUpdate {
    seq: u64,
    pose: Pose2,
    timestamp_ns: u64,
    confidence: f32,
    score: MatchScore,
    keyframes: usize,
    edges: usize,
    odom_edges: usize,
    loop_edges: usize,
    update_count: u64,
    loop_closures: Vec<LoopClosure>,
}

/// What one scan did to the estimator.
enum ScanOutcome {
    /// No new information: duplicate sequence or too few returns.
    Skip,
    /// The first usable scan fixed the graph origin.
    Initialized {
        pose: Pose2,
        points: usize,
        timestamp_ns: u64,
    },
    /// The scan registered and the pose estimate advanced.
    Updated(PoseUpdate),
    /// The scan could not be registered; the previous estimate stands.
    Rejected { seq: u64, points: usize },
}

/// The rolling estimator state. Holding it apart from `main` keeps the loop
/// free to do I/O and lets tests drive the estimator with synthetic scans.
struct PoseTracker {
    last_seq: u64,
    previous: Option<Vec<Point2>>,
    pose: Pose2,
    keyframes: Vec<Keyframe>,
    edges: Vec<GraphEdge>,
    active: usize,
    keyframe_delta: Transform2,
    odom_edges: usize,
    loop_edges: usize,
    updates: u64,
}

impl PoseTracker {
    fn new() -> Self {
        Self {
            last_seq: 0,
            previous: None,
            pose: Pose2::origin(),
            keyframes: Vec::new(),
            edges: Vec::new(),
            active: 0,
            keyframe_delta: Transform2::identity(),
            odom_edges: 0,
            loop_edges: 0,
            updates: 0,
        }
    }

    /// Fold one scan into the estimate.
    fn observe(&mut self, scan: &LidarScanSnapshot) -> ScanOutcome {
        if scan.seq == 0 || scan.seq == self.last_seq {
            return ScanOutcome::Skip;
        }
        self.last_seq = scan.seq;

        let cloud = build_cloud(scan);
        if cloud.len() < MIN_POINTS {
            return ScanOutcome::Skip;
        }
        let timestamp_ns = scan.scan_end_ns.max(scan.scan_start_ns);
        let points = cloud.len();

        let Some(previous) = self.previous.take() else {
            self.keyframes.push(Keyframe {
                pose: self.pose,
                points: cloud.clone(),
            });
            self.active = 0;
            self.previous = Some(cloud);
            return ScanOutcome::Initialized {
                pose: self.pose,
                points,
                timestamp_ns,
            };
        };

        let Some((delta, score)) = match_scan_pair(&previous, &cloud) else {
            self.previous = Some(cloud);
            return ScanOutcome::Rejected {
                seq: scan.seq,
                points,
            };
        };

        self.keyframe_delta = chain(self.keyframe_delta, delta);
        if self.keyframes.is_empty() {
            self.keyframes.push(Keyframe {
                pose: self.pose,
                points: previous,
            });
            self.active = 0;
        }
        self.pose = self.keyframes[self.active].pose.compose(self.keyframe_delta);

        let mut loop_closures = Vec::new();
        if should_keyframe(self.keyframe_delta) {
            let new_index = self.keyframes.len();
            self.edges.push(GraphEdge {
                from: self.active,
                to: new_index,
                measurement: self.keyframe_delta,
                weight: ODOM_EDGE_WEIGHT,
            });
            self.odom_edges += 1;
            self.keyframes.push(Keyframe {
                pose: self.pose,
                points: cloud.clone(),
            });

            let accepted = loop_edges(&self.keyframes, new_index);
            self.loop_edges += accepted.len();
            for edge in &accepted {
                loop_closures.push(LoopClosure {
                    from: edge.from,
                    to: edge.to,
                    tx: edge.measurement.tx,
                    ty: edge.measurement.ty,
                    yaw_rad: edge.measurement.yaw,
                    weight: edge.weight,
                });
            }
            self.edges.extend(accepted);
            relax_graph(&mut self.keyframes, &self.edges);

            self.active = new_index;
            self.pose = self.keyframes[new_index].pose;
            self.keyframe_delta = Transform2::identity();
        }

        self.updates += 1;
        self.previous = Some(cloud);

        ScanOutcome::Updated(PoseUpdate {
            seq: scan.seq,
            pose: self.pose,
            timestamp_ns,
            confidence: confidence_from_score(score),
            score,
            keyframes: self.keyframes.len(),
            edges: self.edges.len(),
            odom_edges: self.odom_edges,
            loop_edges: self.loop_edges,
            update_count: self.updates,
            loop_closures,
        })
    }
}

fn main() {
    let shm_name = std::env::var("QUALIA_SHM_NAME").unwrap_or_else(|_| DEFAULT_SHM_NAME.to_string());
    let poll_ms = parse_or(std::env::var("QUALIA_POSE_POLL_MS").ok(), DEFAULT_POLL_MS);
    let log_every =
        parse_or(std::env::var("QUALIA_POSE_LOG_EVERY_UPDATES").ok(), DEFAULT_LOG_EVERY_UPDATES)
            .max(1);

    let shm = match ShmRegion::open(&shm_name) {
        Ok(shm) => shm,
        Err(err) => {
            eprintln!("qualia-pose: failed to open shm '{shm_name}': {err}");
            process::exit(1);
        }
    };

    println!("qualia-pose: starting lidar pose graph from shm={shm_name}");

    let mut tracker = PoseTracker::new();
    let mut last_log = Instant::now();

    loop {
        let Ok(scan) = shm.lidar_scan().snapshot(8) else {
            thread::sleep(Duration::from_millis(poll_ms));
            continue;
        };

        match tracker.observe(&scan) {
            ScanOutcome::Skip => {}
            ScanOutcome::Initialized {
                pose,
                points,
                timestamp_ns,
            } => {
                publish_pose(&shm, pose, timestamp_ns, INITIAL_CONFIDENCE);
                println!("qualia-pose: initialized pose from first scan points={points}");
                last_log = Instant::now();
            }
            ScanOutcome::Updated(update) => {
                for closure in &update.loop_closures {
                    println!(
                        "qualia-pose: loop_closure from={} to={} tx={:.3} ty={:.3} yaw_deg={:.2} weight={:.2}",
                        closure.from,
                        closure.to,
                        closure.tx,
                        closure.ty,
                        closure.yaw_rad.to_degrees(),
                        closure.weight
                    );
                }
                publish_pose(&shm, update.pose, update.timestamp_ns, update.confidence);
                if update.update_count == 1 || update.update_count % log_every == 0 {
                    println!(
                        "qualia-pose: pose_seq={} x_m={:.3} z_m={:.3} yaw_deg={:.2} score={:.3} matches={} keyframes={} edges={} odom_edges={} loop_edges={}",
                        update.seq,
                        update.pose.x,
                        update.pose.y,
                        update.pose.yaw.to_degrees(),
                        update.score.mean_error_m,
                        update.score.matches,
                        update.keyframes,
                        update.edges,
                        update.odom_edges,
                        update.loop_edges
                    );
                    last_log = Instant::now();
                }
            }
            ScanOutcome::Rejected { seq, points } => {
                if last_log.elapsed() >= Duration::from_secs(2) {
                    println!("qualia-pose: scan match rejected seq={seq} points={points}");
                    last_log = Instant::now();
                }
            }
        }

        thread::sleep(Duration::from_millis(poll_ms));
    }
}

/// Fold an angle into `(-pi, pi]`.
fn wrap_angle(angle: f32) -> f32 {
    let mut wrapped = angle;
    while wrapped > PI {
        wrapped -= 2.0 * PI;
    }
    while wrapped < -PI {
        wrapped += 2.0 * PI;
    }
    wrapped
}

/// Apply a rigid transform to every point.
fn apply_transform(points: &[Point2], transform: Transform2) -> Vec<Point2> {
    let (sin, cos) = transform.yaw.sin_cos();
    points
        .iter()
        .map(|point| Point2 {
            x: cos * point.x - sin * point.y + transform.tx,
            y: sin * point.x + cos * point.y + transform.ty,
        })
        .collect()
}

/// Compose two motions: `a` applied after `b`.
fn chain(a: Transform2, b: Transform2) -> Transform2 {
    let (sin, cos) = a.yaw.sin_cos();
    Transform2 {
        tx: cos * b.tx - sin * b.ty + a.tx,
        ty: sin * b.tx + cos * b.ty + a.ty,
        yaw: wrap_angle(a.yaw + b.yaw),
    }
}

/// The motion that undoes `transform`.
fn invert(transform: Transform2) -> Transform2 {
    let (sin, cos) = transform.yaw.sin_cos();
    Transform2 {
        tx: -(cos * transform.tx + sin * transform.ty),
        ty: -(-sin * transform.tx + cos * transform.ty),
        yaw: wrap_angle(-transform.yaw),
    }
}

/// The motion `a` must undergo to reach `b`, expressed in `a`'s frame.
fn pose_delta(a: Pose2, b: Pose2) -> Transform2 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let (sin, cos) = a.yaw.sin_cos();
    Transform2 {
        tx: cos * dx + sin * dy,
        ty: -sin * dx + cos * dy,
        yaw: wrap_angle(b.yaw - a.yaw),
    }
}

/// Linear position blend and shortest-arc yaw blend.
fn interpolate_pose(a: Pose2, b: Pose2, alpha: f32) -> Pose2 {
    let alpha = alpha.clamp(0.0, 1.0);
    Pose2 {
        x: a.x + (b.x - a.x) * alpha,
        y: a.y + (b.y - a.y) * alpha,
        yaw: wrap_angle(a.yaw + wrap_angle(b.yaw - a.yaw) * alpha),
    }
}

fn distance(a: Point2, b: Point2) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

/// Turn a polar scan into a downsampled Cartesian cloud, dropping self-hits,
/// non-finite ranges and returns flagged as no-return.
fn build_cloud(scan: &LidarScanSnapshot) -> Vec<Point2> {
    let count = (scan.point_count as usize).min(LIDAR_MAX_POINTS);
    if count == 0 {
        return Vec::new();
    }

    let stride = (count / MAX_POINTS_USED).max(1);
    let mut cloud = Vec::with_capacity(count / stride + 1);
    for point in scan.points.iter().take(count).step_by(stride) {
        if !point.distance_m.is_finite() || point.distance_m <= MIN_RETURN_M || point.intensity == 0
        {
            continue;
        }
        let (sin, cos) = point.angle_rad.sin_cos();
        cloud.push(Point2 {
            x: cos * point.distance_m,
            y: sin * point.distance_m,
        });
    }
    cloud
}

/// For every query point, its nearest reference point inside `radius`.
fn nearest_pairs(
    reference: &[Point2],
    query: &[Point2],
    radius: f32,
) -> Option<Vec<(Point2, Point2)>> {
    let radius2 = radius * radius;
    let mut pairs = Vec::with_capacity(query.len());

    for &q in query {
        let mut best: Option<Point2> = None;
        let mut best2 = radius2;
        for &r in reference {
            let dx = q.x - r.x;
            let dy = q.y - r.y;
            let d2 = dx * dx + dy * dy;
            if d2 < best2 {
                best2 = d2;
                best = Some(r);
            }
        }
        if let Some(r) = best {
            pairs.push((r, q));
        }
    }

    if pairs.is_empty() {
        None
    } else {
        Some(pairs)
    }
}

/// Least-squares rigid step that best maps the paired query points onto their
/// reference points.
fn rigid_step(pairs: &[(Point2, Point2)]) -> Option<Transform2> {
    if pairs.len() < 3 {
        return None;
    }

    let inv_n = 1.0 / pairs.len() as f32;
    let mut reference_centroid = Point2 { x: 0.0, y: 0.0 };
    let mut query_centroid = Point2 { x: 0.0, y: 0.0 };
    for (reference, query) in pairs {
        reference_centroid.x += reference.x;
        reference_centroid.y += reference.y;
        query_centroid.x += query.x;
        query_centroid.y += query.y;
    }
    reference_centroid.x *= inv_n;
    reference_centroid.y *= inv_n;
    query_centroid.x *= inv_n;
    query_centroid.y *= inv_n;

    let mut s_xx = 0.0f32;
    let mut s_xy = 0.0f32;
    for (reference, query) in pairs {
        let rx = reference.x - reference_centroid.x;
        let ry = reference.y - reference_centroid.y;
        let qx = query.x - query_centroid.x;
        let qy = query.y - query_centroid.y;
        s_xx += qx * rx + qy * ry;
        s_xy += qx * ry - qy * rx;
    }

    let yaw = s_xy.atan2(s_xx);
    let (sin, cos) = yaw.sin_cos();
    Some(Transform2 {
        tx: reference_centroid.x - (cos * query_centroid.x - sin * query_centroid.y),
        ty: reference_centroid.y - (sin * query_centroid.x + cos * query_centroid.y),
        yaw,
    })
}

/// ICP: register `current` against `previous`, returning the motion that maps
/// current-frame points back into the previous frame.
fn match_scan_pair(previous: &[Point2], current: &[Point2]) -> Option<(Transform2, MatchScore)> {
    let mut motion = Transform2::identity();

    for _ in 0..ICP_MAX_ITERS {
        let moved = apply_transform(current, motion);
        let pairs = nearest_pairs(previous, &moved, MATCH_RADIUS_M)?;
        if pairs.len() < MIN_POINTS / 2 {
            return None;
        }
        let step = rigid_step(&pairs)?;
        motion = chain(step, motion);
        if step.tx.abs() < ICP_CONVERGENCE_M
            && step.ty.abs() < ICP_CONVERGENCE_M
            && step.yaw.abs() < ICP_CONVERGENCE_M
        {
            break;
        }
    }

    if motion.tx.abs() > MAX_STEP_TRANSLATION_M
        || motion.ty.abs() > MAX_STEP_TRANSLATION_M
        || motion.yaw.abs() > MAX_STEP_ROTATION_RAD
    {
        return None;
    }

    let moved = apply_transform(current, motion);
    let pairs = nearest_pairs(previous, &moved, MATCH_RADIUS_M)?;
    let mean_error_m =
        pairs.iter().map(|(r, q)| distance(*r, *q)).sum::<f32>() / pairs.len() as f32;
    if mean_error_m > MAX_MEAN_ERROR_M {
        return None;
    }

    Some((
        motion,
        MatchScore {
            matches: pairs.len(),
            mean_error_m,
        },
    ))
}

fn should_keyframe(delta: Transform2) -> bool {
    delta.tx.hypot(delta.ty) >= KEYFRAME_TRANSLATION_M
        || delta.yaw.abs() >= KEYFRAME_ROTATION_RAD
}

/// Loop candidates for the keyframe just added at `new_index`.
fn loop_edges(keyframes: &[Keyframe], new_index: usize) -> Vec<GraphEdge> {
    if new_index < LOOP_MIN_KEYFRAME_GAP {
        return Vec::new();
    }

    let newest = &keyframes[new_index];
    let mut edges = Vec::new();
    for candidate_index in 0..new_index - LOOP_MIN_KEYFRAME_GAP {
        let candidate = &keyframes[candidate_index];
        let dx = candidate.pose.x - newest.pose.x;
        let dy = candidate.pose.y - newest.pose.y;
        if dx.hypot(dy) > LOOP_SEARCH_RADIUS_M {
            continue;
        }

        let Some((measured, score)) = match_scan_pair(&candidate.points, &newest.points) else {
            continue;
        };
        let predicted = pose_delta(candidate.pose, newest.pose);
        let err_t = (measured.tx - predicted.tx).hypot(measured.ty - predicted.ty);
        let err_r = wrap_angle(measured.yaw - predicted.yaw).abs();
        if err_t > LOOP_ACCEPT_TRANSLATION_M
            || err_r > LOOP_ACCEPT_ROTATION_RAD
            || score.mean_error_m > LOOP_MAX_MEAN_ERROR_M
        {
            continue;
        }

        edges.push(GraphEdge {
            from: candidate_index,
            to: new_index,
            measurement: measured,
            weight: LOOP_EDGE_WEIGHT,
        });
    }
    edges
}

/// Iteratively pull keyframes toward the constraints in `edges`, leaving the
/// origin pinned.
fn relax_graph(keyframes: &mut [Keyframe], edges: &[GraphEdge]) {
    if keyframes.len() < 2 {
        return;
    }

    for _ in 0..GRAPH_RELAX_ITERS {
        for edge in edges {
            let from_pose = keyframes[edge.from].pose;
            let to_pose = keyframes[edge.to].pose;
            let desired_to = from_pose.compose(edge.measurement);
            let desired_from = to_pose.compose(invert(edge.measurement));
            let alpha = (RELAX_BASE_ALPHA * edge.weight).clamp(RELAX_MIN_ALPHA, RELAX_MAX_ALPHA);

            if edge.from != 0 {
                keyframes[edge.from].pose =
                    interpolate_pose(from_pose, desired_from, alpha * RELAX_FROM_SHARE);
            }
            if edge.to != 0 {
                keyframes[edge.to].pose = interpolate_pose(to_pose, desired_to, alpha);
            }
        }
    }
}

fn confidence_from_score(score: MatchScore) -> f32 {
    let match_term = (score.matches as f32 / MAX_POINTS_USED as f32).clamp(0.0, 1.0);
    let error_term = (1.0 - score.mean_error_m / MAX_MEAN_ERROR_M).clamp(0.0, 1.0);
    (CONFIDENCE_FLOOR + CONFIDENCE_MATCH_WEIGHT * match_term + CONFIDENCE_ERROR_WEIGHT * error_term)
        .clamp(0.0, CONFIDENCE_CEILING)
}

/// Publish a pose into the shared world model.
fn publish_pose(shm: &ShmRegion, pose: Pose2, timestamp_ns: u64, confidence: f32) {
    shm.set_robot_pose(NavPose {
        x_m: pose.x,
        y_m: 0.0,
        z_m: pose.y,
        yaw_rad: pose.yaw,
        pitch_rad: 0.0,
        roll_rad: 0.0,
        confidence,
        _pad0: 0.0,
        timestamp_ns,
    });
}

/// Parse an optional decimal setting, falling back when it is absent or junk.
fn parse_or(value: Option<String>, default: u64) -> u64 {
    value.and_then(|raw| raw.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_types::LidarPoint;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected} ± {tolerance}, got {actual}"
        );
    }

    /// Deterministic scattered cloud, dense enough for unambiguous nearby
    /// registration but with no repeating structure to alias against.
    fn scattered_cloud(count: usize, half_extent: f32) -> Vec<Point2> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
        };
        (0..count)
            .map(|_| Point2 {
                x: next() * half_extent,
                y: next() * half_extent,
            })
            .collect()
    }

    fn scan_from_cloud(seq: u64, cloud: &[Point2]) -> LidarScanSnapshot {
        let mut snapshot = LidarScanSnapshot::default();
        snapshot.seq = seq;
        snapshot.scan_start_ns = seq * 1_000_000;
        snapshot.scan_end_ns = seq * 1_000_000 + 500_000;
        snapshot.point_count = cloud.len().min(LIDAR_MAX_POINTS) as u32;
        for (slot, point) in snapshot.points.iter_mut().zip(cloud.iter()) {
            slot.angle_rad = point.y.atan2(point.x);
            slot.distance_m = point.x.hypot(point.y);
            slot.intensity = 200;
        }
        snapshot
    }

    fn lidar_return(angle_rad: f32, distance_m: f32, intensity: u8) -> LidarPoint {
        LidarPoint {
            angle_rad,
            distance_m,
            intensity,
            _pad: [0; 3],
        }
    }

    #[test]
    fn icp_recovers_known_translation_and_rotation() {
        let cloud = scattered_cloud(200, 3.0);
        let motion = Transform2 {
            tx: 0.10,
            ty: -0.05,
            yaw: 1.5 * PI / 180.0,
        };
        let moved = apply_transform(&cloud, motion);

        let (recovered, score) =
            match_scan_pair(&cloud, &moved).expect("a small rigid motion must register");

        // The recovered motion maps the moved cloud back onto the original.
        let expected = invert(motion);
        assert_close(recovered.tx, expected.tx, 0.05);
        assert_close(recovered.ty, expected.ty, 0.05);
        assert_close(recovered.yaw, expected.yaw, 1.0 * PI / 180.0);
        assert!(score.matches >= MIN_POINTS / 2);
        assert!(score.mean_error_m <= 0.05);
    }

    #[test]
    fn icp_rejects_disjoint_clouds() {
        let cloud = scattered_cloud(200, 2.0);
        let far_away: Vec<Point2> = cloud
            .iter()
            .map(|p| Point2 {
                x: p.x + 20.0,
                y: p.y,
            })
            .collect();
        assert!(match_scan_pair(&cloud, &far_away).is_none());
    }

    #[test]
    fn angle_wrap_stays_in_pi_range() {
        assert_close(wrap_angle(0.4), 0.4, 1e-6);
        assert_close(wrap_angle(3.0 * PI), PI, 1e-4);
        assert_close(wrap_angle(-3.0 * PI), -PI, 1e-4);
        assert_close(wrap_angle(PI + 0.25), -PI + 0.25, 1e-4);
        assert!(wrap_angle(120.0).abs() <= PI);
    }

    #[test]
    fn chain_and_invert_are_mutual_inverses() {
        let motion = Transform2 {
            tx: 0.7,
            ty: -1.2,
            yaw: 2.4,
        };
        let undone = chain(motion, invert(motion));
        assert_close(undone.tx, 0.0, 1e-4);
        assert_close(undone.ty, 0.0, 1e-4);
        assert_close(undone.yaw, 0.0, 1e-4);

        let point = Point2 { x: 0.3, y: -0.9 };
        let round_trip = apply_transform(&apply_transform(&[point], motion), invert(motion));
        assert_close(round_trip[0].x, point.x, 1e-4);
        assert_close(round_trip[0].y, point.y, 1e-4);
    }

    #[test]
    fn pose_delta_composes_back_to_target() {
        let from = Pose2 {
            x: 1.0,
            y: 2.0,
            yaw: 0.3,
        };
        let to = Pose2 {
            x: 1.6,
            y: 1.7,
            yaw: -0.2,
        };
        let reached = from.compose(pose_delta(from, to));
        assert_close(reached.x, to.x, 1e-4);
        assert_close(reached.y, to.y, 1e-4);
        assert_close(reached.yaw, to.yaw, 1e-4);
    }

    #[test]
    fn interpolate_pose_takes_short_rotation() {
        let a = Pose2 {
            x: 0.0,
            y: 0.0,
            yaw: 3.0,
        };
        let b = Pose2 {
            x: 2.0,
            y: 0.0,
            yaw: -3.0,
        };
        let mid = interpolate_pose(a, b, 0.5);
        assert_close(mid.x, 1.0, 1e-4);
        // The shortest arc from 3.0 to -3.0 passes through pi, not through 0.
        assert_close(mid.yaw.abs(), PI, 1e-3);
    }

    #[test]
    fn build_cloud_filters_bad_returns_and_downsamples() {
        let mut snapshot = LidarScanSnapshot::default();
        snapshot.point_count = 4;
        snapshot.points[0] = lidar_return(0.0, 1.5, 10);
        snapshot.points[1] = lidar_return(0.0, 0.0, 10); // self-hit
        snapshot.points[2] = lidar_return(1.0, 2.0, 0); // flagged no-return
        snapshot.points[3] = lidar_return(1.0, f32::NAN, 10); // non-finite
        let cloud = build_cloud(&snapshot);
        assert_eq!(cloud.len(), 1);
        assert_close(cloud[0].x, 1.5, 1e-5);
        assert_close(cloud[0].y, 0.0, 1e-5);

        let mut dense = LidarScanSnapshot::default();
        dense.point_count = 600;
        for (index, slot) in dense.points.iter_mut().enumerate() {
            let angle = index as f32 * 0.01;
            *slot = lidar_return(angle, 1.0 + (index % 7) as f32 * 0.1, 5);
        }
        let sampled = build_cloud(&dense);
        assert!(!sampled.is_empty());
        assert!(sampled.len() < 600);
        assert!(sampled.len() <= 2 * MAX_POINTS_USED);
    }

    #[test]
    fn keyframe_threshold_fires_on_translation_or_rotation() {
        assert!(!should_keyframe(Transform2::identity()));
        assert!(should_keyframe(Transform2 {
            tx: KEYFRAME_TRANSLATION_M + 0.01,
            ty: 0.0,
            yaw: 0.0,
        }));
        assert!(should_keyframe(Transform2 {
            tx: 0.0,
            ty: 0.0,
            yaw: KEYFRAME_ROTATION_RAD + 0.01,
        }));
    }

    #[test]
    fn loop_edges_close_a_revisited_keyframe() {
        let cloud = scattered_cloud(200, 2.0);
        let mut keyframes: Vec<Keyframe> = (0..10)
            .map(|index| Keyframe {
                pose: Pose2 {
                    x: index as f32,
                    y: 0.0,
                    yaw: 0.0,
                },
                points: cloud.clone(),
            })
            .collect();
        // The newest keyframe sits right back on top of keyframe 0.
        keyframes[0].pose = Pose2::origin();
        keyframes[9].pose = Pose2 {
            x: 0.1,
            y: 0.0,
            yaw: 0.0,
        };

        let edges = loop_edges(&keyframes, 9);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, 0);
        assert_eq!(edges[0].to, 9);
        assert_close(edges[0].weight, LOOP_EDGE_WEIGHT, 1e-6);
    }

    #[test]
    fn loop_edges_ignore_distant_keyframes() {
        let cloud = scattered_cloud(200, 2.0);
        let mut keyframes: Vec<Keyframe> = (0..10)
            .map(|index| Keyframe {
                pose: Pose2 {
                    x: index as f32,
                    y: 0.0,
                    yaw: 0.0,
                },
                points: cloud.clone(),
            })
            .collect();
        keyframes[0].pose = Pose2 {
            x: 50.0,
            y: 0.0,
            yaw: 0.0,
        };
        keyframes[9].pose = Pose2::origin();
        assert!(loop_edges(&keyframes, 9).is_empty());

        // Too close to the start of the trajectory to have looped anywhere.
        assert!(loop_edges(&keyframes, 3).is_empty());
    }

    #[test]
    fn graph_relaxation_pulls_toward_edge_measurement() {
        let mut keyframes = vec![
            Keyframe {
                pose: Pose2 {
                    x: 0.2,
                    y: 0.0,
                    yaw: 0.0,
                },
                points: Vec::new(),
            },
            Keyframe {
                pose: Pose2 {
                    x: 1.0,
                    y: 0.0,
                    yaw: 0.0,
                },
                points: Vec::new(),
            },
        ];
        let edges = vec![GraphEdge {
            from: 0,
            to: 1,
            measurement: Transform2::identity(),
            weight: ODOM_EDGE_WEIGHT,
        }];
        relax_graph(&mut keyframes, &edges);
        // The origin is pinned; the far node moves toward the constraint.
        assert_close(keyframes[0].pose.x, 0.2, 1e-6);
        assert!(keyframes[1].pose.x < 0.5);
        assert!(keyframes[1].pose.x > 0.2);
    }

    #[test]
    fn confidence_is_bounded_and_monotonic() {
        let best = confidence_from_score(MatchScore {
            matches: MAX_POINTS_USED,
            mean_error_m: 0.0,
        });
        let worst = confidence_from_score(MatchScore {
            matches: 0,
            mean_error_m: MAX_MEAN_ERROR_M,
        });
        assert!(best <= CONFIDENCE_CEILING);
        assert!(worst >= CONFIDENCE_FLOOR);
        assert!(best > worst);

        let middling = confidence_from_score(MatchScore {
            matches: MAX_POINTS_USED / 2,
            mean_error_m: MAX_MEAN_ERROR_M / 2.0,
        });
        assert!(middling > worst && middling < best);
    }

    #[test]
    fn parse_or_handles_missing_and_invalid_values() {
        assert_eq!(parse_or(None, 20), 20);
        assert_eq!(parse_or(Some("7".to_string()), 20), 7);
        assert_eq!(parse_or(Some("0".to_string()), 20), 0);
        assert_eq!(parse_or(Some("nonsense".to_string()), 20), 20);
    }

    #[test]
    fn observe_initializes_from_first_scan() {
        let cloud = scattered_cloud(200, 3.0);
        let mut tracker = PoseTracker::new();
        match tracker.observe(&scan_from_cloud(1, &cloud)) {
            ScanOutcome::Initialized { pose, points, .. } => {
                assert_close(pose.x, 0.0, 1e-6);
                assert_close(pose.y, 0.0, 1e-6);
                assert!(points >= MIN_POINTS);
            }
            _ => panic!("first usable scan must initialize the estimate"),
        }
    }

    #[test]
    fn observe_advances_pose_with_sensor_motion() {
        let cloud = scattered_cloud(200, 4.0);
        // The sensor moves +x, so world points land 15 cm further in -x in the
        // new sensor frame.
        let shifted = apply_transform(
            &cloud,
            Transform2 {
                tx: -0.15,
                ty: 0.0,
                yaw: 0.0,
            },
        );
        let mut tracker = PoseTracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));

        let update = match tracker.observe(&scan_from_cloud(2, &shifted)) {
            ScanOutcome::Updated(update) => update,
            _ => panic!("a small consistent motion must register"),
        };
        assert_close(update.pose.x, 0.15, 0.05);
        assert_close(update.pose.y, 0.0, 0.05);
        assert!(update.score.matches >= MIN_POINTS / 2);
        assert!(update.confidence > CONFIDENCE_FLOOR);
        assert_eq!(update.update_count, 1);
    }

    #[test]
    fn observe_builds_keyframe_after_enough_motion() {
        let cloud = scattered_cloud(200, 4.0);
        let mut tracker = PoseTracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));

        let mut last = None;
        for step in 1..=3 {
            let shifted = apply_transform(
                &cloud,
                Transform2 {
                    tx: -0.15 * step as f32,
                    ty: 0.0,
                    yaw: 0.0,
                },
            );
            last = Some(match tracker.observe(&scan_from_cloud(step as u64 + 1, &shifted)) {
                ScanOutcome::Updated(update) => update,
                _ => panic!("motion {step} must register"),
            });
        }

        let update = last.expect("three updates");
        assert_close(update.pose.x, 0.45, 0.08);
        assert_eq!(update.keyframes, 2);
        assert_eq!(update.odom_edges, 1);
        assert_eq!(update.loop_edges, 0);
    }

    #[test]
    fn observe_rejects_disjoint_scan_and_keeps_pose() {
        let cloud = scattered_cloud(200, 2.0);
        let unrelated: Vec<Point2> = cloud
            .iter()
            .map(|p| Point2 {
                x: p.x + 30.0,
                y: p.y,
            })
            .collect();
        let mut tracker = PoseTracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));

        match tracker.observe(&scan_from_cloud(2, &unrelated)) {
            ScanOutcome::Rejected { seq, points } => {
                assert_eq!(seq, 2);
                assert!(points >= MIN_POINTS);
            }
            _ => panic!("an unrelated scan must be rejected"),
        }
    }

    #[test]
    fn observe_skips_duplicate_and_sparse_scans() {
        let cloud = scattered_cloud(200, 3.0);
        let mut tracker = PoseTracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));
        assert!(matches!(
            tracker.observe(&scan_from_cloud(1, &cloud)),
            ScanOutcome::Skip
        ));

        let mut sparse = LidarScanSnapshot::default();
        sparse.seq = 2;
        sparse.point_count = 4;
        for (index, slot) in sparse.points.iter_mut().take(4).enumerate() {
            *slot = lidar_return(index as f32, 1.0, 10);
        }
        assert!(matches!(tracker.observe(&sparse), ScanOutcome::Skip));
    }

    fn scratch_region(tag: &str) -> ShmRegion {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("/qualia_pose_test_{}_{}_{}", std::process::id(), tag, serial);
        ShmRegion::create(&name).expect("create scratch shm region")
    }

    #[test]
    fn publish_writes_navpose_contract() {
        let region = scratch_region("publish");
        publish_pose(
            &region,
            Pose2 {
                x: 1.25,
                y: -0.5,
                yaw: 0.75,
            },
            42_000,
            0.8,
        );

        let pose = region.world_model().robot_pose;
        assert_eq!(pose.x_m, 1.25);
        assert_eq!(pose.y_m, 0.0);
        assert_eq!(pose.z_m, -0.5);
        assert_eq!(pose.yaw_rad, 0.75);
        assert_eq!(pose.pitch_rad, 0.0);
        assert_eq!(pose.roll_rad, 0.0);
        assert_eq!(pose.confidence, 0.8);
        assert_eq!(pose._pad0, 0.0);
        assert_eq!(pose.timestamp_ns, 42_000);
        assert_eq!(region.world_model().nav_seq.load(Ordering::Acquire), 1);
    }
}
