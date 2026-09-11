//! End-to-end tests for the `qualia-mcap-inspect` binary.
//!
//! The inspector is the operator's lens on a sealed segment: it reports per
//! stream timing and it can audit that an applied-action history is really
//! zero. Both outputs are contracts a human reads, so they are exercised by
//! running the built binary against a segment sealed by the library rather
//! than by calling helper functions directly.

use qualia_mcap::{McapSessionWriter, TOPIC_ACTION_APPLIED, TOPIC_CAMERA};
use serde_json::{json, Value};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_qualia-mcap-inspect");

fn applied_event(entity: &str, history_sequence: u64, applied: f64) -> Value {
    let timestamp_ns = 1_000 + history_sequence;
    json!({
        "schema_version": "qualia.action-evidence.v1",
        "producer_epoch": 1,
        "entity": entity,
        "history_sequence": history_sequence,
        "source_sequence": history_sequence,
        "timestamp_ns": timestamp_ns,
        "interval_start_ns": timestamp_ns - 1,
        "interval_end_ns": timestamp_ns,
        "stage": "transport_accepted",
        "left": applied,
        "right": 0.0,
        "applied_left": applied,
        "applied_right": 0.0,
        "speed_scale": 1.0,
        "authority": "leash",
        "valid": true
    })
}

fn seal(dir: &std::path::Path, session: &str, events: &[(&str, u64, Value)]) -> String {
    let mut writer = McapSessionWriter::create(dir, session).unwrap();
    for (topic, log_time_ns, value) in events {
        writer
            .write_json(topic, *log_time_ns as u32, *log_time_ns, *log_time_ns + 1, value)
            .unwrap();
    }
    writer.finish().unwrap().path
}

fn run(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(BIN).args(args).output().unwrap();
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn inspection_reports_channels_and_stream_timings() {
    let temp = tempfile::tempdir().unwrap();
    let camera = json!({"entity": "guard", "timestamp_ns": 100, "frame": 1});
    let camera_payload = serde_json::to_vec(&camera).unwrap().len() as u64;
    let path = seal(
        temp.path(),
        "viewed",
        &[
            (TOPIC_CAMERA, 100, camera),
            (
                TOPIC_CAMERA,
                200,
                json!({"entity": "guard", "timestamp_ns": 200, "frame": 2}),
            ),
            (
                TOPIC_CAMERA,
                400,
                json!({"entity": "guard", "timestamp_ns": 400, "frame": 3}),
            ),
            (
                TOPIC_ACTION_APPLIED,
                500,
                applied_event("pinkie", 11, 0.0),
            ),
        ],
    );

    let (ok, stdout, stderr) = run(&[&path]);
    assert!(ok, "inspector failed: {stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["schema_version"], "qualia.mcap-inspection.v1");
    assert_eq!(report["path"], path.as_str());
    assert_eq!(report["channels"].as_array().unwrap().len(), 2);

    let streams = report["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    let camera_stream = streams
        .iter()
        .find(|stream| stream["topic"] == TOPIC_CAMERA)
        .unwrap();
    assert_eq!(camera_stream["entity"], "guard");
    assert_eq!(camera_stream["messages"], 3);
    assert_eq!(camera_stream["min_timestamp_ns"], 100);
    assert_eq!(camera_stream["max_timestamp_ns"], 400);
    assert_eq!(camera_stream["mean_gap_ns"], 150);
    assert_eq!(camera_stream["p50_gap_ns"], 200);
    assert_eq!(camera_stream["p95_gap_ns"], 200);
    assert_eq!(camera_stream["max_gap_ns"], 200);
    assert_eq!(camera_stream["payload_bytes"], camera_payload * 3);
    assert_eq!(camera_stream["mean_payload_bytes"], camera_payload);
    assert_eq!(camera_stream["max_payload_bytes"], camera_payload);

    // A message without an `entity` field is attributed to `unscoped`.
    let path = seal(
        temp.path(),
        "unscoped",
        &[(TOPIC_CAMERA, 10, json!({"timestamp_ns": 10}))],
    );
    let (ok, stdout, _) = run(&[&path]);
    assert!(ok);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["streams"][0]["entity"], "unscoped");
}

#[test]
fn zero_action_audit_accepts_a_contiguous_zero_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = seal(
        temp.path(),
        "zero",
        &[
            (TOPIC_ACTION_APPLIED, 1007, applied_event("guard", 7, 0.0)),
            (TOPIC_ACTION_APPLIED, 1011, applied_event("pinkie", 11, 0.0)),
            (TOPIC_ACTION_APPLIED, 1008, applied_event("guard", 8, 0.0)),
            (TOPIC_ACTION_APPLIED, 1012, applied_event("pinkie", 12, 0.0)),
        ],
    );

    let (ok, stdout, stderr) = run(&[&path, "--require-zero-applied", "guard,pinkie"]);
    assert!(ok, "audit failed: {stderr}");
    let audit: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(audit["schema_version"], "qualia.zero-applied-action-audit.v1");
    assert_eq!(audit["verified_zero"], true);
    assert_eq!(audit["total_messages"], 4);
    assert_eq!(audit["required_entities"], json!(["guard", "pinkie"]));
    let entities = audit["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0]["entity"], "guard");
    assert_eq!(entities[0]["messages"], 2);
    assert_eq!(entities[0]["first_history_sequence"], 7);
    assert_eq!(entities[0]["last_history_sequence"], 8);
    assert_eq!(entities[0]["first_timestamp_ns"], 1007);
    assert_eq!(entities[0]["last_timestamp_ns"], 1008);
    assert_eq!(entities[1]["entity"], "pinkie");

    // An entity named in the requirement but absent from the evidence fails.
    let (ok, _, stderr) = run(&[&path, "--require-zero-applied", "guard,pinkie,courier"]);
    assert!(!ok);
    assert!(stderr.contains("courier has no applied-action history"), "{stderr}");
}

#[test]
fn zero_action_audit_rejects_motion_and_broken_history() {
    let temp = tempfile::tempdir().unwrap();

    // A transient nonzero applied interval is caught even though the payload
    // still claims `valid: true`.
    let path = seal(
        temp.path(),
        "motion",
        &[(TOPIC_ACTION_APPLIED, 1007, applied_event("guard", 7, 0.1))],
    );
    let (ok, _, stderr) = run(&[&path, "--require-zero-applied", "guard"]);
    assert!(!ok);
    assert!(stderr.contains("is nonzero"), "{stderr}");

    // A history that skips a sequence number is not a complete record.
    let path = seal(
        temp.path(),
        "gap",
        &[
            (TOPIC_ACTION_APPLIED, 1007, applied_event("guard", 7, 0.0)),
            (TOPIC_ACTION_APPLIED, 1009, applied_event("guard", 9, 0.0)),
        ],
    );
    let (ok, _, stderr) = run(&[&path, "--require-zero-applied", "guard"]);
    assert!(!ok);
    assert!(stderr.contains("not contiguous"), "{stderr}");

    // Malformed required-entity lists are rejected before any file is read.
    let (ok, _, stderr) = run(&[&path, "--require-zero-applied", "guard,,pinkie"]);
    assert!(!ok);
    assert!(stderr.contains("empty or malformed"), "{stderr}");
}

#[test]
fn inspector_usage_and_failures_are_reported() {
    let (ok, stdout, _) = run(&[]);
    assert!(ok);
    assert!(stdout.contains("usage: qualia-mcap-inspect"), "{stdout}");

    let (ok, stdout, _) = run(&["--help"]);
    assert!(ok);
    assert!(stdout.contains("usage: qualia-mcap-inspect"));

    let (ok, _, stderr) = run(&["definitely-absent.mcap"]);
    assert!(!ok);
    assert!(stderr.starts_with("qualia-mcap-inspect: "), "{stderr}");

    let (ok, _, stderr) = run(&["a.mcap", "--unknown-flag"]);
    assert!(!ok);
    assert!(stderr.contains("unknown qualia-mcap-inspect argument"), "{stderr}");

    let (ok, _, stderr) = run(&["a.mcap", "--require-zero-applied"]);
    assert!(!ok);
    assert!(stderr.contains("requires a comma-separated entity list"), "{stderr}");

    let (ok, _, stderr) = run(&["a.mcap", "--require-zero-applied", "guard", "extra"]);
    assert!(!ok);
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}
