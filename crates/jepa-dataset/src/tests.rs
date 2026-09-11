//! Unit tests for the manifest builder, the materializer and the gates.

use super::*;
use qualia_mcap::McapSessionWriter;
use serde_json::json;
use std::path::Path;

/// Write a sealed segment with `frames` camera/lidar/pose triples and one
/// applied action spanning the whole window, then return its evidence entry.
fn sealed_session(
    root: &Path,
    session_id: &str,
    environment_id: &str,
    condition: &str,
    frames: u64,
) -> SessionEvidence {
    let mut writer = McapSessionWriter::create(root, session_id).unwrap();
    writer.register_required_topics().unwrap();
    let thumbnail = (0..CAMERA_PIXELS)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    for frame in 0..frames {
        let sequence = frame + 1;
        let timestamp = sequence * 1_000_000;
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
                    "luminance_mean": 0.4,
                    "luminance_stddev": 0.1,
                    "calibration_id": "guard-cal",
                    "thumb_width": THUMB_WIDTH,
                    "thumb_height": THUMB_HEIGHT,
                    "thumbnail_luma": thumbnail,
                }),
            )
            .unwrap();
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
                    "points": [
                        {"angle_rad": 0.0, "distance_m": 1.0, "intensity": 1},
                        {"angle_rad": 0.1, "distance_m": 2.0, "intensity": 1},
                        {"angle_rad": 0.2, "distance_m": 3.0, "intensity": 1},
                        {"angle_rad": 0.3, "distance_m": 0.0, "intensity": 0},
                    ],
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
                    "x_m": (sequence as f32 - 1.0) * 0.1,
                    "z_m": 0.0,
                    "yaw_rad": 0.0,
                    "confidence": 0.9,
                }),
            )
            .unwrap();
    }
    let end_ns = frames * 1_000_000;
    writer
        .write_json(
            TOPIC_ACTION_APPLIED,
            9_000,
            end_ns,
            end_ns,
            &json!({
                "schema_version": ACTION_SCHEMA,
                "producer_epoch": 6,
                "entity": "guard",
                "history_sequence": 1,
                "source_sequence": 500,
                "timestamp_ns": end_ns,
                "interval_start_ns": 1_000_000,
                "interval_end_ns": end_ns,
                "stage": ACTION_STAGE,
                "left": 0.1,
                "right": 0.2,
                "requested_left": 0.1,
                "requested_right": 0.2,
                "clamped_left": 0.1,
                "clamped_right": 0.2,
                "applied_left": 0.1,
                "applied_right": 0.2,
                "speed_scale": 1.0,
                "safety_flags": 0,
                "valid": true,
                "authority": ACTION_AUTHORITY_LEASH,
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

/// A segment that declares the required topics but carries no LiDAR at all.
fn camera_only_session(root: &Path, session_id: &str) -> SessionEvidence {
    let mut writer = McapSessionWriter::create(root, session_id).unwrap();
    writer.register_required_topics().unwrap();
    for sequence in 1..=2u64 {
        let timestamp = sequence * 1_000_000;
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
                    "luminance_mean": 0.4,
                    "luminance_stddev": 0.1,
                    "calibration_id": "guard-cal",
                }),
            )
            .unwrap();
    }
    let reference = writer.finish().unwrap();
    SessionEvidence {
        session_id: session_id.to_string(),
        environment_id: "room-1".to_string(),
        condition: "day".to_string(),
        primary_entity: "guard".to_string(),
        path: reference.path,
        sha256: reference.sha256,
    }
}

fn one_sample_manifest(root: &Path) -> DatasetManifest {
    let source = sealed_session(root, "session-a", "room-1", "day", 2);
    build_manifest(vec![source], DatasetConfig::default()).unwrap()
}

#[test]
fn manifest_round_trips_through_json_with_the_same_digest() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = one_sample_manifest(temp.path());
    assert_eq!(manifest.audit.candidate_transitions, 1);
    assert_eq!(manifest.audit.valid_transitions, 1);
    assert_eq!(manifest.schema_version, MANIFEST_SCHEMA);
    assert_eq!(manifest.digest.len(), 64);
    validate_dataset_manifest_integrity(&manifest).unwrap();

    let encoded = serde_json::to_vec(&manifest).unwrap();
    let decoded: DatasetManifest = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, manifest);
    assert_eq!(manifest_digest(&decoded).unwrap(), manifest.digest);

    let root = temp.path().join("manifests");
    let path = write_immutable_manifest(&root, &manifest).unwrap();
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(&manifest.digest));
    // A rerun with the same content resolves to the same immutable file.
    assert_eq!(path, write_immutable_manifest(&root, &manifest).unwrap());
}

#[test]
fn digest_changes_when_a_transition_changes() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = one_sample_manifest(temp.path());

    let mut moved_action = manifest.clone();
    moved_action.samples[0].action.left = 0.9;
    assert_ne!(manifest_digest(&moved_action).unwrap(), manifest.digest);

    let mut moved_split = manifest.clone();
    moved_split.samples[0].split = DatasetSplit::Test;
    assert_ne!(manifest_digest(&moved_split).unwrap(), manifest.digest);

    let mut stale = manifest.clone();
    stale.digest = "0".repeat(64);
    assert!(write_immutable_manifest(temp.path().join("stale"), &stale).is_err());
}

#[test]
fn session_missing_a_required_topic_cannot_be_promoted() {
    let temp = tempfile::tempdir().unwrap();
    let source = camera_only_session(temp.path(), "session-a");
    let manifest = build_manifest(vec![source], DatasetConfig::default()).unwrap();

    // The segment is honest about what it holds: no LiDAR means no transition,
    // and the rejection names the missing stream instead of guessing.
    assert!(manifest.samples.is_empty());
    assert_eq!(manifest.audit.valid_transitions, 0);
    assert_eq!(manifest.audit.sessions, 0);
    assert_eq!(manifest.audit.rejected["lidar_missing"], 1);

    // A diagnostic manifest still writes; promotion refuses it because the
    // source contributed nothing.
    write_immutable_manifest(temp.path().join("diagnostic"), &manifest).unwrap();
    let error = validate_dataset_promotion_gate(&manifest)
        .unwrap_err()
        .to_string();
    assert!(error.contains("contribute valid transitions"), "{error}");
}

#[test]
fn materializes_packed_observations_and_robot_centric_occupancy() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = one_sample_manifest(temp.path());
    let materialized = materialize_session(&manifest, "session-a").unwrap();

    assert_eq!(materialized.len(), 1);
    let transition = &materialized[0];
    assert_eq!(transition.observation.len(), OBS_DIM);
    assert_eq!(transition.target_observation.len(), OBS_DIM);
    assert_eq!(transition.observation_sequence, 1);
    assert_eq!(transition.target_sequence, 2);
    assert_eq!(transition.applied_action, [0.1, 0.2, 1.0, 1.0]);
    assert!((transition.delta_seconds - 1.0e-3).abs() < 1.0e-9);
    assert_eq!(
        transition.future_occupancy.occupied,
        [0.0, 1.0, 0.0, 0.0]
    );
    assert_eq!(
        transition.future_occupancy.observed,
        [true, true, false, false]
    );

    let grounding = transition
        .future_occupancy
        .robot_centric_grounding()
        .unwrap();
    assert_eq!(grounding.occupied.len(), GROUNDING_CELLS);
    assert_eq!(grounding.observed.len(), GROUNDING_CELLS);
    assert_eq!(grounding.occupied.iter().sum::<f32>(), 1.0);
    assert_eq!(grounding.observed.iter().sum::<f32>(), 2.0);

    // An unknown session is a named error, not an empty result.
    assert!(materialize_session(&manifest, "session-z").is_err());
}

#[test]
fn tampered_manifest_cannot_be_rematerialized() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = one_sample_manifest(temp.path());

    let mut forged_action = manifest.clone();
    forged_action.samples[0].action.left = 0.9;
    forged_action.digest = manifest_digest(&forged_action).unwrap();
    assert!(materialize_session(&forged_action, "session-a")
        .unwrap_err()
        .to_string()
        .contains("does not match MCAP evidence"));

    let mut forged_digest = manifest.clone();
    forged_digest.digest = "f".repeat(64);
    assert!(materialize_session(&forged_digest, "session-a")
        .unwrap_err()
        .to_string()
        .contains("digest does not match"));
}

#[test]
fn integrity_rejects_forged_audit_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = one_sample_manifest(temp.path());

    let mut forged_environments = manifest.clone();
    forged_environments.audit.environments = 7;
    assert!(validate_dataset_manifest_integrity(&forged_environments).is_err());

    let mut forged_split = manifest.clone();
    forged_split.samples[0].split = DatasetSplit::Test;
    assert!(validate_dataset_manifest_integrity(&forged_split).is_err());

    let mut forged_count = manifest.clone();
    forged_count.audit.valid_transitions = 2;
    assert!(validate_dataset_manifest_integrity(&forged_count).is_err());
}

#[test]
fn environment_splits_are_whole_and_deterministic() {
    let splits =
        assign_environment_splits(["room-a", "room-b", "room-c", "room-a", "room-d", "room-e"]);
    assert_eq!(splits.len(), 5);
    assert!(splits.values().any(|split| *split == DatasetSplit::Train));
    assert!(splits
        .values()
        .any(|split| *split == DatasetSplit::Validation));
    assert!(splits.values().any(|split| *split == DatasetSplit::Test));
    // Catalog order cannot change which room is held out.
    assert_eq!(
        splits,
        assign_environment_splits(["room-e", "room-d", "room-c", "room-b", "room-a"])
    );

    let undersized = assign_environment_splits(["room-a", "room-b"]);
    assert!(undersized
        .values()
        .all(|split| *split == DatasetSplit::Train));
}

#[test]
fn nearest_sensor_prefers_the_earlier_sample_on_a_tie() {
    let timestamps = [100_u64, 200, 300, 400];
    assert_eq!(nearest(&timestamps, 50, |value| *value), Some((&100, 50)));
    assert_eq!(nearest(&timestamps, 250, |value| *value), Some((&200, 50)));
    assert_eq!(nearest(&timestamps, 390, |value| *value), Some((&400, 10)));
    assert_eq!(nearest(&timestamps, 450, |value| *value), Some((&400, 50)));
    assert_eq!(nearest::<u64, _>(&[], 250, |value| *value), None);
}

#[test]
fn duplicate_or_corrupt_sources_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let source = sealed_session(temp.path(), "session-a", "room-1", "day", 2);

    let error = build_manifest(vec![source.clone(), source.clone()], DatasetConfig::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate dataset session id"), "{error}");

    let mut relabeled = source.clone();
    relabeled.session_id = "session-b".to_string();
    let error = build_manifest(vec![source.clone(), relabeled], DatasetConfig::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate MCAP evidence"), "{error}");

    let mut corrupt = source;
    corrupt.sha256 = "0".repeat(64);
    assert!(build_manifest(vec![corrupt], DatasetConfig::default()).is_err());
}
