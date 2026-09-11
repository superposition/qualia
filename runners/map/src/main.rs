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
//! publishes.
//!
//! Nothing here owns state — the arena bytes are the state. `qualia-init`
//! creates the region and this runner attaches to it by name.

use qualia_shm::ShmRegion;
use qualia_types::{
    BinaryMapGrid, LidarPoint, LidarScanSnapshot, NavPose, PersistentMapGrid, MAP_GRID_CELLS,
    MAP_GRID_H, MAP_GRID_W,
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
/// Skip-integration warnings are rate-limited to one per this many nanoseconds.
const SKIP_LOG_INTERVAL_NS: u64 = 2_000_000_000;
/// Torn-read retries attempted per scan snapshot.
const SNAPSHOT_ATTEMPTS: usize = 8;
/// The three states a binary map cell can hold: never seen, seen empty, seen
/// as a surface.
const CELL_UNKNOWN: u8 = 0;
const CELL_FREE: u8 = 1;
const CELL_OCCUPIED: u8 = 2;
/// The sequence a freshly initialised slot carries. Never zero, so a reader
/// cannot mistake zeroed bytes for a published empty map.
const FIRST_SEQ: u64 = 1;

/// Everything the runner reads from its environment, resolved once at start.
struct Settings {
    shm_name: String,
    poll_ms: u64,
    log_every: u64,
    reset_on_start: bool,
    min_pose_confidence: f32,
    max_pose_age_ms: u64,
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
        }
    }
}

fn env_parsed<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(default)
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
    println!(
        "qualia-map: starting persistent lidar mapping from shm={}",
        settings.shm_name
    );

    let mut last_scan_seq = 0u64;
    let mut updates = 0u64;
    let mut last_skip_log_ns = 0u64;

    loop {
        let Ok(scan) = shm.lidar_scan().snapshot(SNAPSHOT_ATTEMPTS) else {
            idle(settings.poll_ms);
            continue;
        };
        if scan.seq == 0 || scan.seq == last_scan_seq {
            idle(settings.poll_ms);
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
            idle(settings.poll_ms);
            continue;
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

        idle(settings.poll_ms);
    }
}

/// Sleeps for one poll interval; the loop's only way to wait.
fn idle(poll_ms: u64) {
    thread::sleep(Duration::from_millis(poll_ms));
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
    if !reset_on_start && grid_is_live(map) {
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
    map.seq.store(FIRST_SEQ, Ordering::Release);

    let binary = shm.binary_map_mut();
    stamp_binary_geometry(binary, origin_m, origin_m);
    binary.last_update_ns = 0;
    binary.occupied_cells = 0;
    binary.free_cells = 0;
    binary.unknown_cells = MAP_GRID_CELLS as u32;
    binary.cells.fill(0);
    binary.seq.store(FIRST_SEQ, Ordering::Release);
}

/// Whether the arena already holds a map of the shape this build speaks.
fn grid_is_live(map: &PersistentMapGrid) -> bool {
    map.width == MAP_GRID_W as u32
        && map.height == MAP_GRID_H as u32
        && map.seq.load(Ordering::Acquire) != 0
}

/// Copies the shared grid geometry onto the binary slot.
fn stamp_binary_geometry(binary: &mut BinaryMapGrid, origin_x_m: f32, origin_z_m: f32) {
    binary.width = MAP_GRID_W as u32;
    binary.height = MAP_GRID_H as u32;
    binary.resolution_m = MAP_RESOLUTION_M;
    binary.origin_x_m = origin_x_m;
    binary.origin_y_m = origin_z_m;
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
/// Every usable return marks its cell a hit and every cell the ray crosses a
/// miss. The binary slot is then re-derived from the log-odds, thinned and
/// cleared under the robot, and both sequence words advance so readers see a
/// consistent map.
fn integrate_scan(
    shm: &ShmRegion,
    scan: &LidarScanSnapshot,
    pose_x: f32,
    pose_z: f32,
    pose_yaw: f32,
) {
    let map = shm.map_grid_mut();
    let Some(robot) = world_to_map(map.origin_x_m, map.origin_y_m, pose_x, pose_z) else {
        return;
    };

    let point_count = (scan.point_count as usize).min(scan.points.len());
    for point in scan.points.iter().take(point_count) {
        if !return_is_usable(point) {
            continue;
        }
        let bearing = pose_yaw + point.angle_rad;
        let hit = world_to_map(
            map.origin_x_m,
            map.origin_y_m,
            pose_x + bearing.cos() * point.distance_m,
            pose_z + bearing.sin() * point.distance_m,
        );
        let Some((hit_gx, hit_gz)) = hit else {
            continue;
        };
        trace_free_cells(map, robot, (hit_gx, hit_gz));
        bump_cell(map, hit_gx, hit_gz, LOG_ODDS_HIT);
    }

    let stamp_ns = scan.scan_end_ns.max(scan.scan_start_ns);
    let binary = shm.binary_map_mut();
    stamp_binary_geometry(binary, map.origin_x_m, map.origin_y_m);
    binary.last_update_ns = stamp_ns;
    for (cell, &log_odds) in binary.cells.iter_mut().zip(map.log_odds.iter()) {
        *cell = classify_cell(log_odds);
    }
    postprocess_binary_map(binary, map.origin_x_m, map.origin_y_m, pose_x, pose_z);

    let (occupied, observed) = map_cell_counts(&map.log_odds);
    map.occupied_cells = occupied;
    map.observed_cells = observed;
    map.last_update_ns = stamp_ns;
    let (occupied, free, unknown) = binary_cell_counts(&binary.cells);
    binary.occupied_cells = occupied;
    binary.free_cells = free;
    binary.unknown_cells = unknown;

    let next_map = map.seq.load(Ordering::Acquire).wrapping_add(1);
    map.seq.store(next_map, Ordering::Release);
    let next_binary = binary.seq.load(Ordering::Acquire).wrapping_add(1);
    binary.seq.store(next_binary, Ordering::Release);
}

/// Whether a return is worth integrating: bright enough and inside the usable
/// range. Written as negated comparisons so a non-finite distance reaches the
/// grid conversion instead of being dropped here.
fn return_is_usable(point: &LidarPoint) -> bool {
    point.intensity != 0
        && !(point.distance_m < MIN_INTEGRATION_RANGE_M)
        && !(point.distance_m > MAX_INTEGRATION_RANGE_M)
}

/// Thresholds one cell's accumulated log-odds into a binary map state.
fn classify_cell(log_odds: i16) -> u8 {
    if log_odds >= OCCUPIED_THRESHOLD {
        CELL_OCCUPIED
    } else if log_odds <= OBSERVED_THRESHOLD {
        CELL_FREE
    } else {
        CELL_UNKNOWN
    }
}

/// How many cells the persistent map counts as occupied and how many as seen.
fn map_cell_counts(log_odds: &[i16; MAP_GRID_CELLS]) -> (u32, u32) {
    let mut occupied = 0u32;
    let mut observed = 0u32;
    for &value in log_odds {
        let is_occupied = value >= OCCUPIED_THRESHOLD;
        let is_seen = is_occupied || value <= OBSERVED_THRESHOLD;
        occupied += u32::from(is_occupied);
        observed += u32::from(is_seen);
    }
    (occupied, observed)
}

/// How many cells the binary map publishes in each of its three states.
fn binary_cell_counts(cells: &[u8; MAP_GRID_CELLS]) -> (u32, u32, u32) {
    let mut occupied = 0;
    let mut free = 0;
    let mut unknown = 0;
    for &cell in cells {
        match cell {
            CELL_OCCUPIED => occupied += 1,
            CELL_FREE => free += 1,
            _ => unknown += 1,
        }
    }
    (occupied, free, unknown)
}

/// Walks the integer line between two grid cells, marking every crossed cell
/// free. The destination is left alone so the caller's hit lands on it.
fn trace_free_cells(map: &mut PersistentMapGrid, from: (i32, i32), to: (i32, i32)) {
    let delta_x = (to.0 - from.0).abs();
    let delta_z = -(to.1 - from.1).abs();
    let step_x = if from.0 < to.0 { 1 } else { -1 };
    let step_z = if from.1 < to.1 { 1 } else { -1 };
    let mut error = delta_x + delta_z;
    let (mut x, mut z) = from;

    while (x, z) != to {
        bump_cell(map, x, z, LOG_ODDS_MISS);
        let doubled = error * 2;
        if doubled >= delta_z {
            error += delta_z;
            x += step_x;
        }
        if doubled <= delta_x {
            error += delta_x;
            z += step_z;
        }
        if !in_grid(x, z) {
            break;
        }
    }
}

/// Applies a clamped log-odds delta to one in-grid cell; off-grid is a no-op.
fn bump_cell(map: &mut PersistentMapGrid, gx: i32, gz: i32, delta: i16) {
    if !in_grid(gx, gz) {
        return;
    }
    let slot = &mut map.log_odds[cell_index(gx, gz)];
    *slot = (*slot + delta).clamp(LOG_ODDS_MIN, LOG_ODDS_MAX);
}

/// Thins the binary map and stamps free space under the robot.
///
/// An occupied cell with fewer than [`OCCUPIED_NEIGHBOR_MIN`] occupied
/// neighbours is a speck rather than a surface, so it is demoted to free. The
/// disc under the robot is forced free afterwards because the robot's own body
/// returns would otherwise wall it in.
fn postprocess_binary_map(
    binary: &mut BinaryMapGrid,
    origin_x: f32,
    origin_z: f32,
    pose_x: f32,
    pose_z: f32,
) {
    let mut thinned = binary.cells;
    let mut slot = 0usize;
    for gz in 0..MAP_GRID_H as i32 {
        for gx in 0..MAP_GRID_W as i32 {
            if binary.cells[slot] == CELL_OCCUPIED
                && occupied_neighbors(&binary.cells, gx, gz) < OCCUPIED_NEIGHBOR_MIN
            {
                thinned[slot] = CELL_FREE;
            }
            slot += 1;
        }
    }
    if let Some(robot) = world_to_map(origin_x, origin_z, pose_x, pose_z) {
        stamp_robot_clearance(&mut thinned, robot);
    }
    binary.cells = thinned;
}

/// Forces the disc the robot occupies to free space.
fn stamp_robot_clearance(cells: &mut [u8; MAP_GRID_CELLS], robot: (i32, i32)) {
    let radius_sq = ROBOT_CLEAR_RADIUS_CELLS * ROBOT_CLEAR_RADIUS_CELLS;
    for dz in -ROBOT_CLEAR_RADIUS_CELLS..=ROBOT_CLEAR_RADIUS_CELLS {
        for dx in -ROBOT_CLEAR_RADIUS_CELLS..=ROBOT_CLEAR_RADIUS_CELLS {
            if dx * dx + dz * dz > radius_sq {
                continue;
            }
            let gx = robot.0 + dx;
            let gz = robot.1 + dz;
            if in_grid(gx, gz) {
                cells[cell_index(gx, gz)] = CELL_FREE;
            }
        }
    }
}

/// Occupied count among the eight neighbours of a cell. Off-grid neighbours
/// count as unknown, never as occupied.
fn occupied_neighbors(cells: &[u8; MAP_GRID_CELLS], gx: i32, gz: i32) -> usize {
    let mut count = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if (dx, dz) == (0, 0) {
                continue;
            }
            let (nx, nz) = (gx + dx, gz + dz);
            if in_grid(nx, nz) && cells[cell_index(nx, nz)] == CELL_OCCUPIED {
                count += 1;
            }
        }
    }
    count
}

/// Whether a grid coordinate lies inside the square map.
fn in_grid(gx: i32, gz: i32) -> bool {
    (0..MAP_GRID_W as i32).contains(&gx) && (0..MAP_GRID_H as i32).contains(&gz)
}

/// Index of an in-grid cell in the flat map arrays.
fn cell_index(gx: i32, gz: i32) -> usize {
    gz as usize * MAP_GRID_W + gx as usize
}

/// Converts a world point on the map plane to a grid cell, or `None` outside.
fn world_to_map(origin_x: f32, origin_z: f32, x_m: f32, z_m: f32) -> Option<(i32, i32)> {
    let gx = ((x_m - origin_x) / MAP_RESOLUTION_M).floor() as i32;
    let gz = ((z_m - origin_z) / MAP_RESOLUTION_M).floor() as i32;
    in_grid(gx, gz).then_some((gx, gz))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_types::{
        LidarPoint, MAP_GRID_CELLS, MAP_GRID_H, MAP_GRID_W, LIDAR_MAX_POINTS,
    };
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
}
