//! Process-level smoke test: the shipped binary, not the library, must attach
//! to an existing arena and publish a tracked frame.
//!
//! The library tests cover the front end's decisions; this one covers the
//! artefact the supervisor actually spawns. It creates the region itself, runs
//! the binary with the documented environment keys, publishes camera frames
//! until the front end reports one, and then reads the child's own log. It
//! never touches the loop's internals and leaves no mapping behind.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use qualia_shm::*;

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn publish_frame(shm: &ShmRegion, index: u64) -> u64 {
    let mut thumbnail_luma = [0u8; CAMERA_THUMB_PIXELS];
    for (pixel_index, pixel) in thumbnail_luma.iter_mut().enumerate() {
        let h = (pixel_index as u32)
            .wrapping_mul(2_654_435_761)
            .wrapping_add((index as u32).wrapping_mul(2_246_822_519));
        *pixel = ((h >> 11) & 0xff) as u8;
    }
    let snapshot = CameraFrameSnapshot {
        seq: index,
        timestamp_ns: index * 1_000_000,
        source_width: 640,
        source_height: 480,
        thumb_width: CAMERA_THUMB_W as u32,
        thumb_height: CAMERA_THUMB_H as u32,
        luminance_mean: 0.5,
        luminance_stddev: 0.2,
        valid: true,
        thumbnail_luma,
    };
    shm.camera_frame_mut()
        .publish(&snapshot)
        .expect("publish camera frame")
}

#[test]
fn binary_attaches_and_publishes_a_tracked_frame() {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    let name = format!("/qualia_vslam_binary_{}_{}", std::process::id(), index);
    let shm = ShmRegion::create(&name).expect("create region");

    let mut child = Command::new(env!("CARGO_BIN_EXE_qualia-vslam"))
        .env("QUALIA_SHM_NAME", &name)
        .env("QUALIA_VSLAM_POLL_MS", "5")
        .env("QUALIA_VSLAM_PUBLISH_POSE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn qualia-vslam");

    // A fresh frame every round removes the start-up race: whenever the child
    // attaches, the next published frame is new to it.
    let mut published = 0u64;
    let mut frames_seen = 0u64;
    for round in 1..=400u64 {
        published = publish_frame(&shm, round);
        std::thread::sleep(Duration::from_millis(5));
        frames_seen = shm.vslam_frontend().seq.load(Ordering::Acquire);
        if frames_seen >= 1 {
            break;
        }
    }
    let pose_timestamp = shm.world_model().robot_pose.timestamp_ns;
    let frame_seq = shm.vslam_frontend().frame_seq;

    let _ = child.kill();
    let output = child.wait_with_output().expect("collect child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        frames_seen >= 1,
        "the binary never published a front-end frame; stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(frame_seq, published, "the child tracked the newest frame");
    assert_ne!(pose_timestamp, 0, "the pose-writer path published a pose");
    assert!(
        stdout.contains("qualia-vslam: consuming camera thumbnails from shm="),
        "the operator banner is missing from {stdout:?}"
    );
    assert!(
        stdout.contains("qualia-vslam: frame_seq="),
        "the frame record is missing from {stdout:?}"
    );
}
