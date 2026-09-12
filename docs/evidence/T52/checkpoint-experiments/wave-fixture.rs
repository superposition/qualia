//! Throwaway fixture writer for T52 step 1 (ticket #225).
//!
//! Two profiles, both built from the recipe the crate's own `sealed_session`
//! unit test uses (`crates/jepa-dataset/src/tests.rs`):
//!
//! * `static` — the T50 gate-minimum recipe verbatim: a constant thumbnail, a
//!   fixed 4-ray LiDAR sweep, a pose that advances 0.1 m per frame, and one
//!   constant applied action for the whole session.
//! * `wave` — the same envelope but observable: the camera carries a moving
//!   sinusoidal grating, LiDAR ranges and the pose oscillate in the same
//!   phase, and the per-session `speed_scale` sets that phase rate, so the
//!   transition is both non-degenerate (the no-change baseline is wrong) and
//!   action-conditioned (the action input carries the rate).
//!
//! Lives in scratch, never in the repository.

use qualia_jepa_dataset::SessionEvidence;
use qualia_mcap::{McapSessionWriter, TOPIC_ACTION_APPLIED, TOPIC_CAMERA, TOPIC_LIDAR, TOPIC_POSE};
use serde_json::json;
use std::path::{Path, PathBuf};

const CAMERA_PIXELS: usize = 64 * 48;
const THUMB_WIDTH: u64 = 64;
const THUMB_HEIGHT: u64 = 48;
const AUTHORITY_LEASH: u64 = 1;
const CONDITIONS: [&str; 3] = ["day", "dim", "motion"];
const TAU: f32 = std::f32::consts::TAU;

#[derive(Clone, Copy, PartialEq)]
enum Profile {
    Static,
    Wave,
}

fn static_thumbnail() -> Vec<u8> {
    (0..CAMERA_PIXELS).map(|index| (index % 251) as u8).collect()
}

fn wave_thumbnail(phase: f32) -> Vec<u8> {
    let mut luma = Vec::with_capacity(CAMERA_PIXELS);
    for y in 0..THUMB_HEIGHT as usize {
        for x in 0..THUMB_WIDTH as usize {
            let xf = x as f32;
            let yf = y as f32;
            let value = 0.5
                + 0.30 * (TAU * (xf / 8.0 + phase)).sin() * (TAU * (yf / 16.0)).cos()
                + 0.10 * (TAU * (yf / 12.0 - phase * 0.5)).cos();
            let clamped = value.clamp(0.05, 0.95);
            luma.push((clamped * 255.0).round() as u8);
        }
    }
    luma
}

fn sealed_session(
    root: &Path,
    session_id: &str,
    environment_id: &str,
    condition: &str,
    frames: u64,
    profile: Profile,
    speed: f32,
    left: f32,
    right: f32,
    chain: u64,
    phase_step: f32,
) -> SessionEvidence {
    let mut writer = McapSessionWriter::create(root, session_id).unwrap();
    writer.register_required_topics().unwrap();
    let static_luma = static_thumbnail();
    let mut declared_mean = 0.4_f32;
    let mut declared_stddev = 0.1_f32;
    let mut clock = 0u64;
    for frame in 0..frames {
        // A chain-break: jump the clock past `max_frame_gap_ns` so the pair
        // that spans the gap is rejected and the free-running rollout restarts
        // on the next transition. Chain length becomes `chain`, not `frames`.
        if chain > 0 && frame > 0 && frame % chain == 0 {
            clock += 600_000_000;
        }
        clock += 1_000_000;
        let sequence = frame + 1;
        let timestamp = clock;
        let phase = frame as f32 * phase_step * speed;
        let luma = match profile {
            Profile::Static => static_luma.clone(),
            Profile::Wave => {
                declared_mean = 0.5;
                declared_stddev = 0.2;
                wave_thumbnail(phase)
            }
        };
        writer
            .write_json(
                TOPIC_CAMERA,
                sequence as u32,
                timestamp,
                timestamp,
                &json!({
                    "schema_version": "qualia.camera-evidence.v1",
                    "producer_epoch": 3,
                    "entity": "guard",
                    "source_sequence": sequence,
                    "timestamp_ns": timestamp,
                    "valid": true,
                    "luminance_mean": declared_mean,
                    "luminance_stddev": declared_stddev,
                    "calibration_id": "guard-cal",
                    "thumb_width": THUMB_WIDTH,
                    "thumb_height": THUMB_HEIGHT,
                    "thumbnail_luma": luma,
                }),
            )
            .unwrap();
        let points = match profile {
            Profile::Static => vec![
                json!({"angle_rad": 0.0, "distance_m": 1.0, "intensity": 1}),
                json!({"angle_rad": 0.1, "distance_m": 2.0, "intensity": 1}),
                json!({"angle_rad": 0.2, "distance_m": 3.0, "intensity": 1}),
                json!({"angle_rad": 0.3, "distance_m": 0.0, "intensity": 0}),
            ],
            Profile::Wave => {
                let wobble = 0.5 + 0.5 * (TAU * phase).sin();
                vec![
                    json!({"angle_rad": 0.0, "distance_m": 1.0 + 2.0 * wobble, "intensity": 1}),
                    json!({"angle_rad": 0.1, "distance_m": 3.5 - 1.5 * wobble, "intensity": 1}),
                    json!({"angle_rad": 0.2, "distance_m": 3.0 - 1.0 * wobble, "intensity": 1}),
                    json!({"angle_rad": 0.3, "distance_m": 0.0, "intensity": 0}),
                ]
            }
        };
        writer
            .write_json(
                TOPIC_LIDAR,
                (1_000 + sequence) as u32,
                timestamp,
                timestamp,
                &json!({
                    "schema_version": "qualia.lidar-evidence.v1",
                    "producer_epoch": 4,
                    "entity": "guard",
                    "source_sequence": 1_000 + sequence,
                    "timestamp_ns": timestamp,
                    "points": points,
                    "validity_mask": [true, true, true, false],
                    "occupancy": {
                        "width": 2,
                        "height": 2,
                        "resolution_m": 0.05,
                        "origin_x_m": -0.05,
                        "origin_y_m": -0.05,
                        "cells": [0, 255, 0, 0],
                        "observed": [true, true, false, false],
                    },
                }),
            )
            .unwrap();
        let pose = match profile {
            Profile::Static => json!({
                "x_m": (sequence as f32 - 1.0) * 0.1,
                "z_m": 0.0,
                "yaw_rad": 0.0,
            }),
            Profile::Wave => json!({
                "x_m": 0.25 * (TAU * phase).sin(),
                "z_m": 0.25 * (TAU * phase * 0.5).cos(),
                "yaw_rad": 0.2 * (TAU * phase).sin(),
            }),
        };
        writer
            .write_json(
                TOPIC_POSE,
                (2_000 + sequence) as u32,
                timestamp,
                timestamp,
                &json!({
                    "schema_version": "qualia.pose-evidence.v1",
                    "producer_epoch": 5,
                    "entity": "guard",
                    "source_sequence": 2_000 + sequence,
                    "timestamp_ns": timestamp,
                    "x_m": pose["x_m"],
                    "z_m": pose["z_m"],
                    "yaw_rad": pose["yaw_rad"],
                    "confidence": 0.9,
                }),
            )
            .unwrap();
    }
    let end_ns = clock;
    writer
        .write_json(
            TOPIC_ACTION_APPLIED,
            9_000,
            end_ns,
            end_ns,
            &json!({
                "schema_version": "qualia.action-evidence.v1",
                "producer_epoch": 6,
                "entity": "guard",
                "history_sequence": 1,
                "source_sequence": 500,
                "timestamp_ns": end_ns,
                "interval_start_ns": 1_000_000,
                "interval_end_ns": end_ns,
                "stage": "transport_accepted",
                "left": left,
                "right": right,
                "requested_left": left,
                "requested_right": right,
                "clamped_left": left,
                "clamped_right": right,
                "applied_left": left,
                "applied_right": right,
                "speed_scale": speed,
                "safety_flags": 0,
                "valid": true,
                "authority": AUTHORITY_LEASH,
                "armed": true,
                "deadman_active": true,
                "collision_clamped": false,
            }),
        )
        .unwrap();
    let reference = writer.finish().unwrap();
    SessionEvidence {
        session_id: session_id.to_string(),
        environment_id: environment_id.to_string(),
        condition: condition.to_string(),
        primary_entity: "guard".to_string(),
        path: reference.path,
        sha256: reference.sha256,
    }
}

fn main() {
    let mut argv = std::env::args().skip(1);
    let mut root = PathBuf::from("/mnt/c/tmp/Impl225PromotedE2E/fixture/data");
    let mut environments = 12usize;
    let mut sessions_per_environment = 66usize;
    let mut frames = 65u64;
    let mut profile = Profile::Wave;
    let mut chain = 0u64;
    let mut phase_step = 0.07_f32;
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().expect("flag needs a value");
        match flag.as_str() {
            "--root" => root = PathBuf::from(value()),
            "--envs" => environments = value().parse().unwrap(),
            "--sessions" => sessions_per_environment = value().parse().unwrap(),
            "--frames" => frames = value().parse().unwrap(),
            "--chain" => chain = value().parse().unwrap(),
            "--phase-step" => phase_step = value().parse().unwrap(),
            "--profile" => {
                profile = match value().as_str() {
                    "static" => Profile::Static,
                    "wave" => Profile::Wave,
                    other => panic!("unknown profile {other}"),
                }
            }
            other => panic!("unknown flag {other}"),
        }
    }
    let session_root = root.join("sessions");
    std::fs::create_dir_all(&session_root).unwrap();
    let mut entries = Vec::new();
    for environment in 0..environments {
        let environment_id = format!("room-{environment:02}");
        for session in 0..sessions_per_environment {
            let session_id = format!("session-{environment}-{session}");
            let condition = CONDITIONS[(environment + session) % CONDITIONS.len()];
            let speed = if (environment + session) % 2 == 0 { 0.5 } else { 1.0 };
            let left = [-0.4_f32, 0.0, 0.4][(environment * 7 + session) % 3];
            let right = [-0.4_f32, 0.0, 0.4][(environment * 5 + session) % 3];
            entries.push(sealed_session(
                &session_root,
                &session_id,
                &environment_id,
                condition,
                frames,
                profile,
                speed,
                left,
                right,
                chain,
                phase_step,
            ));
        }
    }
    let catalog = json!({ "sources": entries });
    let catalog_path = root.join("catalog.json");
    std::fs::write(&catalog_path, serde_json::to_vec_pretty(&catalog).unwrap()).unwrap();
    println!("{}", catalog_path.display());
}
