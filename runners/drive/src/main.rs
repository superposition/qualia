//! `qualia-drive`: the closed-loop differential-drive motor runner.
//!
//! This is the only process that turns a world goal into wheel motion. Each
//! tick it maps the shared body region, reads the newest pose and lidar scan,
//! resolves the active [`NavGoal`] into left/right wheel speeds and writes one
//! JSON frame to the serial link. Every tick also records one
//! [`AppliedActionSnapshot`] interval — what was requested, what the safety
//! arbitration allowed and what actually reached the wire — so the ledger can
//! replay the decisions that produced the motion.
//!
//! Safety gates, applied in precedence order: the estop file, a stale pose, a
//! stale lidar scan, an inactive goal, a blocked front arc, a stalled goal and
//! arrival. When the runner is disarmed the serial link is never written and
//! the published interval carries zero applied speed plus
//! [`ACTION_SAFETY_DISARMED`]; when a write fails the interval carries
//! [`ACTION_SAFETY_TRANSPORT_ERROR`] instead. The runner holds zero speed while
//! armed at startup, and it never fails to start because a sensor is quiet.

use qualia_shm::ShmRegion;
use qualia_types::{
    AppliedActionSnapshot, LidarScanSnapshot, NavGoal, NavPose, ACTION_AUTHORITY_QUALIA_DRIVE,
    ACTION_SAFETY_COLLISION_CLAMP, ACTION_SAFETY_DISARMED, ACTION_SAFETY_ESTOP,
    ACTION_SAFETY_GOAL_INACTIVE, ACTION_SAFETY_GOAL_REACHED, ACTION_SAFETY_PROGRESS_STALL,
    ACTION_SAFETY_STALE_LIDAR, ACTION_SAFETY_STALE_POSE, ACTION_SAFETY_TRANSPORT_ERROR,
    LIDAR_MAX_POINTS,
};
use serialport::SerialPort;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_SHM_NAME: &str = "/qualia_body";
const DEFAULT_PORT: &str = "/dev/ttyTHS1";
const DEFAULT_BAUD: u32 = 115_200;
const DEFAULT_TICK_MS: u64 = 100;
const POSE_STALE_NS: u64 = 1_500_000_000;
const LIDAR_STALE_NS: u64 = 1_500_000_000;
const GOAL_REACHED_M: f32 = 0.25;
const MAX_WHEEL_CMD: f32 = 0.34;
const MAX_TURN_CMD: f32 = 0.20;
const HEADING_GAIN: f32 = 0.75;
const FORWARD_GAIN: f32 = 0.45;
const ROTATE_IN_PLACE_RAD: f32 = 0.85;
const FRONT_STOP_DIST_M: f32 = 0.48;
const FRONT_ARC_RAD: f32 = 0.35;
const SIDE_CLEAR_ARC_INNER_RAD: f32 = 0.40;
const SIDE_CLEAR_ARC_OUTER_RAD: f32 = 1.20;
const BLOCKED_TURN_CMD: f32 = 0.16;
const ESTOP_FILE: &str = "/tmp/qualia-estop";
const PROGRESS_TIMEOUT_SECS: u64 = 5;
const MIN_PROGRESS_M: f32 = 0.10;

/// Steering and progress limits, each overridable from the environment so a
/// stack manifest can retune the runner without a rebuild.
#[derive(Clone, Copy)]
struct DriveTuning {
    goal_reached_m: f32,
    max_wheel_cmd: f32,
    max_turn_cmd: f32,
    heading_gain: f32,
    forward_gain: f32,
    rotate_in_place_rad: f32,
    front_stop_dist_m: f32,
    front_arc_rad: f32,
    side_clear_arc_inner_rad: f32,
    side_clear_arc_outer_rad: f32,
    blocked_turn_cmd: f32,
    progress_timeout_secs: u64,
    min_progress_m: f32,
}

impl Default for DriveTuning {
    fn default() -> Self {
        Self {
            goal_reached_m: GOAL_REACHED_M,
            max_wheel_cmd: MAX_WHEEL_CMD,
            max_turn_cmd: MAX_TURN_CMD,
            heading_gain: HEADING_GAIN,
            forward_gain: FORWARD_GAIN,
            rotate_in_place_rad: ROTATE_IN_PLACE_RAD,
            front_stop_dist_m: FRONT_STOP_DIST_M,
            front_arc_rad: FRONT_ARC_RAD,
            side_clear_arc_inner_rad: SIDE_CLEAR_ARC_INNER_RAD,
            side_clear_arc_outer_rad: SIDE_CLEAR_ARC_OUTER_RAD,
            blocked_turn_cmd: BLOCKED_TURN_CMD,
            progress_timeout_secs: PROGRESS_TIMEOUT_SECS,
            min_progress_m: MIN_PROGRESS_M,
        }
    }
}

impl DriveTuning {
    fn from_env() -> Self {
        Self {
            goal_reached_m: env_f32("QUALIA_DRIVE_GOAL_REACHED_M", GOAL_REACHED_M),
            max_wheel_cmd: env_f32("QUALIA_DRIVE_MAX_WHEEL_CMD", MAX_WHEEL_CMD),
            max_turn_cmd: env_f32("QUALIA_DRIVE_MAX_TURN_CMD", MAX_TURN_CMD),
            heading_gain: env_f32("QUALIA_DRIVE_HEADING_GAIN", HEADING_GAIN),
            forward_gain: env_f32("QUALIA_DRIVE_FORWARD_GAIN", FORWARD_GAIN),
            rotate_in_place_rad: env_f32("QUALIA_DRIVE_ROTATE_IN_PLACE_RAD", ROTATE_IN_PLACE_RAD),
            front_stop_dist_m: env_f32("QUALIA_DRIVE_FRONT_STOP_DIST_M", FRONT_STOP_DIST_M),
            front_arc_rad: env_f32("QUALIA_DRIVE_FRONT_ARC_RAD", FRONT_ARC_RAD),
            side_clear_arc_inner_rad: env_f32(
                "QUALIA_DRIVE_SIDE_CLEAR_ARC_INNER_RAD",
                SIDE_CLEAR_ARC_INNER_RAD,
            ),
            side_clear_arc_outer_rad: env_f32(
                "QUALIA_DRIVE_SIDE_CLEAR_ARC_OUTER_RAD",
                SIDE_CLEAR_ARC_OUTER_RAD,
            ),
            blocked_turn_cmd: env_f32("QUALIA_DRIVE_BLOCKED_TURN_CMD", BLOCKED_TURN_CMD),
            progress_timeout_secs: env_u64(
                "QUALIA_DRIVE_PROGRESS_TIMEOUT_SECS",
                PROGRESS_TIMEOUT_SECS,
            ),
            min_progress_m: env_f32("QUALIA_DRIVE_MIN_PROGRESS_M", MIN_PROGRESS_M),
        }
    }
}

/// Everything the runner reads once at startup; the keys are the runner's
/// operational contract.
struct DriveConfig {
    shm_name: String,
    port: String,
    baud: u32,
    tick_ms: u64,
    armed: bool,
    publish_action_telemetry: bool,
    tuning: DriveTuning,
}

impl DriveConfig {
    fn from_env() -> Self {
        Self {
            shm_name: env_string("QUALIA_SHM_NAME", DEFAULT_SHM_NAME),
            port: env_string("QUALIA_DRIVE_PORT", DEFAULT_PORT),
            baud: env_u32("QUALIA_DRIVE_BAUD", DEFAULT_BAUD),
            tick_ms: env_u64("QUALIA_DRIVE_TICK_MS", DEFAULT_TICK_MS),
            armed: std::env::var("QUALIA_DRIVE_ARMED")
                .is_ok_and(|value| parse_armed(&value)),
            publish_action_telemetry: std::env::var("QUALIA_ACTION_TELEMETRY_OWNER")
                .is_ok_and(|value| value == "qualia-drive"),
            tuning: DriveTuning::from_env(),
        }
    }
}

/// A command is only ever written by this runner and only when armed, so the
/// arming spelling is kept deliberately strict rather than treating any
/// non-empty value as consent.
fn parse_armed(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "YES")
}

fn main() {
    let config = DriveConfig::from_env();

    let shm = match ShmRegion::open(&config.shm_name) {
        Ok(shm) => shm,
        Err(error) => {
            eprintln!(
                "qualia-drive: failed to open shm '{}': {error}",
                config.shm_name
            );
            std::process::exit(1);
        }
    };

    let mut port = match serialport::new(&config.port, config.baud)
        .timeout(Duration::from_millis(50))
        .open()
    {
        Ok(port) => port,
        Err(error) => {
            eprintln!(
                "qualia-drive: failed to open serial '{}': {error}",
                config.port
            );
            std::process::exit(2);
        }
    };

    println!(
        "qualia-drive: driving over {} @ {} armed={}",
        config.port, config.baud, config.armed
    );
    if config.armed {
        // Hold zero speed the moment the link is claimed, before any goal.
        let _ = send_speed(&mut *port, 0.0, 0.0);
    }

    let mut progress_distance = f32::INFINITY;
    let mut progress_started = Instant::now();
    let mut last_log = Instant::now();
    let producer_epoch = now_ns();
    let mut action_sequence = 0u64;
    let mut pending_action: Option<AppliedActionSnapshot> = None;

    loop {
        let now = now_ns();

        // Close the previous interval now that its end time is known, and
        // publish it. A publish failure is reported, not fatal: motion has
        // already happened and the next interval is the one that matters.
        if let Some(mut completed) = pending_action.take() {
            completed.interval_end_ns = now;
            if config.publish_action_telemetry {
                if let Err(error) = shm.applied_action().publish(completed) {
                    eprintln!("qualia-drive: applied action publish failed: {error}");
                }
            } else if completed.action_sequence == 1 {
                eprintln!(
                    "qualia-drive: action telemetry disabled; Leash bridge remains authoritative"
                );
            }
        }

        let world = shm.world_model();
        let pose = world.robot_pose;
        let goal = world.nav_goal;

        let pose_fresh =
            pose.timestamp_ns != 0 && now.saturating_sub(pose.timestamp_ns) <= POSE_STALE_NS;
        let lidar = shm.lidar_scan().snapshot(8).ok();
        let lidar_fresh = lidar.as_ref().is_some_and(|scan| {
            scan.seq != 0
                && now.saturating_sub(scan.scan_end_ns.max(scan.scan_start_ns)) <= LIDAR_STALE_NS
        });
        let estop = std::path::Path::new(ESTOP_FILE).exists();
        let clearance = if pose_fresh && lidar_fresh {
            lidar
                .as_ref()
                .and_then(|scan| lidar_clearance(scan, pose, config.tuning))
        } else {
            None
        };
        let obstacle_stop = clearance
            .and_then(|reading| reading.front)
            .is_some_and(|distance| distance <= config.tuning.front_stop_dist_m);

        let goal_distance = goal_distance_m(pose, goal);
        if goal.active != 0 && goal_distance + config.tuning.min_progress_m < progress_distance {
            progress_distance = goal_distance;
            progress_started = Instant::now();
        }
        let progress_stalled = goal.active != 0
            && progress_distance.is_finite()
            && progress_started.elapsed() >= Duration::from_secs(config.tuning.progress_timeout_secs)
            && goal_distance + 0.01 >= progress_distance;

        let command = if goal.active == 0 {
            DriveCommand::WheelSpeeds {
                left: 0.0,
                right: 0.0,
            }
        } else {
            control_to_goal(pose, goal, config.tuning)
        };
        let (requested_left, requested_right) = match command {
            DriveCommand::GoalReached => (0.0, 0.0),
            DriveCommand::WheelSpeeds { left, right } => (left, right),
        };

        let decision = arbitrate(DriveInput {
            estop,
            pose_fresh,
            lidar_fresh,
            goal_active: goal.active != 0,
            obstacle_stop,
            progress_stalled,
            command,
            clearance,
            tuning: config.tuning,
        });
        if decision.clear_goal {
            clear_goal(&shm, goal);
        }
        if decision.reset_progress {
            progress_distance = f32::INFINITY;
            progress_started = Instant::now();
        }

        let transport_ok = transmit(config.armed, &mut *port, decision.left, decision.right);

        action_sequence = action_sequence.wrapping_add(1);
        pending_action = Some(build_action_record(
            decision.state,
            config.armed,
            transport_ok,
            requested_left,
            requested_right,
            decision.left,
            decision.right,
            producer_epoch,
            action_sequence,
            now,
        ));

        if last_log.elapsed() >= Duration::from_secs(1) {
            println!(
                "qualia-drive: state={} left={:.3} right={:.3} pose=({:.2},{:.2},{:.1}deg) goal_active={}",
                decision.state,
                decision.left,
                decision.right,
                pose.x_m,
                pose.z_m,
                pose.yaw_rad.to_degrees(),
                goal.active
            );
            if let Some(reading) = clearance {
                println!(
                    "qualia-drive: clearance front={:.3?} left={:.3?} right={:.3?}",
                    reading.front, reading.left, reading.right
                );
            }
            last_log = Instant::now();
        }

        thread::sleep(Duration::from_millis(config.tick_ms));
    }
}

/// The safety bits published with one interval. The state bit and the
/// arm/transport bits are independent: a disarmed runner still reports why it
/// was idle, and an armed runner that failed to write reports both.
fn action_safety_flags(state: &str, armed: bool, transport_ok: bool) -> u32 {
    let mut flags = match state {
        "estop" => ACTION_SAFETY_ESTOP,
        "stale_pose" => ACTION_SAFETY_STALE_POSE,
        "stale_lidar" => ACTION_SAFETY_STALE_LIDAR,
        "idle" => ACTION_SAFETY_GOAL_INACTIVE,
        "obstacle_turn" => ACTION_SAFETY_COLLISION_CLAMP,
        "progress_stall" => ACTION_SAFETY_PROGRESS_STALL,
        "goal_reached" => ACTION_SAFETY_GOAL_REACHED,
        _ => 0,
    };
    if !armed {
        flags |= ACTION_SAFETY_DISARMED;
    } else if !transport_ok {
        flags |= ACTION_SAFETY_TRANSPORT_ERROR;
    }
    flags
}

/// Assembles the interval record published for one tick. `clamped_*` is the
/// post-arbitration candidate; `applied_*` is zero unless the command was both
/// armed and accepted by the link.
fn build_action_record(
    state: &str,
    armed: bool,
    transport_ok: bool,
    requested_left: f32,
    requested_right: f32,
    clamped_left: f32,
    clamped_right: f32,
    producer_epoch: u64,
    action_sequence: u64,
    interval_start_ns: u64,
) -> AppliedActionSnapshot {
    let accepted = armed && transport_ok;
    AppliedActionSnapshot {
        producer_epoch,
        action_sequence,
        interval_start_ns,
        requested_left,
        requested_right,
        clamped_left,
        clamped_right,
        applied_left: if accepted { clamped_left } else { 0.0 },
        applied_right: if accepted { clamped_right } else { 0.0 },
        speed_scale: 1.0,
        safety_flags: action_safety_flags(state, armed, transport_ok),
        authority: ACTION_AUTHORITY_QUALIA_DRIVE,
        valid: accepted,
        armed,
        collision_clamped: state == "obstacle_turn",
        ..AppliedActionSnapshot::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DriveCommand {
    GoalReached,
    WheelSpeeds { left: f32, right: f32 },
}

/// Pure proportional controller: aim at the goal, ease off the forward speed
/// while the heading error is large, and clamp both wheels.
fn control_to_goal(pose: NavPose, goal: NavGoal, tuning: DriveTuning) -> DriveCommand {
    let dx = goal.x_m - pose.x_m;
    let dz = goal.z_m - pose.z_m;
    let distance = dx.hypot(dz);
    if distance <= tuning.goal_reached_m {
        return DriveCommand::GoalReached;
    }

    let goal_heading = dz.atan2(dx);
    let heading_error = normalize_angle(goal_heading - pose.yaw_rad);
    let turn = (heading_error * tuning.heading_gain).clamp(-tuning.max_turn_cmd, tuning.max_turn_cmd);
    let forward = if heading_error.abs() < tuning.rotate_in_place_rad {
        (distance * tuning.forward_gain).clamp(0.0, tuning.max_wheel_cmd)
    } else {
        0.0
    };

    let left = (forward - turn).clamp(-tuning.max_wheel_cmd, tuning.max_wheel_cmd);
    let right = (forward + turn).clamp(-tuning.max_wheel_cmd, tuning.max_wheel_cmd);
    DriveCommand::WheelSpeeds { left, right }
}

/// Nearest obstacle distance in the front arc and on each flank, in the robot
/// frame. A reading is a candidate only if it carries intensity and is closer
/// than the sensor's own blind spot.
#[derive(Clone, Copy)]
struct LidarClearance {
    front: Option<f32>,
    left: Option<f32>,
    right: Option<f32>,
}

fn lidar_clearance(
    scan: &LidarScanSnapshot,
    pose: NavPose,
    tuning: DriveTuning,
) -> Option<LidarClearance> {
    let point_count = (scan.point_count as usize).min(LIDAR_MAX_POINTS);
    let mut front: Option<f32> = None;
    let mut left: Option<f32> = None;
    let mut right: Option<f32> = None;
    for point in scan.points.iter().take(point_count) {
        if point.distance_m <= 0.05 || point.intensity == 0 {
            continue;
        }
        let rel = normalize_angle(point.angle_rad);
        if rel.abs() <= tuning.front_arc_rad {
            // A point behind the robot must not close the front arc even if a
            // widened arc would otherwise reach it.
            let world_heading = normalize_angle(point.angle_rad + pose.yaw_rad);
            let ahead = normalize_angle(world_heading - pose.yaw_rad).cos();
            if ahead <= 0.0 {
                continue;
            }
            front = Some(front.map_or(point.distance_m, |v| v.min(point.distance_m)));
            continue;
        }
        if (tuning.side_clear_arc_inner_rad..=tuning.side_clear_arc_outer_rad).contains(&rel) {
            left = Some(left.map_or(point.distance_m, |v| v.min(point.distance_m)));
        } else if (-tuning.side_clear_arc_outer_rad..=-tuning.side_clear_arc_inner_rad).contains(&rel)
        {
            right = Some(right.map_or(point.distance_m, |v| v.min(point.distance_m)));
        }
    }
    Some(LidarClearance { front, left, right })
}

/// Rotate toward whichever flank is clearer; with no reading, prefer the left.
fn blocked_turn_command(clearance: Option<LidarClearance>, tuning: DriveTuning) -> f32 {
    match clearance {
        Some(reading) => {
            let left = reading.left.unwrap_or(10.0);
            let right = reading.right.unwrap_or(10.0);
            if left >= right {
                tuning.blocked_turn_cmd
            } else {
                -tuning.blocked_turn_cmd
            }
        }
        None => tuning.blocked_turn_cmd,
    }
}

fn goal_distance_m(pose: NavPose, goal: NavGoal) -> f32 {
    if goal.active == 0 {
        f32::INFINITY
    } else {
        (goal.x_m - pose.x_m).hypot(goal.z_m - pose.z_m)
    }
}

/// One tick's arbitration inputs; keeping them in one value makes the
/// precedence order testable without a live region or link.
#[derive(Clone, Copy)]
struct DriveInput {
    estop: bool,
    pose_fresh: bool,
    lidar_fresh: bool,
    goal_active: bool,
    obstacle_stop: bool,
    progress_stalled: bool,
    command: DriveCommand,
    clearance: Option<LidarClearance>,
    tuning: DriveTuning,
}

struct Decision {
    left: f32,
    right: f32,
    state: &'static str,
    /// The goal should be deactivated this tick.
    clear_goal: bool,
    /// The progress counters should be rearmed this tick.
    reset_progress: bool,
}

fn arbitrate(input: DriveInput) -> Decision {
    let (left, right, state, clear_goal, reset_progress) = if input.estop {
        (0.0, 0.0, "estop", false, false)
    } else if !input.pose_fresh {
        (0.0, 0.0, "stale_pose", false, false)
    } else if !input.lidar_fresh {
        (0.0, 0.0, "stale_lidar", false, false)
    } else if !input.goal_active {
        (0.0, 0.0, "idle", false, true)
    } else if input.obstacle_stop {
        let turn = blocked_turn_command(input.clearance, input.tuning);
        (-turn, turn, "obstacle_turn", false, false)
    } else if input.progress_stalled {
        (0.0, 0.0, "progress_stall", true, true)
    } else {
        match input.command {
            DriveCommand::GoalReached => (0.0, 0.0, "goal_reached", true, true),
            DriveCommand::WheelSpeeds { left, right } => (left, right, "tracking", false, false),
        }
    };
    Decision {
        left,
        right,
        state,
        clear_goal,
        reset_progress,
    }
}

/// The one frame the firmware expects on the wire: newline-terminated JSON
/// with the tick, the left speed and the right speed.
fn speed_line(left: f32, right: f32) -> String {
    let mut line = serde_json::json!({
        "T": 1,
        "L": left,
        "R": right,
    })
    .to_string();
    line.push('\n');
    line
}

/// A byte sink for one speed frame. Implemented for the serial port and, in
/// tests, for an in-memory recorder so the disarm gate is provable without a
/// device.
trait SpeedLink {
    fn send_line(&mut self, line: &str) -> std::io::Result<()>;
}

impl SpeedLink for dyn SerialPort + '_ {
    fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.write_all(line.as_bytes())?;
        self.flush()
    }
}

fn send_speed<L: SpeedLink + ?Sized>(
    link: &mut L,
    left: f32,
    right: f32,
) -> std::io::Result<()> {
    link.send_line(&speed_line(left, right))
}

/// Write one frame when armed, returning whether the transport accepted it.
/// A disarmed runner never touches the link.
fn transmit<L: SpeedLink + ?Sized>(armed: bool, link: &mut L, left: f32, right: f32) -> bool {
    if !armed {
        return false;
    }
    match send_speed(link, left, right) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("qualia-drive: serial write failed: {error}");
            false
        }
    }
}

/// Deactivate the shared goal without disturbing its coordinates, and bump the
/// nav sequence so consumers see the change.
fn clear_goal(shm: &ShmRegion, goal: NavGoal) {
    let world = shm.world_model_mut();
    world.nav_goal = NavGoal { active: 0, ..goal };
    world.nav_seq.fetch_add(1, Ordering::AcqRel);
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn normalize_angle(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= 2.0 * std::f32::consts::PI;
    }
    while angle < -std::f32::consts::PI {
        angle += 2.0 * std::f32::consts::PI;
    }
    angle
}

fn env_string(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_types::LidarPoint;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct LineRecorder {
        lines: Vec<String>,
    }

    impl SpeedLink for LineRecorder {
        fn send_line(&mut self, line: &str) -> std::io::Result<()> {
            self.lines.push(line.to_owned());
            Ok(())
        }
    }

    struct FailingLink;

    impl SpeedLink for FailingLink {
        fn send_line(&mut self, _line: &str) -> std::io::Result<()> {
            Err(std::io::Error::other("mock link down"))
        }
    }

    fn default_tuning() -> DriveTuning {
        DriveTuning::default()
    }

    fn pose_at(x: f32, z: f32, yaw: f32) -> NavPose {
        NavPose {
            x_m: x,
            y_m: 0.0,
            z_m: z,
            yaw_rad: yaw,
            pitch_rad: 0.0,
            roll_rad: 0.0,
            confidence: 1.0,
            _pad0: 0.0,
            timestamp_ns: 1,
        }
    }

    fn active_goal(x: f32, z: f32) -> NavGoal {
        NavGoal {
            active: 1,
            _pad0: [0; 3],
            cell_x: 0,
            cell_z: 0,
            x_m: x,
            y_m: 0.0,
            z_m: z,
            yaw_rad: 0.0,
            timestamp_ns: 1,
        }
    }

    fn scan_with(points: &[(f32, f32, u8)]) -> LidarScanSnapshot {
        let mut scan = LidarScanSnapshot::default();
        scan.point_count = points.len() as u32;
        for (index, (angle_rad, distance_m, intensity)) in points.iter().enumerate() {
            scan.points[index] = LidarPoint {
                angle_rad: *angle_rad,
                distance_m: *distance_m,
                intensity: *intensity,
                _pad: [0; 3],
            };
        }
        scan
    }

    #[test]
    fn armed_flag_accepts_each_documented_spelling() {
        for yes in ["1", "true", "TRUE", "yes", "YES"] {
            assert!(parse_armed(yes), "{yes:?} should arm");
        }
        for no in ["0", "false", "no", "on", "", "2"] {
            assert!(!parse_armed(no), "{no:?} should stay disarmed");
        }
    }

    #[test]
    fn serial_config_keys_are_read_from_the_environment() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let keys = [
            ("QUALIA_SHM_NAME", "/qualia_test"),
            ("QUALIA_DRIVE_PORT", "/dev/ttyTEST0"),
            ("QUALIA_DRIVE_BAUD", "57600"),
            ("QUALIA_DRIVE_TICK_MS", "25"),
            ("QUALIA_DRIVE_ARMED", "yes"),
            ("QUALIA_ACTION_TELEMETRY_OWNER", "qualia-drive"),
            ("QUALIA_DRIVE_MAX_WHEEL_CMD", "0.20"),
        ];
        for (key, value) in keys {
            std::env::set_var(key, value);
        }

        let configured = DriveConfig::from_env();
        assert_eq!(configured.shm_name, "/qualia_test");
        assert_eq!(configured.port, "/dev/ttyTEST0");
        assert_eq!(configured.baud, 57_600);
        assert_eq!(configured.tick_ms, 25);
        assert!(configured.armed);
        assert!(configured.publish_action_telemetry);
        assert_eq!(configured.tuning.max_wheel_cmd, 0.20);

        for (key, _) in keys {
            std::env::remove_var(key);
        }
        let fallback = DriveConfig::from_env();
        assert_eq!(fallback.shm_name, DEFAULT_SHM_NAME);
        assert_eq!(fallback.port, DEFAULT_PORT);
        assert_eq!(fallback.baud, DEFAULT_BAUD);
        assert_eq!(fallback.tick_ms, DEFAULT_TICK_MS);
        assert!(!fallback.armed);
        assert!(!fallback.publish_action_telemetry);
        assert_eq!(fallback.tuning.max_wheel_cmd, MAX_WHEEL_CMD);
    }

    #[test]
    fn serial_line_is_the_documented_json_frame() {
        let line = speed_line(0.25, -0.125);
        assert!(line.ends_with('\n'), "the firmware frame is newline terminated");
        let frame: serde_json::Value =
            serde_json::from_str(line.trim_end()).expect("the frame is valid json");
        assert_eq!(frame.as_object().expect("a json object").len(), 3);
        assert_eq!(frame["T"], 1);
        assert_eq!(frame["L"].as_f64().expect("a number") as f32, 0.25f32);
        assert_eq!(frame["R"].as_f64().expect("a number") as f32, -0.125f32);
    }

    #[test]
    fn disarmed_link_writes_nothing() {
        let mut recorder = LineRecorder::default();
        assert!(!transmit(false, &mut recorder, 0.3, -0.3));
        assert!(recorder.lines.is_empty(), "a disarmed runner must not write");
    }

    #[test]
    fn armed_link_writes_the_speed_frame() {
        let mut recorder = LineRecorder::default();
        assert!(transmit(true, &mut recorder, 0.3, -0.2));
        assert_eq!(recorder.lines.len(), 1, "one frame per tick");
        let frame: serde_json::Value =
            serde_json::from_str(recorder.lines[0].trim_end()).expect("the frame is valid json");
        assert_eq!(frame["T"], 1);
        assert_eq!(frame["L"].as_f64().expect("a number") as f32, 0.3f32);
        assert_eq!(frame["R"].as_f64().expect("a number") as f32, -0.2f32);
    }

    #[test]
    fn failing_link_reports_transport_error_and_applies_nothing() {
        let mut link = FailingLink;
        assert!(!transmit(true, &mut link, 0.3, 0.3));
    }

    #[test]
    fn action_flags_preserve_safety_and_transport_provenance() {
        assert_eq!(
            action_safety_flags("obstacle_turn", false, false),
            ACTION_SAFETY_COLLISION_CLAMP | ACTION_SAFETY_DISARMED
        );
        assert_eq!(
            action_safety_flags("tracking", true, false),
            ACTION_SAFETY_TRANSPORT_ERROR
        );
        assert_eq!(action_safety_flags("tracking", true, true), 0);
        assert_eq!(action_safety_flags("estop", true, true), ACTION_SAFETY_ESTOP);
        assert_eq!(
            action_safety_flags("idle", true, true),
            ACTION_SAFETY_GOAL_INACTIVE
        );
        assert_eq!(
            action_safety_flags("stale_pose", true, true),
            ACTION_SAFETY_STALE_POSE
        );
        assert_eq!(
            action_safety_flags("stale_lidar", true, true),
            ACTION_SAFETY_STALE_LIDAR
        );
        assert_eq!(
            action_safety_flags("progress_stall", true, true),
            ACTION_SAFETY_PROGRESS_STALL
        );
        assert_eq!(
            action_safety_flags("goal_reached", true, true),
            ACTION_SAFETY_GOAL_REACHED
        );
    }

    #[test]
    fn disarmed_action_record_applies_nothing() {
        let record =
            build_action_record("tracking", false, false, 0.3, -0.3, 0.3, -0.3, 7, 9, 100);
        assert_eq!(record.authority, ACTION_AUTHORITY_QUALIA_DRIVE);
        assert!(!record.valid);
        assert!(!record.armed);
        assert_eq!(record.applied_left, 0.0);
        assert_eq!(record.applied_right, 0.0);
        assert_eq!(record.clamped_left, 0.3);
        assert_eq!(record.clamped_right, -0.3);
        assert_ne!(record.safety_flags & ACTION_SAFETY_DISARMED, 0);
        assert_eq!(record.producer_epoch, 7);
        assert_eq!(record.action_sequence, 9);
        assert_eq!(record.interval_start_ns, 100);
        assert_eq!(record.speed_scale, 1.0);
    }

    #[test]
    fn transport_failure_is_recorded_but_not_applied() {
        let record = build_action_record("tracking", true, false, 0.3, -0.3, 0.3, -0.3, 1, 1, 1);
        assert!(!record.valid);
        assert!(record.armed);
        assert_eq!(record.applied_left, 0.0);
        assert_eq!(record.safety_flags, ACTION_SAFETY_TRANSPORT_ERROR);
    }

    #[test]
    fn obstacle_turn_is_clamped_and_flagged() {
        let record =
            build_action_record("obstacle_turn", true, true, 0.3, -0.3, -0.16, 0.16, 1, 2, 3);
        assert!(record.valid);
        assert!(record.collision_clamped);
        assert_eq!(record.applied_left, -0.16);
        assert_eq!(record.applied_right, 0.16);
        assert_eq!(record.safety_flags, ACTION_SAFETY_COLLISION_CLAMP);
    }

    #[test]
    fn goal_distance_is_infinite_when_inactive() {
        let mut goal = active_goal(5.0, 0.0);
        assert_eq!(goal_distance_m(pose_at(0.0, 0.0, 0.0), goal), 5.0);
        goal.active = 0;
        assert!(goal_distance_m(pose_at(0.0, 0.0, 0.0), goal).is_infinite());
    }

    #[test]
    fn control_stops_inside_the_goal_radius() {
        let command =
            control_to_goal(pose_at(0.0, 0.0, 0.0), active_goal(0.10, 0.0), default_tuning());
        assert_eq!(command, DriveCommand::GoalReached);
    }

    #[test]
    fn control_rotates_in_place_when_the_heading_error_is_large() {
        let command =
            control_to_goal(pose_at(0.0, 0.0, 0.0), active_goal(0.0, 2.0), default_tuning());
        assert_eq!(
            command,
            DriveCommand::WheelSpeeds {
                left: -MAX_TURN_CMD,
                right: MAX_TURN_CMD,
            }
        );
    }

    #[test]
    fn control_drives_forward_and_clamps_at_the_wheel_limit() {
        let command =
            control_to_goal(pose_at(0.0, 0.0, 0.0), active_goal(5.0, 0.0), default_tuning());
        assert_eq!(
            command,
            DriveCommand::WheelSpeeds {
                left: MAX_WHEEL_CMD,
                right: MAX_WHEEL_CMD,
            }
        );
    }

    #[test]
    fn normalize_angle_wraps_into_the_principal_range() {
        let pi = std::f32::consts::PI;
        assert!((normalize_angle(0.5) - 0.5).abs() < 1e-6);
        assert!((normalize_angle(3.0 * pi) - pi).abs() < 1e-4);
        assert!((normalize_angle(-3.0 * pi) + pi).abs() < 1e-4);
    }

    #[test]
    fn lidar_clearance_separates_front_and_flanks() {
        let scan = scan_with(&[
            (0.10, 0.50, 5),
            (0.60, 1.00, 5),
            (-0.60, 2.00, 5),
            (0.10, 0.01, 5),
            (0.60, 3.00, 0),
        ]);
        let clearance = lidar_clearance(&scan, pose_at(0.0, 0.0, 0.0), default_tuning())
            .expect("a scan always yields a clearance reading");
        assert_eq!(clearance.front, Some(0.50));
        assert_eq!(clearance.left, Some(1.00));
        assert_eq!(clearance.right, Some(2.00));
    }

    #[test]
    fn blocked_turn_steers_toward_the_clearer_flank() {
        let tuning = default_tuning();
        let left_clear = LidarClearance {
            front: None,
            left: Some(1.20),
            right: Some(0.50),
        };
        let right_clear = LidarClearance {
            front: None,
            left: Some(0.50),
            right: Some(1.20),
        };
        assert_eq!(
            blocked_turn_command(Some(left_clear), tuning),
            BLOCKED_TURN_CMD
        );
        assert_eq!(
            blocked_turn_command(Some(right_clear), tuning),
            -BLOCKED_TURN_CMD
        );
        assert_eq!(blocked_turn_command(None, tuning), BLOCKED_TURN_CMD);
    }

    fn input(tuning: DriveTuning) -> DriveInput {
        DriveInput {
            estop: false,
            pose_fresh: true,
            lidar_fresh: true,
            goal_active: true,
            obstacle_stop: false,
            progress_stalled: false,
            command: DriveCommand::WheelSpeeds {
                left: 0.1,
                right: 0.1,
            },
            clearance: None,
            tuning,
        }
    }

    #[test]
    fn arbitration_lets_estop_override_every_other_condition() {
        let mut state = input(default_tuning());
        state.estop = true;
        state.pose_fresh = false;
        state.command = DriveCommand::WheelSpeeds {
            left: 0.3,
            right: 0.3,
        };
        let decision = arbitrate(state);
        assert_eq!(decision.state, "estop");
        assert_eq!((decision.left, decision.right), (0.0, 0.0));
        assert!(!decision.clear_goal);
    }

    #[test]
    fn arbitration_holds_zero_when_pose_or_lidar_is_stale() {
        let mut state = input(default_tuning());
        state.pose_fresh = false;
        let decision = arbitrate(state);
        assert_eq!(decision.state, "stale_pose");
        assert_eq!((decision.left, decision.right), (0.0, 0.0));

        let mut state = input(default_tuning());
        state.lidar_fresh = false;
        let decision = arbitrate(state);
        assert_eq!(decision.state, "stale_lidar");
        assert_eq!((decision.left, decision.right), (0.0, 0.0));
    }

    #[test]
    fn arbitration_idles_in_place_when_no_goal_is_active() {
        let mut state = input(default_tuning());
        state.goal_active = false;
        let decision = arbitrate(state);
        assert_eq!(decision.state, "idle");
        assert_eq!((decision.left, decision.right), (0.0, 0.0));
        assert!(!decision.clear_goal);
        assert!(decision.reset_progress);
    }

    #[test]
    fn arbitration_spins_away_from_a_blocked_front() {
        let mut state = input(default_tuning());
        state.obstacle_stop = true;
        state.clearance = Some(LidarClearance {
            front: Some(0.30),
            left: Some(1.20),
            right: Some(0.40),
        });
        let decision = arbitrate(state);
        assert_eq!(decision.state, "obstacle_turn");
        assert_eq!(
            (decision.left, decision.right),
            (-BLOCKED_TURN_CMD, BLOCKED_TURN_CMD)
        );
    }

    #[test]
    fn arbitration_requests_a_goal_clear_when_stalled_or_arrived() {
        let mut state = input(default_tuning());
        state.progress_stalled = true;
        let decision = arbitrate(state);
        assert_eq!(decision.state, "progress_stall");
        assert!(decision.clear_goal);
        assert!(decision.reset_progress);
        assert_eq!((decision.left, decision.right), (0.0, 0.0));

        let mut state = input(default_tuning());
        state.command = DriveCommand::GoalReached;
        let decision = arbitrate(state);
        assert_eq!(decision.state, "goal_reached");
        assert!(decision.clear_goal);
    }

    #[test]
    fn arbitration_tracks_the_command_when_every_guard_is_satisfied() {
        let mut state = input(default_tuning());
        state.command = DriveCommand::WheelSpeeds {
            left: 0.21,
            right: 0.05,
        };
        let decision = arbitrate(state);
        assert_eq!(decision.state, "tracking");
        assert_eq!((decision.left, decision.right), (0.21, 0.05));
        assert!(!decision.clear_goal);
        assert!(!decision.reset_progress);
    }

    #[test]
    fn clear_goal_deactivates_and_bumps_the_nav_sequence() {
        let name = format!("/qualia-drive-test-{}", std::process::id());
        let shm = ShmRegion::create(&name).expect("test region");
        let goal = active_goal(1.0, 2.0);
        shm.set_nav_goal(goal);
        let before = shm.world_model().nav_seq.load(Ordering::Acquire);

        clear_goal(&shm, goal);

        let world = shm.world_model();
        assert_eq!(world.nav_goal.active, 0);
        assert_eq!(world.nav_goal.x_m, 1.0);
        assert_eq!(world.nav_seq.load(Ordering::Acquire), before + 1);
    }
}
