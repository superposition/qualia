//! End-to-end tests for the `qualia-jepa-dataset` command line.
//!
//! These drive the real binary against catalogs of sealed MCAP segments, so
//! the arguments, the stdout contract and the exit codes are exercised exactly
//! as an operator or a supervisor would see them.

use qualia_mcap::{McapSessionWriter, TOPIC_ACTION_APPLIED, TOPIC_CAMERA, TOPIC_LIDAR, TOPIC_POSE};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_qualia-jepa-dataset");

/// Quote a value as a JSON string, since the test builds catalogs by hand.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Seal one segment with `frames` camera/LiDAR/pose triples and one applied
/// action spanning the window, and return its catalog entry as JSON.
///
/// The payloads carry only what the builder reads: a build-only catalog may
/// legitimately omit the camera thumbnail, which materialization would need
/// but which the promotion gate does not.
fn sealed_catalog_entry(
    root: &Path,
    index: usize,
    condition: &str,
    frames: u64,
) -> String {
    let session_id = format!("session-{index}");
    let mut writer = McapSessionWriter::create(root, &session_id).unwrap();
    writer.register_required_topics().unwrap();
    for frame in 0..frames {
        let sequence = frame + 1;
        let timestamp = sequence;
        let camera = format!(
            "{{\"schema_version\":\"qualia.camera-evidence.v1\",\"producer_epoch\":3,\
             \"entity\":\"guard\",\"source_sequence\":{sequence},\"timestamp_ns\":{timestamp},\
             \"valid\":true,\"luminance_mean\":0.4,\"luminance_stddev\":0.1,\
             \"calibration_id\":\"guard-cal\"}}"
        );
        writer
            .write_bytes(
                TOPIC_CAMERA,
                sequence as u32,
                timestamp,
                timestamp,
                camera.into_bytes(),
            )
            .unwrap();
        let lidar = format!(
            "{{\"schema_version\":\"qualia.lidar-evidence.v1\",\"producer_epoch\":4,\
             \"entity\":\"guard\",\"source_sequence\":{sequence},\"timestamp_ns\":{timestamp},\
             \"validity_mask\":[true,true,true,false],\"occupancy\":{{\"width\":2,\"height\":2,\
             \"resolution_m\":0.05,\"origin_x_m\":-0.05,\"origin_y_m\":-0.05,\
             \"cells\":[0,255,0,0],\"observed\":[true,true,false,false]}}}}"
        );
        writer
            .write_bytes(
                TOPIC_LIDAR,
                sequence as u32,
                timestamp,
                timestamp,
                lidar.into_bytes(),
            )
            .unwrap();
        let pose = format!(
            "{{\"schema_version\":\"qualia.pose-evidence.v1\",\"producer_epoch\":5,\
             \"entity\":\"guard\",\"source_sequence\":{sequence},\"timestamp_ns\":{timestamp},\
             \"x_m\":0.0,\"z_m\":0.0,\"yaw_rad\":0.0,\"confidence\":0.9}}"
        );
        writer
            .write_bytes(
                TOPIC_POSE,
                sequence as u32,
                timestamp,
                timestamp,
                pose.into_bytes(),
            )
            .unwrap();
    }
    let action = format!(
        "{{\"schema_version\":\"qualia.action-evidence.v1\",\"producer_epoch\":6,\
         \"entity\":\"guard\",\"history_sequence\":1,\"source_sequence\":500,\
         \"timestamp_ns\":{frames},\"interval_start_ns\":1,\"interval_end_ns\":{frames},\
         \"stage\":\"transport_accepted\",\"left\":0.1,\"right\":0.2,\"requested_left\":0.1,\
         \"requested_right\":0.2,\"clamped_left\":0.1,\"clamped_right\":0.2,\"applied_left\":0.1,\
         \"applied_right\":0.2,\"speed_scale\":1.0,\"safety_flags\":0,\"valid\":true,\
         \"authority\":1,\"armed\":true,\"deadman_active\":true,\"collision_clamped\":false}}"
    );
    writer
        .write_bytes(
            TOPIC_ACTION_APPLIED,
            9_000,
            frames,
            frames,
            action.into_bytes(),
        )
        .unwrap();
    let reference = writer.finish().unwrap();
    format!(
        "{{\"session_id\":{},\"environment_id\":{},\"condition\":{},\"primary_entity\":\"guard\",\
         \"path\":{},\"sha256\":{}}}",
        json_string(&session_id),
        json_string(&format!("room-{index}")),
        json_string(condition),
        json_string(&reference.path),
        json_string(&reference.sha256),
    )
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_prints_usage_and_exits_zero() {
    let output = run_cli(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("usage: qualia-jepa-dataset"), "{stdout}");
}

#[test]
fn bad_input_paths_fail_non_zero() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("datasets");

    let missing = temp.path().join("absent.json");
    let output = run_cli(&[
        "--catalog",
        missing.to_str().unwrap(),
        "--output-dir",
        output_dir.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr_of(&output).contains("qualia-jepa-dataset:"));

    // A catalog that names a segment which is not there must not be mistaken
    // for an empty dataset.
    let catalog = temp.path().join("catalog.json");
    fs::write(
        &catalog,
        format!(
            "{{\"sources\":[{{\"session_id\":\"session-a\",\"environment_id\":\"room-1\",\
             \"condition\":\"day\",\"primary_entity\":\"guard\",\"path\":{},\
             \"sha256\":\"{}\"}}]}}",
            json_string(temp.path().join("missing.mcap").to_str().unwrap()),
            "0".repeat(64),
        ),
    )
    .unwrap();
    let output = run_cli(&[
        "--catalog",
        catalog.to_str().unwrap(),
        "--output-dir",
        output_dir.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!output_dir.exists());

    // An unknown argument is a usage error, not a silent default.
    let output = run_cli(&["--nonsense"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn writes_manifest_and_exits_zero_when_the_evidence_gate_passes() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let conditions = ["day", "dim", "motion"];

    // Twelve sessions, four per condition and one per environment, each with
    // enough transitions that every environment clears the held-out floor.
    let mut entries = Vec::new();
    for index in 0..12 {
        entries.push(sealed_catalog_entry(
            &sessions,
            index,
            conditions[index % conditions.len()],
            4_168,
        ));
    }
    let catalog = temp.path().join("catalog.json");
    fs::write(
        &catalog,
        format!("{{\"sources\":[{}]}}", entries.join(",")),
    )
    .unwrap();
    let output_dir = temp.path().join("datasets");

    let output = run_cli(&[
        "--catalog",
        catalog.to_str().unwrap(),
        "--output-dir",
        output_dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        stderr_of(&output)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    let manifest_path = Path::new(lines.next().unwrap());
    assert!(manifest_path.exists(), "{}", manifest_path.display());
    let summary = lines.next().unwrap();
    assert!(summary.starts_with("valid=50004"), "{summary}");
    assert!(summary.contains("candidates=50004"), "{summary}");
    assert!(summary.contains("sessions=12"), "{summary}");
    assert!(summary.contains("environments=12"), "{summary}");
    assert!(summary.contains("conditions=3"), "{summary}");

    let file_name = manifest_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(file_name.starts_with("jepa-dataset-"), "{file_name}");
    assert!(file_name.ends_with(".json"), "{file_name}");
    let digest = file_name
        .trim_start_matches("jepa-dataset-")
        .trim_end_matches(".json");
    assert_eq!(digest.len(), 64);
    assert!(digest.chars().all(|ch| ch.is_ascii_hexdigit()));
    assert_eq!(fs::read_dir(&output_dir).unwrap().count(), 1);
}
