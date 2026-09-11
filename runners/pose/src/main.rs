//! `qualia-pose` estimates where the robot is and publishes that estimate into
//! the shared world model.
//!
//! The estimator is a 2-D lidar pose graph. Each accepted scan is registered
//! against the previous one with a small iterative closest-point (ICP) solve;
//! registered motion accumulates into a pending delta, and once that delta is
//! large enough it becomes a new anchor node in the graph, joined to its
//! predecessor by an odometry constraint. Coming back near an old anchor closes
//! a loop: a heavier constraint joins the two, and the graph is relaxed so the
//! accumulated drift is shared along the trajectory instead of sitting on the
//! newest node.
//!
//! This is deliberately *not* a Kalman or particle filter. There is no
//! covariance and no motion model here, only the graph of registered scans, so
//! the estimator cannot smooth over a bad registration; it can only register
//! the next good one. The single value published outwards is a [`NavPose`] in
//! shared memory.
//!
//! Inputs arrive from the lidar runner through the seqlocked scan slot. The
//! region name and cadence come from the environment:
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
const POLL_MS_FALLBACK: u64 = 20;
const LOG_EVERY_FALLBACK: u64 = 10;

/// Below this many usable returns there is not enough structure to register.
const MIN_SCAN_POINTS: usize = 48;
/// Registration downsamples each cloud toward this size.
const CLOUD_TARGET_POINTS: usize = 160;
/// ICP may never report motion faster than this for a single scan.
const ICP_STEP_LIMIT_M: f32 = 0.35;
const ICP_TURN_LIMIT_RAD: f32 = 20.0 * PI / 180.0;
/// An iteration that moves less than this counts as settled.
const ICP_SETTLE_M: f32 = 0.002;
/// Correspondence search radius within one ICP iteration.
const ICP_ASSOC_RADIUS_M: f32 = 0.35;
/// Registrations whose mean correspondence error exceeds this are discarded.
const ICP_MAX_ERROR_M: f32 = 0.12;
/// Returns closer than this are self-hits and are dropped.
const MIN_RETURN_M: f32 = 0.05;
/// Upper bound on ICP iterations; real scans settle well inside it.
const ICP_ITERATIONS: usize = 8;

/// Pending motion that earns a new anchor once it is this large.
const ANCHOR_SHIFT_M: f32 = 0.20;
const ANCHOR_TURN_RAD: f32 = 8.0 * PI / 180.0;

/// A loop candidate must be this many anchors behind the tip, and within this
/// radius in the current estimate, before it is even considered.
const LOOP_ANCHOR_GAP: usize = 8;
const LOOP_RADIUS_M: f32 = 1.0;
/// A loop is accepted only when its measurement agrees with the graph to
/// within these tolerances.
const LOOP_SHIFT_TOL_M: f32 = 0.45;
const LOOP_TURN_TOL_RAD: f32 = 18.0 * PI / 180.0;
const LOOP_ERROR_TOL_M: f32 = 0.06;

/// Loop constraints pull harder than odometry ones; neither may move anchor 0.
const ODOMETRY_STRENGTH: f32 = 1.0;
const LOOP_STRENGTH: f32 = 2.5;
const RELAX_PASSES: usize = 12;
const RELAX_RATE: f32 = 0.10;
const RELAX_FLOOR: f32 = 0.02;
const RELAX_CEIL: f32 = 0.35;
/// Within one relaxation step the near node moves this fraction of the far one.
const RELAX_NEAR_SHARE: f32 = 0.5;

/// Confidence published before the first registration has succeeded.
const BOOT_CONFIDENCE: f32 = 0.35;
const CONFIDENCE_MIN: f32 = 0.35;
const CONFIDENCE_MAX: f32 = 0.95;
const CONFIDENCE_COUNT_GAIN: f32 = 0.45;
const CONFIDENCE_ERROR_GAIN: f32 = 0.20;

/// A point in whichever frame produced it.
#[derive(Debug, Clone, Copy)]
struct ScanPoint {
    x: f32,
    y: f32,
}

/// A rigid motion: turn by `turn`, then shift by `(shift_x, shift_y)`.
#[derive(Debug, Clone, Copy)]
struct Motion {
    shift_x: f32,
    shift_y: f32,
    turn: f32,
}

impl Motion {
    const IDENTITY: Self = Self {
        shift_x: 0.0,
        shift_y: 0.0,
        turn: 0.0,
    };

    /// This motion applied after `earlier`.
    fn chain(self, earlier: Motion) -> Motion {
        let (sin, cos) = self.turn.sin_cos();
        Motion {
            shift_x: cos * earlier.shift_x - sin * earlier.shift_y + self.shift_x,
            shift_y: sin * earlier.shift_x + cos * earlier.shift_y + self.shift_y,
            turn: wrap_angle(self.turn + earlier.turn),
        }
    }

    /// The motion that undoes this one.
    fn inverse(self) -> Motion {
        let (sin, cos) = self.turn.sin_cos();
        Motion {
            shift_x: -cos * self.shift_x - sin * self.shift_y,
            shift_y: sin * self.shift_x - cos * self.shift_y,
            turn: wrap_angle(-self.turn),
        }
    }
}

/// A pose in the map frame.
#[derive(Debug, Clone, Copy)]
struct MapPose {
    east_m: f32,
    north_m: f32,
    heading_rad: f32,
}

impl MapPose {
    const HOME: Self = Self {
        east_m: 0.0,
        north_m: 0.0,
        heading_rad: 0.0,
    };

    /// Advance this pose by a motion expressed in its own frame.
    fn advance(self, motion: Motion) -> Self {
        let (sin, cos) = self.heading_rad.sin_cos();
        Self {
            east_m: self.east_m + cos * motion.shift_x - sin * motion.shift_y,
            north_m: self.north_m + sin * motion.shift_x + cos * motion.shift_y,
            heading_rad: wrap_angle(self.heading_rad + motion.turn),
        }
    }
}

/// How well a registration fitted.
#[derive(Debug, Clone, Copy)]
struct FitQuality {
    count: usize,
    error_m: f32,
}

/// One registered scan retained as a graph node.
#[derive(Debug, Clone)]
struct Anchor {
    pose: MapPose,
    cloud: Vec<ScanPoint>,
}

/// A measured constraint between two anchors.
#[derive(Debug, Clone, Copy)]
struct Constraint {
    a: usize,
    b: usize,
    measured: Motion,
    strength: f32,
}

/// A loop closure accepted this update, kept for the log.
#[derive(Debug, Clone, Copy)]
struct ClosedLoop {
    a: usize,
    b: usize,
    shift_x: f32,
    shift_y: f32,
    turn: f32,
    strength: f32,
}

/// Everything the main loop needs to publish and report after one update.
struct Tick {
    seq: u64,
    pose: MapPose,
    stamp_ns: u64,
    confidence: f32,
    quality: FitQuality,
    anchors: usize,
    links: usize,
    odometry_links: usize,
    loop_links: usize,
    tick: u64,
    loops: Vec<ClosedLoop>,
}

/// What one scan did to the estimator.
enum Outcome {
    /// Nothing new: duplicate sequence or too few returns.
    Ignore,
    /// The first usable scan fixed the graph origin.
    Bootstrapped {
        pose: MapPose,
        points: usize,
        stamp_ns: u64,
    },
    /// The scan registered and the estimate advanced.
    Tracked(Tick),
    /// The scan could not be registered; the previous estimate stands.
    Lost { seq: u64, points: usize },
}

/// Rolling estimator state. Keeping it out of `main` lets tests drive it with
/// synthetic scans while the loop keeps doing shared-memory I/O.
struct Tracker {
    last_seq: u64,
    previous: Option<Vec<ScanPoint>>,
    pose: MapPose,
    anchors: Vec<Anchor>,
    links: Vec<Constraint>,
    active: usize,
    pending: Motion,
    odometry_links: usize,
    loop_links: usize,
    ticks: u64,
}

impl Tracker {
    fn new() -> Self {
        Self {
            last_seq: 0,
            previous: None,
            pose: MapPose::HOME,
            anchors: Vec::new(),
            links: Vec::new(),
            active: 0,
            pending: Motion::IDENTITY,
            odometry_links: 0,
            loop_links: 0,
            ticks: 0,
        }
    }

    /// Fold one scan into the estimate.
    fn observe(&mut self, scan: &LidarScanSnapshot) -> Outcome {
        if scan.seq == 0 || scan.seq == self.last_seq {
            return Outcome::Ignore;
        }
        self.last_seq = scan.seq;

        let cloud = scan_to_points(scan);
        if cloud.len() < MIN_SCAN_POINTS {
            return Outcome::Ignore;
        }
        let stamp_ns = scan.scan_end_ns.max(scan.scan_start_ns);
        let points = cloud.len();

        let Some(previous) = self.previous.take() else {
            self.anchors.push(Anchor {
                pose: self.pose,
                cloud: cloud.clone(),
            });
            self.active = 0;
            self.previous = Some(cloud);
            return Outcome::Bootstrapped {
                pose: self.pose,
                points,
                stamp_ns,
            };
        };

        let Some((delta, quality)) = register(&previous, &cloud) else {
            self.previous = Some(cloud);
            return Outcome::Lost {
                seq: scan.seq,
                points,
            };
        };

        self.pending = delta.chain(self.pending);
        if self.anchors.is_empty() {
            self.anchors.push(Anchor {
                pose: self.pose,
                cloud: previous,
            });
            self.active = 0;
        }
        self.pose = self.anchors[self.active].pose.advance(self.pending);

        let mut loops = Vec::new();
        if deserves_anchor(self.pending) {
            let fresh = self.anchors.len();
            self.links.push(Constraint {
                a: self.active,
                b: fresh,
                measured: self.pending,
                strength: ODOMETRY_STRENGTH,
            });
            self.odometry_links += 1;
            self.anchors.push(Anchor {
                pose: self.pose,
                cloud: cloud.clone(),
            });

            let found = loop_constraints(&self.anchors, fresh);
            self.loop_links += found.len();
            for link in &found {
                loops.push(ClosedLoop {
                    a: link.a,
                    b: link.b,
                    shift_x: link.measured.shift_x,
                    shift_y: link.measured.shift_y,
                    turn: link.measured.turn,
                    strength: link.strength,
                });
            }
            self.links.extend(found);
            settle_graph(&mut self.anchors, &self.links);

            self.active = fresh;
            self.pose = self.anchors[fresh].pose;
            self.pending = Motion::IDENTITY;
        }

        self.ticks += 1;
        self.previous = Some(cloud);

        Outcome::Tracked(Tick {
            seq: scan.seq,
            pose: self.pose,
            stamp_ns,
            confidence: confidence_of(quality),
            quality,
            anchors: self.anchors.len(),
            links: self.links.len(),
            odometry_links: self.odometry_links,
            loop_links: self.loop_links,
            tick: self.ticks,
            loops,
        })
    }
}

fn main() {
    let region_name =
        std::env::var("QUALIA_SHM_NAME").unwrap_or_else(|_| DEFAULT_SHM_NAME.to_string());
    let pace = Duration::from_millis(setting(
        std::env::var("QUALIA_POSE_POLL_MS").ok(),
        POLL_MS_FALLBACK,
    ));
    let report_every = setting(
        std::env::var("QUALIA_POSE_LOG_EVERY_UPDATES").ok(),
        LOG_EVERY_FALLBACK,
    )
    .max(1);

    let region = match ShmRegion::open(&region_name) {
        Ok(region) => region,
        Err(err) => {
            eprintln!("qualia-pose: failed to open shm '{region_name}': {err}");
            process::exit(1);
        }
    };

    println!("qualia-pose: starting lidar pose graph from shm={region_name}");

    let mut tracker = Tracker::new();
    let mut last_report = Instant::now();

    loop {
        let scan = match region.lidar_scan().snapshot(8) {
            Ok(scan) => scan,
            Err(_) => {
                thread::sleep(pace);
                continue;
            }
        };

        match tracker.observe(&scan) {
            Outcome::Ignore => {}
            Outcome::Bootstrapped {
                pose,
                points,
                stamp_ns,
            } => {
                publish(&region, pose, stamp_ns, BOOT_CONFIDENCE);
                println!("qualia-pose: initialized pose from first scan points={points}");
                last_report = Instant::now();
            }
            Outcome::Tracked(tick) => {
                for closure in &tick.loops {
                    println!(
                        "qualia-pose: loop_closure from={} to={} tx={:.3} ty={:.3} yaw_deg={:.2} weight={:.2}",
                        closure.a,
                        closure.b,
                        closure.shift_x,
                        closure.shift_y,
                        closure.turn.to_degrees(),
                        closure.strength
                    );
                }
                publish(&region, tick.pose, tick.stamp_ns, tick.confidence);
                if tick.tick == 1 || tick.tick % report_every == 0 {
                    println!(
                        "qualia-pose: pose_seq={} x_m={:.3} z_m={:.3} yaw_deg={:.2} score={:.3} matches={} keyframes={} edges={} odom_edges={} loop_edges={}",
                        tick.seq,
                        tick.pose.east_m,
                        tick.pose.north_m,
                        tick.pose.heading_rad.to_degrees(),
                        tick.quality.error_m,
                        tick.quality.count,
                        tick.anchors,
                        tick.links,
                        tick.odometry_links,
                        tick.loop_links
                    );
                    last_report = Instant::now();
                }
            }
            Outcome::Lost { seq, points } => {
                if last_report.elapsed() >= Duration::from_secs(2) {
                    println!("qualia-pose: scan match rejected seq={seq} points={points}");
                    last_report = Instant::now();
                }
            }
        }

        thread::sleep(pace);
    }
}

/// Bring an angle into `(-pi, pi]`.
fn wrap_angle(angle: f32) -> f32 {
    const FULL_TURN: f32 = 2.0 * PI;
    let mut wrapped = angle;
    while wrapped < -PI {
        wrapped += FULL_TURN;
    }
    while wrapped > PI {
        wrapped -= FULL_TURN;
    }
    wrapped
}

/// Rotate then shift every point, returning a new cloud.
fn apply_motion(points: &[ScanPoint], motion: Motion) -> Vec<ScanPoint> {
    let (sin, cos) = motion.turn.sin_cos();
    points
        .iter()
        .map(|point| ScanPoint {
            x: cos * point.x + motion.shift_x - sin * point.y,
            y: sin * point.x + cos * point.y + motion.shift_y,
        })
        .collect()
}

/// The motion `from` must undergo to reach `to`, expressed in `from`'s frame.
fn frame_delta(from: MapPose, to: MapPose) -> Motion {
    let (sin, cos) = from.heading_rad.sin_cos();
    let east = to.east_m - from.east_m;
    let north = to.north_m - from.north_m;
    Motion {
        shift_x: cos * east + sin * north,
        shift_y: cos * north - sin * east,
        turn: wrap_angle(to.heading_rad - from.heading_rad),
    }
}

/// Position blend plus shortest-arc heading blend.
fn blend_poses(from: MapPose, to: MapPose, weight: f32) -> MapPose {
    let weight = weight.clamp(0.0, 1.0);
    let turn = wrap_angle(to.heading_rad - from.heading_rad);
    MapPose {
        east_m: from.east_m + (to.east_m - from.east_m) * weight,
        north_m: from.north_m + (to.north_m - from.north_m) * weight,
        heading_rad: wrap_angle(from.heading_rad + turn * weight),
    }
}

fn separation(a: ScanPoint, b: ScanPoint) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// Turn a polar scan into a downsampled Cartesian cloud, dropping self-hits,
/// non-finite ranges and returns flagged as no-return.
fn scan_to_points(scan: &LidarScanSnapshot) -> Vec<ScanPoint> {
    let usable = (scan.point_count as usize).min(LIDAR_MAX_POINTS);
    let every = (usable / CLOUD_TARGET_POINTS).max(1);
    let mut cloud = Vec::with_capacity(usable / every + 1);
    for beam in scan.points.iter().take(usable).step_by(every) {
        let range = beam.distance_m;
        if beam.intensity == 0 || !range.is_finite() || range <= MIN_RETURN_M {
            continue;
        }
        let (sin, cos) = beam.angle_rad.sin_cos();
        cloud.push(ScanPoint {
            x: cos * range,
            y: sin * range,
        });
    }
    cloud
}

/// For each moving point, its nearest fixed point that is inside `radius`.
fn associate(
    fixed: &[ScanPoint],
    moving: &[ScanPoint],
    radius: f32,
) -> Option<Vec<(ScanPoint, ScanPoint)>> {
    let limit = radius * radius;
    let mut pairs = Vec::new();

    for point in moving {
        let mut chosen: Option<ScanPoint> = None;
        let mut chosen_gap = limit;
        for candidate in fixed {
            let gap = (point.x - candidate.x).powi(2) + (point.y - candidate.y).powi(2);
            if gap < chosen_gap {
                chosen_gap = gap;
                chosen = Some(*candidate);
            }
        }
        if let Some(candidate) = chosen {
            pairs.push((candidate, *point));
        }
    }

    if pairs.is_empty() {
        None
    } else {
        Some(pairs)
    }
}

/// Least-squares rigid step mapping the paired moving points onto their fixed
/// partners.
fn fit_motion(pairs: &[(ScanPoint, ScanPoint)]) -> Option<Motion> {
    if pairs.len() < 3 {
        return None;
    }

    let scale = 1.0 / pairs.len() as f32;
    let fixed_sum = pairs
        .iter()
        .fold((0.0f32, 0.0f32), |acc, (fixed, _)| {
            (acc.0 + fixed.x, acc.1 + fixed.y)
        });
    let moving_sum = pairs
        .iter()
        .fold((0.0f32, 0.0f32), |acc, (_, moving)| {
            (acc.0 + moving.x, acc.1 + moving.y)
        });
    let fixed_mean = (fixed_sum.0 * scale, fixed_sum.1 * scale);
    let moving_mean = (moving_sum.0 * scale, moving_sum.1 * scale);

    let (cross, dot) = pairs
        .iter()
        .fold((0.0f32, 0.0f32), |acc, (fixed, moving)| {
            let fx = fixed.x - fixed_mean.0;
            let fy = fixed.y - fixed_mean.1;
            let mx = moving.x - moving_mean.0;
            let my = moving.y - moving_mean.1;
            (acc.0 + mx * fy - my * fx, acc.1 + mx * fx + my * fy)
        });

    let turn = cross.atan2(dot);
    let (sin, cos) = turn.sin_cos();
    Some(Motion {
        shift_x: fixed_mean.0 - (cos * moving_mean.0 - sin * moving_mean.1),
        shift_y: fixed_mean.1 - (sin * moving_mean.0 + cos * moving_mean.1),
        turn,
    })
}

/// ICP: register `scan` against `reference`, returning the motion that maps
/// scan-frame points back into the reference frame.
fn register(reference: &[ScanPoint], scan: &[ScanPoint]) -> Option<(Motion, FitQuality)> {
    let mut motion = Motion::IDENTITY;

    for _ in 0..ICP_ITERATIONS {
        let moved = apply_motion(scan, motion);
        let pairs = associate(reference, &moved, ICP_ASSOC_RADIUS_M)?;
        if pairs.len() * 2 < MIN_SCAN_POINTS {
            return None;
        }
        let step = fit_motion(&pairs)?;
        motion = step.chain(motion);
        let settled = step.shift_x.abs() < ICP_SETTLE_M
            && step.shift_y.abs() < ICP_SETTLE_M
            && step.turn.abs() < ICP_SETTLE_M;
        if settled {
            break;
        }
    }

    if motion.shift_x.abs() > ICP_STEP_LIMIT_M
        || motion.shift_y.abs() > ICP_STEP_LIMIT_M
        || motion.turn.abs() > ICP_TURN_LIMIT_RAD
    {
        return None;
    }

    let moved = apply_motion(scan, motion);
    let pairs = associate(reference, &moved, ICP_ASSOC_RADIUS_M)?;
    let total: f32 = pairs
        .iter()
        .map(|(fixed, moving)| separation(*fixed, *moving))
        .sum();
    let error_m = total / pairs.len() as f32;
    if error_m > ICP_MAX_ERROR_M {
        return None;
    }

    Some((
        motion,
        FitQuality {
            count: pairs.len(),
            error_m,
        },
    ))
}

fn deserves_anchor(motion: Motion) -> bool {
    motion.shift_x.hypot(motion.shift_y) >= ANCHOR_SHIFT_M
        || motion.turn.abs() >= ANCHOR_TURN_RAD
}

/// Loop candidates for the anchor just added at index `newest`.
fn loop_constraints(anchors: &[Anchor], newest: usize) -> Vec<Constraint> {
    if newest < LOOP_ANCHOR_GAP {
        return Vec::new();
    }

    let tip = &anchors[newest];
    let mut found = Vec::new();
    for older in 0..newest - LOOP_ANCHOR_GAP {
        let candidate = &anchors[older];
        let gap = (candidate.pose.east_m - tip.pose.east_m)
            .hypot(candidate.pose.north_m - tip.pose.north_m);
        if gap > LOOP_RADIUS_M {
            continue;
        }

        let Some((measured, quality)) = register(&candidate.cloud, &tip.cloud) else {
            continue;
        };
        let expected = frame_delta(candidate.pose, tip.pose);
        let drift = (measured.shift_x - expected.shift_x).hypot(measured.shift_y - expected.shift_y);
        let twist = wrap_angle(measured.turn - expected.turn).abs();
        if drift > LOOP_SHIFT_TOL_M || twist > LOOP_TURN_TOL_RAD || quality.error_m > LOOP_ERROR_TOL_M
        {
            continue;
        }

        found.push(Constraint {
            a: older,
            b: newest,
            measured,
            strength: LOOP_STRENGTH,
        });
    }
    found
}

/// Pull anchors toward the constraints in `links`, leaving anchor 0 pinned.
fn settle_graph(anchors: &mut [Anchor], links: &[Constraint]) {
    if anchors.len() < 2 {
        return;
    }

    for _ in 0..RELAX_PASSES {
        for link in links {
            let here = anchors[link.a].pose;
            let there = anchors[link.b].pose;
            let wanted_b = here.advance(link.measured);
            let wanted_a = there.advance(link.measured.inverse());
            let rate = (RELAX_RATE * link.strength).clamp(RELAX_FLOOR, RELAX_CEIL);

            if link.a != 0 {
                anchors[link.a].pose = blend_poses(here, wanted_a, rate * RELAX_NEAR_SHARE);
            }
            if link.b != 0 {
                anchors[link.b].pose = blend_poses(there, wanted_b, rate);
            }
        }
    }
}

fn confidence_of(quality: FitQuality) -> f32 {
    let coverage = (quality.count as f32 / CLOUD_TARGET_POINTS as f32).min(1.0);
    let tightness = (1.0 - quality.error_m / ICP_MAX_ERROR_M).clamp(0.0, 1.0);
    (CONFIDENCE_MIN + CONFIDENCE_COUNT_GAIN * coverage + CONFIDENCE_ERROR_GAIN * tightness)
        .clamp(0.0, CONFIDENCE_MAX)
}

/// Publish a pose into the shared world model.
fn publish(shm: &ShmRegion, pose: MapPose, stamp_ns: u64, confidence: f32) {
    // Literal field order is free; the ABI layout comes from `NavPose` itself.
    shm.set_robot_pose(NavPose {
        confidence,
        timestamp_ns: stamp_ns,
        x_m: pose.east_m,
        y_m: 0.0,
        z_m: pose.north_m,
        yaw_rad: pose.heading_rad,
        pitch_rad: 0.0,
        roll_rad: 0.0,
        _pad0: 0.0,
    });
}

/// Read a decimal setting, falling back when it is absent or malformed.
fn setting(raw: Option<String>, fallback: u64) -> u64 {
    match raw.as_deref().map(str::parse::<u64>) {
        Some(Ok(parsed)) => parsed,
        _ => fallback,
    }
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

    /// Deterministic scattered cloud: dense enough for unambiguous local
    /// registration, with no repeating structure to alias against.
    fn scattered_cloud(count: usize, half_extent: f32) -> Vec<ScanPoint> {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
        };
        (0..count)
            .map(|_| ScanPoint {
                x: next() * half_extent,
                y: next() * half_extent,
            })
            .collect()
    }

    fn scan_from_cloud(seq: u64, cloud: &[ScanPoint]) -> LidarScanSnapshot {
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
        let motion = Motion {
            shift_x: 0.10,
            shift_y: -0.05,
            turn: 1.5 * PI / 180.0,
        };
        let moved = apply_motion(&cloud, motion);

        let (recovered, quality) =
            register(&cloud, &moved).expect("a small rigid motion must register");

        // The recovered motion maps the moved cloud back onto the original.
        let expected = motion.inverse();
        assert_close(recovered.shift_x, expected.shift_x, 0.05);
        assert_close(recovered.shift_y, expected.shift_y, 0.05);
        assert_close(recovered.turn, expected.turn, 1.0 * PI / 180.0);
        assert!(quality.count >= MIN_SCAN_POINTS / 2);
        assert!(quality.error_m <= 0.05);
    }

    #[test]
    fn icp_rejects_disjoint_clouds() {
        let cloud = scattered_cloud(200, 2.0);
        let far_away: Vec<ScanPoint> = cloud
            .iter()
            .map(|p| ScanPoint {
                x: p.x + 20.0,
                y: p.y,
            })
            .collect();
        assert!(register(&cloud, &far_away).is_none());
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
    fn chain_and_inverse_are_mutual_inverses() {
        let motion = Motion {
            shift_x: 0.7,
            shift_y: -1.2,
            turn: 2.4,
        };
        let undone = motion.chain(motion.inverse());
        assert_close(undone.shift_x, 0.0, 1e-4);
        assert_close(undone.shift_y, 0.0, 1e-4);
        assert_close(undone.turn, 0.0, 1e-4);

        let point = ScanPoint { x: 0.3, y: -0.9 };
        let round_trip = apply_motion(&apply_motion(&[point], motion), motion.inverse());
        assert_close(round_trip[0].x, point.x, 1e-4);
        assert_close(round_trip[0].y, point.y, 1e-4);
    }

    #[test]
    fn frame_delta_advances_to_the_target() {
        let from = MapPose {
            east_m: 1.0,
            north_m: 2.0,
            heading_rad: 0.3,
        };
        let to = MapPose {
            east_m: 1.6,
            north_m: 1.7,
            heading_rad: -0.2,
        };
        let reached = from.advance(frame_delta(from, to));
        assert_close(reached.east_m, to.east_m, 1e-4);
        assert_close(reached.north_m, to.north_m, 1e-4);
        assert_close(reached.heading_rad, to.heading_rad, 1e-4);
    }

    #[test]
    fn blend_poses_takes_short_rotation() {
        let a = MapPose {
            east_m: 0.0,
            north_m: 0.0,
            heading_rad: 3.0,
        };
        let b = MapPose {
            east_m: 2.0,
            north_m: 0.0,
            heading_rad: -3.0,
        };
        let mid = blend_poses(a, b, 0.5);
        assert_close(mid.east_m, 1.0, 1e-4);
        // The shortest arc from 3.0 to -3.0 passes through pi, not through 0.
        assert_close(mid.heading_rad.abs(), PI, 1e-3);
    }

    #[test]
    fn scan_to_points_filters_bad_returns_and_downsamples() {
        let mut snapshot = LidarScanSnapshot::default();
        snapshot.point_count = 4;
        snapshot.points[0] = lidar_return(0.0, 1.5, 10);
        snapshot.points[1] = lidar_return(0.0, 0.0, 10); // self-hit
        snapshot.points[2] = lidar_return(1.0, 2.0, 0); // flagged no-return
        snapshot.points[3] = lidar_return(1.0, f32::NAN, 10); // non-finite
        let cloud = scan_to_points(&snapshot);
        assert_eq!(cloud.len(), 1);
        assert_close(cloud[0].x, 1.5, 1e-5);
        assert_close(cloud[0].y, 0.0, 1e-5);

        let mut dense = LidarScanSnapshot::default();
        dense.point_count = 600;
        for (index, slot) in dense.points.iter_mut().enumerate() {
            let angle = index as f32 * 0.01;
            *slot = lidar_return(angle, 1.0 + (index % 7) as f32 * 0.1, 5);
        }
        let sampled = scan_to_points(&dense);
        assert!(!sampled.is_empty());
        assert!(sampled.len() < 600);
        assert!(sampled.len() <= 2 * CLOUD_TARGET_POINTS);
    }

    #[test]
    fn anchor_threshold_fires_on_shift_or_turn() {
        assert!(!deserves_anchor(Motion::IDENTITY));
        assert!(deserves_anchor(Motion {
            shift_x: ANCHOR_SHIFT_M + 0.01,
            shift_y: 0.0,
            turn: 0.0,
        }));
        assert!(deserves_anchor(Motion {
            shift_x: 0.0,
            shift_y: 0.0,
            turn: ANCHOR_TURN_RAD + 0.01,
        }));
    }

    #[test]
    fn loop_constraints_close_a_revisited_anchor() {
        let cloud = scattered_cloud(200, 2.0);
        let mut anchors: Vec<Anchor> = (0..10)
            .map(|index| Anchor {
                pose: MapPose {
                    east_m: index as f32,
                    north_m: 0.0,
                    heading_rad: 0.0,
                },
                cloud: cloud.clone(),
            })
            .collect();
        // The newest anchor sits right back on top of anchor 0.
        anchors[0].pose = MapPose::HOME;
        anchors[9].pose = MapPose {
            east_m: 0.1,
            north_m: 0.0,
            heading_rad: 0.0,
        };

        let found = loop_constraints(&anchors, 9);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].a, 0);
        assert_eq!(found[0].b, 9);
        assert_close(found[0].strength, LOOP_STRENGTH, 1e-6);
    }

    #[test]
    fn loop_constraints_ignore_distant_anchors() {
        let cloud = scattered_cloud(200, 2.0);
        let mut anchors: Vec<Anchor> = (0..10)
            .map(|index| Anchor {
                pose: MapPose {
                    east_m: index as f32,
                    north_m: 0.0,
                    heading_rad: 0.0,
                },
                cloud: cloud.clone(),
            })
            .collect();
        anchors[0].pose = MapPose {
            east_m: 50.0,
            north_m: 0.0,
            heading_rad: 0.0,
        };
        anchors[9].pose = MapPose::HOME;
        assert!(loop_constraints(&anchors, 9).is_empty());

        // Too close to the start of the trajectory to have looped anywhere.
        assert!(loop_constraints(&anchors, 3).is_empty());
    }

    #[test]
    fn settle_graph_pulls_toward_the_constraint() {
        let mut anchors = vec![
            Anchor {
                pose: MapPose {
                    east_m: 0.2,
                    north_m: 0.0,
                    heading_rad: 0.0,
                },
                cloud: Vec::new(),
            },
            Anchor {
                pose: MapPose {
                    east_m: 1.0,
                    north_m: 0.0,
                    heading_rad: 0.0,
                },
                cloud: Vec::new(),
            },
        ];
        let links = vec![Constraint {
            a: 0,
            b: 1,
            measured: Motion::IDENTITY,
            strength: ODOMETRY_STRENGTH,
        }];
        settle_graph(&mut anchors, &links);
        // Anchor 0 is pinned; the far node moves toward the constraint.
        assert_close(anchors[0].pose.east_m, 0.2, 1e-6);
        assert!(anchors[1].pose.east_m < 0.5);
        assert!(anchors[1].pose.east_m > 0.2);
    }

    #[test]
    fn confidence_is_bounded_and_monotonic() {
        let best = confidence_of(FitQuality {
            count: CLOUD_TARGET_POINTS,
            error_m: 0.0,
        });
        let worst = confidence_of(FitQuality {
            count: 0,
            error_m: ICP_MAX_ERROR_M,
        });
        assert!(best <= CONFIDENCE_MAX);
        assert!(worst >= CONFIDENCE_MIN);
        assert!(best > worst);

        let middling = confidence_of(FitQuality {
            count: CLOUD_TARGET_POINTS / 2,
            error_m: ICP_MAX_ERROR_M / 2.0,
        });
        assert!(middling > worst && middling < best);
    }

    #[test]
    fn setting_handles_missing_and_invalid_values() {
        assert_eq!(setting(None, 20), 20);
        assert_eq!(setting(Some("7".to_string()), 20), 7);
        assert_eq!(setting(Some("0".to_string()), 20), 0);
        assert_eq!(setting(Some("nonsense".to_string()), 20), 20);
    }

    #[test]
    fn observe_bootstraps_from_first_scan() {
        let cloud = scattered_cloud(200, 3.0);
        let mut tracker = Tracker::new();
        match tracker.observe(&scan_from_cloud(1, &cloud)) {
            Outcome::Bootstrapped { pose, points, .. } => {
                assert_close(pose.east_m, 0.0, 1e-6);
                assert_close(pose.north_m, 0.0, 1e-6);
                assert!(points >= MIN_SCAN_POINTS);
            }
            _ => panic!("first usable scan must bootstrap the estimate"),
        }
    }

    #[test]
    fn observe_advances_pose_with_sensor_motion() {
        let cloud = scattered_cloud(200, 4.0);
        // The sensor moves +x, so world points land 15 cm further in -x in the
        // new sensor frame.
        let shifted = apply_motion(
            &cloud,
            Motion {
                shift_x: -0.15,
                shift_y: 0.0,
                turn: 0.0,
            },
        );
        let mut tracker = Tracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));

        let tick = match tracker.observe(&scan_from_cloud(2, &shifted)) {
            Outcome::Tracked(tick) => tick,
            _ => panic!("a small consistent motion must register"),
        };
        assert_close(tick.pose.east_m, 0.15, 0.05);
        assert_close(tick.pose.north_m, 0.0, 0.05);
        assert!(tick.quality.count >= MIN_SCAN_POINTS / 2);
        assert!(tick.confidence > CONFIDENCE_MIN);
        assert_eq!(tick.tick, 1);
    }

    #[test]
    fn observe_anchors_after_enough_motion() {
        let cloud = scattered_cloud(200, 4.0);
        let mut tracker = Tracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));

        let mut last = None;
        for step in 1..=3 {
            let shifted = apply_motion(
                &cloud,
                Motion {
                    shift_x: -0.15 * step as f32,
                    shift_y: 0.0,
                    turn: 0.0,
                },
            );
            last = Some(match tracker.observe(&scan_from_cloud(step as u64 + 1, &shifted)) {
                Outcome::Tracked(tick) => tick,
                _ => panic!("motion {step} must register"),
            });
        }

        let tick = last.expect("three updates");
        assert_close(tick.pose.east_m, 0.45, 0.08);
        assert_eq!(tick.anchors, 2);
        assert_eq!(tick.odometry_links, 1);
        assert_eq!(tick.loop_links, 0);
    }

    #[test]
    fn observe_rejects_disjoint_scan_and_keeps_pose() {
        let cloud = scattered_cloud(200, 2.0);
        let unrelated: Vec<ScanPoint> = cloud
            .iter()
            .map(|p| ScanPoint {
                x: p.x + 30.0,
                y: p.y,
            })
            .collect();
        let mut tracker = Tracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));

        match tracker.observe(&scan_from_cloud(2, &unrelated)) {
            Outcome::Lost { seq, points } => {
                assert_eq!(seq, 2);
                assert!(points >= MIN_SCAN_POINTS);
            }
            _ => panic!("an unrelated scan must be rejected"),
        }
    }

    #[test]
    fn observe_ignores_duplicate_and_sparse_scans() {
        let cloud = scattered_cloud(200, 3.0);
        let mut tracker = Tracker::new();
        tracker.observe(&scan_from_cloud(1, &cloud));
        assert!(matches!(
            tracker.observe(&scan_from_cloud(1, &cloud)),
            Outcome::Ignore
        ));

        let mut sparse = LidarScanSnapshot::default();
        sparse.seq = 2;
        sparse.point_count = 4;
        for (index, slot) in sparse.points.iter_mut().take(4).enumerate() {
            *slot = lidar_return(index as f32, 1.0, 10);
        }
        assert!(matches!(tracker.observe(&sparse), Outcome::Ignore));
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
        publish(
            &region,
            MapPose {
                east_m: 1.25,
                north_m: -0.5,
                heading_rad: 0.75,
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
