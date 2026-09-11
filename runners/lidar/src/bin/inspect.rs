//! `qualia-lidar-inspect`: prints the LiDAR frames currently in shared memory.
//!
//! Opens the arena named by `QUALIA_SHM_NAME` (default `/qualia_body`), takes
//! one coherent snapshot of the polar scan and one of the occupancy grid, and
//! prints both. It is a read-only probe: it never writes the arena.

use qualia_shm::ShmRegion;

fn main() {
    let shm_name = std::env::var("QUALIA_SHM_NAME").unwrap_or_else(|_| "/qualia_body".to_string());
    let shm = ShmRegion::open(&shm_name).unwrap_or_else(|error| {
        panic!("qualia-lidar-inspect: failed to open shm '{shm_name}': {error}");
    });

    let scan = shm.lidar_scan().snapshot(32).expect("coherent LiDAR scan");
    let grid = shm.lidar_grid().snapshot(32).expect("coherent LiDAR grid");

    let sample = scan
        .points
        .iter()
        .take((scan.point_count as usize).min(8))
        .map(|point| {
            format!(
                "{:.3}rad:{:.3}m@{}",
                point.angle_rad, point.distance_m, point.intensity
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let occupied = grid.cells.iter().filter(|&&cell| cell != 0).count();

    println!("qualia-lidar-inspect: scan_seq={}", scan.seq);
    println!(
        "qualia-lidar-inspect: point_count={} scan_start_ns={} scan_end_ns={}",
        scan.point_count, scan.scan_start_ns, scan.scan_end_ns
    );
    println!("qualia-lidar-inspect: sample_points={sample}");
    println!("qualia-lidar-inspect: grid_seq={}", grid.seq);
    println!(
        "qualia-lidar-inspect: grid={}x{} resolution_m={} occupied_cells={}",
        grid.width, grid.height, grid.resolution_m, occupied
    );
}
