//! `qualia-drive`: the closed-loop differential-drive motor runner.
//!
//! This is the only process that turns a world goal into wheel motion. Each
//! tick it maps the shared body region, reads the newest pose and lidar scan,
//! resolves the active [`NavGoal`] into left/right wheel speeds and writes one
//! newline-terminated JSON frame to the serial link. Each tick also records one
//! [`AppliedActionSnapshot`] interval — what was requested, what the safety
//! arbitration allowed and what actually reached the wire — so the ledger can
//! replay the decisions that produced the motion.
//!
//! Safety gates, in precedence order: the estop file, a stale pose, a stale
//! lidar scan, an inactive goal, a blocked front arc, a stalled goal and
//! arrival. A disarmed runner never writes the link; its published interval
//! carries zero applied speed plus [`ACTION_SAFETY_DISARMED`]. A failed write
//! carries [`ACTION_SAFETY_TRANSPORT_ERROR`]. The runner holds zero speed while
//! armed at startup, and it never fails to start because a sensor is quiet.

use qualia_shm::{ShmRegion, StatsWriter};
use qualia_types::{
    AppliedActionSnapshot, LidarScanSnapshot, NavGoal, NavPose, LIDAR_MAX_POINTS,
    ACTION_AUTHORITY_QUALIA_DRIVE, ACTION_SAFETY_COLLISION_CLAMP, ACTION_SAFETY_DISARMED,
    ACTION_SAFETY_ESTOP, ACTION_SAFETY_GOAL_INACTIVE, ACTION_SAFETY_GOAL_REACHED,
    ACTION_SAFETY_PROGRESS_STALL, ACTION_SAFETY_STALE_LIDAR, ACTION_SAFETY_STALE_POSE,
    ACTION_SAFETY_TRANSPORT_ERROR,
};
use serialport::SerialPort;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

// Serial link and shared-region defaults.
const DEFAULT_SHM_NAME: &str = "/qualia_body";
const DEFAULT_PORT: &str = "/dev/ttyTHS1";
/// The name this runner publishes its telemetry frame under: the crate and the
/// stack manifest both call it `qualia-drive`.
const RUNNER_NAME: &str = "qualia-drive";
const DEFAULT_BAUD: u32 = 115_200;
const DEFAULT_TICK_MS: u64 = 100;

// A pose or scan older than this is not trustworthy for motion.
const POSE_STALE_NS: u64 = 1_500_000_000;
const LIDAR_STALE_NS: u64 = 1_500_000_000;

// Control law.
const GOAL_REACHED_M: f32 = 0.25;
const MAX_WHEEL_CMD: f32 = 0.34;
const MAX_TURN_CMD: f32 = 0.20;
const HEADING_GAIN: f32 = 0.75;
const FORWARD_GAIN: f32 = 0.45;
const ROTATE_IN_PLACE_RAD: f32 = 0.85;

// Front-sector stop and flank-turn geometry.
const FRONT_STOP_DIST_M: f32 = 0.48;
const FRONT_ARC_RAD: f32 = 0.35;
const SIDE_CLEAR_ARC_INNER_RAD: f32 = 0.40;
const SIDE_CLEAR_ARC_OUTER_RAD: f32 = 1.20;
const BLOCKED_TURN_CMD: f32 = 0.16;

// Latching stop and stall watchdog.
const ESTOP_FILE: &str = "/tmp/qualia-estop";
const PROGRESS_TIMEOUT_SECS: u64 = 5;
const MIN_PROGRESS_M: f32 = 0.10;

/// How far a point sits from the body centre before the runner trusts it: the
/// sensor reports its own housing as a very close return.
const LIDAR_BLIND_SPOT_M: f32 = 0.05;
/// Flank distance assumed when no reading exists on that side, so the clearer
/// side always wins a comparison.
const OPEN_FLANK_M: f32 = 10.0;
/// Progress is measured against the closest approach so far; this much slack
/// absorbs pose noise before the runner calls it a stall.
const STALL_SLACK_M: f32 = 0.01;
const LIDAR_SNAPSHOT_TRIES: usize = 8;
const SERIAL_TIMEOUT_MS: u64 = 50;
const LOG_PERIOD: Duration = Duration::from_secs(1);

/// Steering and progress limits. Each is overridable from the environment so a
/// stack manifest can retune the runner without a rebuild.
#[derive(Clone, Copy)]
struct DriveTuning {
    /// Distance at which the goal counts as reached.
    goal_reached_m: f32,
    max_wheel_cmd: f32,
    max_turn_cmd: f32,
    heading_gain: f32,
    forward_gain: f32,
    /// Heading error beyond which the runner pivots instead of driving.
    rotate_in_place_rad: f32,
    /// Front-sector stop distance and half-width.
    front_stop_dist_m: f32,
    front_arc_rad: f32,
    /// Flank sector used to choose a turn direction.
    side_clear_arc_inner_rad: f32,
    side_clear_arc_outer_rad: f32,
    blocked_turn_cmd: f32,
    /// Progress watchdog.
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
    /// Overlay every `QUALIA_DRIVE_*` override onto the compiled defaults.
    fn from_env() -> Self {
        let mut tuning = Self::default();
        over_f32("QUALIA_DRIVE_GOAL_REACHED_M", &mut tuning.goal_reached_m);
        over_f32("QUALIA_DRIVE_MAX_WHEEL_CMD", &mut tuning.max_wheel_cmd);
        over_f32("QUALIA_DRIVE_MAX_TURN_CMD", &mut tuning.max_turn_cmd);
        over_f32("QUALIA_DRIVE_HEADING_GAIN", &mut tuning.heading_gain);
        over_f32("QUALIA_DRIVE_FORWARD_GAIN", &mut tuning.forward_gain);
        over_f32(
            "QUALIA_DRIVE_ROTATE_IN_PLACE_RAD",
            &mut tuning.rotate_in_place_rad,
        );
        over_f32("QUALIA_DRIVE_FRONT_STOP_DIST_M", &mut tuning.front_stop_dist_m);
        over_f32("QUALIA_DRIVE_FRONT_ARC_RAD", &mut tuning.front_arc_rad);
        over_f32(
            "QUALIA_DRIVE_SIDE_CLEAR_ARC_INNER_RAD",
            &mut tuning.side_clear_arc_inner_rad,
        );
        over_f32(
            "QUALIA_DRIVE_SIDE_CLEAR_ARC_OUTER_RAD",
            &mut tuning.side_clear_arc_outer_rad,
        );
        over_f32("QUALIA_DRIVE_BLOCKED_TURN_CMD", &mut tuning.blocked_turn_cmd);
        over_f32("QUALIA_DRIVE_MIN_PROGRESS_M", &mut tuning.min_progress_m);
        tuning.progress_timeout_secs = env_u64(
            "QUALIA_DRIVE_PROGRESS_TIMEOUT_SECS",
            tuning.progress_timeout_secs,
        );
        tuning
    }
}

/// Everything the runner reads once at startup; the key names below are the
/// runner's operational contract and must not drift.
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
            armed: parse_armed(&std::env::var("QUALIA_DRIVE_ARMED").unwrap_or_default()),
            publish_action_telemetry: std::env::var("QUALIA_ACTION_TELEMETRY_OWNER")
                .is_ok_and(|owner| owner == "qualia-drive"),
            tuning: DriveTuning::from_env(),
        }
    }
}

/// Arming is deliberately strict: only these spellings are consent, so a
/// typo or an empty value leaves the motors disarmed.
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
        .timeout(Duration::from_millis(SERIAL_TIMEOUT_MS))
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
        // Claim the link at zero speed before any goal can be honoured.
        let _ = send_speed(&mut *port, 0.0, 0.0);
    }

    let mut progress = ProgressGuard::new();
    let mut last_log = Instant::now();
    let producer_epoch = now_ns();
    let mut action_sequence = 0u64;
    let mut pending: Option<AppliedActionSnapshot> = None;
    // The runner's own telemetry frame: ticks/s, the wheel speeds it last wrote
    // and whether the link took them.
    let mut telemetry = StatsWriter::attach(&config.shm_name, RUNNER_NAME);

    loop {
        let now = now_ns();

        // The previous interval's end time only exists now, so it is closed
        // and published first. A publish failure is reported, never fatal.
        if let Some(interval) = pending.take() {
            finish_interval(&shm, &config, interval, now);
        }

        let sensors = read_sensors(&shm, config.tuning, now);
        let stalled = progress.stalled(sensors.pose, sensors.goal, config.tuning);

        let command = if sensors.goal.active == 0 {
            DriveCommand::WheelSpeeds {
                left: 0.0,
                right: 0.0,
            }
        } else {
            control_to_goal(sensors.pose, sensors.goal, config.tuning)
        };
        let (requested_left, requested_right) = match command {
            DriveCommand::GoalReached => (0.0, 0.0),
            DriveCommand::WheelSpeeds { left, right } => (left, right),
        };

        let decision = arbitrate(DriveInput {
            estop: sensors.estop,
            pose_fresh: sensors.pose_fresh,
            lidar_fresh: sensors.lidar_fresh,
            goal_active: sensors.goal.active != 0,
            obstacle_stop: sensors.obstacle_stop,
            progress_stalled: stalled,
            command,
            clearance: sensors.clearance,
            tuning: config.tuning,
        });
        if decision.clear_goal {
            clear_goal(&shm, sensors.goal);
        }
        if decision.reset_progress {
            progress.rearm();
        }

        let transport_ok = transmit(config.armed, &mut *port, decision.left, decision.right);

        action_sequence = action_sequence.wrapping_add(1);
        pending = Some(build_action_record(
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

        if last_log.elapsed() >= LOG_PERIOD {
            log_tick(&decision, &sensors);
            last_log = Instant::now();
        }

        if let Some(telemetry) = telemetry.as_mut() {
            telemetry.tick();
            telemetry.set_value(0, "left", decision.left);
            telemetry.set_value(1, "right", decision.right);
            telemetry.set_value(2, "goal active", if sensors.goal.active != 0 { 1.0 } else { 0.0 });
            telemetry.set_value(3, "link ok", if transport_ok { 1.0 } else { 0.0 });
            if !transport_ok {
                telemetry.record_error();
            }
        }

        thread::sleep(Duration::from_millis(config.tick_ms));
    }
}

/// One tick's coherent sensor view.
struct Sensors {
    pose: NavPose,
    goal: NavGoal,
    pose_fresh: bool,
    lidar_fresh: bool,
    estop: bool,
    clearance: Option<LidarClearance>,
    obstacle_stop: bool,
}

fn read_sensors(shm: &ShmRegion, tuning: DriveTuning, now: u64) -> Sensors {
    let world = shm.world_model();
    let pose = world.robot_pose;
    let goal = world.nav_goal;

    let pose_fresh =
        pose.timestamp_ns != 0 && now.saturating_sub(pose.timestamp_ns) <= POSE_STALE_NS;
    let scan = shm.lidar_scan().snapshot(LIDAR_SNAPSHOT_TRIES).ok();
    let lidar_fresh = scan.as_ref().is_some_and(|scan| {
        scan.seq != 0
            && now.saturating_sub(scan.scan_end_ns.max(scan.scan_start_ns)) <= LIDAR_STALE_NS
    });

    let clearance = if pose_fresh && lidar_fresh {
        scan.as_ref()
            .and_then(|scan| lidar_clearance(scan, pose, tuning))
    } else {
        None
    };
    let obstacle_stop = clearance
        .and_then(|reading| reading.front)
        .is_some_and(|distance| distance <= tuning.front_stop_dist_m);

    Sensors {
        pose,
        goal,
        pose_fresh,
        lidar_fresh,
        estop: std::path::Path::new(ESTOP_FILE).exists(),
        clearance,
        obstacle_stop,
    }
}

/// Tracks the closest approach to the goal and how long ago it improved. The
/// runner is stalled when the goal has not been approached within the timeout.
struct ProgressGuard {
    closest_m: f32,
    improved_at: Instant,
}

impl ProgressGuard {
    fn new() -> Self {
        Self {
            closest_m: f32::INFINITY,
            improved_at: Instant::now(),
        }
    }

    fn rearm(&mut self) {
        self.closest_m = f32::INFINITY;
        self.improved_at = Instant::now();
    }

    fn stalled(&mut self, pose: NavPose, goal: NavGoal, tuning: DriveTuning) -> bool {
        if goal.active == 0 {
            return false;
        }
        let range = goal_distance_m(pose, goal);
        if range + tuning.min_progress_m < self.closest_m {
            self.closest_m = range;
            self.improved_at = Instant::now();
        }
        self.closest_m.is_finite()
            && self.improved_at.elapsed() >= Duration::from_secs(tuning.progress_timeout_secs)
            && range + STALL_SLACK_M >= self.closest_m
    }
}

/// Stamp and publish one completed interval. Without telemetry ownership the
/// runner still says once that the Leash bridge is the record instead.
fn finish_interval(
    shm: &ShmRegion,
    config: &DriveConfig,
    mut interval: AppliedActionSnapshot,
    now: u64,
) {
    interval.interval_end_ns = now;
    if config.publish_action_telemetry {
        if let Err(error) = shm.applied_action().publish(interval) {
            eprintln!("qualia-drive: applied action publish failed: {error}");
        }
    } else if interval.action_sequence == 1 {
        eprintln!("qualia-drive: action telemetry disabled; Leash bridge remains authoritative");
    }
}

fn log_tick(decision: &Decision, sensors: &Sensors) {
    println!(
        "qualia-drive: state={} left={:.3} right={:.3} pose=({:.2},{:.2},{:.1}deg) goal_active={}",
        decision.state,
        decision.left,
        decision.right,
        sensors.pose.x_m,
        sensors.pose.z_m,
        sensors.pose.yaw_rad.to_degrees(),
        sensors.goal.active
    );
    if let Some(reading) = sensors.clearance {
        println!(
            "qualia-drive: clearance front={:.3?} left={:.3?} right={:.3?}",
            reading.front, reading.left, reading.right
        );
    }
}

/// The safety bits published with one interval. The navigation-state bit and
/// the arm/link bits are independent: a disarmed runner still reports why it
/// sat still, and an armed runner whose write failed reports both.
fn action_safety_flags(state: &str, armed: bool, transport_ok: bool) -> u32 {
    let reason = match state {
        "estop" => ACTION_SAFETY_ESTOP,
        "stale_pose" => ACTION_SAFETY_STALE_POSE,
        "stale_lidar" => ACTION_SAFETY_STALE_LIDAR,
        // Standing still with no goal is expected, but it is still why.
        "idle" => ACTION_SAFETY_GOAL_INACTIVE,
        "obstacle_turn" => ACTION_SAFETY_COLLISION_CLAMP,
        "progress_stall" => ACTION_SAFETY_PROGRESS_STALL,
        "goal_reached" => ACTION_SAFETY_GOAL_REACHED,
        _ => 0,
    };
    match (armed, transport_ok) {
        (false, _) => reason | ACTION_SAFETY_DISARMED,
        (true, false) => reason | ACTION_SAFETY_TRANSPORT_ERROR,
        (true, true) => reason,
    }
}

/// Assembles the interval published for one tick. `clamped_*` is the
/// post-arbitration candidate; `applied_*` stays zero unless the command was
/// both armed and accepted by the link.
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
    let offset_x = goal.x_m - pose.x_m;
    let offset_z = goal.z_m - pose.z_m;
    let range = offset_x.hypot(offset_z);
    if range <= tuning.goal_reached_m {
        return DriveCommand::GoalReached;
    }

    let bearing = normalize_angle(offset_z.atan2(offset_x) - pose.yaw_rad);
    let yaw_cmd = (bearing * tuning.heading_gain).clamp(-tuning.max_turn_cmd, tuning.max_turn_cmd);
    let drive_cmd = if bearing.abs() < tuning.rotate_in_place_rad {
        (range * tuning.forward_gain).clamp(0.0, tuning.max_wheel_cmd)
    } else {
        0.0
    };

    DriveCommand::WheelSpeeds {
        left: (drive_cmd - yaw_cmd).clamp(-tuning.max_wheel_cmd, tuning.max_wheel_cmd),
        right: (drive_cmd + yaw_cmd).clamp(-tuning.max_wheel_cmd, tuning.max_wheel_cmd),
    }
}

/// Nearest usable obstacle in the front sector and on each flank, in the robot
/// frame.
#[derive(Clone, Copy)]
struct LidarClearance {
    /// Nearest return inside the front sector.
    front: Option<f32>,
    /// Nearest return on the left flank.
    left: Option<f32>,
    /// Nearest return on the right flank.
    right: Option<f32>,
}

fn lidar_clearance(
    scan: &LidarScanSnapshot,
    pose: NavPose,
    tuning: DriveTuning,
) -> Option<LidarClearance> {
    let usable = (scan.point_count as usize).min(LIDAR_MAX_POINTS);
    let mut reading = LidarClearance {
        front: None,
        left: None,
        right: None,
    };
    for point in scan.points[..usable].iter() {
        if point.distance_m <= LIDAR_BLIND_SPOT_M || point.intensity == 0 {
            continue;
        }
        let bearing = normalize_angle(point.angle_rad);
        if bearing.abs() <= tuning.front_arc_rad {
            if !ahead_of(pose, point.angle_rad) {
                continue;
            }
            keep_nearest(&mut reading.front, point.distance_m);
        } else if in_sector(bearing, tuning.side_clear_arc_inner_rad, tuning.side_clear_arc_outer_rad)
        {
            keep_nearest(&mut reading.left, point.distance_m);
        } else if in_sector(-bearing, tuning.side_clear_arc_inner_rad, tuning.side_clear_arc_outer_rad)
        {
            keep_nearest(&mut reading.right, point.distance_m);
        }
    }
    Some(reading)
}

/// A widened front sector must not swallow returns from behind the robot.
fn ahead_of(pose: NavPose, bearing_rad: f32) -> bool {
    let world_bearing = normalize_angle(bearing_rad + pose.yaw_rad);
    normalize_angle(world_bearing - pose.yaw_rad).cos() > 0.0
}

fn in_sector(bearing_rad: f32, inner: f32, outer: f32) -> bool {
    bearing_rad >= inner && bearing_rad <= outer
}

fn keep_nearest(slot: &mut Option<f32>, distance_m: f32) {
    if slot.map_or(true, |current| distance_m < current) {
        *slot = Some(distance_m);
    }
}

/// Rotate toward whichever flank is clearer; with no reading at all, prefer
/// the left.
fn blocked_turn_command(clearance: Option<LidarClearance>, tuning: DriveTuning) -> f32 {
    let Some(LidarClearance { left, right, .. }) = clearance else {
        return tuning.blocked_turn_cmd;
    };
    let left = left.unwrap_or(OPEN_FLANK_M);
    let right = right.unwrap_or(OPEN_FLANK_M);
    if left >= right {
        tuning.blocked_turn_cmd
    } else {
        -tuning.blocked_turn_cmd
    }
}

fn goal_distance_m(pose: NavPose, goal: NavGoal) -> f32 {
    if goal.active == 0 {
        return f32::INFINITY;
    }
    let offset_x = goal.x_m - pose.x_m;
    let offset_z = goal.z_m - pose.z_m;
    offset_x.hypot(offset_z)
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
    let (state, left, right, clear_goal, reset_progress) = if input.estop {
        ("estop", 0.0, 0.0, false, false)
    } else if !input.pose_fresh {
        ("stale_pose", 0.0, 0.0, false, false)
    } else if !input.lidar_fresh {
        ("stale_lidar", 0.0, 0.0, false, false)
    } else if !input.goal_active {
        ("idle", 0.0, 0.0, false, true)
    } else if input.obstacle_stop {
        let turn = blocked_turn_command(input.clearance, input.tuning);
        ("obstacle_turn", -turn, turn, false, false)
    } else if input.progress_stalled {
        ("progress_stall", 0.0, 0.0, true, true)
    } else {
        match input.command {
            DriveCommand::GoalReached => ("goal_reached", 0.0, 0.0, true, true),
            DriveCommand::WheelSpeeds { left, right } => ("tracking", left, right, false, false),
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
/// carrying the tick, the left speed and the right speed.
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

fn send_speed<L: SpeedLink + ?Sized>(link: &mut L, left: f32, right: f32) -> std::io::Result<()> {
    link.send_line(&speed_line(left, right))
}

/// Write one frame only while armed, returning whether the transport took it.
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
/// nav sequence so consumers observe the change.
fn clear_goal(shm: &ShmRegion, goal: NavGoal) {
    let world = shm.world_model_mut();
    let deactivated = NavGoal { active: 0, ..goal };
    world.nav_goal = deactivated;
    world.nav_seq.fetch_add(1, Ordering::AcqRel);
}

fn now_ns() -> u64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    since_epoch.as_nanos() as u64
}

fn normalize_angle(mut angle: f32) -> f32 {
    const FULL_TURN: f32 = std::f32::consts::TAU;
    while angle > std::f32::consts::PI {
        angle -= FULL_TURN;
    }
    while angle < -std::f32::consts::PI {
        angle += FULL_TURN;
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

/// Overlay an f32 override in place, keeping the compiled default on a parse
/// failure or an unset key.
fn over_f32(name: &str, slot: &mut f32) {
    *slot = env_f32(name, *slot);
    let _ = slot;
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
    fn safety_bits_track_state_and_link_health() {
        let cases = [
            ("estop", ACTION_SAFETY_ESTOP),
            ("stale_pose", ACTION_SAFETY_STALE_POSE),
            ("stale_lidar", ACTION_SAFETY_STALE_LIDAR),
            ("idle", ACTION_SAFETY_GOAL_INACTIVE),
            ("obstacle_turn", ACTION_SAFETY_COLLISION_CLAMP),
            ("progress_stall", ACTION_SAFETY_PROGRESS_STALL),
            ("goal_reached", ACTION_SAFETY_GOAL_REACHED),
            ("tracking", 0),
        ];
        for (state, expected) in cases {
            assert_eq!(action_safety_flags(state, true, true), expected, "{state}");
        }

        let disarmed = action_safety_flags("idle", false, true);
        assert_ne!(disarmed & ACTION_SAFETY_DISARMED, 0);
        assert_ne!(disarmed & ACTION_SAFETY_GOAL_INACTIVE, 0, "still reports idle");
        assert_eq!(
            action_safety_flags("tracking", true, false),
            ACTION_SAFETY_TRANSPORT_ERROR
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
