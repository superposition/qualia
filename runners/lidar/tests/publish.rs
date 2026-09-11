//! The bytes this runner publishes into shared memory, read back through the
//! same seqlocked accessors every other runner uses.

use std::sync::atomic::{AtomicU64, Ordering};

use qualia_lidar::{publish_scan, DevicePoint};
use qualia_shm::{
    LidarOccupancyGridSnapshot, ShmRegion, LIDAR_GRID_H, LIDAR_GRID_W, LIDAR_MAX_POINTS,
};

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region(tag: &str) -> ShmRegion {
    let name = format!(
        "/qualia_lidar_publish_{}_{}_{}",
        std::process::id(),
        tag,
        NEXT_REGION.fetch_add(1, Ordering::Relaxed)
    );
    ShmRegion::create(&name).expect("create region")
}

fn point(angle_deg: f32, distance_mm: u16, intensity: u8) -> DevicePoint {
    DevicePoint {
        angle_deg,
        distance_mm,
        intensity,
    }
}

fn occupied(grid: &LidarOccupancyGridSnapshot) -> Vec<(usize, u8)> {
    grid.cells
        .iter()
        .enumerate()
        .filter(|(_, value)| **value != 0)
        .map(|(index, value)| (index, *value))
        .collect()
}

#[test]
fn publishing_writes_a_polar_scan_readers_can_decrypt() {
    let shm = region("scan");
    publish_scan(
        &shm,
        &[
            point(0.0, 1000, 50),
            point(90.0, 2000, 7),
            point(180.0, 3000, 9),
        ],
    );

    let scan = shm.lidar_scan().snapshot(32).expect("coherent scan");
    assert_eq!(scan.seq, 2, "one publish advances the sequence once");
    assert_eq!(scan.point_count, 3);
    assert_eq!(scan.scan_start_ns, scan.scan_end_ns);

    assert!(scan.points[0].angle_rad.abs() < 1e-6);
    assert!((scan.points[0].distance_m - 1.0).abs() < 1e-6);
    assert_eq!(scan.points[0].intensity, 50);

    assert!((scan.points[1].angle_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert!((scan.points[1].distance_m - 2.0).abs() < 1e-6);
    assert_eq!(scan.points[1].intensity, 7);

    assert!(
        scan.points[3..]
            .iter()
            .all(|slot| slot.distance_m == 0.0 && slot.intensity == 0),
        "slots past the point count stay zeroed"
    );
}

#[test]
fn publishing_marks_the_returned_cells_of_the_occupancy_grid() {
    let shm = region("grid");
    publish_scan(
        &shm,
        &[
            point(0.0, 1000, 50),
            point(90.0, 1000, 7),
            point(180.0, 1000, 9),
            point(270.0, 1000, 11),
        ],
    );

    let grid = shm.lidar_grid().snapshot(32).expect("coherent grid");
    assert_eq!(grid.seq, 2);
    assert_eq!((grid.width, grid.height), (LIDAR_GRID_W as u32, LIDAR_GRID_H as u32));
    assert_eq!(grid.resolution_m, 0.05);
    assert!((grid.origin_x_m + 6.4).abs() < 1e-4, "{}", grid.origin_x_m);
    assert!((grid.origin_y_m + 6.4).abs() < 1e-4, "{}", grid.origin_y_m);

    let cells = occupied(&grid);
    assert_eq!(cells.len(), 4, "each valid return marks exactly one cell");
    assert!(cells.iter().all(|(_, value)| *value == 255));

    // Four unit returns on the axes balance around the grid centre; the exact
    // cell index depends on f32 rounding, so allow a cell of slack.
    let centre_row = LIDAR_GRID_H as f32 * 0.5;
    let centre_col = LIDAR_GRID_W as f32 * 0.5;
    let mean_row = cells.iter().map(|(index, _)| (index / LIDAR_GRID_W) as f32).sum::<f32>() / 4.0;
    let mean_col = cells.iter().map(|(index, _)| (index % LIDAR_GRID_W) as f32).sum::<f32>() / 4.0;
    assert!((mean_row - centre_row).abs() <= 1.5, "mean row {mean_row}");
    assert!((mean_col - centre_col).abs() <= 1.5, "mean col {mean_col}");
}

#[test]
fn returns_without_range_or_intensity_mark_no_cells() {
    let shm = region("skip");
    publish_scan(
        &shm,
        &[
            point(0.0, 0, 100),
            point(0.0, 5000, 0),
            point(90.0, 65_000, 100),
        ],
    );

    let scan = shm.lidar_scan().snapshot(32).expect("coherent scan");
    assert_eq!(scan.point_count, 3, "every device point is still published");

    let grid = shm.lidar_grid().snapshot(32).expect("coherent grid");
    assert_eq!(occupied(&grid).len(), 0);
}

#[test]
fn the_scan_clamps_to_the_shared_memory_point_capacity() {
    let shm = region("clamp");
    let points: Vec<DevicePoint> = (0..LIDAR_MAX_POINTS + 40)
        .map(|index| point((index % 360) as f32, 1000, 1))
        .collect();
    publish_scan(&shm, &points);

    let scan = shm.lidar_scan().snapshot(32).expect("coherent scan");
    assert_eq!(scan.point_count as usize, LIDAR_MAX_POINTS);
    assert!(scan.points.iter().all(|slot| slot.distance_m > 0.0));
}

#[test]
fn each_publish_advances_the_sequence_and_replaces_the_grid() {
    let shm = region("sequence");

    publish_scan(&shm, &[point(0.0, 1000, 9)]);
    let first_scan = shm.lidar_scan().snapshot(32).expect("first scan");
    let first_grid = shm.lidar_grid().snapshot(32).expect("first grid");
    assert_eq!(first_scan.seq, 2);
    assert_eq!(first_grid.seq, 2);

    publish_scan(&shm, &[point(90.0, 1000, 9)]);
    let second_scan = shm.lidar_scan().snapshot(32).expect("second scan");
    let second_grid = shm.lidar_grid().snapshot(32).expect("second grid");
    assert_eq!(second_scan.seq, 4);
    assert_eq!(second_grid.seq, 4);

    assert_eq!(occupied(&second_grid).len(), 1, "a fresh frame, not an accumulation");
    assert_ne!(
        occupied(&first_grid)[0].0,
        occupied(&second_grid)[0].0,
        "the second return lands on a different cell"
    );
}
