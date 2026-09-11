//! The `qualia-lidar-inspect` binary: a second process opens the region the
//! runner published into and prints the observable scan and grid summary.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use qualia_lidar::{publish_scan, DevicePoint};
use qualia_shm::ShmRegion;

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

#[test]
fn inspect_prints_the_published_scan_and_grid() {
    let name = format!(
        "/qualia_lidar_inspect_{}_probe_{}",
        std::process::id(),
        NEXT_REGION.fetch_add(1, Ordering::Relaxed)
    );
    let shm = ShmRegion::create(&name).expect("create region");
    publish_scan(
        &shm,
        &[
            DevicePoint {
                bearing_deg: 0.0,
                range_mm: 1000,
                signal: 50,
            },
            DevicePoint {
                bearing_deg: 90.0,
                range_mm: 2000,
                signal: 7,
            },
        ],
    );

    let output = Command::new(env!("CARGO_BIN_EXE_qualia-lidar-inspect"))
        .env("QUALIA_SHM_NAME", &name)
        .output()
        .expect("run qualia-lidar-inspect");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("qualia-lidar-inspect: scan_seq=2"), "{stdout}");
    assert!(
        stdout.contains("qualia-lidar-inspect: point_count=2 scan_start_ns="),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "qualia-lidar-inspect: sample_points=0.000rad:1.000m@50, 1.571rad:2.000m@7"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("qualia-lidar-inspect: grid_seq=2"), "{stdout}");
    assert!(
        stdout.contains("qualia-lidar-inspect: grid=256x256 resolution_m=0.05 occupied_cells=2"),
        "{stdout}"
    );

    drop(shm);
}
