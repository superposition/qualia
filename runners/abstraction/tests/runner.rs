//! End-to-end tests for `qualia-abstraction`.
//!
//! Each test runs the real binary against a real SQLite store and then reads
//! back what a consumer sees: the rows under each abstraction space, the epoch
//! and space metadata, the pretty JSON on stdout, and the stderr diagnostic
//! plus exit code of a refused run.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use qualia_session_store::{SessionStore, SessionUpsert};
use serde_json::Value;

/// A private directory per test, removed when the test ends.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "qualia-abstraction-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Self { dir }
    }

    fn write(&self, name: &str, body: &str) -> String {
        let path = self.dir.join(name);
        fs::write(&path, body).expect("write fixture");
        path.to_string_lossy().into_owned()
    }

    /// A store holding one imported session, as a prior runner would leave it.
    fn store_with_session(&self) -> String {
        let path = self.dir.join("store.sqlite");
        let store = SessionStore::open(path.to_str().expect("utf-8 path")).expect("open store");
        store
            .upsert_session(&SessionUpsert {
                path: "C:/captures/clip.mp4".into(),
                filename: "clip.mp4".into(),
                media_kind: "flight_capture".into(),
                analysis_kind: "monocular_pose_trace".into(),
                status: "ready".into(),
                duration_sec: 4.0,
                imported_at: "2026-09-11T00:00:00Z".into(),
            })
            .expect("seed session");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_qualia-abstraction"))
        .args(args)
        .output()
        .expect("run qualia-abstraction")
}

fn stdout_json(output: &Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).expect("stdout is utf-8");
    serde_json::from_str(&text).expect("stdout is one JSON document")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is utf-8")
}

fn payload(row: &qualia_session_store::AbstractStateSampleRow) -> Value {
    serde_json::from_str(&row.payload_json).expect("payload is JSON")
}

fn named<'a>(
    spaces: &'a [qualia_session_store::AbstractionSpaceRow],
    name: &str,
) -> &'a qualia_session_store::AbstractionSpaceRow {
    spaces
        .iter()
        .find(|space| space.abstraction_name == name)
        .unwrap_or_else(|| panic!("no space named {name}"))
}

const POSE: &str = "step,timestamp_sec,x_m,y_m,z_m,yaw_rad\n\
0,0.0,0.0,0.0,0.0,0.0\n\
1,0.5,0.05,0.0,0.0,0.0\n\
2,1.0,0.10,0.0,2.0,0.0\n\
3,1.5,3.0,0.0,2.0,0.0\n\
4,2.0,6.0,0.0,2.0,0.0\n";

const CONTROL: &str = "step,timestamp_sec,throttle,yaw,roll,pitch\n\
0,0.0,0.0,0.0,0.0,0.0\n\
1,0.1,0.5,0.0,0.0,0.0\n\
2,0.2,-0.3,0.0,0.0,0.0\n";

const GIMBAL: &str = "step,timestamp_sec,gimbal_yaw,gimbal_pitch,gimbal_roll\n\
0,0.0,0.0,0.0,0.0\n\
1,0.1,0.5,0.0,0.0\n\
2,0.2,0.0,0.9,0.0\n";

#[test]
fn publishes_regimes_symbols_and_metadata_for_every_trace() {
    let scratch = Scratch::new("full");
    let store_path = scratch.store_with_session();
    let pose = scratch.write("pose.csv", POSE);
    let control = scratch.write("control.csv", CONTROL);
    let gimbal = scratch.write("gimbal.csv", GIMBAL);

    let output = run(&[
        "--store",
        &store_path,
        "--session-id",
        "1",
        "--pose-csv",
        &pose,
        "--control-csv",
        &control,
        "--gimbal-csv",
        &gimbal,
        "--epoch-base",
        "3",
        "--created-at",
        "2026-09-11T00:00:00Z",
        "--completed-at",
        "2026-09-11T00:01:00Z",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr_text(&output));

    let summary = stdout_json(&output);
    assert_eq!(summary["session_id"], 1);
    let sensorimotor = summary["sensorimotor_epoch_id"].as_i64().expect("epoch id");
    let navigation = summary["navigation_epoch_id"].as_i64().expect("epoch id");
    assert_ne!(sensorimotor, navigation);

    let names: Vec<&str> = summary["spaces"]
        .as_array()
        .expect("spaces")
        .iter()
        .map(|space| space["name"].as_str().expect("space name"))
        .collect();
    assert_eq!(
        names,
        ["pose_regime", "control_regime", "gimbal_regime", "route_regime"]
    );
    let counts: Vec<u64> = summary["spaces"]
        .as_array()
        .expect("spaces")
        .iter()
        .map(|space| space["samples"].as_u64().expect("sample count"))
        .collect();
    assert_eq!(counts, [5, 3, 3, 5]);
    let epoch_of: Vec<i64> = summary["spaces"]
        .as_array()
        .expect("spaces")
        .iter()
        .map(|space| space["epoch_id"].as_i64().expect("epoch id"))
        .collect();
    assert_eq!(epoch_of, [sensorimotor, sensorimotor, sensorimotor, navigation]);

    let store = SessionStore::open(&store_path).expect("reopen store");
    let epochs = store.list_epochs(1).expect("epochs");
    assert_eq!(epochs.len(), 2);
    assert_eq!(epochs[0].id, sensorimotor);
    assert_eq!(epochs[0].epoch_index, 3);
    assert_eq!(epochs[0].layer_name, "sensorimotor");
    assert_eq!(epochs[0].abstraction_family, "paired_observation_action");
    assert_eq!(epochs[0].status, "complete");
    assert_eq!(epochs[0].created_at, "2026-09-11T00:00:00Z");
    assert_eq!(
        epochs[0].completed_at.as_deref(),
        Some("2026-09-11T00:01:00Z")
    );
    let summary_json: Value = serde_json::from_str(&epochs[0].summary_json).expect("summary json");
    assert_eq!(summary_json["pose_csv"], pose);
    assert_eq!(summary_json["pose_rows"], 5);
    assert_eq!(summary_json["control_rows"], 3);
    assert_eq!(summary_json["gimbal_rows"], 3);
    assert_eq!(epochs[1].id, navigation);
    assert_eq!(epochs[1].epoch_index, 4);
    assert_eq!(epochs[1].layer_name, "navigation");
    assert_eq!(epochs[1].abstraction_family, "route_regime");

    let spaces = store.list_spaces(sensorimotor).expect("spaces");
    assert_eq!(spaces.len(), 3);
    let pose_space = named(&spaces, "pose_regime");
    assert_eq!(pose_space.space_family, "observation");
    assert_eq!(pose_space.source_kind, "pose_csv");
    assert_eq!(pose_space.representation_kind, "discrete+continuous");
    assert_eq!(pose_space.uncertainty_kind, "deterministic");
    assert_eq!(pose_space.dimensionality, 6);
    let schema: Value = serde_json::from_str(&pose_space.schema_json).expect("schema json");
    assert_eq!(schema["label"], "grounded pose regime");
    assert_eq!(
        schema["fields"],
        serde_json::json!(["x", "y", "z", "yaw", "speed", "turn_rate"])
    );
    let control_space = named(&spaces, "control_regime");
    assert_eq!(control_space.space_family, "action");
    assert_eq!(control_space.source_kind, "control_csv");
    assert_eq!(control_space.uncertainty_kind, "estimated");
    assert_eq!(control_space.dimensionality, 4);
    let gimbal_space = named(&spaces, "gimbal_regime");
    assert_eq!(gimbal_space.source_kind, "gimbal_csv");
    assert_eq!(gimbal_space.dimensionality, 3);
    let route_spaces = store.list_spaces(navigation).expect("route spaces");
    assert_eq!(route_spaces.len(), 1);
    assert_eq!(route_spaces[0].abstraction_name, "route_regime");
    assert_eq!(route_spaces[0].space_family, "state");
    assert_eq!(route_spaces[0].source_kind, "pose_csv");
    assert_eq!(route_spaces[0].uncertainty_kind, "estimated");
    assert_eq!(route_spaces[0].dimensionality, 5);

    let pose_rows = store
        .list_state_samples(pose_space.id, None, None, None)
        .expect("pose samples");
    let pose_symbols: Vec<&str> = pose_rows.iter().map(|row| row.symbol_key.as_str()).collect();
    assert_eq!(
        pose_symbols,
        ["hover", "hover", "climb", "transit_fast", "transit_fast"]
    );
    assert_eq!(pose_rows[0].epoch_id, sensorimotor);
    assert_eq!(pose_rows[0].confidence, 0.9);
    assert_eq!(pose_rows[0].sample_hash.len(), 64);
    let climb = payload(&pose_rows[2]);
    assert_eq!(climb["vertical_rate"], 4.0);
    assert_eq!(climb["x"], 0.1);
    assert_eq!(climb["z"], 2.0);

    let control_rows = store
        .list_state_samples(control_space.id, None, None, None)
        .expect("control samples");
    let control_symbols: Vec<&str> = control_rows
        .iter()
        .map(|row| row.symbol_key.as_str())
        .collect();
    assert_eq!(control_symbols, ["neutral", "throttle_up", "throttle_down"]);
    assert_eq!(control_rows[0].confidence, 0.85);
    let throttle_up = payload(&control_rows[1]);
    assert_eq!(throttle_up["normalized"]["throttle"], 0.5);
    assert_eq!(throttle_up["delta"]["throttle"], 0.5);
    assert!(throttle_up["action_summary"]
        .as_str()
        .expect("summary")
        .starts_with("throttle_up:"));
    let throttle_down = payload(&control_rows[2]);
    assert_eq!(throttle_down["delta"]["throttle"], -0.8);

    let gimbal_rows = store
        .list_state_samples(gimbal_space.id, None, None, None)
        .expect("gimbal samples");
    let gimbal_symbols: Vec<&str> = gimbal_rows
        .iter()
        .map(|row| row.symbol_key.as_str())
        .collect();
    assert_eq!(gimbal_symbols, ["centered", "yaw_scan", "pitch_scan"]);
    assert_eq!(gimbal_rows[0].confidence, 0.82);
    assert_eq!(payload(&gimbal_rows[2])["magnitude"], 0.9);

    let route_rows = store
        .list_state_samples(route_spaces[0].id, None, None, None)
        .expect("route samples");
    let route_symbols: Vec<&str> = route_rows.iter().map(|row| row.symbol_key.as_str()).collect();
    assert_eq!(
        route_symbols,
        [
            "station_keeping",
            "corridor_follow",
            "corridor_follow",
            "corridor_follow",
            "egress"
        ]
    );
    assert_eq!(route_rows[0].epoch_id, navigation);
    assert_eq!(route_rows[0].confidence, 0.78);
    let last = payload(&route_rows[4]);
    assert!(last["progress"].as_f64().expect("progress") >= 0.99);
    assert_eq!(payload(&route_rows[1])["segment_length"], 0.05);
}

#[test]
fn resolves_aliased_headers_and_falls_back_to_row_defaults() {
    let scratch = Scratch::new("aliases");
    let store_path = scratch.store_with_session();
    // "x_ft"/"yaw_deg" and "t" are aliases the reader knows; the second run's
    // header carries no step or timestamp column at all.
    let aliased = scratch.write(
        "aliased.csv",
        "frame,t,tx,ty,tz,yaw_deg\n\
0,0.0,0.0,0.0,0.0,0.0\n\
7,0.5,5.0,0.0,0.0,0.0\n",
    );
    let bare = scratch.write(
        "bare.csv",
        "x_m,y_m,z_m\n\
0.0,0.0,0.0\n\
1.0,0.0,0.0\n\
2.0,0.0,0.0\n",
    );
    let created = "2026-09-11T00:00:00Z";

    let output = run(&[
        "--store", &store_path, "--session-id", "1", "--pose-csv", &aliased, "--created-at", created,
    ]);
    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let summary = stdout_json(&output);
    let names: Vec<&str> = summary["spaces"]
        .as_array()
        .expect("spaces")
        .iter()
        .map(|space| space["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["pose_regime", "route_regime"]);
    assert_eq!(summary["spaces"][0]["samples"], 2);

    let store = SessionStore::open(&store_path).expect("reopen store");
    let epochs = store.list_epochs(1).expect("epochs");
    assert_eq!(epochs.len(), 2, "one epoch per regime layer");
    let spaces = store.list_spaces(epochs[0].id).expect("spaces");
    let rows = store
        .list_state_samples(spaces[0].id, None, None, None)
        .expect("pose samples");
    assert_eq!(rows[0].step, 0);
    assert_eq!(rows[1].step, 7, "step read from the `frame` column");
    assert_eq!(rows[1].timestamp_sec, 0.5, "timestamp read from `t`");
    let second = payload(&rows[1]);
    assert_eq!(second["x"], 5.0, "position read from the `tx` column");

    let output = run(&[
        "--store", &store_path, "--session-id", "1", "--pose-csv", &bare, "--created-at", created,
    ]);
    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let store = SessionStore::open(&store_path).expect("reopen store");
    let epochs = store.list_epochs(1).expect("epochs");
    let spaces = store.list_spaces(epochs[0].id).expect("spaces");
    let rows = store
        .list_state_samples(spaces[0].id, None, None, None)
        .expect("pose samples");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].step, 0);
    assert_eq!(rows[2].step, 2, "step defaults to the row index");
    assert_eq!(rows[1].timestamp_sec, 1.0 / 30.0);
}

#[test]
fn refuses_an_unknown_session_before_reading_any_trace() {
    let scratch = Scratch::new("no-session");
    let store_path = scratch.store_with_session();
    let missing_pose = scratch.dir.join("does-not-exist.csv");
    let missing_pose = missing_pose.to_string_lossy().into_owned();

    let output = run(&[
        "--store",
        &store_path,
        "--session-id",
        "42",
        "--pose-csv",
        &missing_pose,
        "--created-at",
        "2026-09-11T00:00:00Z",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains(&format!("session 42 not found in {store_path}")),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("open pose csv"),
        "the trace is not opened when the session is unknown: {stderr}"
    );
}

#[test]
fn refuses_a_non_finite_control_row() {
    let scratch = Scratch::new("non-finite");
    let store_path = scratch.store_with_session();
    let pose = scratch.write("pose.csv", POSE);
    let control = scratch.write(
        "control.csv",
        "step,timestamp_sec,throttle,yaw,roll,pitch\n\
0,0.0,0.0,0.0,0.0,0.0\n\
1,0.1,NaN,0.0,0.0,0.0\n",
    );

    let output = run(&[
        "--store",
        &store_path,
        "--session-id",
        "1",
        "--pose-csv",
        &pose,
        "--control-csv",
        &control,
        "--created-at",
        "2026-09-11T00:00:00Z",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("has non-finite throttle value"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("row 2"), "stderr: {stderr}");
    let store = SessionStore::open(&store_path).expect("reopen store");
    assert!(store.list_epochs(1).expect("epochs").is_empty());
}
