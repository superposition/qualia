//! End-to-end smoke test: the real binary opens a shared-memory region, reads
//! scans another process publishes into it, and writes a pose back.
//!
//! The unit tests exercise the estimator and the `NavPose` layout directly;
//! this test covers the wiring the supervisor relies on — a region made by one
//! process, `qualia-pose` attaching by name, and the published bytes landing in
//! the world model the rest of the stack reads.

use qualia_shm::ShmRegion;
use qualia_types::{LidarPoint, LidarScanSnapshot, NavPose, LIDAR_MAX_POINTS};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Deterministic scattered cloud: same generator as the unit tests, so the two
/// agree on what a registerable scan looks like.
fn scattered_cloud(count: usize, half_extent: f32) -> Vec<(f32, f32)> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
    };
    (0..count)
        .map(|_| (next() * half_extent, next() * half_extent))
        .collect()
}

fn snapshot_of(cloud: &[(f32, f32)], start_ns: u64, end_ns: u64) -> LidarScanSnapshot {
    let mut snapshot = LidarScanSnapshot::default();
    snapshot.scan_start_ns = start_ns;
    snapshot.scan_end_ns = end_ns;
    snapshot.point_count = cloud.len().min(LIDAR_MAX_POINTS) as u32;
    for (slot, (x, y)) in snapshot.points.iter_mut().zip(cloud.iter()) {
        *slot = LidarPoint {
            angle_rad: y.atan2(*x),
            distance_m: x.hypot(*y),
            intensity: 200,
            _pad: [0; 3],
        };
    }
    snapshot
}

fn scratch_region() -> (ShmRegion, String) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = format!("/qualia_pose_smoke_{}_{}", std::process::id(), serial);
    let region = ShmRegion::create(&name).expect("create smoke shm region");
    (region, name)
}

fn spawn_runner(region_name: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_qualia-pose"))
        .env("QUALIA_SHM_NAME", region_name)
        .env("QUALIA_POSE_POLL_MS", "5")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn qualia-pose")
}

fn wait_for_pose(region: &ShmRegion, timestamp_ns: u64, timeout: Duration) -> NavPose {
    let deadline = Instant::now() + timeout;
    loop {
        let pose = region.world_model().robot_pose;
        if pose.timestamp_ns == timestamp_ns {
            return pose;
        }
        assert!(
            Instant::now() <= deadline,
            "timed out waiting for a pose stamped {timestamp_ns}; the runner last published {}",
            pose.timestamp_ns
        );
        sleep(Duration::from_millis(10));
    }
}

#[test]
fn runner_reads_published_scans_and_writes_the_pose_back() {
    let (region, region_name) = scratch_region();
    let mut child = spawn_runner(&region_name);

    let cloud = scattered_cloud(200, 4.0);
    let shifted: Vec<(f32, f32)> = cloud.iter().map(|(x, y)| (x - 0.15, *y)).collect();

    region
        .lidar_scan_mut()
        .publish(&snapshot_of(&cloud, 1_000_000, 1_500_000))
        .expect("publish first scan");

    // The first accepted scan fixes the graph origin.
    let initial = wait_for_pose(&region, 1_500_000, Duration::from_secs(10));
    assert_eq!(initial.x_m, 0.0);
    assert_eq!(initial.z_m, 0.0);
    assert!(initial.confidence > 0.0);

    region
        .lidar_scan_mut()
        .publish(&snapshot_of(&shifted, 2_000_000, 2_500_000))
        .expect("publish second scan");

    // A 15 cm sensor displacement must advance the published pose by ~15 cm.
    let advanced = wait_for_pose(&region, 2_500_000, Duration::from_secs(10));
    assert!(
        (advanced.x_m - 0.15).abs() <= 0.06,
        "expected the published x_m near 0.15, got {}",
        advanced.x_m
    );
    assert!((advanced.z_m).abs() <= 0.06);
    assert_eq!(advanced.y_m, 0.0);

    child.kill().expect("stop the runner");
    let _ = child.wait();
}
