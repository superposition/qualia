//! Observable behaviour of the `qualia-session` binary: its stdout JSON reports, the rows it
//! persists, and the diagnostics it writes to stderr. Every assertion goes through the real
//! executable, so nothing here depends on how the binary is written inside.

use qualia_session_store::mission_types::{
    AnalysisJobKind, AnalysisJobStatus, DomainKind, ExactnessKind, GraphForm, GraphKind, RegionKind,
};
use qualia_session_store::SessionStore;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const POSE_TRACE_CSV: &str = concat!(
    "step,timestamp_sec,tx,ty,tz,qw,qx,qy,qz,matches,inliers,motion_px\n",
    "0,0.0,1.0,2.0,3.0,1.0,0.0,0.0,0.0,120,90,3.5\n",
    "1,0.25,4.0,5.0,6.0,0.9238795,0.0,0.0,0.3826834,150,120,4.25\n"
);

const CONTROLLER_CSV: &str = concat!(
    "step,timestamp_sec,left_u,left_v,right_u,right_v,left_confidence,right_confidence,controller_confidence\n",
    "0,0.0,0.0,0.0,0.0,0.0,0.22,0.28,0.31\n",
    "1,0.25,-0.35,0.40,0.62,-0.15,0.71,0.83,0.79\n"
);

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_qualia-session")
}

fn scratch(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "qualia-session-cli-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn run(store: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .arg("--store")
        .arg(store)
        .args(args)
        .output()
        .expect("spawn qualia-session")
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("write fixture");
}

fn open_store(path: &Path) -> SessionStore {
    SessionStore::open(path.to_str().expect("store path")).expect("open store")
}

#[test]
fn list_prints_an_empty_array_for_a_fresh_store() {
    let dir = scratch("list-empty");
    let store_path = dir.join("store.sqlite");

    let output = run(&store_path, &["list"]);

    assert_eq!(stdout_json(&output), Value::Array(Vec::new()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn import_media_defaults_to_a_primary_video_observation_stream() {
    let dir = scratch("import-media");
    let store_path = dir.join("store.sqlite");
    let media = dir.join("yard.mp4");
    let media_text = media.to_string_lossy().into_owned();

    let output = run(
        &store_path,
        &["import-media", "--path", &media_text, "--media-kind", "video"],
    );
    let envelope = stdout_json(&output);

    assert_eq!(envelope["session"]["path"], Value::String(media_text.clone()));
    assert_eq!(envelope["session"]["filename"], "yard.mp4");
    assert_eq!(envelope["session"]["media_kind"], "video");
    assert_eq!(envelope["session"]["analysis_kind"], "raw_observation");
    assert_eq!(envelope["session"]["status"], "ready");
    assert_eq!(envelope["session"]["duration_sec"], 0.0);

    let streams = envelope["streams"].as_array().expect("streams");
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["stream_key"], "primary");
    assert_eq!(streams[0]["stream_kind"], "video");
    assert_eq!(streams[0]["role"], "observation");
    assert_eq!(streams[0]["sync_group"], "default");
    assert_eq!(streams[0]["path"], Value::String(media_text));

    let store = open_store(&store_path);
    let sessions = store.list_sessions().expect("list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].duration_sec, 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn import_media_honours_stream_specs_and_sync_group() {
    let dir = scratch("import-streams");
    let store_path = dir.join("store.sqlite");
    let media_text = dir.join("flight.mp4").to_string_lossy().into_owned();

    let output = run(
        &store_path,
        &[
            "import-media",
            "--path",
            &media_text,
            "--media-kind",
            "video",
            "--duration-sec",
            "12.5",
            "--stream",
            "ego:video:observation:/data/ego.mp4:capture",
            "--stream",
            "imu:imu:sensor:/data/imu.csv",
        ],
    );
    let envelope = stdout_json(&output);

    assert_eq!(envelope["session"]["duration_sec"], 12.5);
    let streams = envelope["streams"].as_array().expect("streams");
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["stream_key"], "ego");
    assert_eq!(streams[0]["stream_kind"], "video");
    assert_eq!(streams[0]["role"], "observation");
    assert_eq!(streams[0]["path"], "/data/ego.mp4");
    assert_eq!(streams[0]["sync_group"], "capture");
    assert_eq!(streams[1]["stream_key"], "imu");
    assert_eq!(streams[1]["stream_kind"], "imu");
    assert_eq!(streams[1]["role"], "sensor");
    assert_eq!(streams[1]["sync_group"], "default");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn import_media_rejects_a_stream_spec_that_is_missing_a_field() {
    let dir = scratch("import-bad-stream");
    let store_path = dir.join("store.sqlite");

    let output = run(
        &store_path,
        &[
            "import-media",
            "--path",
            "/data/a.mp4",
            "--media-kind",
            "video",
            "--stream",
            "ego:video:observation",
        ],
    );

    assert!(!output.status.success());
    assert!(
        stderr_text(&output).contains("invalid --stream spec"),
        "unexpected stderr: {}",
        stderr_text(&output)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn import_manifest_reads_streams_and_infers_a_missing_filename() {
    let dir = scratch("import-manifest");
    let store_path = dir.join("store.sqlite");
    let manifest_path = dir.join("session.json");
    write(
        &manifest_path,
        r#"{
            "path": "/data/dawn_patrol.mp4",
            "media_kind": "video",
            "duration_sec": 3.5,
            "streams": [
                {"key": "ego", "kind": "video", "path": "/data/dawn_patrol.mp4"},
                {"key": "gps", "kind": "telemetry", "role": "sensor", "path": "/data/gps.csv"}
            ]
        }"#,
    );

    let output = run(
        &store_path,
        &[
            "import-manifest",
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
        ],
    );
    let envelope = stdout_json(&output);

    assert_eq!(envelope["session"]["filename"], "dawn_patrol.mp4");
    assert_eq!(envelope["session"]["analysis_kind"], "raw_observation");
    assert_eq!(envelope["session"]["status"], "ready");
    let streams = envelope["streams"].as_array().expect("streams");
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["stream_key"], "ego");
    assert_eq!(streams[0]["role"], "observation");
    assert_eq!(streams[0]["sync_group"], "default");
    assert_eq!(streams[1]["stream_key"], "gps");
    assert_eq!(streams[1]["role"], "sensor");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn show_names_a_session_that_does_not_exist() {
    let dir = scratch("show-missing");
    let store_path = dir.join("store.sqlite");

    let output = run(&store_path, &["show", "--session-id", "77"]);

    assert!(!output.status.success());
    assert!(
        stderr_text(&output).contains("session 77 not found"),
        "unexpected stderr: {}",
        stderr_text(&output)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn import_ros2_jsonl_reports_topics_and_persists_five_spaces() {
    let dir = scratch("ros2");
    let store_path = dir.join("store.sqlite");
    let clone = dir.join("ego.mp4");
    write(&clone, "");
    let clone_text = clone.to_string_lossy().into_owned();
    let import_media = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            &clone_text,
            "--media-kind",
            "telemetry",
        ],
    ));
    let session_id = import_media["session"]["id"].as_i64().expect("session id");

    let jsonl_path = dir.join("ros2.jsonl");
    let rows = [
        r#"{"topic":"/odom","msg":{"header":{"stamp":{"sec":1,"nanosec":0},"frame_id":"odom"},"child_frame_id":"base_link","pose":{},"twist":{}}}"#,
        r#"{"topic":"/scan","msg":{"header":{"stamp":{"sec":1,"nanosec":100000000},"frame_id":"laser"},"ranges":[1.0,2.0],"range_min":0.1,"range_max":10.0}}"#,
        r#"{"topic":"/cmd_vel","stamp":{"sec":1,"nanosec":200000000},"msg":{"linear":{"x":0.2},"angular":{"z":0.0}}}"#,
        r#"{"topic":"/tf","msg":{"transforms":[{"header":{"frame_id":"odom","stamp":{"sec":1,"nanosec":300000000}},"child_frame_id":"base_link","transform":{}}]}}"#,
        r#"{"topic":"/tf_static","msg":{"transforms":[{"header":{"frame_id":"map","stamp":{"sec":1,"nanosec":400000000}},"child_frame_id":"odom","transform":{}}]}}"#,
        r#"{"topic":"/battery","msg":{"voltage":12.1}}"#,
        r#"{"noise":true}"#,
    ]
    .join("\n");
    write(&jsonl_path, &rows);
    let jsonl_text = jsonl_path.to_string_lossy().into_owned();

    let output = run(
        &store_path,
        &[
            "import-ros2-jsonl",
            "--session-id",
            &session_id.to_string(),
            "--input",
            &jsonl_text,
        ],
    );
    let report = stdout_json(&output);

    assert_eq!(report["session_id"], session_id);
    assert_eq!(report["ingested_total"], 5);
    assert_eq!(report["ignored_total"], 2);
    assert_eq!(report["topics"]["/tf"], 1);
    assert_eq!(report["topics"]["/tf_static"], 1);
    assert_eq!(report["topics"]["/odom"], 1);
    assert_eq!(report["topics"]["/scan"], 1);
    assert_eq!(report["topics"]["/cmd_vel"], 1);

    let store = open_store(&store_path);
    let epochs = store.list_epochs(session_id).expect("epochs");
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0].layer_name, "ros2_ingest");
    assert_eq!(epochs[0].abstraction_family, "ros2_core_topics");
    let spaces = store.list_spaces(epochs[0].id).expect("spaces");
    assert_eq!(spaces.len(), 5);
    let tf_space = spaces
        .iter()
        .find(|space| space.abstraction_name == "tf_observation")
        .expect("tf space");
    assert_eq!(tf_space.space_family, "state");
    assert_eq!(tf_space.source_kind, "ros2");
    let tf_samples = store
        .list_state_samples(tf_space.id, None, None, None)
        .expect("tf samples");
    assert_eq!(tf_samples.len(), 1);
    assert_eq!(tf_samples[0].symbol_key, "odom->base_link");
    assert!((tf_samples[0].timestamp_sec - 1.3).abs() < 1e-9);

    let cmd_space = spaces
        .iter()
        .find(|space| space.abstraction_name == "cmd_vel_action")
        .expect("cmd_vel space");
    assert_eq!(cmd_space.space_family, "action");
    let cmd_samples = store
        .list_state_samples(cmd_space.id, None, None, None)
        .expect("cmd_vel samples");
    assert_eq!(cmd_samples[0].symbol_key, "forward");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn attach_daaam_output_persists_a_loopy_fragment_and_a_world_region() {
    let dir = scratch("daaam");
    let store_path = dir.join("store.sqlite");
    let media = dir.join("analyzed.mp4");
    write(&media, "");
    let media_text = media.to_string_lossy().into_owned();
    let session_id = stdout_json(&run(
        &store_path,
        &["import-media", "--path", &media_text, "--media-kind", "video"],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let output_dir = dir.join("daaam_out");
    std::fs::create_dir_all(&output_dir).expect("daaam dir");
    write(
        &output_dir.join("dsg_updated.json"),
        r#"{
            "layers": {
                "places": {"nodes": [{"id": "p0", "position": [1.0, 2.0, 0.0]}]},
                "objects": {"nodes": [
                    {"id": "o0", "position": [3.0, 4.0, 0.0], "confidence": 0.9},
                    {"id": "o1", "position": [5.0, 6.0, 0.0], "score": 0.8}
                ]}
            },
            "edges": [{"source": "p0", "target": "o0"}, {"source": "o0", "target": "o1"}]
        }"#,
    );
    write(&output_dir.join("stats.json"), "{}");
    let output_text = output_dir.to_string_lossy().into_owned();

    let output = run(
        &store_path,
        &[
            "attach-daaam-output",
            "--session-id",
            &session_id.to_string(),
            "--output-dir",
            &output_text,
        ],
    );
    let report = stdout_json(&output);

    assert_eq!(report["session_id"], session_id);
    assert_eq!(report["dsg_artifact_path"], "dsg_updated.json");
    assert_eq!(report["node_count"], 3);
    assert_eq!(report["edge_count"], 2);
    assert_eq!(report["layer_counts"]["objects"], 2);
    assert_eq!(report["layer_counts"]["places"], 1);
    assert!(report["artifact_paths"]
        .as_array()
        .expect("artifact paths")
        .iter()
        .any(|path| path == "stats.json"));

    let store = open_store(&store_path);
    let jobs = store.list_analysis_jobs(session_id).expect("jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].job_kind, AnalysisJobKind::GraphInference);
    assert_eq!(jobs[0].status, AnalysisJobStatus::Complete);
    assert_eq!(jobs[0].environment_id, Some(session_id));

    let fragments = store.list_graph_fragments(session_id).expect("fragments");
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].graph_kind, GraphKind::LocalSpace);
    assert_eq!(fragments[0].graph_form, GraphForm::Loopy);
    assert_eq!(fragments[0].exactness, ExactnessKind::Approximate);
    assert_eq!(fragments[0].variable_count, 3);
    assert_eq!(fragments[0].factor_count, 2);
    assert_eq!(fragments[0].window_end_sec, 0.0);
    assert_eq!(fragments[0].root_variable_key.as_deref(), Some("o0"));

    let regions = store.list_world_regions(session_id).expect("regions");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].region_kind, RegionKind::LocalSpace);
    assert_eq!(regions[0].source_fragment_id, fragments[0].id);
    assert_eq!(regions[0].support_point_count, 3);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn attach_daaam_output_rejects_a_missing_output_directory() {
    let dir = scratch("daaam-missing");
    let store_path = dir.join("store.sqlite");

    let output = run(
        &store_path,
        &[
            "attach-daaam-output",
            "--session-id",
            "1",
            "--output-dir",
            dir.join("nowhere").to_str().expect("path"),
        ],
    );

    assert!(!output.status.success());
    assert!(
        stderr_text(&output).contains("session 1 not found"),
        "unexpected stderr: {}",
        stderr_text(&output)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn analyze_session_with_a_trace_csv_persists_the_full_pose_trace_chain() {
    let dir = scratch("analyze");
    let store_path = dir.join("store.sqlite");
    let video = dir.join("flight.mp4");
    write(&video, "");
    let video_text = video.to_string_lossy().into_owned();

    let imported = stdout_json(&run(
        &store_path,
        &["import-media", "--path", &video_text, "--media-kind", "video"],
    ));
    let session_id = imported["session"]["id"].as_i64().expect("session id");

    let trace = dir.join("input_pose_trace.csv");
    write(&trace, POSE_TRACE_CSV);
    let trace_text = trace.to_string_lossy().into_owned();

    let output = run(
        &store_path,
        &[
            "analyze-session",
            "--session-id",
            &session_id.to_string(),
            "--trace-csv",
            &trace_text,
            "--epoch-index",
            "7",
        ],
    );
    let report = stdout_json(&output);

    assert_eq!(report["session_id"], session_id);
    assert_eq!(report["input_path"], Value::String(video_text.clone()));
    assert_eq!(report["sample_count"], 2);
    assert_eq!(report["rc_input_sample_count"], 0);
    assert_eq!(report["analysis_kind"], "monocular_pose_trace");
    assert_eq!(report["duration_sec"], 0.25);
    assert!(report["controller_capture_path"].is_null());
    assert!(report["scene_cloud_path"].is_null());
    assert_eq!(report["scene_cloud_point_count"], 0);
    assert_eq!(report["trace_stream_key"], "pose_trace");

    let store = open_store(&store_path);
    let session = store.session_by_id(session_id).expect("lookup").expect("session");
    assert_eq!(session.analysis_kind, "monocular_pose_trace");
    assert!((session.duration_sec - 0.25).abs() < 1e-9);

    // The trace is copied into the default per-session artifact directory.
    let copied = dir.join(format!("session_{session_id}")).join("pose_trace.csv");
    assert_eq!(std::fs::read_to_string(&copied).expect("copied trace"), POSE_TRACE_CSV);
    assert_eq!(report["trace_path"], Value::String(copied.to_string_lossy().into_owned()));

    let epochs = store.list_epochs(session_id).expect("epochs");
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0].epoch_index, 7);
    assert_eq!(epochs[0].layer_name, "sensorimotor");
    assert_eq!(epochs[0].abstraction_family, "paired_observation_action");

    let spaces = store.list_spaces(epochs[0].id).expect("spaces");
    assert_eq!(spaces.len(), 5);
    let names = spaces
        .iter()
        .map(|space| space.abstraction_name.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "frame_observation",
        "pose_observation",
        "control_proxy",
        "gimbal_form",
        "flight_regime",
    ] {
        assert!(names.contains(&expected), "missing space {expected}");
    }

    let pose_space = spaces
        .iter()
        .find(|space| space.abstraction_name == "pose_observation")
        .expect("pose space");
    assert_eq!(pose_space.space_family, "observation");
    assert_eq!(pose_space.source_kind, "video_trace");
    assert_eq!(pose_space.representation_kind, "continuous");
    assert_eq!(pose_space.dimensionality, 12);
    let pose_samples = store
        .list_state_samples(pose_space.id, None, None, None)
        .expect("pose samples");
    assert_eq!(pose_samples.len(), 2);
    assert_eq!(pose_samples[0].symbol_key, "pose_observation");
    let payload: Value = serde_json::from_str(&pose_samples[1].payload_json).expect("payload");
    assert!((payload["x_m"].as_f64().expect("x_m") - 1.2192).abs() < 1e-9);
    assert!(
        (payload["yaw_rad"].as_f64().expect("yaw_rad") - std::f64::consts::FRAC_PI_4).abs() < 1e-6
    );
    assert_eq!(payload["source_unit"], "ft");
    assert_eq!(payload["position_source"]["tx_ft"], 4.0);

    let frame_space = spaces
        .iter()
        .find(|space| space.abstraction_name == "frame_observation")
        .expect("frame space");
    assert_eq!(frame_space.representation_kind, "discrete");
    let frame_samples = store
        .list_state_samples(frame_space.id, None, None, None)
        .expect("frame samples");
    assert_eq!(frame_samples.len(), 2);
    let frame_payload: Value =
        serde_json::from_str(&frame_samples[1].payload_json).expect("frame payload");
    assert_eq!(frame_payload["frame_index"], 1);
    assert_eq!(frame_payload["artifact_path"], Value::String(video_text));

    // The derived control chain is rebuilt from the trace, not from a capture file.
    let control_space = spaces
        .iter()
        .find(|space| space.abstraction_name == "control_proxy")
        .expect("control space");
    assert_eq!(control_space.space_family, "action");
    assert_eq!(control_space.source_kind, "video_trace_derived");
    let control_samples = store
        .list_state_samples(control_space.id, None, None, None)
        .expect("control samples");
    assert_eq!(control_samples.len(), 2);
    let control_payload: Value =
        serde_json::from_str(&control_samples[1].payload_json).expect("control payload");
    assert!(control_payload["pitch_cmd"].as_f64().expect("pitch").abs() > 0.0);

    let gimbal_samples = store
        .list_state_samples(
            spaces
                .iter()
                .find(|space| space.abstraction_name == "gimbal_form")
                .expect("gimbal space")
                .id,
            None,
            None,
            None,
        )
        .expect("gimbal samples");
    let gimbal_payload: Value =
        serde_json::from_str(&gimbal_samples[1].payload_json).expect("gimbal payload");
    assert!(gimbal_payload["gimbal_yaw"].as_f64().expect("yaw").abs() > 40.0);

    // The bootstrap inference leaves a complete fragment, a belief per space and messages.
    let jobs = store.list_analysis_jobs(session_id).expect("jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, AnalysisJobStatus::Complete);
    let fragments = store.list_graph_fragments(session_id).expect("fragments");
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].graph_kind, GraphKind::Sensorimotor);
    assert_eq!(fragments[0].epoch_id, Some(epochs[0].id));
    let beliefs = store.list_belief_matrices(fragments[0].id).expect("beliefs");
    assert_eq!(beliefs.len(), 5);
    assert!(beliefs.iter().all(|belief| belief.domain_kind == DomainKind::Discrete));
    assert_eq!(
        store
            .list_message_snapshots(fragments[0].id)
            .expect("messages")
            .len(),
        8
    );

    let regions = store.list_world_regions(session_id).expect("regions");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].region_kind, RegionKind::LocalSpace);
    let metadata: Value =
        serde_json::from_str(&regions[0].metadata_json).expect("region metadata");
    assert_eq!(metadata["source"], "video_trace_bootstrap");
    assert_eq!(metadata["sample_count"], 2);

    let splines = store.list_world_model_spline_controls().expect("splines");
    assert_eq!(splines.len(), 1);
    assert_eq!(
        splines[0].key,
        format!("spline/session_{session_id}/epoch_7")
    );
    assert!(splines[0].artifact_ref.is_some());
    let spline_path = splines[0]
        .artifact_ref
        .as_ref()
        .expect("artifact ref")
        .uri
        .clone();
    let spline_path = spline_path.strip_prefix("file://").expect("file uri");
    let artifact: Value =
        serde_json::from_str(&std::fs::read_to_string(spline_path).expect("spline artifact"))
            .expect("spline json");
    assert_eq!(artifact["schema"], "trajectory_spline.v1");
    assert_eq!(artifact["knot_count"], 2);
    assert_eq!(artifact["sample_count"], 2);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn analyze_session_uses_an_explicit_controller_capture() {
    let dir = scratch("analyze-controller");
    let store_path = dir.join("store.sqlite");
    let video = dir.join("CleanShot controller.mp4");
    write(&video, "");
    let video_text = video.to_string_lossy().into_owned();
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            &video_text,
            "--media-kind",
            "desktop_capture",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let trace = dir.join("pose_trace.csv");
    write(&trace, POSE_TRACE_CSV);
    let controller = dir.join("controller.csv");
    write(&controller, CONTROLLER_CSV);

    let output = run(
        &store_path,
        &[
            "analyze-session",
            "--session-id",
            &session_id.to_string(),
            "--trace-csv",
            trace.to_str().expect("trace"),
            "--controller-csv",
            controller.to_str().expect("controller"),
        ],
    );
    let report = stdout_json(&output);

    assert_eq!(report["rc_input_sample_count"], 2);
    assert!(report["controller_capture_path"]
        .as_str()
        .expect("controller path")
        .ends_with("controller_capture.csv"));

    let store = open_store(&store_path);
    let epochs = store.list_epochs(session_id).expect("epochs");
    let spaces = store.list_spaces(epochs[0].id).expect("spaces");
    let rc_space = spaces
        .iter()
        .find(|space| space.abstraction_name == "rc_input_observation")
        .expect("rc space");
    assert_eq!(rc_space.space_family, "action");
    assert_eq!(rc_space.source_kind, "controller_capture");
    assert_eq!(rc_space.dimensionality, 7);
    let rc_samples = store
        .list_state_samples(rc_space.id, None, None, None)
        .expect("rc samples");
    assert_eq!(rc_samples.len(), 2);
    let payload: Value = serde_json::from_str(&rc_samples[1].payload_json).expect("payload");
    assert_eq!(payload["left_u"], -0.35);
    assert_eq!(payload["controller_confidence"], 0.79);

    let streams = store.list_streams(session_id).expect("streams");
    let capture = streams
        .iter()
        .find(|stream| stream.stream_key == "controller_capture")
        .expect("controller stream");
    assert_eq!(capture.stream_kind, "controller_capture_csv");
    assert_eq!(capture.role, "observation");

    let fragments = store.list_graph_fragments(session_id).expect("fragments");
    let beliefs = store
        .list_belief_matrices(fragments[fragments.len() - 1].id)
        .expect("beliefs");
    assert!(beliefs
        .iter()
        .any(|belief| belief.variable_key == "rc_input_observation"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn analyze_session_rejects_a_pose_trace_row_with_the_wrong_column_count() {
    let dir = scratch("analyze-bad-trace");
    let store_path = dir.join("store.sqlite");
    let video = dir.join("flight.mp4");
    write(&video, "");
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            video.to_str().expect("video"),
            "--media-kind",
            "video",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let trace = dir.join("bad.csv");
    write(
        &trace,
        "step,timestamp_sec,tx,ty,tz,qw,qx,qy,qz,matches,inliers,motion_px\n0,0.0,1.0,2.0\n",
    );

    let output = run(
        &store_path,
        &[
            "analyze-session",
            "--session-id",
            &session_id.to_string(),
            "--trace-csv",
            trace.to_str().expect("trace"),
        ],
    );

    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("malformed pose trace row") && stderr.contains("expected 12 columns"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn analyze_session_reports_an_empty_pose_trace() {
    let dir = scratch("analyze-empty-trace");
    let store_path = dir.join("store.sqlite");
    let video = dir.join("flight.mp4");
    write(&video, "");
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            video.to_str().expect("video"),
            "--media-kind",
            "video",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let trace = dir.join("empty.csv");
    write(
        &trace,
        "step,timestamp_sec,tx,ty,tz,qw,qx,qy,qz,matches,inliers,motion_px\n",
    );

    let output = run(
        &store_path,
        &[
            "analyze-session",
            "--session-id",
            &session_id.to_string(),
            "--trace-csv",
            trace.to_str().expect("trace"),
        ],
    );

    assert!(!output.status.success());
    assert!(
        stderr_text(&output).contains("had no rows"),
        "unexpected stderr: {}",
        stderr_text(&output)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn analyze_session_reports_a_missing_session_before_touching_the_trace() {
    let dir = scratch("analyze-missing");
    let store_path = dir.join("store.sqlite");

    let output = run(
        &store_path,
        &["analyze-session", "--session-id", "41", "--trace-csv", "/dev/null"],
    );

    assert!(!output.status.success());
    assert!(
        stderr_text(&output).contains("session 41 not found"),
        "unexpected stderr: {}",
        stderr_text(&output)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn analyze_session_reports_a_failed_video_trace_launch() {
    let dir = scratch("analyze-launch");
    let store_path = dir.join("store.sqlite");
    let video = dir.join("flight.mp4");
    write(&video, "");
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            video.to_str().expect("video"),
            "--media-kind",
            "video",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");
    let missing_bin = dir.join("no-such-drone-fg");

    let output = run(
        &store_path,
        &[
            "analyze-session",
            "--session-id",
            &session_id.to_string(),
            "--video-trace-bin",
            missing_bin.to_str().expect("bin"),
        ],
    );

    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("no-such-drone-fg"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_inference_persists_a_completed_job_and_prints_the_result() {
    let dir = scratch("inference");
    let store_path = dir.join("store.sqlite");
    let media = dir.join("yard.mp4");
    write(&media, "");
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            media.to_str().expect("media"),
            "--media-kind",
            "video",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let jsonl = dir.join("ros2.jsonl");
    write(
        &jsonl,
        concat!(
            r#"{"topic":"/cmd_vel","stamp":{"sec":1,"nanosec":0},"msg":{"linear":{"x":0.4},"angular":{"z":0.0}}}"#,
            "\n",
            r#"{"topic":"/cmd_vel","stamp":{"sec":2,"nanosec":0},"msg":{"linear":{"x":0.4},"angular":{"z":0.0}}}"#,
            "\n",
            r#"{"topic":"/cmd_vel","stamp":{"sec":3,"nanosec":0},"msg":{"linear":{"x":0.0},"angular":{"z":0.5}}}"#,
            "\n"
        ),
    );
    stdout_json(&run(
        &store_path,
        &[
            "import-ros2-jsonl",
            "--session-id",
            &session_id.to_string(),
            "--input",
            jsonl.to_str().expect("jsonl"),
        ],
    ));

    let output = run(
        &store_path,
        &["run-inference", "--session-id", &session_id.to_string()],
    );
    let result = stdout_json(&output);

    assert_eq!(result["exact_fragment_count"], 1);
    assert_eq!(result["approximate_fragment_count"], 0);
    assert_eq!(result["fragment_ids"].as_array().expect("fragments").len(), 1);
    assert_eq!(result["belief_matrix_ids"].as_array().expect("beliefs").len(), 1);
    assert_eq!(
        result["message_snapshot_ids"]
            .as_array()
            .expect("messages")
            .len(),
        0
    );
    let job_id = result["job_id"].as_i64().expect("job id");

    let store = open_store(&store_path);
    let jobs = store.list_analysis_jobs(session_id).expect("jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job_id);
    assert_eq!(jobs[0].status, AnalysisJobStatus::Complete);
    assert_eq!(jobs[0].job_kind, AnalysisJobKind::GraphInference);
    let beliefs = store
        .list_belief_matrices(result["fragment_ids"][0].as_i64().expect("fragment id"))
        .expect("beliefs");
    assert_eq!(beliefs.len(), 1);
    assert_eq!(beliefs[0].variable_key, "cmd_vel_action");
    assert_eq!(beliefs[0].values_json, r#"{"forward":0.6666666666666666,"turn_left":0.3333333333333333}"#);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_inference_reports_a_session_without_epochs() {
    let dir = scratch("inference-no-epochs");
    let store_path = dir.join("store.sqlite");
    let media = dir.join("yard.mp4");
    write(&media, "");
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            media.to_str().expect("media"),
            "--media-kind",
            "video",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let output = run(
        &store_path,
        &["run-inference", "--session-id", &session_id.to_string()],
    );

    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("no abstraction epochs"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_inference_rejects_an_unknown_graph_kind() {
    let dir = scratch("inference-bad-kind");
    let store_path = dir.join("store.sqlite");
    let media = dir.join("yard.mp4");
    write(&media, "");
    let session_id = stdout_json(&run(
        &store_path,
        &[
            "import-media",
            "--path",
            media.to_str().expect("media"),
            "--media-kind",
            "video",
        ],
    ))["session"]["id"]
        .as_i64()
        .expect("session id");

    let output = run(
        &store_path,
        &[
            "run-inference",
            "--session-id",
            &session_id.to_string(),
            "--graph-kind",
            "telepathy",
        ],
    );

    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("invalid --graph-kind `telepathy`"),
        "unexpected stderr: {stderr}"
    );
    assert!(stderr.contains("sensorimotor"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn store_defaults_live_under_the_artifacts_directory() {
    let dir = scratch("default-store");
    let empty = dir.join("empty");
    std::fs::create_dir_all(empty.join("artifacts")).expect("artifacts dir");

    let output = Command::new(binary())
        .current_dir(&empty)
        .arg("list")
        .output()
        .expect("spawn qualia-session");

    assert!(output.status.success());
    assert!(empty.join("artifacts/qualia_session_store.sqlite").exists());
    let _ = std::fs::remove_dir_all(dir);
}
