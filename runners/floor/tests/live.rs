//! End-to-end smoke test for the `qualia-floor` binary.
//!
//! The unit tests in `main.rs` cover the derivation and the published bytes.
//! This file runs the real process against a real arena, because the loop's
//! contract is only observable from outside: a new camera frame becomes exactly
//! one published grid, a frame the runner already consumed is never derived
//! twice, and an unreachable arena is a fatal line with a non-zero exit.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use qualia_shm::ShmRegion;
use qualia_types::{
    CameraFrameSnapshot, CAMERA_THUMB_H, CAMERA_THUMB_PIXELS, CAMERA_THUMB_W, VOXEL_D, VOXEL_W,
};

/// Unique per test and per run, so a region a killed run left behind is never
/// attached to by accident.
static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region_name(tag: &str) -> String {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    format!("/qualia_floor_live_{}_{}_{}", std::process::id(), tag, index)
}

/// The spawned runner, stopped and reaped when the test leaves its scope.
struct Runner(Child);

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn frame(luma: u8, timestamp_ns: u64) -> CameraFrameSnapshot {
    CameraFrameSnapshot {
        seq: 0,
        timestamp_ns,
        source_width: 640,
        source_height: 480,
        thumb_width: CAMERA_THUMB_W as u32,
        thumb_height: CAMERA_THUMB_H as u32,
        luminance_mean: f32::from(luma),
        luminance_stddev: 0.0,
        valid: true,
        thumbnail_luma: [luma; CAMERA_THUMB_PIXELS],
    }
}

/// Wait until the published grid sequence reaches `target`, then return it.
fn wait_for_sequence(region: &ShmRegion, target: u64, timeout: Duration) -> u64 {
    let deadline = Instant::now() + timeout;
    loop {
        let published = region.camera_floor().seq.load(Ordering::Acquire);
        if published >= target {
            return published;
        }
        assert!(
            Instant::now() < deadline,
            "the runner never published grid {target} (last was {published})"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn each_new_camera_frame_is_derived_exactly_once() {
    let name = region_name("derive");
    let region = ShmRegion::create(&name).expect("create");

    let _runner = Runner(
        Command::new(env!("CARGO_BIN_EXE_qualia-floor"))
            .env("QUALIA_SHM_NAME", &name)
            .env("QUALIA_FLOOR_POLL_MS", "5")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the floor runner"),
    );

    // A bright frame saturates the near rows and stamps its timestamp.
    region
        .camera_frame_mut()
        .publish(&frame(255, 4242))
        .expect("publish the first frame");
    assert_eq!(wait_for_sequence(&region, 1, Duration::from_secs(10)), 1);
    {
        let grid = region.camera_floor();
        assert_eq!(grid.width, VOXEL_W as u32);
        assert_eq!(grid.height, VOXEL_D as u32);
        assert_eq!(grid.last_update_ns, 4242);
        assert_eq!(grid.cells[0], 255);
    }

    // The same frame must not be derived a second time.
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(
        region.camera_floor().seq.load(Ordering::Acquire),
        1,
        "an already consumed frame was derived again"
    );

    // A new sequence is derived and stamped with its own timestamp.
    region
        .camera_frame_mut()
        .publish(&frame(0, 9000))
        .expect("publish the second frame");
    assert_eq!(wait_for_sequence(&region, 2, Duration::from_secs(10)), 2);
    let grid = region.camera_floor();
    assert_eq!(grid.last_update_ns, 9000);
    assert_eq!(grid.cells[0], 63);
}

#[test]
fn an_absent_arena_is_a_fatal_line_and_a_non_zero_exit() {
    let name = region_name("absent");
    let output = Command::new(env!("CARGO_BIN_EXE_qualia-floor"))
        .env("QUALIA_SHM_NAME", &name)
        .output()
        .expect("run the floor runner");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("qualia-floor: failed to open shm '{name}'")),
        "unexpected stderr: {stderr}"
    );
}
