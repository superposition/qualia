//! `qualia-map`: the persistent occupancy map runner.
//!
//! The runner owns the two map slots of the shared arena. `PersistentMapGrid`
//! accumulates integer log-odds for every cell a range return has touched, and
//! `BinaryMapGrid` reduces the same world to occupied / free / unknown for
//! consumers that would rather not threshold log-odds themselves. A scan is
//! integrated only while its pose is fresh and confident, because a wrong pose
//! smears every return, and a scan whose robot pose falls outside the grid is
//! dropped whole. The map plane is the world's x–z plane: the grid's second
//! axis carries the world's `z_m`, matching the orientation the lidar runner
//! publishes. Publishing is paced by the belief layers: a scan waits for a
//! recent belief commit before it is integrated, and only a belief silence past
//! `QUALIA_BELIEF_PACE_MS * 8` lets the map publish without one.
//!
//! Nothing here owns state — the arena bytes are the state. `qualia-init`
//! creates the region and this runner attaches to it by name.

use qualia_shm::{LayerReader, ShmRegion, NUM_LAYERS};
use qualia_types::{
    BinaryMapGrid, LidarScanSnapshot, NavPose, PersistentMapGrid, MAP_GRID_CELLS, MAP_GRID_H,
    MAP_GRID_W,
};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Arena name used when `QUALIA_SHM_NAME` is unset.
const DEFAULT_SHM_NAME: &str = "/qualia_body";
/// Poll interval between scans, overridable with `QUALIA_MAP_POLL_MS`.
const DEFAULT_POLL_MS: u64 = 50;
/// Integrations between progress lines, overridable with `QUALIA_MAP_LOG_EVERY_UPDATES`.
const DEFAULT_LOG_EVERY_UPDATES: u64 = 20;
/// Side of one map cell in metres, matching the ABI.
const MAP_RESOLUTION_M: f32 = 0.10;
/// Log-odds added to a cell a return lands in.
const LOG_ODDS_HIT: i16 = 10;
/// Log-odds added to a cell a ray crosses without stopping.
const LOG_ODDS_MISS: i16 = -3;
/// Lower clamp on a cell's accumulated log-odds.
const LOG_ODDS_MIN: i16 = -80;
/// Upper clamp on a cell's accumulated log-odds.
const LOG_ODDS_MAX: i16 = 80;
/// Log-odds at or above which a cell reads as occupied.
const OCCUPIED_THRESHOLD: i16 = 15;
/// Log-odds at or below which a cell reads as observed free space.
const OBSERVED_THRESHOLD: i16 = -5;
/// Returns closer than this are ignored; they sit inside the robot shell.
const MIN_INTEGRATION_RANGE_M: f32 = 0.15;
/// Returns past this range are ignored as too sparse to trust.
const MAX_INTEGRATION_RANGE_M: f32 = 5.5;
/// Radius of the free-space disc stamped under the robot, in cells.
const ROBOT_CLEAR_RADIUS_CELLS: i32 = 3;
/// Occupied neighbours an occupied cell needs to survive the thin-speck filter.
const OCCUPIED_NEIGHBOR_MIN: usize = 2;
/// Default pose confidence floor, overridable with `QUALIA_MAP_MIN_POSE_CONFIDENCE`.
const DEFAULT_MIN_POSE_CONFIDENCE: f32 = 0.70;
/// Default pose age ceiling in milliseconds, overridable with `QUALIA_MAP_MAX_POSE_AGE_MS`.
const DEFAULT_MAX_POSE_AGE_MS: u64 = 250;
/// Default belief pace window, overridable with `QUALIA_BELIEF_PACE_MS`.
const DEFAULT_BELIEF_PACE_MS: u64 = 250;
/// Pace windows of belief silence after which a publisher stops waiting.
const BELIEF_PACE_STALE_WINDOWS: u64 = 8;
/// Skip-integration warnings are rate-limited to one per this many nanoseconds.
const SKIP_LOG_INTERVAL_NS: u64 = 2_000_000_000;
/// Torn-read retries attempted per scan snapshot.
const SNAPSHOT_ATTEMPTS: usize = 8;

/// Everything the runner reads from its environment, resolved once at start.
struct Settings {
    shm_name: String,
    poll_ms: u64,
    log_every: u64,
    reset_on_start: bool,
    min_pose_confidence: f32,
    max_pose_age_ms: u64,
    belief_pace_ms: u64,
}

impl Settings {
    fn from_env() -> Self {
        Self {
            shm_name: std::env::var("QUALIA_SHM_NAME")
                .unwrap_or_else(|_| DEFAULT_SHM_NAME.to_string()),
            poll_ms: env_parsed("QUALIA_MAP_POLL_MS", DEFAULT_POLL_MS),
            log_every: env_parsed("QUALIA_MAP_LOG_EVERY_UPDATES", DEFAULT_LOG_EVERY_UPDATES),
            reset_on_start: std::env::var("QUALIA_MAP_RESET_ON_START")
                .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
                .unwrap_or(false),
            min_pose_confidence: env_parsed(
                "QUALIA_MAP_MIN_POSE_CONFIDENCE",
                DEFAULT_MIN_POSE_CONFIDENCE,
            ),
            max_pose_age_ms: env_parsed("QUALIA_MAP_MAX_POSE_AGE_MS", DEFAULT_MAX_POSE_AGE_MS),
            belief_pace_ms: env_parsed("QUALIA_BELIEF_PACE_MS", DEFAULT_BELIEF_PACE_MS),
        }
    }
}

fn env_parsed<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(default)
}

/// What the belief pace gate decided about one publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BeliefPace {
    /// A belief commit landed inside the pace window; publish right away.
    Fresh,
    /// Belief is lagging but not yet stale; keep waiting for it.
    Lagging,
    /// Belief has been silent past the stale window; publish anyway.
    Stale { lag_ms: u64 },
}

/// Holds a publisher back until the belief layers have committed recently.
///
/// A layer commits by flipping its slot's write index, so the front buffer's
/// `timestamp_ns` is that layer's newest accepted tick, already part of the
/// shared ABI. The gate lets a publish through as soon as some layer ticked
/// inside `pace_ms`; if every layer has been silent for `pace_ms * 8` it stops
/// waiting and hands back the lag it measured, so navigation degrades instead
/// of stopping. A `pace_ms` of zero disables the gate and leaves the publisher
/// behaving exactly as it did before the gate existed.
struct BeliefPaceGate {
    pace_ms: u64,
    started_ns: u64,
}

impl BeliefPaceGate {
    fn new(pace_ms: u64, now_ns: u64) -> Self {
        Self {
            pace_ms,
            started_ns: now_ns,
        }
    }

    /// How far the newest belief commit lags `now_ns`.
    ///
    /// While no layer has committed at all there is no tick to measure from,
    /// so the lag runs from the moment the gate was created and a cold stack
    /// still reports a real number instead of an invented one.
    fn lag_ms(&self, newest_tick_ns: Option<u64>, now_ns: u64) -> u64 {
        now_ns.saturating_sub(newest_tick_ns.unwrap_or(self.started_ns)) / 1_000_000
    }

    /// Classify the newest belief tick against the pace and stale windows.
    fn classify(&self, newest_tick_ns: Option<u64>, now_ns: u64) -> BeliefPace {
        if self.pace_ms == 0 {
            return BeliefPace::Fresh;
        }
        let lag_ms = self.lag_ms(newest_tick_ns, now_ns);
        if lag_ms < self.pace_ms {
            BeliefPace::Fresh
        } else if lag_ms <= self.pace_ms.saturating_mul(BELIEF_PACE_STALE_WINDOWS) {
            BeliefPace::Lagging
        } else {
            BeliefPace::Stale { lag_ms }
        }
    }

    /// Wait for belief to come inside the pace window.
    ///
    /// Returns `None` when a recent commit is available, or `Some(lag_ms)` when
    /// the layers went stale and the caller should publish anyway.
    fn await_pace(&self, shm: &ShmRegion, poll_ms: u64) -> Option<u64> {
        loop {
            match self.classify(newest_belief_tick_ns(shm), now_ns()) {
                BeliefPace::Fresh => return None,
                BeliefPace::Lagging => thread::sleep(Duration::from_millis(poll_ms.max(1))),
                BeliefPace::Stale { lag_ms } => return Some(lag_ms),
            }
        }
    }
}

/// Newest accepted belief tick across the layer slots.
///
/// The front buffer of a slot nobody has committed into is still zero, and
/// zero is the ABI's "never", so it does not count as a tick.
fn newest_belief_tick_ns(shm: &ShmRegion) -> Option<u64> {
    (0..NUM_LAYERS)
        .filter_map(|layer| {
            let tick = LayerReader::new(shm.layer_slot(layer)).read().timestamp_ns;
            (tick != 0).then_some(tick)
        })
        .max()
}

fn main() {
    let settings = Settings::from_env();
    let shm = match ShmRegion::open(&settings.shm_name) {
        Ok(shm) => shm,
        Err(err) => {
            eprintln!(
                "qualia-map: failed to open shm '{}': {err}",
                settings.shm_name
            );
            std::process::exit(1);
        }
    };

    init_map_grid(&shm, settings.reset_on_start);
    let pace = BeliefPaceGate::new(settings.belief_pace_ms, now_ns());
    println!(
        "qualia-map: starting persistent lidar mapping from shm={}",
        settings.shm_name
    );

    let mut last_scan_seq = 0u64;
    let mut updates = 0u64;
    let mut last_skip_log_ns = 0u64;

    loop {
        let Ok(scan) = shm.lidar_scan().snapshot(SNAPSHOT_ATTEMPTS) else {
            thread::sleep(Duration::from_millis(settings.poll_ms));
            continue;
        };
        if scan.seq == 0 || scan.seq == last_scan_seq {
            thread::sleep(Duration::from_millis(settings.poll_ms));
            continue;
        }

        let pose = shm.world_model().robot_pose;
        let now = now_ns();
        if !pose_is_usable(
            &pose,
            now,
            settings.min_pose_confidence,
            settings.max_pose_age_ms,
        ) {
            if now.saturating_sub(last_skip_log_ns) >= SKIP_LOG_INTERVAL_NS {
                println!(
                    "qualia-map: skip integration pose_conf={:.3} pose_age_ms={} min_conf={:.3} max_age_ms={}",
                    pose.confidence,
                    now.saturating_sub(pose.timestamp_ns) / 1_000_000,
                    settings.min_pose_confidence,
                    settings.max_pose_age_ms
                );
                last_skip_log_ns = now;
            }
            thread::sleep(Duration::from_millis(settings.poll_ms));
            continue;
        }

        if let Some(lag_ms) = pace.await_pace(&shm, settings.poll_ms) {
            println!("qualia-map: belief pace: stale, publishing map at {lag_ms} ms");
        }

        integrate_scan(&shm, &scan, pose.x_m, pose.z_m, pose.yaw_rad);
        last_scan_seq = scan.seq;
        updates = updates.wrapping_add(1);

        if updates == 1 || (settings.log_every != 0 && updates % settings.log_every == 0) {
            let map = shm.map_grid();
            let binary = shm.binary_map();
            println!(
                "qualia-map: map_seq={} occupied_cells={} observed_cells={} bin_occ={} bin_free={} bin_unknown={} last_update_ns={}",
                map.seq.load(Ordering::Acquire),
                map.occupied_cells,
                map.observed_cells,
                binary.occupied_cells,
                binary.free_cells,
                binary.unknown_cells,
                map.last_update_ns
            );
        }

        thread::sleep(Duration::from_millis(settings.poll_ms));
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

/// Writes the grid geometry into the arena, or keeps a live map intact.
///
/// An existing grid with the right shape and a non-zero sequence is left
/// alone unless `reset_on_start` is set, so a runner restart does not throw
/// away the map the previous process built.
fn init_map_grid(shm: &ShmRegion, reset_on_start: bool) {
    let map = shm.map_grid_mut();
    if !reset_on_start
        && map.width == MAP_GRID_W as u32
        && map.height == MAP_GRID_H as u32
        && map.seq.load(Ordering::Acquire) != 0
    {
        return;
    }

    let origin_m = -(MAP_GRID_W as f32 * MAP_RESOLUTION_M * 0.5);
    map.width = MAP_GRID_W as u32;
    map.height = MAP_GRID_H as u32;
    map.resolution_m = MAP_RESOLUTION_M;
    map.origin_x_m = origin_m;
    map.origin_y_m = origin_m;
    map.last_update_ns = 0;
    map.occupied_cells = 0;
    map.observed_cells = 0;
    map.log_odds.fill(0);
    map.seq.store(1, Ordering::Release);

    let binary = shm.binary_map_mut();
    binary.width = MAP_GRID_W as u32;
    binary.height = MAP_GRID_H as u32;
    binary.resolution_m = MAP_RESOLUTION_M;
    binary.origin_x_m = origin_m;
    binary.origin_y_m = origin_m;
    binary.last_update_ns = 0;
    binary.occupied_cells = 0;
    binary.free_cells = 0;
    binary.unknown_cells = MAP_GRID_CELLS as u32;
    binary.cells.fill(0);
    binary.seq.store(1, Ordering::Release);
}

/// Whether a pose is fresh and confident enough to integrate a scan under.
fn pose_is_usable(pose: &NavPose, now_ns: u64, min_confidence: f32, max_pose_age_ms: u64) -> bool {
    if pose.timestamp_ns == 0 || pose.confidence < min_confidence {
        return false;
    }
    now_ns.saturating_sub(pose.timestamp_ns) / 1_000_000 <= max_pose_age_ms
}

/// Integrates one scan into both map slots from the pose it was taken at.
///
/// Each usable return marks its cell a hit and every cell between the robot
/// and it a miss. The binary slot is then re-derived from the log-odds, thinned
/// and cleared under the robot, and both sequence words advance so readers can
/// see a consistent map.
fn integrate_scan(
    shm: &ShmRegion,
    scan: &LidarScanSnapshot,
    pose_x: f32,
    pose_z: f32,
    pose_yaw: f32,
) {
    let map = shm.map_grid_mut();
    let point_count = (scan.point_count as usize).min(scan.points.len());
    let Some((robot_gx, robot_gz)) = world_to_map(map.origin_x_m, map.origin_y_m, pose_x, pose_z)
    else {
        return;
    };

    for point in scan.points.iter().take(point_count) {
        if point.intensity == 0
            || point.distance_m < MIN_INTEGRATION_RANGE_M
            || point.distance_m > MAX_INTEGRATION_RANGE_M
        {
            continue;
        }

        let bearing = pose_yaw + point.angle_rad;
        let world_x = pose_x + bearing.cos() * point.distance_m;
        let world_z = pose_z + bearing.sin() * point.distance_m;
        let Some((hit_gx, hit_gz)) =
            world_to_map(map.origin_x_m, map.origin_y_m, world_x, world_z)
        else {
            continue;
        };

        trace_free_cells(map, robot_gx, robot_gz, hit_gx, hit_gz);
        bump_cell(map, hit_gx, hit_gz, LOG_ODDS_HIT);
    }

    let stamp_ns = scan.scan_end_ns.max(scan.scan_start_ns);
    let binary = shm.binary_map_mut();
    binary.width = MAP_GRID_W as u32;
    binary.height = MAP_GRID_H as u32;
    binary.resolution_m = MAP_RESOLUTION_M;
    binary.origin_x_m = map.origin_x_m;
    binary.origin_y_m = map.origin_y_m;
    binary.last_update_ns = stamp_ns;
    for (idx, &value) in map.log_odds.iter().enumerate() {
        binary.cells[idx] = if value >= OCCUPIED_THRESHOLD {
            2
        } else if value <= OBSERVED_THRESHOLD {
            1
        } else {
            0
        };
    }
    postprocess_binary_map(binary, map.origin_x_m, map.origin_y_m, pose_x, pose_z);

    let mut occupied = 0u32;
    let mut observed = 0u32;
    binary.occupied_cells = 0;
    binary.free_cells = 0;
    binary.unknown_cells = 0;
    for (idx, &value) in map.log_odds.iter().enumerate() {
        if value >= OCCUPIED_THRESHOLD {
            occupied += 1;
            observed += 1;
        } else if value <= OBSERVED_THRESHOLD {
            observed += 1;
        }
        match binary.cells[idx] {
            2 => binary.occupied_cells += 1,
            1 => binary.free_cells += 1,
            _ => binary.unknown_cells += 1,
        }
    }
    map.occupied_cells = occupied;
    map.observed_cells = observed;
    map.last_update_ns = stamp_ns;

    let next_map = map.seq.load(Ordering::Acquire).wrapping_add(1);
    map.seq.store(next_map, Ordering::Release);
    let next_binary = binary.seq.load(Ordering::Acquire).wrapping_add(1);
    binary.seq.store(next_binary, Ordering::Release);
}

/// Walks the integer line from the robot cell to the hit cell, marking every
/// crossed cell free. The hit cell itself is left for `bump_cell`'s hit.
fn trace_free_cells(map: &mut PersistentMapGrid, x0: i32, z0: i32, x1: i32, z1: i32) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dz = -(z1 - z0).abs();
    let sz = if z0 < z1 { 1 } else { -1 };
    let mut err = dx + dz;
    let (mut x, mut z) = (x0, z0);

    while x != x1 || z != z1 {
        bump_cell(map, x, z, LOG_ODDS_MISS);
        let e2 = 2 * err;
        if e2 >= dz {
            err += dz;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            z += sz;
        }
        if x < 0 || z < 0 || x >= MAP_GRID_W as i32 || z >= MAP_GRID_H as i32 {
            break;
        }
    }
}

/// Applies a clamped log-odds delta to one in-grid cell; off-grid is a no-op.
fn bump_cell(map: &mut PersistentMapGrid, gx: i32, gz: i32, delta: i16) {
    if gx < 0 || gz < 0 || gx >= MAP_GRID_W as i32 || gz >= MAP_GRID_H as i32 {
        return;
    }
    let idx = gz as usize * MAP_GRID_W + gx as usize;
    if idx >= MAP_GRID_CELLS {
        return;
    }
    map.log_odds[idx] = (map.log_odds[idx] + delta).clamp(LOG_ODDS_MIN, LOG_ODDS_MAX);
}

/// Thins the binary map and stamps free space under the robot.
///
/// An occupied cell with fewer than [`OCCUPIED_NEIGHBOR_MIN`] occupied
/// neighbours is a speck, not a surface, so it is demoted to free. The disc
/// under the robot is forced free afterwards because the robot's own body
/// returns would otherwise wall it in.
fn postprocess_binary_map(
    binary: &mut BinaryMapGrid,
    origin_x: f32,
    origin_z: f32,
    pose_x: f32,
    pose_z: f32,
) {
    let mut filtered = binary.cells;
    for gz in 0..MAP_GRID_H as i32 {
        for gx in 0..MAP_GRID_W as i32 {
            let idx = gz as usize * MAP_GRID_W + gx as usize;
            if binary.cells[idx] != 2 {
                continue;
            }
            if occupied_neighbors(&binary.cells, gx, gz) < OCCUPIED_NEIGHBOR_MIN {
                filtered[idx] = 1;
            }
        }
    }

    if let Some((robot_gx, robot_gz)) = world_to_map(origin_x, origin_z, pose_x, pose_z) {
        for dz in -ROBOT_CLEAR_RADIUS_CELLS..=ROBOT_CLEAR_RADIUS_CELLS {
            for dx in -ROBOT_CLEAR_RADIUS_CELLS..=ROBOT_CLEAR_RADIUS_CELLS {
                if dx * dx + dz * dz > ROBOT_CLEAR_RADIUS_CELLS * ROBOT_CLEAR_RADIUS_CELLS {
                    continue;
                }
                let gx = robot_gx + dx;
                let gz = robot_gz + dz;
                if gx < 0 || gz < 0 || gx >= MAP_GRID_W as i32 || gz >= MAP_GRID_H as i32 {
                    continue;
                }
                filtered[gz as usize * MAP_GRID_W + gx as usize] = 1;
            }
        }
    }

    binary.cells = filtered;
}

/// Occupied count among the eight neighbours of a cell. Off-grid neighbours
/// count as unknown, never as occupied.
fn occupied_neighbors(cells: &[u8; MAP_GRID_CELLS], gx: i32, gz: i32) -> usize {
    let mut count = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            if cell_at(cells, gx + dx, gz + dz) == Some(2) {
                count += 1;
            }
        }
    }
    count
}

fn cell_at(cells: &[u8; MAP_GRID_CELLS], gx: i32, gz: i32) -> Option<u8> {
    if gx < 0 || gz < 0 || gx >= MAP_GRID_W as i32 || gz >= MAP_GRID_H as i32 {
        return None;
    }
    Some(cells[gz as usize * MAP_GRID_W + gx as usize])
}

/// Converts a world point on the map plane to a grid cell, or `None` outside.
fn world_to_map(origin_x: f32, origin_z: f32, x_m: f32, z_m: f32) -> Option<(i32, i32)> {
    let gx = ((x_m - origin_x) / MAP_RESOLUTION_M).floor() as i32;
    let gz = ((z_m - origin_z) / MAP_RESOLUTION_M).floor() as i32;
    if gx < 0 || gz < 0 || gx >= MAP_GRID_W as i32 || gz >= MAP_GRID_H as i32 {
        None
    } else {
        Some((gx, gz))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_types::{
        LidarPoint, MAP_GRID_CELLS, MAP_GRID_H, MAP_GRID_W, LIDAR_MAX_POINTS,
    };
    use qualia_shm::LayerWriter;
    use std::sync::atomic::{AtomicU64, Ordering};

    const RESOLUTION_M: f32 = 0.10;
    const HIT: i16 = 10;
    const MISS: i16 = -3;
    const CLAMP_HI: i16 = 80;
    const CLAMP_LO: i16 = -80;

    fn origin_m() -> f32 {
        -(MAP_GRID_W as f32 * RESOLUTION_M * 0.5)
    }

    fn cell_index(gx: i32, gz: i32) -> usize {
        gz as usize * MAP_GRID_W + gx as usize
    }

    fn test_region(tag: &str) -> ShmRegion {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("/qualia-map-test-{tag}-{}-{n}", std::process::id());
        ShmRegion::create(&name).expect("create test region")
    }

    fn scan(points: &[(f32, f32, u8)], end_ns: u64) -> LidarScanSnapshot {
        let mut snapshot = LidarScanSnapshot::default();
        snapshot.scan_start_ns = end_ns.saturating_sub(10_000_000);
        snapshot.scan_end_ns = end_ns;
        snapshot.point_count = points.len().min(LIDAR_MAX_POINTS) as u32;
        for (slot, &(angle_rad, distance_m, intensity)) in points.iter().enumerate() {
            snapshot.points[slot] = LidarPoint {
                angle_rad,
                distance_m,
                intensity,
                _pad: [0; 3],
            };
        }
        snapshot
    }

    fn pose_at(x_m: f32, z_m: f32, yaw_rad: f32, confidence: f32, timestamp_ns: u64) -> NavPose {
        NavPose {
            x_m,
            y_m: 0.0,
            z_m,
            yaw_rad,
            pitch_rad: 0.0,
            roll_rad: 0.0,
            confidence,
            _pad0: 0.0,
            timestamp_ns,
        }
    }

    /// Three hits straight ahead at 2.0/2.1/2.2 m land on grid cells
    /// (148,128), (149,128) and (150,128) with the robot at the grid centre.
    fn three_forward_hits(end_ns: u64) -> LidarScanSnapshot {
        scan(&[(0.0, 2.0, 255), (0.0, 2.1, 255), (0.0, 2.2, 255)], end_ns)
    }

    #[test]
    fn init_publishes_grid_geometry() {
        let shm = test_region("geometry");
        init_map_grid(&shm, false);

        let map = shm.map_grid();
        assert_eq!(map.width, MAP_GRID_W as u32);
        assert_eq!(map.height, MAP_GRID_H as u32);
        assert_eq!(map.resolution_m, RESOLUTION_M);
        assert_eq!(map.origin_x_m, origin_m());
        assert_eq!(map.origin_y_m, origin_m());
        assert_eq!(map.seq.load(Ordering::Acquire), 1);
        assert_eq!(map.occupied_cells, 0);
        assert_eq!(map.observed_cells, 0);
        assert_eq!(map.last_update_ns, 0);
        assert!(map.log_odds.iter().all(|&v| v == 0));

        let binary = shm.binary_map();
        assert_eq!(binary.width, MAP_GRID_W as u32);
        assert_eq!(binary.height, MAP_GRID_H as u32);
        assert_eq!(binary.resolution_m, RESOLUTION_M);
        assert_eq!(binary.origin_x_m, map.origin_x_m);
        assert_eq!(binary.origin_y_m, map.origin_y_m);
        assert_eq!(binary.seq.load(Ordering::Acquire), 1);
        assert_eq!(binary.occupied_cells, 0);
        assert_eq!(binary.free_cells, 0);
        assert_eq!(binary.unknown_cells, MAP_GRID_CELLS as u32);
        assert!(binary.cells.iter().all(|&c| c == 0));
    }

    #[test]
    fn init_preserves_live_grid_and_resets_on_request() {
        let shm = test_region("reinit");
        init_map_grid(&shm, false);
        shm.map_grid().seq.store(7, Ordering::Release);
        shm.binary_map_mut().occupied_cells = 3;

        init_map_grid(&shm, false);
        assert_eq!(shm.map_grid().seq.load(Ordering::Acquire), 7);
        assert_eq!(shm.binary_map().occupied_cells, 3);

        init_map_grid(&shm, true);
        assert_eq!(shm.map_grid().seq.load(Ordering::Acquire), 1);
        assert!(shm.map_grid().log_odds.iter().all(|&v| v == 0));
        assert_eq!(shm.binary_map().occupied_cells, 0);
        assert_eq!(shm.binary_map().unknown_cells, MAP_GRID_CELLS as u32);
        assert!(shm.binary_map().cells.iter().all(|&c| c == 0));
    }

    #[test]
    fn world_to_map_maps_and_rejects_out_of_grid() {
        let origin = origin_m();
        assert_eq!(world_to_map(origin, origin, origin, origin), Some((0, 0)));
        assert_eq!(world_to_map(origin, origin, 0.0, 0.0), Some((128, 128)));
        assert_eq!(world_to_map(origin, origin, 12.75, 0.0), Some((255, 128)));
        assert_eq!(world_to_map(origin, origin, 13.0, 0.0), None);
        assert_eq!(world_to_map(origin, origin, -12.81, 0.0), None);
        assert_eq!(world_to_map(origin, origin, 0.0, -13.5), None);
    }

    #[test]
    fn bump_cell_clamps_and_ignores_out_of_bounds() {
        let shm = test_region("clamp");
        init_map_grid(&shm, false);
        let map = shm.map_grid_mut();

        for _ in 0..20 {
            bump_cell(map, 10, 10, HIT);
        }
        assert_eq!(map.log_odds[cell_index(10, 10)], CLAMP_HI);

        for _ in 0..40 {
            bump_cell(map, 20, 20, MISS);
        }
        assert_eq!(map.log_odds[cell_index(20, 20)], CLAMP_LO);

        bump_cell(map, -1, 5, HIT);
        bump_cell(map, 5, MAP_GRID_H as i32, HIT);
        bump_cell(map, MAP_GRID_W as i32, 5, HIT);
        assert_eq!(map.log_odds.iter().filter(|&&v| v == HIT).count(), 0);
    }

    #[test]
    fn pose_gate_rejects_stale_low_confidence_and_unset_pose() {
        let now = 1_000_000_000_000u64;
        assert!(pose_is_usable(
            &pose_at(0.0, 0.0, 0.0, 1.0, now),
            now,
            0.70,
            250
        ));
        assert!(pose_is_usable(
            &pose_at(0.0, 0.0, 0.0, 0.70, now - 250_000_000),
            now,
            0.70,
            250
        ));
        assert!(!pose_is_usable(
            &pose_at(0.0, 0.0, 0.0, 1.0, now - 251_000_000),
            now,
            0.70,
            250
        ));
        assert!(!pose_is_usable(
            &pose_at(0.0, 0.0, 0.0, 0.69, now),
            now,
            0.70,
            250
        ));
        assert!(!pose_is_usable(
            &pose_at(0.0, 0.0, 0.0, 1.0, 0),
            now,
            0.70,
            250
        ));
    }

    #[test]
    fn integration_publishes_log_odds_hits_and_free_cells() {
        let shm = test_region("integrate");
        init_map_grid(&shm, false);
        let end_ns = 5_000_000_000u64;

        for _ in 0..2 {
            let snapshot = three_forward_hits(end_ns);
            shm.lidar_scan_mut().publish(&snapshot).unwrap();
            integrate_scan(&shm, &snapshot, 0.0, 0.0, 0.0);
        }

        let map = shm.map_grid();
        assert_eq!(map.seq.load(Ordering::Acquire), 3);
        assert_eq!(map.last_update_ns, end_ns);
        // Three rays share the corridor behind the hits, so each cell on it
        // takes one miss per ray and per tick.
        assert_eq!(map.log_odds[cell_index(128, 128)], -18);
        assert_eq!(map.log_odds[cell_index(147, 128)], -18);
        // 148 is crossed by the rays aimed at 149 and 150 only; 149 by the one
        // aimed at 150.
        assert_eq!(map.log_odds[cell_index(148, 128)], 8);
        assert_eq!(map.log_odds[cell_index(149, 128)], 14);
        assert_eq!(map.log_odds[cell_index(150, 128)], 20);
        assert_eq!(map.occupied_cells, 1);
        assert_eq!(map.observed_cells, 21);

        let binary = shm.binary_map();
        assert_eq!(binary.seq.load(Ordering::Acquire), 3);
        assert_eq!(binary.last_update_ns, end_ns);
        // 150 crosses the occupied threshold on its own but is an isolated
        // occupied cell, so the neighbour filter demotes it to free.
        assert_eq!(binary.cells[cell_index(150, 128)], 1);
        assert_eq!(binary.cells[cell_index(147, 128)], 1);
        assert_eq!(binary.cells[cell_index(128, 128)], 1);
        assert_eq!(binary.cells[cell_index(160, 128)], 0);
        assert_eq!(binary.occupied_cells, 0);
        // The 20 corridor cells, cell 150, and the 25 robot-clearance cells
        // outside the corridor.
        assert_eq!(binary.free_cells, 46);
        assert_eq!(binary.unknown_cells, MAP_GRID_CELLS as u32 - 46);
    }

    #[test]
    fn integration_keeps_clustered_occupied_cell() {
        let shm = test_region("cluster");
        init_map_grid(&shm, false);
        let end_ns = 9_000_000_000u64;

        for _ in 0..4 {
            let snapshot = three_forward_hits(end_ns);
            integrate_scan(&shm, &snapshot, 0.0, 0.0, 0.0);
        }

        let map = shm.map_grid();
        assert_eq!(map.log_odds[cell_index(148, 128)], 16);
        assert_eq!(map.log_odds[cell_index(149, 128)], 28);
        assert_eq!(map.log_odds[cell_index(150, 128)], 40);
        assert_eq!(map.occupied_cells, 3);
        assert_eq!(map.observed_cells, 23);

        let binary = shm.binary_map();
        // The middle cell has two occupied neighbours so it survives the
        // filter; both ends have one and are demoted.
        assert_eq!(binary.cells[cell_index(149, 128)], 2);
        assert_eq!(binary.cells[cell_index(148, 128)], 1);
        assert_eq!(binary.cells[cell_index(150, 128)], 1);
        assert_eq!(binary.occupied_cells, 1);
        assert_eq!(binary.free_cells, 47);
        assert_eq!(binary.unknown_cells, MAP_GRID_CELLS as u32 - 48);
    }

    #[test]
    fn integration_ignores_out_of_range_and_dark_returns() {
        let shm = test_region("skip");
        init_map_grid(&shm, false);
        let end_ns = 7_000_000_000u64;
        let snapshot = scan(&[(0.0, 2.0, 0), (0.0, 0.10, 255), (0.0, 6.0, 255)], end_ns);

        integrate_scan(&shm, &snapshot, 0.0, 0.0, 0.0);

        let map = shm.map_grid();
        assert_eq!(map.seq.load(Ordering::Acquire), 2);
        assert_eq!(map.last_update_ns, end_ns);
        assert!(map.log_odds.iter().all(|&v| v == 0));
        assert_eq!(map.occupied_cells, 0);
        assert_eq!(map.observed_cells, 0);

        let binary = shm.binary_map();
        assert_eq!(binary.last_update_ns, end_ns);
        assert_eq!(binary.cells[cell_index(150, 128)], 0);
        assert_eq!(binary.occupied_cells, 0);
        // Only the robot-clearance footprint is published.
        assert_eq!(binary.free_cells, 29);
        assert_eq!(binary.unknown_cells, MAP_GRID_CELLS as u32 - 29);
    }

    #[test]
    fn scan_off_the_grid_does_not_publish() {
        let shm = test_region("outside");
        init_map_grid(&shm, false);
        let end_ns = 3_000_000_000u64;
        let snapshot = three_forward_hits(end_ns);

        // A pose far outside the grid leaves the map exactly as it was.
        integrate_scan(&shm, &snapshot, 1_000.0, 1_000.0, 0.0);

        let map = shm.map_grid();
        assert_eq!(map.seq.load(Ordering::Acquire), 1);
        assert!(map.log_odds.iter().all(|&v| v == 0));
        assert_eq!(map.last_update_ns, 0);
        assert_eq!(shm.binary_map().seq.load(Ordering::Acquire), 1);
    }

    /// Publish a belief tick the way a belief runner commits one: write the
    /// back buffer, then flip the slot's index.
    fn commit_belief(shm: &ShmRegion, layer: usize, timestamp_ns: u64) {
        let writer = LayerWriter::new(shm.layer_slot(layer));
        writer.back_buffer().timestamp_ns = timestamp_ns;
        writer.publish();
    }

    #[test]
    fn newest_belief_tick_is_the_newest_committed_layer() {
        let shm = test_region("pace-layers");
        assert_eq!(newest_belief_tick_ns(&shm), None);

        commit_belief(&shm, 2, 111_000_000);
        commit_belief(&shm, NUM_LAYERS - 1, 999_000_000);
        assert_eq!(newest_belief_tick_ns(&shm), Some(999_000_000));
    }

    #[test]
    fn pace_classifies_fresh_lagging_and_stale_belief() {
        let shm = test_region("pace-classify");
        let now = 5_000_000_000_000u64;
        let gate = BeliefPaceGate::new(DEFAULT_BELIEF_PACE_MS, now);

        commit_belief(&shm, 0, now - 100_000_000);
        assert_eq!(
            gate.classify(newest_belief_tick_ns(&shm), now),
            BeliefPace::Fresh
        );

        // Inside the stale window the publisher waits for belief to catch up.
        commit_belief(&shm, 0, now - 1_000_000_000);
        assert_eq!(
            gate.classify(newest_belief_tick_ns(&shm), now),
            BeliefPace::Lagging
        );

        // Past pace * 8 the publisher goes ahead at the lag it measured.
        commit_belief(&shm, 0, now - 3_000_000_000);
        assert_eq!(
            gate.classify(newest_belief_tick_ns(&shm), now),
            BeliefPace::Stale { lag_ms: 3_000 }
        );
    }

    #[test]
    fn publishes_when_belief_lag_exceeded() {
        let shm = test_region("pace-stale");
        let stale_ms = DEFAULT_BELIEF_PACE_MS * BELIEF_PACE_STALE_WINDOWS;
        let now = now_ns();
        commit_belief(&shm, 0, now - (stale_ms + 1) * 1_000_000);

        let gate = BeliefPaceGate::new(DEFAULT_BELIEF_PACE_MS, now);
        let lag_ms = gate
            .await_pace(&shm, 1)
            .expect("belief past the stale window must not block the publisher");
        assert!(lag_ms >= stale_ms + 1, "reported lag was {lag_ms} ms");
    }

    #[test]
    fn cold_layers_wait_out_the_stale_window_then_publish() {
        let shm = test_region("pace-cold");
        let start = 2_000_000_000u64;
        let gate = BeliefPaceGate::new(10, start);

        assert_eq!(
            gate.classify(newest_belief_tick_ns(&shm), start),
            BeliefPace::Fresh
        );
        assert_eq!(
            gate.classify(newest_belief_tick_ns(&shm), start + 50_000_000),
            BeliefPace::Lagging
        );
        assert_eq!(
            gate.classify(newest_belief_tick_ns(&shm), start + 90_000_000),
            BeliefPace::Stale { lag_ms: 90 }
        );
    }

    #[test]
    fn zero_pace_disables_the_gate() {
        let shm = test_region("pace-off");
        let now = 1_000_000_000u64;
        let gate = BeliefPaceGate::new(0, now);

        assert_eq!(gate.classify(None, now), BeliefPace::Fresh);
        assert_eq!(gate.await_pace(&shm, 1), None);
    }
}
