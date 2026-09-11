//! `qualia-session`: ingest, analyse and browse typed ego-process sessions.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use qualia_session_store::{
    mission_types::{
        AnalysisJobKind, AnalysisJobStatus, ExactnessKind, GraphForm, GraphKind, RegionKind,
    },
    AbstractStateSample, AbstractionEpochUpsert, AbstractionSpaceUpsert, AnalysisJobUpsert,
    GraphFragmentUpsert, Rosbag2ExportOptions, Rosbag2ImportOptions, SessionStore,
    SessionStreamUpsert, SessionUpsert, WorldRegionUpsert,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::util::{
    infer_filename, now_iso8601, relative_path_or_display, stable_hash, stream_metadata_json,
};

mod analysis;
mod offline_inference;
mod util;

use offline_inference::{run_offline_inference, InferenceJobSpec};

const DEFAULT_STORE_PATH: &str = "artifacts/qualia_session_store.sqlite";

#[derive(Parser)]
#[command(
    name = "qualia-session",
    about = "Ingest and browse typed ego-process sessions"
)]
struct Cli {
    #[arg(long, default_value = DEFAULT_STORE_PATH, global = true)]
    store: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    ImportManifest {
        #[arg(long)]
        manifest: String,
    },
    ImportMedia {
        #[arg(long)]
        path: String,
        #[arg(long)]
        media_kind: String,
        #[arg(long, default_value = "raw_observation")]
        analysis_kind: String,
        #[arg(long, default_value = "ready")]
        status: String,
        #[arg(long, default_value_t = 0.0)]
        duration_sec: f64,
        #[arg(long = "stream", value_name = "KEY:KIND:ROLE:PATH[:SYNC_GROUP]")]
        streams: Vec<String>,
    },
    AnalyzeSession {
        #[arg(long)]
        session_id: i64,
        #[arg(long)]
        input: Option<String>,
        #[arg(long)]
        trace_csv: Option<String>,
        #[arg(long)]
        controller_csv: Option<String>,
        #[arg(long)]
        video_trace_bin: Option<String>,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        optimized_output: Option<String>,
        #[arg(long, default_value_t = 0)]
        max_frames: i64,
        #[arg(long, default_value_t = 1)]
        frame_stride: i64,
        #[arg(long, default_value_t = 3600)]
        max_features: i64,
        #[arg(long, default_value_t = 1.25)]
        min_motion_px: f64,
        #[arg(long)]
        epoch_index: Option<i64>,
        #[arg(long, default_value = "sensorimotor")]
        layer_name: String,
        #[arg(long, default_value = "paired_observation_action")]
        abstraction_family: String,
        #[arg(long, default_value = "pose_trace")]
        stream_key: String,
    },
    List,
    Show {
        #[arg(long)]
        session_id: i64,
    },
    RunInference {
        #[arg(long)]
        session_id: i64,
        #[arg(long)]
        environment_id: Option<i64>,
        #[arg(long)]
        window_start_sec: Option<f64>,
        #[arg(long)]
        window_end_sec: Option<f64>,
        #[arg(long, default_value = "sensorimotor")]
        graph_kind: String,
        #[arg(long, default_value_t = true)]
        require_exact: bool,
    },
    ImportRos2Jsonl {
        #[arg(long)]
        session_id: i64,
        #[arg(long)]
        input: String,
        #[arg(long, default_value_t = 0)]
        epoch_index: i64,
        #[arg(long, default_value = "ros2_ingest")]
        layer_name: String,
        #[arg(long, default_value = "ros2_core_topics")]
        abstraction_family: String,
        #[arg(long, default_value = "ros2")]
        stream_key: String,
    },
    ImportRosbag2 {
        #[arg(long)]
        session_id: i64,
        #[arg(long)]
        bag: String,
        #[arg(long, default_value_t = 0)]
        epoch_index: i64,
        #[arg(long, default_value = "rosbag2_ingest")]
        layer_name: String,
        #[arg(long, default_value = "rosbag2_topics")]
        abstraction_family: String,
        #[arg(long, default_value = "rosbag2")]
        stream_key: String,
    },
    ExportRosbag2 {
        #[arg(long)]
        session_id: i64,
        #[arg(long)]
        out_dir: String,
        #[arg(long)]
        epoch_id: Option<i64>,
        #[arg(long)]
        bag_name: Option<String>,
    },
    AttachDaaamOutput {
        #[arg(long)]
        session_id: i64,
        #[arg(long)]
        output_dir: String,
        #[arg(long)]
        dsg: Option<String>,
        #[arg(long, default_value = "daaam_dsg")]
        stream_key: String,
        #[arg(long)]
        fragment_key: Option<String>,
        #[arg(long, default_value = "local_space")]
        graph_kind: String,
        #[arg(long)]
        window_start_sec: Option<f64>,
        #[arg(long)]
        window_end_sec: Option<f64>,
    },
}

#[derive(Debug, Deserialize)]
struct SessionManifest {
    path: String,
    #[serde(default)]
    filename: Option<String>,
    media_kind: String,
    #[serde(default = "default_analysis_kind")]
    analysis_kind: String,
    #[serde(default = "default_status")]
    status: String,
    #[serde(default)]
    duration_sec: f64,
    #[serde(default)]
    imported_at: Option<String>,
    #[serde(default)]
    streams: Vec<StreamManifest>,
}

#[derive(Debug, Deserialize)]
struct StreamManifest {
    key: String,
    kind: String,
    #[serde(default = "default_role")]
    role: String,
    path: String,
    #[serde(default = "default_sync_group")]
    sync_group: String,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Serialize)]
struct SessionEnvelope {
    session: Value,
    streams: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct Ros2IngestReport {
    session_id: i64,
    epoch_id: i64,
    ingested_total: usize,
    ignored_total: usize,
    topics: HashMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct DaaamAttachReport {
    session_id: i64,
    analysis_job_id: i64,
    graph_fragment_id: i64,
    world_region_id: i64,
    dsg_artifact_path: String,
    artifact_paths: Vec<String>,
    node_count: usize,
    edge_count: usize,
    layer_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
struct DaaamGraphScan {
    node_count: usize,
    edge_count: usize,
    layer_counts: BTreeMap<String, usize>,
    root_node_key: Option<String>,
    points: Vec<DaaamPoint>,
    confidence_sum: f64,
    confidence_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct DaaamPoint {
    x: f64,
    y: f64,
    z: f64,
}

/// One ROS2 abstraction space plus the samples collected for it before the epoch is written.
struct SpaceBuffer {
    upsert: AbstractionSpaceUpsert,
    samples: Vec<AbstractStateSample>,
}

fn default_analysis_kind() -> String {
    "raw_observation".to_string()
}

fn default_status() -> String {
    "ready".to_string()
}

fn default_role() -> String {
    "observation".to_string()
}

fn default_sync_group() -> String {
    "default".to_string()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut store =
        SessionStore::open(&cli.store).with_context(|| format!("open store {}", cli.store))?;

    match cli.command {
        Command::ImportManifest { manifest } => import_manifest(&store, &manifest),
        Command::ImportMedia {
            path,
            media_kind,
            analysis_kind,
            status,
            duration_sec,
            streams,
        } => import_media(
            &store,
            &path,
            &media_kind,
            &analysis_kind,
            &status,
            duration_sec,
            &streams,
        ),
        Command::AnalyzeSession {
            session_id,
            input,
            trace_csv,
            controller_csv,
            video_trace_bin,
            output,
            optimized_output,
            max_frames,
            frame_stride,
            max_features,
            min_motion_px,
            epoch_index,
            layer_name,
            abstraction_family,
            stream_key,
        } => analysis::analyze_session(
            &store,
            &cli.store,
            analysis::AnalyzeRequest {
                session_id,
                input: input.as_deref(),
                trace_csv: trace_csv.as_deref(),
                controller_csv: controller_csv.as_deref(),
                video_trace_bin: video_trace_bin.as_deref(),
                output: output.as_deref(),
                optimized_output: optimized_output.as_deref(),
                max_frames,
                frame_stride,
                max_features,
                min_motion_px,
                epoch_index,
                layer_name: &layer_name,
                abstraction_family: &abstraction_family,
                trace_stream_key: &stream_key,
            },
        ),
        Command::List => list_sessions(&store),
        Command::Show { session_id } => show_session(&store, session_id),
        Command::RunInference {
            session_id,
            environment_id,
            window_start_sec,
            window_end_sec,
            graph_kind,
            require_exact,
        } => run_inference(
            &store,
            session_id,
            environment_id,
            window_start_sec,
            window_end_sec,
            &graph_kind,
            require_exact,
        ),
        Command::ImportRos2Jsonl {
            session_id,
            input,
            epoch_index,
            layer_name,
            abstraction_family,
            stream_key,
        } => import_ros2_jsonl(
            &mut store,
            session_id,
            &input,
            epoch_index,
            &layer_name,
            &abstraction_family,
            &stream_key,
        ),
        Command::ImportRosbag2 {
            session_id,
            bag,
            epoch_index,
            layer_name,
            abstraction_family,
            stream_key,
        } => {
            let report = store.import_rosbag2(
                session_id,
                &bag,
                &Rosbag2ImportOptions {
                    epoch_index,
                    layer_name,
                    abstraction_family,
                    stream_key,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Command::ExportRosbag2 {
            session_id,
            out_dir,
            epoch_id,
            bag_name,
        } => {
            let report = store.export_rosbag2(
                session_id,
                &out_dir,
                &Rosbag2ExportOptions { epoch_id, bag_name },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Command::AttachDaaamOutput {
            session_id,
            output_dir,
            dsg,
            stream_key,
            fragment_key,
            graph_kind,
            window_start_sec,
            window_end_sec,
        } => attach_daaam_output(
            &store,
            session_id,
            &output_dir,
            dsg.as_deref(),
            &stream_key,
            fragment_key.as_deref(),
            &graph_kind,
            window_start_sec,
            window_end_sec,
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Session import and browsing
// ---------------------------------------------------------------------------------------------

fn import_manifest(store: &SessionStore, manifest_path: &str) -> Result<()> {
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("read manifest {manifest_path}"))?;
    let manifest: SessionManifest = serde_json::from_str(&text)
        .with_context(|| format!("parse manifest {manifest_path}"))?;

    let filename = manifest
        .filename
        .unwrap_or_else(|| infer_filename(&manifest.path));
    let imported_at = manifest.imported_at.unwrap_or_else(now_iso8601);
    let session_id = store.upsert_session(&SessionUpsert {
        path: manifest.path,
        filename,
        media_kind: manifest.media_kind,
        analysis_kind: manifest.analysis_kind,
        status: manifest.status,
        duration_sec: manifest.duration_sec,
        imported_at,
    })?;

    for stream in manifest.streams {
        upsert_manifest_stream(store, session_id, stream)?;
    }
    show_session(store, session_id)
}

fn import_media(
    store: &SessionStore,
    path: &str,
    media_kind: &str,
    analysis_kind: &str,
    status: &str,
    duration_sec: f64,
    stream_specs: &[String],
) -> Result<()> {
    let session_id = store.upsert_session(&SessionUpsert {
        path: path.to_string(),
        filename: infer_filename(path),
        media_kind: media_kind.to_string(),
        analysis_kind: analysis_kind.to_string(),
        status: status.to_string(),
        duration_sec,
        imported_at: now_iso8601(),
    })?;

    if stream_specs.is_empty() {
        store.upsert_stream(&SessionStreamUpsert {
            session_id,
            stream_key: "primary".to_string(),
            stream_kind: "video".to_string(),
            role: "observation".to_string(),
            path: path.to_string(),
            sync_group: "default".to_string(),
            metadata_json: stream_metadata_json("video", path, json!({}))?,
        })?;
    } else {
        for spec in stream_specs {
            upsert_manifest_stream(store, session_id, parse_stream_spec(spec)?)?;
        }
    }

    show_session(store, session_id)
}

/// Parses one `--stream KEY:KIND:ROLE:PATH[:SYNC_GROUP]` spec.
fn parse_stream_spec(spec: &str) -> Result<StreamManifest> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() < 4 {
        bail!("invalid --stream spec `{spec}`; expected KEY:KIND:ROLE:PATH[:SYNC_GROUP]");
    }
    Ok(StreamManifest {
        key: parts[0].to_string(),
        kind: parts[1].to_string(),
        role: parts[2].to_string(),
        path: parts[3].to_string(),
        sync_group: parts.get(4).copied().unwrap_or("default").to_string(),
        metadata: json!({}),
    })
}

fn upsert_manifest_stream(
    store: &SessionStore,
    session_id: i64,
    stream: StreamManifest,
) -> Result<()> {
    let metadata_json = stream_metadata_json(&stream.kind, &stream.path, stream.metadata)?;
    store.upsert_stream(&SessionStreamUpsert {
        session_id,
        stream_key: stream.key,
        stream_kind: stream.kind,
        role: stream.role,
        path: stream.path,
        sync_group: stream.sync_group,
        metadata_json,
    })?;
    Ok(())
}

fn list_sessions(store: &SessionStore) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&store.list_sessions()?)?);
    Ok(())
}

fn show_session(store: &SessionStore, session_id: i64) -> Result<()> {
    let Some(session) = store.session_by_id(session_id)? else {
        bail!("session {session_id} not found");
    };
    let streams = store.list_streams(session_id)?;
    let envelope = SessionEnvelope {
        session: json!(session),
        streams: streams.into_iter().map(|row| json!(row)).collect(),
    };
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// ROS2 JSONL ingest
// ---------------------------------------------------------------------------------------------

fn import_ros2_jsonl(
    store: &mut SessionStore,
    session_id: i64,
    input: &str,
    epoch_index: i64,
    layer_name: &str,
    abstraction_family: &str,
    stream_key: &str,
) -> Result<()> {
    let Some(session) = store.session_by_id(session_id)? else {
        bail!("session {session_id} not found");
    };

    store.upsert_stream(&SessionStreamUpsert {
        session_id,
        stream_key: stream_key.to_string(),
        stream_kind: "ros2_jsonl".to_string(),
        role: "observation".to_string(),
        path: input.to_string(),
        sync_group: "ros2".to_string(),
        metadata_json: json!({"contract":"ros2-jsonl.v1"}).to_string(),
    })?;

    let now = now_iso8601();
    let epoch_id = store.upsert_epoch(&AbstractionEpochUpsert {
        session_id,
        epoch_index,
        layer_name: layer_name.to_string(),
        abstraction_family: abstraction_family.to_string(),
        status: "complete".to_string(),
        created_at: now.clone(),
        completed_at: Some(now),
        summary_json: json!({"source":"ros2-jsonl","path":input}).to_string(),
    })?;

    let mut spaces = make_ros2_spaces(epoch_id);
    let mut topics: HashMap<String, usize> = HashMap::new();
    let mut ingested_total = 0usize;
    let mut ignored_total = 0usize;

    let reader = BufReader::new(
        std::fs::File::open(input).with_context(|| format!("open {input}"))?,
    );
    let mut row_index = 0usize;
    for raw in reader.lines() {
        let raw = raw.with_context(|| format!("read line {}", row_index + 1))?;
        let row = ingest_ros2_row(&raw, row_index)?;
        row_index += 1;
        let Some(row) = row else {
            ignored_total += 1;
            continue;
        };
        *topics.entry(row.topic).or_insert(0) += row.samples.len();
        for (space_name, sample) in row.samples {
            if let Some(buffer) = spaces.get_mut(space_name) {
                ingested_total += 1;
                buffer.samples.push(sample);
            }
        }
    }

    for buffer in spaces.values_mut() {
        buffer.samples.sort_by_key(|sample| sample.step);
        let space_id = store.upsert_space(&buffer.upsert)?;
        store.replace_state_samples(epoch_id, space_id, &buffer.samples)?;
    }

    let report = Ros2IngestReport {
        session_id: session.id,
        epoch_id,
        ingested_total,
        ignored_total,
        topics,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// The five ROS2 topics this ingest accepts; every other topic is counted as ignored.
const ROS2_TOPICS: [&str; 5] = ["/tf", "/tf_static", "/odom", "/scan", "/cmd_vel"];

/// One accepted JSONL line: the topic that named it and the samples its mapper produced.
struct Ros2Row {
    topic: String,
    samples: Vec<(&'static str, AbstractStateSample)>,
}

/// Maps one JSONL line, or reports `None` when the line is blank, unnamed, off-contract or
/// produced no sample at all.
fn ingest_ros2_row(raw: &str, row_index: usize) -> Result<Option<Ros2Row>> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_str(raw).with_context(|| format!("parse json line {}", row_index + 1))?;
    let Some(topic) = value.get("topic").and_then(Value::as_str) else {
        return Ok(None);
    };
    if !ROS2_TOPICS.contains(&topic) {
        return Ok(None);
    }
    let samples = map_ros2_message(topic, &value, row_index as i64);
    if samples.is_empty() {
        return Ok(None);
    }
    Ok(Some(Ros2Row {
        topic: topic.to_string(),
        samples,
    }))
}

/// The five core ROS2 topic spaces, keyed by the space name the mapper emits.
fn make_ros2_spaces(epoch_id: i64) -> HashMap<&'static str, SpaceBuffer> {
    let specs: [(&'static str, &'static str, &'static str, Value); 5] = [
        (
            "tf_observation",
            "state",
            "frames",
            json!({"topic":"/tf","fields":["parent_frame_id","child_frame_id","translation","rotation","stamp"]}),
        ),
        (
            "tf_static_observation",
            "state",
            "frames",
            json!({"topic":"/tf_static","fields":["parent_frame_id","child_frame_id","translation","rotation","stamp"]}),
        ),
        (
            "odom_observation",
            "state",
            "pose_twist",
            json!({"topic":"/odom","fields":["frame_id","child_frame_id","pose","twist","stamp"]}),
        ),
        (
            "scan_observation",
            "state",
            "laser_scan",
            json!({"topic":"/scan","fields":["frame_id","angle_min","angle_max","ranges","range_min","range_max","stamp"]}),
        ),
        (
            "cmd_vel_action",
            "action",
            "twist",
            json!({"topic":"/cmd_vel","fields":["linear","angular","stamp"]}),
        ),
    ];

    specs
        .into_iter()
        .map(|(name, family, representation, schema)| {
            (
                name,
                SpaceBuffer {
                    upsert: AbstractionSpaceUpsert {
                        epoch_id,
                        space_family: family.to_string(),
                        abstraction_name: name.to_string(),
                        source_kind: "ros2".to_string(),
                        representation_kind: representation.to_string(),
                        uncertainty_kind: "reported".to_string(),
                        dimensionality: 1,
                        schema_json: schema.to_string(),
                    },
                    samples: Vec::new(),
                },
            )
        })
        .collect()
}

fn map_ros2_message(topic: &str, line: &Value, line_idx: i64) -> Vec<(&'static str, AbstractStateSample)> {
    let msg = line.get("msg").unwrap_or(line);
    let envelope_stamp = parse_stamp_value(line).or_else(|| parse_stamp_value(msg));

    match topic {
        "/tf" | "/tf_static" => tf_samples(topic, msg, line_idx, envelope_stamp),
        "/odom" => {
            let timestamp_sec = header_stamp(msg).or(envelope_stamp).unwrap_or(line_idx as f64);
            let child = msg.get("child_frame_id").and_then(Value::as_str).unwrap_or("");
            let symbol_key = format!("{}->{child}", header_frame_id(msg));
            vec![whole_message_sample(
                "odom_observation",
                topic,
                symbol_key,
                msg,
                line_idx,
                timestamp_sec,
            )]
        }
        "/scan" => {
            let timestamp_sec = header_stamp(msg).or(envelope_stamp).unwrap_or(line_idx as f64);
            let frame_id = msg
                .get("header")
                .and_then(|header| header.get("frame_id"))
                .and_then(Value::as_str)
                .unwrap_or("laser");
            vec![whole_message_sample(
                "scan_observation",
                topic,
                frame_id.to_string(),
                msg,
                line_idx,
                timestamp_sec,
            )]
        }
        "/cmd_vel" => {
            let timestamp_sec = envelope_stamp.unwrap_or(line_idx as f64);
            vec![whole_message_sample(
                "cmd_vel_action",
                topic,
                infer_cmd_vel_symbol(msg),
                msg,
                line_idx,
                timestamp_sec,
            )]
        }
        _ => Vec::new(),
    }
}

/// One sample per transform in a `/tf` or `/tf_static` message.
fn tf_samples(
    topic: &str,
    msg: &Value,
    line_idx: i64,
    envelope_stamp: Option<f64>,
) -> Vec<(&'static str, AbstractStateSample)> {
    let space_name = if topic == "/tf" {
        "tf_observation"
    } else {
        "tf_static_observation"
    };
    let transforms = msg
        .get("transforms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    transforms
        .into_iter()
        .enumerate()
        .map(|(index, transform)| {
            let header = transform.get("header");
            let timestamp_sec = header
                .and_then(parse_stamp_value)
                .or(envelope_stamp)
                .unwrap_or(line_idx as f64);
            let step = line_idx * 1000 + index as i64;
            let parent = header
                .and_then(|header| header.get("frame_id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let child = transform
                .get("child_frame_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let symbol_key = format!("{parent}->{child}");
            (
                space_name,
                AbstractStateSample {
                    step,
                    timestamp_sec,
                    symbol_key: symbol_key.clone(),
                    payload_json: transform.to_string(),
                    confidence: 1.0,
                    sample_hash: stable_hash(&format!("{topic}|{step}|{symbol_key}|{transform}")),
                },
            )
        })
        .collect()
}

/// One sample carrying the whole message the line delivered, stamped at the line's ordinal.
fn whole_message_sample(
    space_name: &'static str,
    topic: &str,
    symbol_key: String,
    msg: &Value,
    line_idx: i64,
    timestamp_sec: f64,
) -> (&'static str, AbstractStateSample) {
    (
        space_name,
        AbstractStateSample {
            step: line_idx,
            timestamp_sec,
            symbol_key,
            payload_json: msg.to_string(),
            confidence: 1.0,
            sample_hash: stable_hash(&format!("{topic}|{line_idx}|{msg}")),
        },
    )
}

fn header_stamp(msg: &Value) -> Option<f64> {
    msg.get("header").and_then(parse_stamp_value)
}

fn header_frame_id(msg: &Value) -> &str {
    msg.get("header")
        .and_then(|header| header.get("frame_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn parse_stamp_value(value: &Value) -> Option<f64> {
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    if let Some(stamp) = value.get("stamp") {
        return parse_stamp_struct(stamp);
    }
    for key in ["timestamp", "time", "t"] {
        if let Some(number) = value.get(key).and_then(Value::as_f64) {
            return Some(number);
        }
    }
    None
}

fn parse_stamp_struct(stamp: &Value) -> Option<f64> {
    if let Some(number) = stamp.as_f64() {
        return Some(number);
    }
    let seconds = stamp
        .get("sec")
        .or_else(|| stamp.get("secs"))
        .or_else(|| stamp.get("s"))
        .and_then(Value::as_f64)?;
    let nanoseconds = stamp
        .get("nanosec")
        .or_else(|| stamp.get("nsec"))
        .or_else(|| stamp.get("nsecs"))
        .or_else(|| stamp.get("ns"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some(seconds + nanoseconds / 1_000_000_000.0)
}

fn infer_cmd_vel_symbol(msg: &Value) -> String {
    let linear_x = msg
        .get("linear")
        .and_then(|linear| linear.get("x"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let angular_z = msg
        .get("angular")
        .and_then(|angular| angular.get("z"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    match (linear_x.abs() > 1e-6, angular_z.abs() > 1e-6) {
        (false, false) => "idle".to_string(),
        (true, false) => {
            if linear_x > 0.0 {
                "forward".to_string()
            } else {
                "reverse".to_string()
            }
        }
        (false, true) => {
            if angular_z > 0.0 {
                "turn_left".to_string()
            } else {
                "turn_right".to_string()
            }
        }
        (true, true) => "arc".to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Offline inference
// ---------------------------------------------------------------------------------------------

fn run_inference(
    store: &SessionStore,
    session_id: i64,
    environment_id: Option<i64>,
    window_start_sec: Option<f64>,
    window_end_sec: Option<f64>,
    graph_kind: &str,
    require_exact: bool,
) -> Result<()> {
    let spec = InferenceJobSpec {
        session_id,
        environment_id,
        window_start_sec,
        window_end_sec,
        graph_kind: parse_graph_kind(graph_kind)?,
        require_exact,
    };
    let result = run_offline_inference(store, spec)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn parse_graph_kind(raw: &str) -> Result<GraphKind> {
    match raw.to_ascii_lowercase().as_str() {
        "sensorimotor" => Ok(GraphKind::Sensorimotor),
        "local_space" | "local-space" | "localspace" => Ok(GraphKind::LocalSpace),
        "navigation" => Ok(GraphKind::Navigation),
        "object" => Ok(GraphKind::Object),
        "task" => Ok(GraphKind::Task),
        "communication" => Ok(GraphKind::Communication),
        _ => bail!(
            "invalid --graph-kind `{raw}`; expected one of: sensorimotor, local_space, navigation, object, task, communication"
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// DAAAM attachment
// ---------------------------------------------------------------------------------------------

fn attach_daaam_output(
    store: &SessionStore,
    session_id: i64,
    output_dir: &str,
    dsg_override: Option<&str>,
    stream_key: &str,
    fragment_key_override: Option<&str>,
    graph_kind: &str,
    window_start_sec: Option<f64>,
    window_end_sec: Option<f64>,
) -> Result<()> {
    let Some(session) = store.session_by_id(session_id)? else {
        bail!("session {session_id} not found");
    };

    let output_dir = Path::new(output_dir);
    if !output_dir.is_dir() {
        bail!("DAAAM output directory {} not found", output_dir.display());
    }

    let (dsg_path, dsg_text, scan) = load_daaam_graph(output_dir, dsg_override)?;
    let dsg_artifact_path = relative_path_or_display(&dsg_path, output_dir);
    let artifact_paths = list_daaam_artifact_paths(output_dir)?;
    let graph_kind = parse_graph_kind(graph_kind)?;
    let window_start_sec = window_start_sec.unwrap_or(0.0);
    let window_end_sec =
        window_end_sec.unwrap_or_else(|| session.duration_sec.max(window_start_sec));
    let fragment_key =
        daaam_fragment_key(fragment_key_override, output_dir, &dsg_path, &dsg_artifact_path);
    let root_node_key = scan
        .root_node_key
        .clone()
        .unwrap_or_else(|| "daaam/root".to_string());
    let confidence = daaam_region_confidence(&scan);
    let requested_at = now_iso8601();
    let completed_at = requested_at.clone();

    let spec_json = daaam_spec_json(
        output_dir,
        &dsg_artifact_path,
        stream_key,
        &fragment_key,
        &graph_kind,
    );
    let summary_json = daaam_summary_json(output_dir, &dsg_artifact_path, &artifact_paths, &scan);

    let job_upsert = AnalysisJobUpsert {
        session_id,
        environment_id: Some(session_id),
        job_kind: AnalysisJobKind::GraphInference,
        status: AnalysisJobStatus::Complete,
        requested_at,
        started_at: None,
        completed_at: Some(completed_at),
        window_start_sec: Some(window_start_sec),
        window_end_sec: Some(window_end_sec),
        spec_json,
        summary_json: summary_json.clone(),
        failure_json: None,
    };
    let analysis_job_id = store
        .upsert_analysis_job(&job_upsert)
        .context("persist DAAAM analysis job")?;

    let fragment_upsert = GraphFragmentUpsert {
        analysis_job_id,
        session_id,
        epoch_id: None,
        stream_key: stream_key.to_string(),
        fragment_key: fragment_key.clone(),
        graph_kind,
        graph_form: GraphForm::Loopy,
        exactness: ExactnessKind::Approximate,
        variable_count: scan.node_count as i64,
        factor_count: scan.edge_count as i64,
        tree_width: None,
        root_variable_key: Some(root_node_key),
        window_start_sec,
        window_end_sec,
        summary_json,
    };
    let graph_fragment_id = store
        .upsert_graph_fragment(&fragment_upsert)
        .context("persist DAAAM graph fragment")?;

    let (centroid_json, bounds_json, support_point_count) = daaam_region_geometry(&scan);
    let region_upsert = WorldRegionUpsert {
        environment_id: Some(session_id),
        session_id,
        source_fragment_id: graph_fragment_id,
        region_key: format!("daaam_world_{}", stable_hash(&fragment_key)),
        region_kind: RegionKind::LocalSpace,
        support_point_count,
        confidence,
        centroid_json,
        bounds_json,
        signature_hash: daaam_signature_hash(&fragment_key, &dsg_text),
        metadata_json: daaam_region_metadata_json(&fragment_key, &dsg_artifact_path, &scan),
    };
    let world_region_id = store
        .upsert_world_region(&region_upsert)
        .context("persist DAAAM world region")?;

    let report = DaaamAttachReport {
        session_id,
        analysis_job_id,
        graph_fragment_id,
        world_region_id,
        dsg_artifact_path,
        artifact_paths,
        node_count: scan.node_count,
        edge_count: scan.edge_count,
        layer_counts: scan.layer_counts,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Loads the DSG artifact and scans it, rejecting one that exposes neither node nor edge.
fn load_daaam_graph(
    output_dir: &Path,
    dsg_override: Option<&str>,
) -> Result<(PathBuf, String, DaaamGraphScan)> {
    let dsg_path = resolve_daaam_dsg_path(output_dir, dsg_override)?;
    let dsg_text = read_dsg_artifact(&dsg_path)?;
    let scan = scan_dsg_artifact(&dsg_path, &dsg_text)?;
    Ok((dsg_path, dsg_text, scan))
}

/// Reads the DSG artifact, naming it in any failure.
fn read_dsg_artifact(dsg_path: &Path) -> Result<String> {
    std::fs::read_to_string(dsg_path)
        .with_context(|| format!("read DAAAM DSG artifact {}", dsg_path.display()))
}

/// Parses a DSG artifact and rejects one that exposes neither node nor edge.
fn scan_dsg_artifact(dsg_path: &Path, dsg_text: &str) -> Result<DaaamGraphScan> {
    let dsg_json = serde_json::from_str::<Value>(dsg_text)
        .with_context(|| format!("parse DAAAM DSG artifact {}", dsg_path.display()))?;
    let scan = scan_daaam_graph(&dsg_json);
    match (scan.node_count, scan.edge_count) {
        (0, 0) => bail!(
            "DAAAM DSG artifact {} did not expose nodes or edges",
            dsg_path.display()
        ),
        _ => Ok(scan),
    }
}

/// The attachment's fragment key: the caller's override, else `daaam:<stem>:<location digest>`.
fn daaam_fragment_key(
    override_key: Option<&str>,
    output_dir: &Path,
    dsg_path: &Path,
    dsg_artifact_path: &str,
) -> String {
    if let Some(explicit) = override_key {
        return explicit.to_string();
    }
    let stem = dsg_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("dsg");
    let location = stable_hash(&format!("{}|{}", output_dir.display(), dsg_artifact_path));
    format!("daaam:{stem}:{location}")
}

/// The analysis job's spec payload.
fn daaam_spec_json(
    output_dir: &Path,
    dsg_artifact_path: &str,
    stream_key: &str,
    fragment_key: &str,
    graph_kind: &GraphKind,
) -> String {
    json!({
        "source": "daaam",
        "output_dir": output_dir.display().to_string(),
        "dsg_artifact_path": dsg_artifact_path,
        "stream_key": stream_key,
        "fragment_key": fragment_key,
        "graph_kind": graph_kind,
        "source_repo": "https://github.com/MIT-SPARK/DAAAM",
    })
    .to_string()
}

/// The analysis job's summary payload, carrying the scan's counts.
fn daaam_summary_json(
    output_dir: &Path,
    dsg_artifact_path: &str,
    artifact_paths: &[String],
    scan: &DaaamGraphScan,
) -> String {
    json!({
        "source": "daaam",
        "schema": "qualia.daaam_attachment.v1",
        "output_dir": output_dir.display().to_string(),
        "dsg_artifact_path": dsg_artifact_path,
        "artifact_paths": artifact_paths,
        "node_count": scan.node_count,
        "edge_count": scan.edge_count,
        "layer_counts": scan.layer_counts,
        "coordinate_sample_count": scan.points.len(),
        "source_repo": "https://github.com/MIT-SPARK/DAAAM",
    })
    .to_string()
}

/// The attachment's signature: its fragment key over the digest of the DSG text.
fn daaam_signature_hash(fragment_key: &str, dsg_text: &str) -> String {
    let artifact = stable_hash(dsg_text);
    stable_hash(&format!("{fragment_key}|{artifact}"))
}

/// The world region's provenance payload.
fn daaam_region_metadata_json(
    fragment_key: &str,
    dsg_artifact_path: &str,
    scan: &DaaamGraphScan,
) -> String {
    json!({
        "source": "daaam",
        "fragment_key": fragment_key,
        "dsg_artifact_path": dsg_artifact_path,
        "node_count": scan.node_count,
        "edge_count": scan.edge_count,
        "coordinate_sample_count": scan.points.len(),
        "coordinate_frame": "daaam_native",
    })
    .to_string()
}

fn resolve_daaam_dsg_path(output_dir: &Path, explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        let candidate = PathBuf::from(explicit);
        let resolved = match candidate.is_absolute() {
            true => candidate,
            false => output_dir.join(candidate),
        };
        if resolved.is_file() {
            return Ok(resolved);
        }
        bail!("DAAAM DSG artifact {} not found", resolved.display());
    }
    let expected = ["clustered_dsg.json", "dsg_updated.json", "dsg.json"];
    let found = expected
        .iter()
        .map(|name| output_dir.join(name))
        .find(|candidate| candidate.is_file());
    match found {
        Some(path) => Ok(path),
        None => bail!(
            "no DAAAM DSG artifact found in {}; expected clustered_dsg.json, dsg_updated.json, or dsg.json",
            output_dir.display()
        ),
    }
}

fn list_daaam_artifact_paths(output_dir: &Path) -> Result<Vec<String>> {
    let listing = std::fs::read_dir(output_dir)
        .with_context(|| format!("read DAAAM output directory {}", output_dir.display()))?;
    let mut paths = Vec::new();
    for entry in listing {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        paths.push(relative_path_or_display(&path, output_dir));
    }
    paths.sort();
    Ok(paths)
}

fn scan_daaam_graph(value: &Value) -> DaaamGraphScan {
    let mut scan = DaaamGraphScan::default();
    scan_daaam_value(value, None, &mut scan);
    scan
}

/// Walks one DSG JSON node, folding whatever evidence it carries into `scan`.
fn scan_daaam_value(value: &Value, layer: Option<&str>, scan: &mut DaaamGraphScan) {
    match value {
        Value::Object(object) => scan_daaam_object(object, layer, scan),
        Value::Array(values) => {
            for child in values {
                scan_daaam_value(child, layer, scan);
            }
        }
        _ => {}
    }
}

/// Counts and recurses through one DSG object's graph members, then visits every other child.
fn scan_daaam_object(object: &Map<String, Value>, layer: Option<&str>, scan: &mut DaaamGraphScan) {
    if let Some(point) = daaam_point_from_object(object) {
        scan.points.push(point);
    }
    if let Some(confidence) = daaam_confidence_from_object(object) {
        scan.confidence_sum += confidence;
        scan.confidence_count += 1;
    }
    if let Some(nodes) = object.get("nodes") {
        let count = daaam_collection_count(nodes);
        scan.node_count += count;
        if count > 0 {
            let layer_key = layer.unwrap_or("graph").to_string();
            *scan.layer_counts.entry(layer_key).or_insert(0) += count;
            if scan.root_node_key.is_none() {
                scan.root_node_key = first_daaam_node_key(nodes);
            }
        }
        scan_daaam_value(nodes, layer, scan);
    }
    for container in ["edges", "interlayer_edges", "layer_edges"] {
        if let Some(edges) = object.get(container) {
            scan.edge_count += daaam_collection_count(edges);
            scan_daaam_value(edges, layer, scan);
        }
    }
    if let Some(layers) = object.get("layers") {
        scan_daaam_layers(layers, layer, scan);
    }
    for (key, child) in object {
        if !is_daaam_graph_member(key) {
            scan_daaam_value(child, layer, scan);
        }
    }
}

/// True for the DSG keys that `scan_daaam_object` already visits by name.
fn is_daaam_graph_member(key: &str) -> bool {
    matches!(
        key,
        "nodes" | "edges" | "interlayer_edges" | "layer_edges" | "layers"
    )
}

/// Recurses through a `layers` member, labelling the layer it tracks where the DSG names one.
fn scan_daaam_layers(layers: &Value, layer: Option<&str>, scan: &mut DaaamGraphScan) {
    match layers {
        Value::Object(layer_map) => {
            for (name, layer_value) in layer_map {
                scan_daaam_value(layer_value, Some(name), scan);
            }
        }
        Value::Array(layer_values) => {
            for layer_value in layer_values {
                let name = daaam_layer_name(layer_value);
                scan_daaam_value(layer_value, Some(name), scan);
            }
        }
        _ => scan_daaam_value(layers, layer, scan),
    }
}

/// The layer name a DSG layer entry declares, or `layer` when it declares none.
fn daaam_layer_name(layer_value: &Value) -> &str {
    layer_value
        .get("name")
        .or_else(|| layer_value.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("layer")
}

fn daaam_collection_count(value: &Value) -> usize {
    value
        .as_array()
        .map(Vec::len)
        .or_else(|| value.as_object().map(Map::len))
        .unwrap_or(0)
}

fn first_daaam_node_key(value: &Value) -> Option<String> {
    let Value::Object(object) = value else {
        return value
            .as_array()
            .and_then(|values| values.iter().find_map(node_key_from_value));
    };
    object
        .keys()
        .next()
        .cloned()
        .or_else(|| object.values().find_map(node_key_from_value))
}

fn node_key_from_value(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let fields = ["id", "key", "node_id", "label", "name"];
    fields
        .iter()
        .find_map(|field| object.get(*field).and_then(daaam_identifier))
}

/// The identifier a DSG value carries, as text: its string form, else its integer form.
fn daaam_identifier(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    value.as_i64().map(|number| number.to_string())
}

fn daaam_point_from_object(object: &Map<String, Value>) -> Option<DaaamPoint> {
    for field in ["position", "pos", "centroid", "translation"] {
        if let Some(point) = object.get(field).and_then(point_from_array) {
            return Some(point);
        }
    }
    Some(DaaamPoint {
        x: number_field(object, &["x", "tx", "px"])?,
        y: number_field(object, &["y", "ty", "py"])?,
        z: number_field(object, &["z", "tz", "pz"]).unwrap_or(0.0),
    })
}

fn point_from_array(value: &Value) -> Option<DaaamPoint> {
    let coords = value.as_array()?;
    if coords.len() < 2 {
        return None;
    }
    Some(DaaamPoint {
        x: coords[0].as_f64()?,
        y: coords[1].as_f64()?,
        z: coords.get(2).and_then(Value::as_f64).unwrap_or(0.0),
    })
}

fn number_field(object: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(number) = object.get(*key).and_then(Value::as_f64) {
            return Some(number);
        }
    }
    None
}

fn daaam_confidence_from_object(object: &Map<String, Value>) -> Option<f64> {
    let reported = number_field(object, &["confidence", "score", "probability"])?;
    Some(reported.clamp(0.0, 1.0))
}

fn daaam_region_confidence(scan: &DaaamGraphScan) -> f64 {
    match scan.confidence_count {
        0 => 1.0,
        count => (scan.confidence_sum / count as f64).clamp(0.0, 1.0),
    }
}

fn daaam_region_geometry(scan: &DaaamGraphScan) -> (String, String, i64) {
    if scan.points.is_empty() {
        let bounds = json!([[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]).to_string();
        return (json!([0.0, 0.0]).to_string(), bounds, scan.node_count as i64);
    }
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let (mut sum_x, mut sum_y, mut sum_z) = (0.0f64, 0.0f64, 0.0f64);
    for point in &scan.points {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
        sum_x += point.x;
        sum_y += point.y;
        sum_z += point.z;
    }
    let count = scan.points.len() as f64;
    let centroid = json!([sum_x / count, sum_y / count, sum_z / count]).to_string();
    let bounds = json!([[min_x, min_y], [max_x, min_y], [max_x, max_y], [min_x, max_y]]).to_string();
    (centroid, bounds, scan.points.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::parse_ffprobe_frame_rate;

    #[test]
    fn frame_rate_rationals_and_decimals_are_both_accepted() {
        assert_eq!(parse_ffprobe_frame_rate("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_ffprobe_frame_rate(" 25 "), Some(25.0));
        assert_eq!(parse_ffprobe_frame_rate("0/0"), None);
        assert_eq!(parse_ffprobe_frame_rate(""), None);
        assert_eq!(parse_ffprobe_frame_rate("abc"), None);
    }

    #[test]
    fn cmd_vel_symbols_cover_every_axis_combination() {
        let symbol = |linear_x: f64, angular_z: f64| {
            infer_cmd_vel_symbol(&json!({
                "linear": {"x": linear_x},
                "angular": {"z": angular_z}
            }))
        };
        assert_eq!(symbol(0.0, 0.0), "idle");
        assert_eq!(symbol(0.4, 0.0), "forward");
        assert_eq!(symbol(-0.4, 0.0), "reverse");
        assert_eq!(symbol(0.0, 0.2), "turn_left");
        assert_eq!(symbol(0.0, -0.2), "turn_right");
        assert_eq!(symbol(0.4, 0.2), "arc");
    }

    #[test]
    fn stamp_shapes_cover_ros2_legacy_and_plain_numbers() {
        assert_eq!(parse_stamp_value(&json!(1.5)), Some(1.5));
        assert_eq!(
            parse_stamp_value(&json!({"stamp": {"sec": 3, "nanosec": 250000000}})),
            Some(3.25)
        );
        assert_eq!(
            parse_stamp_value(&json!({"stamp": {"secs": 4, "nsecs": 500000000}})),
            Some(4.5)
        );
        assert_eq!(parse_stamp_value(&json!({"timestamp": 9.0})), Some(9.0));
        assert_eq!(parse_stamp_value(&json!({"stamp": {}})), None);
    }

    #[test]
    fn daaam_scan_counts_layers_nodes_edges_and_confidence() {
        let dsg = json!({
            "layers": {
                "places": {"nodes": [{"id": "p0", "position": [1.0, 2.0, 0.0]}]},
                "objects": {"nodes": [
                    {"id": "o0", "position": [3.0, 4.0, 0.0], "confidence": 0.9},
                    {"id": "o1", "position": [5.0, 6.0, 0.0], "score": 0.7}
                ]}
            },
            "edges": [{"source": "p0", "target": "o0"}]
        });
        let scan = scan_daaam_graph(&dsg);
        assert_eq!(scan.node_count, 3);
        assert_eq!(scan.edge_count, 1);
        assert_eq!(scan.layer_counts["places"], 1);
        assert_eq!(scan.layer_counts["objects"], 2);
        assert_eq!(scan.root_node_key.as_deref(), Some("o0"));
        assert_eq!(scan.points.len(), 3);
        assert!((daaam_region_confidence(&scan) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn graph_kind_parsing_accepts_aliases_and_rejects_unknown_names() {
        assert_eq!(parse_graph_kind("sensorimotor").unwrap(), GraphKind::Sensorimotor);
        assert_eq!(parse_graph_kind("Localspace").unwrap(), GraphKind::LocalSpace);
        assert_eq!(parse_graph_kind("local-space").unwrap(), GraphKind::LocalSpace);
        assert!(parse_graph_kind("telepathy").is_err());
    }
}
