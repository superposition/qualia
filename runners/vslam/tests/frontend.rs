//! Behavioural tests for the visual-SLAM frontend.
//!
//! Everything asserted here is either a byte the frontend publishes into the
//! shared arena or a pure decision function whose result those bytes are built
//! from: the sequence and confidence fields of `VslamFrontendState`, the pose
//! bytes handed to `WorldModel::robot_pose`, the deadline at which an
//! established pose writer yields, and the shift a pair of shifted frames must
//! recover. Nothing here inspects the frontend's internals, and each test owns
//! a uniquely named region so the suite is safe to run in parallel.

use std::sync::atomic::{AtomicU64, Ordering};

use qualia_shm::*;
use qualia_vslam::*;

/// Unique per test *and* per run, so a region left behind by a killed process
/// is never reattached and two tests never share bytes.
static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region(tag: &str) -> ShmRegion {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    let name = format!("/qualia_vslam_test_{}_{}_{}", std::process::id(), tag, index);
    ShmRegion::create(&name).expect("create region")
}

fn config(publish_pose: bool) -> FrontendConfig {
    FrontendConfig {
        shm_name: String::from("/unused"),
        poll_ms: 1,
        publish_pose,
        force_pose: false,
    }
}

/// A deterministic, aperiodic luma texture: two different offsets never match
/// as well as the offset that produced the second frame.
fn texture() -> [u8; CAMERA_THUMB_PIXELS] {
    let mut thumb = [0u8; CAMERA_THUMB_PIXELS];
    for y in 0..CAMERA_THUMB_H {
        for x in 0..CAMERA_THUMB_W {
            let h = (x as u32).wrapping_mul(2_654_435_761) ^ (y as u32).wrapping_mul(40_503);
            thumb[y * CAMERA_THUMB_W + x] = ((h >> 13) & 0xff) as u8;
        }
    }
    thumb
}

fn flat() -> [u8; CAMERA_THUMB_PIXELS] {
    [128u8; CAMERA_THUMB_PIXELS]
}

/// `src` translated by `(dx, dy)`, with out-of-frame pixels zeroed.
fn translated(src: &[u8], width: usize, height: usize, dx: isize, dy: isize) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let sx = x as isize - dx;
            let sy = y as isize - dy;
            if sx < 0 || sy < 0 || sx >= width as isize || sy >= height as isize {
                continue;
            }
            out[y * width + x] = src[sy as usize * width + sx as usize];
        }
    }
    out
}

fn publish_frame(shm: &ShmRegion, thumb: &[u8], timestamp_ns: u64) -> u64 {
    let mut thumbnail_luma = [0u8; CAMERA_THUMB_PIXELS];
    thumbnail_luma.copy_from_slice(thumb);
    let snapshot = CameraFrameSnapshot {
        seq: 0,
        timestamp_ns,
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

fn publish_floor(shm: &ShmRegion, cells: &[u8], seq: u64, updated_ns: u64) {
    let floor = shm.camera_floor_mut();
    floor.width = VOXEL_W as u32;
    floor.height = VOXEL_D as u32;
    floor.confidence_scale = 1.0;
    floor.last_update_ns = updated_ns;
    floor.cells.copy_from_slice(cells);
    floor.seq.store(seq, Ordering::Release);
}

fn external_pose(shm: &ShmRegion, x_m: f32, confidence: f32, timestamp_ns: u64) {
    shm.set_robot_pose(NavPose {
        x_m,
        y_m: 0.0,
        z_m: 0.0,
        yaw_rad: 0.0,
        pitch_rad: 0.0,
        roll_rad: 0.0,
        confidence,
        _pad0: 0.0,
        timestamp_ns,
    });
}

#[test]
fn first_frame_publishes_the_frontend_state() {
    let shm = region("first_frame");
    let thumb = texture();
    let seq = publish_frame(&shm, &thumb, 1_000_000);
    let mut frontend = Frontend::new(config(false));
    frontend.init_state(&shm);

    let report = frontend.tick(&shm).expect("a fresh frame is tracked");
    let state = shm.vslam_frontend();

    assert_eq!(state.seq.load(Ordering::Acquire), 1);
    assert_eq!(state.frame_seq, seq);
    assert_eq!(state.timestamp_ns, 1_000_000);
    assert_eq!(state.feature_count, report.feature_count);
    assert!(state.feature_count > 0, "a textured frame has corners");
    assert!(state.mean_gradient > 0.0);
    assert!(state.mean_cornerness > 0.0);
    assert_eq!(state.loop_closure_count, 0);
    assert_eq!(state.keyframe_count, 1);
    assert_eq!(state.tracking_confidence, report.tracking_confidence);
    assert_eq!(
        state.tracking_ok.load(Ordering::Acquire),
        state.tracking_confidence >= 0.55
    );
    // A first frame has no motion to integrate: the pose stays at the origin.
    assert_eq!(state.motion_dx_cells, 0.0);
    assert_eq!(state.motion_dz_cells, 0.0);
    assert_eq!(state.pose_x_m, 0.0);
    assert_eq!(state.pose_z_m, 0.0);
}

#[test]
fn repeated_and_invalid_frames_do_not_advance_the_state() {
    let shm = region("idle_frames");
    let mut frontend = Frontend::new(config(false));
    frontend.init_state(&shm);

    assert!(frontend.tick(&shm).is_none(), "no frame has been published");
    assert_eq!(shm.vslam_frontend().seq.load(Ordering::Acquire), 0);

    publish_frame(&shm, &texture(), 5);
    assert!(frontend.tick(&shm).is_some());
    assert_eq!(shm.vslam_frontend().seq.load(Ordering::Acquire), 1);

    // The same sequence must not be processed twice.
    assert!(frontend.tick(&shm).is_none());
    assert_eq!(shm.vslam_frontend().seq.load(Ordering::Acquire), 1);

    // Nor may a frame the camera marked invalid.
    shm.camera_frame_mut()
        .publish(&CameraFrameSnapshot {
            valid: false,
            ..CameraFrameSnapshot::default()
        })
        .expect("publish invalid frame");
    assert!(frontend.tick(&shm).is_none());
    assert_eq!(shm.vslam_frontend().seq.load(Ordering::Acquire), 1);
}

#[test]
fn publish_pose_flag_controls_the_pose_writer() {
    let shm = region("publish_flag");
    let mut frontend = Frontend::new(config(false));
    frontend.init_state(&shm);
    publish_frame(&shm, &texture(), 42);

    frontend.tick(&shm).expect("tracked");
    assert_eq!(shm.world_model().robot_pose.timestamp_ns, 0);
    assert_eq!(shm.vslam_frontend().pose_writer_active, 0);

    let shm2 = region("publish_flag_on");
    let mut enabled = Frontend::new(config(true));
    enabled.init_state(&shm2);
    let seq = publish_frame(&shm2, &texture(), 42);
    enabled.tick(&shm2).expect("tracked");
    assert_eq!(shm2.world_model().robot_pose.timestamp_ns, 42);
    assert_eq!(shm2.vslam_frontend().pose_writer_active, 1);
    assert_eq!(shm2.vslam_frontend().frame_seq, seq);
}

#[test]
fn published_pose_carries_the_frontend_pose() {
    let shm = region("pose_bytes");
    let first = texture();
    let second = translated(&first, CAMERA_THUMB_W, CAMERA_THUMB_H, 3, -2);
    let mut frontend = Frontend::new(config(true));
    frontend.init_state(&shm);

    publish_frame(&shm, &first, 1_000);
    frontend.tick(&shm).expect("first frame");
    let seq = publish_frame(&shm, &second, 2_000);
    let report = frontend.tick(&shm).expect("second frame");

    let state = shm.vslam_frontend();
    assert_eq!(state.frame_seq, seq);
    let pose = shm.world_model().robot_pose;
    assert_eq!(pose.timestamp_ns, 2_000);
    assert_eq!(pose.x_m, state.pose_x_m);
    assert_eq!(pose.z_m, state.pose_z_m);
    assert_eq!(pose.yaw_rad, state.yaw_rad);
    assert_eq!(pose.confidence, state.pose_confidence);
    assert_eq!((pose.y_m, pose.pitch_rad, pose.roll_rad), (0.0, 0.0, 0.0));
    assert!(pose.timestamp_ns == report.timestamp_ns);
    assert!(
        pose.x_m != 0.0 || pose.z_m != 0.0 || pose.yaw_rad != 0.0,
        "a translated thumbnail must move the published pose"
    );
}

#[test]
fn floor_translation_drives_the_published_motion() {
    let shm = region("floor_motion");
    let floor_a = texture()[..VOXEL_W * VOXEL_D].to_vec();
    let floor_b = translated(&floor_a, VOXEL_W, VOXEL_D, 2, 0);

    let mut frontend = Frontend::new(config(true));
    frontend.init_state(&shm);

    publish_floor(&shm, &floor_a, 2, 1_000);
    publish_frame(&shm, &texture(), 1_000);
    frontend.tick(&shm).expect("baseline frame");
    assert_eq!(shm.vslam_frontend().keyframe_count, 1);

    publish_floor(&shm, &floor_b, 4, 2_000);
    publish_frame(&shm, &texture(), 2_000);
    frontend.tick(&shm).expect("shifted frame");

    let state = shm.vslam_frontend();
    // The camera moving right by two floor cells is a body-frame motion of
    // -2 cells along X, and the shift was strong enough to open a keyframe.
    assert_eq!(state.motion_dx_cells, -2.0);
    assert_eq!(state.motion_dz_cells, 0.0);
    assert_eq!(state.keyframe_count, 2);
    assert!(shm.world_model().robot_pose.x_m != 0.0);
}

#[test]
fn shift_estimation_recovers_a_known_translation() {
    let prev = texture();
    let curr = translated(&prev, CAMERA_THUMB_W, CAMERA_THUMB_H, 3, 2);
    let match_ = estimate_shift(
        &prev,
        &curr,
        ImageGrid::THUMB,
        MatchRegion {
            x0: 6,
            x1: CAMERA_THUMB_W - 6,
            y0: 6,
            y1: CAMERA_THUMB_H - 6,
        },
        3,
        2,
    );
    assert_eq!((match_.dx, match_.dy), (3, 2));
    assert!(
        match_.score > 0.95,
        "an exact translation must score near one, got {}",
        match_.score
    );
    assert!(match_.samples > 64);
}

#[test]
fn shift_estimation_abstains_without_evidence() {
    let flat = flat();
    let found = estimate_shift(
        &flat,
        &flat,
        ImageGrid::THUMB,
        MatchRegion {
            x0: 4,
            x1: CAMERA_THUMB_W - 4,
            y0: 4,
            y1: CAMERA_THUMB_H - 4,
        },
        3,
        3,
    );
    assert_eq!((found.dx, found.dy, found.score), (0, 0, 0.0));
    assert!(found.samples > 64);

    // A region too small to hold a stable match reports no samples at all.
    let tiny = estimate_shift(
        &flat,
        &flat,
        ImageGrid::THUMB,
        MatchRegion {
            x0: 0,
            x1: 4,
            y0: 0,
            y1: 4,
        },
        1,
        1,
    );
    assert_eq!(tiny.samples, 0);
    assert_eq!(tiny.score, 0.0);
}

#[test]
fn yaw_delta_is_signed_by_opposed_half_regions() {
    let matching = |dx: isize, score: f32| ShiftMatch {
        dx,
        dy: 0,
        error: 0.1,
        score,
        samples: 200,
    };
    // Right half sliding left while the left half slides right is a turn to
    // the left of magnitude half the opposed displacement.
    let (yaw, score): (f32, f32) = estimate_yaw_delta(
        matching(3, 0.5),
        matching(-3, 0.5),
        matching(0, 0.0),
    );
    assert!((yaw - (-0.015)).abs() < 1e-6, "yaw was {yaw}");
    assert!((score - 0.5).abs() < 1e-6, "score was {score}");

    // Without samples in both halves the turn is unknown, not zero-ish noise.
    let none = ShiftMatch::default();
    assert_eq!(estimate_yaw_delta(none, none, none), (0.0, 0.0));
}

#[test]
fn integration_clamps_the_step_and_wraps_the_yaw() {
    let mut pose = LocalPose {
        x_m: 0.0,
        z_m: 0.0,
        yaw_rad: 0.0,
    };
    // A wildly over-large displacement may not teleport the pose.
    integrate_local_pose(&mut pose, 100.0, 0.0, 0.0, 1.0);
    assert_eq!(pose.x_m, MAX_STEP_M);
    assert_eq!(pose.z_m, 0.0);

    // Turning past the wrap point comes back as a negative angle.
    let mut turning = LocalPose {
        x_m: 0.0,
        z_m: 0.0,
        yaw_rad: std::f32::consts::PI - 0.05,
    };
    integrate_local_pose(&mut turning, 0.0, 0.0, MAX_YAW_STEP_RAD, 1.0);
    assert!(turning.yaw_rad < std::f32::consts::PI);
    assert!((turning.yaw_rad - (-std::f32::consts::PI + 0.15)).abs() < 1e-5);
    assert!(normalize_angle(10.0 * std::f32::consts::PI).abs() <= std::f32::consts::PI);
}

#[test]
fn pose_writer_yields_to_an_external_update() {
    let shm = region("writer_handoff");
    let pose = LocalPose {
        x_m: 1.0,
        z_m: 2.0,
        yaw_rad: 0.1,
    };
    let mut publisher = PosePublisher::new();
    assert!(publisher.publish_if_needed(&shm, pose, 1_000, 0.9, false));
    assert_eq!(shm.world_model().robot_pose.x_m, 1.0);

    external_pose(&shm, 7.5, 0.9, 2_000);
    assert!(
        !publisher.publish_if_needed(&shm, pose, 3_000, 0.9, false),
        "an external writer owns the pose now"
    );
    assert_eq!(shm.world_model().robot_pose.x_m, 7.5);
    assert!(!publisher.is_publishing());

    // After yielding the frontend may publish again once the pose goes stale.
    assert!(publisher.publish_if_needed(&shm, pose, 2_000 + VSLAM_POSE_STALE_NS + 1, 0.9, false));
    assert_eq!(shm.world_model().robot_pose.x_m, 1.0);
}

#[test]
fn stale_pose_is_republished_only_after_the_deadline() {
    let shm = region("pose_deadline");
    external_pose(&shm, 0.0, 0.9, 1_000_000_000);
    let pose = LocalPose {
        x_m: 0.5,
        z_m: 0.0,
        yaw_rad: 0.0,
    };
    let mut publisher = PosePublisher::new();

    assert!(!publisher.publish_if_needed(
        &shm,
        pose,
        1_000_000_000 + VSLAM_POSE_STALE_NS - 1,
        0.9,
        false
    ));
    assert!(!publisher.publish_if_needed(
        &shm,
        pose,
        1_000_000_000 + VSLAM_POSE_STALE_NS,
        0.9,
        false
    ));
    assert!(publisher.publish_if_needed(
        &shm,
        pose,
        1_000_000_000 + VSLAM_POSE_STALE_NS + 1,
        0.9,
        false
    ));
    assert_eq!(shm.world_model().robot_pose.x_m, 0.5);
    assert_eq!(
        shm.world_model().robot_pose.timestamp_ns,
        1_000_000_000 + VSLAM_POSE_STALE_NS + 1
    );
    assert_eq!(shm.world_model().robot_pose.confidence, 0.9);
}

#[test]
fn force_overrides_an_established_writer() {
    let shm = region("force_pose");
    external_pose(&shm, 9.0, 0.9, 5_000);
    let pose = LocalPose {
        x_m: -3.0,
        z_m: 0.0,
        yaw_rad: 0.0,
    };
    let mut publisher = PosePublisher::new();
    assert!(!publisher.publish_if_needed(&shm, pose, 6_000, 0.9, false));
    assert!(publisher.publish_if_needed(&shm, pose, 6_000, 0.9, true));
    assert_eq!(shm.world_model().robot_pose.x_m, -3.0);

    // A pose nobody has published yet is always fair game, deadline or not.
    let fresh = region("force_pose_fresh");
    assert!(publisher.publish_if_needed(&fresh, pose, 1, 0.2, false));
    assert_eq!(fresh.world_model().robot_pose.confidence, 0.2);
}

#[test]
fn tracking_confidence_rewards_texture_and_motion() {
    assert_eq!(tracking_confidence(0, 0.0, 0.0), 0.20);
    assert_eq!(tracking_confidence(900, 0.18, 0.12), 0.98);
    let flat = flat();
    let textured = texture();
    let still = TrackingStats::measure(Some(&textured), &textured);
    let moved = TrackingStats::measure(Some(&textured), &moved_frame());
    assert!(still.frame_delta < moved.frame_delta);
    assert!(still.confidence < moved.confidence);
    assert_eq!(TrackingStats::measure(None, &flat).frame_delta, 0.0);
    assert!(TrackingStats::measure(Some(&flat), &flat).feature_count == 0);
}

fn moved_frame() -> [u8; CAMERA_THUMB_PIXELS] {
    let mut thumb = texture();
    for (index, pixel) in thumb.iter_mut().enumerate() {
        *pixel = pixel.wrapping_add((index % 97) as u8);
    }
    thumb
}

#[test]
fn pose_confidence_bonuses_floor_evidence_and_saturates() {
    assert_eq!(pose_confidence(0.5, 0.0, 0.0, false), 0.5);
    assert!((pose_confidence(0.5, 0.0, 0.0, true) - 0.6).abs() < 1e-6);
    assert_eq!(pose_confidence(0.98, 1.0, 1.0, true), 0.98);
}

#[test]
fn config_reads_the_documented_environment_keys() {
    let defaults = FrontendConfig::from_lookup(|_| None);
    assert_eq!(defaults.shm_name, DEFAULT_SHM_NAME);
    assert_eq!(defaults.poll_ms, DEFAULT_POLL_MS);
    assert!(defaults.publish_pose);
    assert!(!defaults.force_pose);

    let overridden = FrontendConfig::from_lookup(|key| match key {
        "QUALIA_SHM_NAME" => Some(String::from("/qualia_test")),
        "QUALIA_VSLAM_POLL_MS" => Some(String::from("50")),
        "QUALIA_VSLAM_PUBLISH_POSE" => Some(String::from("0")),
        "QUALIA_VSLAM_FORCE_POSE" => Some(String::from("yes")),
        _ => None,
    });
    assert_eq!(overridden.shm_name, "/qualia_test");
    assert_eq!(overridden.poll_ms, 50);
    assert!(!overridden.publish_pose);
    assert!(overridden.force_pose);

    // An unparsable interval falls back to the default rather than to zero.
    let bad = FrontendConfig::from_lookup(|key| match key {
        "QUALIA_VSLAM_POLL_MS" => Some(String::from("not-a-number")),
        _ => None,
    });
    assert_eq!(bad.poll_ms, DEFAULT_POLL_MS);
}

#[test]
fn status_line_is_the_greppable_frame_record() {
    let report = FrameReport {
        frame_seq: 12,
        timestamp_ns: 9,
        feature_count: 300,
        keyframe_count: 4,
        tracking_confidence: 0.8121,
        mean_gradient: 0.2,
        mean_cornerness: 0.1,
        pose: LocalPose {
            x_m: 1.5,
            z_m: -2.25,
            yaw_rad: 0.5,
        },
        pose_confidence: 0.8769,
        motion_dx_cells: -3.0,
        motion_dz_cells: 1.5,
        motion_yaw_delta_rad: -0.02,
        pose_writer_active: true,
    };
    assert_eq!(
        report.log_line(),
        "qualia-vslam: frame_seq=12 track_conf=0.812 pose=(1.50,-2.25) yaw=0.50 \
pose_conf=0.877 floor_shift=(-3.0,1.5) yaw_delta=-0.020 features=300 keyframes=4 writer=1"
    );
}
