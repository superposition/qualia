//! `qualia-abstraction`: turn recorded pose, control and gimbal traces into
//! grounded abstraction epochs, spaces and state samples in the session store.
//!
//! Each trace yields one space under the sensorimotor epoch; the pose trace
//! yields a second space under the navigation epoch. Every stored sample
//! carries a discrete symbol, a continuous payload and a content hash.

use std::collections::HashMap;
use std::fs::File;

use anyhow::{bail, Context, Result};
use clap::Parser;
use csv::{Reader, ReaderBuilder, StringRecord};
use qualia_session_store::{
    AbstractStateSample, AbstractionEpochUpsert, AbstractionSpaceUpsert, SessionStore,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Store opened when `--store` is not given.
const DEFAULT_STORE_PATH: &str = "artifacts/qualia_session_store.sqlite";

#[derive(Debug, Parser)]
#[command(name = "qualia-abstraction")]
#[command(about = "Build grounded abstraction epochs from pose, control, and gimbal traces")]
struct Cli {
    #[arg(long, default_value = DEFAULT_STORE_PATH)]
    store: String,
    #[arg(long)]
    session_id: i64,
    #[arg(long)]
    pose_csv: String,
    #[arg(long)]
    created_at: String,
    #[arg(long)]
    completed_at: Option<String>,
    #[arg(long, default_value_t = 0)]
    epoch_base: i64,
    #[arg(long)]
    control_csv: Option<String>,
    #[arg(long)]
    gimbal_csv: Option<String>,
}

/// One pose trace row.
#[derive(Debug, Clone)]
struct PoseSample {
    step: i64,
    timestamp_sec: f64,
    x: f64,
    y: f64,
    z: f64,
    yaw: f64,
}

/// One control trace row.
#[derive(Debug, Clone)]
struct ControlSample {
    step: i64,
    timestamp_sec: f64,
    throttle: f64,
    yaw: f64,
    roll: f64,
    pitch: f64,
}

/// One gimbal trace row.
#[derive(Debug, Clone)]
struct GimbalSample {
    step: i64,
    timestamp_sec: f64,
    yaw: f64,
    pitch: f64,
    roll: f64,
}

/// What one run published, as printed to stdout.
#[derive(Debug, Serialize)]
struct RunSummary {
    session_id: i64,
    sensorimotor_epoch_id: i64,
    navigation_epoch_id: i64,
    spaces: Vec<SpaceSummary>,
}

/// One published space within the run summary.
#[derive(Debug, Serialize)]
struct SpaceSummary {
    epoch_id: i64,
    name: String,
    samples: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store =
        SessionStore::open(&cli.store).with_context(|| format!("open store {}", cli.store))?;
    require_session(&store, &cli)?;
    let traces = Traces::read(&cli)?;
    let report = publish(&store, &cli, &traces)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Refuse to build anything for a session the store does not hold.
fn require_session(store: &SessionStore, cli: &Cli) -> Result<()> {
    match store.session_by_id(cli.session_id)? {
        Some(_) => Ok(()),
        None => bail!("session {} not found in {}", cli.session_id, cli.store),
    }
}

/// The traces a run was given, already validated.
struct Traces {
    pose: Vec<PoseSample>,
    control: Option<Vec<ControlSample>>,
    gimbal: Option<Vec<GimbalSample>>,
}

impl Traces {
    fn read(cli: &Cli) -> Result<Self> {
        let pose = read_pose_samples(&cli.pose_csv)?;
        if pose.is_empty() {
            bail!("pose csv {} produced no rows", cli.pose_csv);
        }
        let control = match cli.control_csv.as_deref() {
            Some(path) => {
                let rows = read_control_samples(path)?;
                if rows.is_empty() {
                    bail!("control csv {} produced no rows", path);
                }
                Some(rows)
            }
            None => None,
        };
        let gimbal = match cli.gimbal_csv.as_deref() {
            Some(path) => {
                let rows = read_gimbal_samples(path)?;
                if rows.is_empty() {
                    bail!("gimbal csv {} produced no rows", path);
                }
                Some(rows)
            }
            None => None,
        };
        Ok(Self {
            pose,
            control,
            gimbal,
        })
    }
}

/// Persist the epochs, spaces and derived samples, and describe the run.
fn publish(store: &SessionStore, cli: &Cli, traces: &Traces) -> Result<RunSummary> {
    let sensorimotor_epoch_id = store.upsert_epoch(&epoch_upsert(
        cli,
        cli.epoch_base,
        "sensorimotor",
        "paired_observation_action",
        json!({
            "pose_csv": cli.pose_csv,
            "control_csv": cli.control_csv,
            "gimbal_csv": cli.gimbal_csv,
            "pose_rows": traces.pose.len(),
            "control_rows": traces.control.as_ref().map(Vec::len).unwrap_or(0),
            "gimbal_rows": traces.gimbal.as_ref().map(Vec::len).unwrap_or(0),
        }),
    ))?;
    let navigation_epoch_id = store.upsert_epoch(&epoch_upsert(
        cli,
        cli.epoch_base + 1,
        "navigation",
        "route_regime",
        json!({
            "pose_csv": cli.pose_csv,
            "pose_rows": traces.pose.len(),
        }),
    ))?;

    let mut report = RunSummary {
        session_id: cli.session_id,
        sensorimotor_epoch_id,
        navigation_epoch_id,
        spaces: Vec::new(),
    };

    let pose_space_id = store.upsert_space(&AbstractionSpaceUpsert {
        epoch_id: sensorimotor_epoch_id,
        space_family: "observation".into(),
        abstraction_name: "pose_regime".into(),
        source_kind: "pose_csv".into(),
        representation_kind: "discrete+continuous".into(),
        uncertainty_kind: "deterministic".into(),
        dimensionality: 6,
        schema_json: schema(
            "grounded pose regime",
            &["x", "y", "z", "yaw", "speed", "turn_rate"],
        ),
    })?;
    let pose_states = pose_regime(&traces.pose);
    store.replace_state_samples(sensorimotor_epoch_id, pose_space_id, &pose_states)?;
    report.spaces.push(SpaceSummary {
        epoch_id: sensorimotor_epoch_id,
        name: "pose_regime".into(),
        samples: pose_states.len(),
    });

    if let Some(control) = &traces.control {
        let control_space_id = store.upsert_space(&AbstractionSpaceUpsert {
            epoch_id: sensorimotor_epoch_id,
            space_family: "action".into(),
            abstraction_name: "control_regime".into(),
            source_kind: "control_csv".into(),
            representation_kind: "discrete+continuous".into(),
            uncertainty_kind: "estimated".into(),
            dimensionality: 4,
            schema_json: schema("grounded action regime", &["throttle", "yaw", "roll", "pitch"]),
        })?;
        let control_states = control_regime(control);
        store.replace_state_samples(sensorimotor_epoch_id, control_space_id, &control_states)?;
        report.spaces.push(SpaceSummary {
            epoch_id: sensorimotor_epoch_id,
            name: "control_regime".into(),
            samples: control_states.len(),
        });
    }

    if let Some(gimbal) = &traces.gimbal {
        let gimbal_space_id = store.upsert_space(&AbstractionSpaceUpsert {
            epoch_id: sensorimotor_epoch_id,
            space_family: "action".into(),
            abstraction_name: "gimbal_regime".into(),
            source_kind: "gimbal_csv".into(),
            representation_kind: "discrete+continuous".into(),
            uncertainty_kind: "estimated".into(),
            dimensionality: 3,
            schema_json: schema("centered gimbal regime", &["yaw", "pitch", "roll"]),
        })?;
        let gimbal_states = gimbal_regime(gimbal);
        store.replace_state_samples(sensorimotor_epoch_id, gimbal_space_id, &gimbal_states)?;
        report.spaces.push(SpaceSummary {
            epoch_id: sensorimotor_epoch_id,
            name: "gimbal_regime".into(),
            samples: gimbal_states.len(),
        });
    }

    let route_space_id = store.upsert_space(&AbstractionSpaceUpsert {
        epoch_id: navigation_epoch_id,
        space_family: "state".into(),
        abstraction_name: "route_regime".into(),
        source_kind: "pose_csv".into(),
        representation_kind: "discrete+continuous".into(),
        uncertainty_kind: "estimated".into(),
        dimensionality: 5,
        schema_json: schema(
            "trajectory geometry regime",
            &["progress", "segment_length", "speed", "turn_rate", "heading_delta"],
        ),
    })?;
    let route_states = route_regime(&traces.pose);
    store.replace_state_samples(navigation_epoch_id, route_space_id, &route_states)?;
    report.spaces.push(SpaceSummary {
        epoch_id: navigation_epoch_id,
        name: "route_regime".into(),
        samples: route_states.len(),
    });

    Ok(report)
}

/// The epoch row a run writes, stamped with the run's timestamps.
fn epoch_upsert(
    cli: &Cli,
    epoch_index: i64,
    layer: &str,
    family: &str,
    summary: Value,
) -> AbstractionEpochUpsert {
    AbstractionEpochUpsert {
        session_id: cli.session_id,
        epoch_index,
        layer_name: layer.into(),
        abstraction_family: family.into(),
        status: "complete".into(),
        created_at: cli.created_at.clone(),
        completed_at: cli.completed_at.clone(),
        summary_json: summary.to_string(),
    }
}

/// The `label`/`fields` document a space publishes as its schema.
fn schema(label: &str, fields: &[&str]) -> String {
    json!({ "label": label, "fields": fields }).to_string()
}

/// Open a trace CSV, naming the trace in the failure.
fn trace_reader(path: &str, trace: &str) -> Result<Reader<File>> {
    ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("open {} csv {}", trace, path))
}

fn read_pose_samples(path: &str) -> Result<Vec<PoseSample>> {
    let mut reader = trace_reader(path, "pose")?;
    let columns = Columns::from_headers(
        reader
            .headers()
            .with_context(|| format!("read headers {}", path))?,
    );
    let mut samples = Vec::new();
    for (row, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("read row {} from {}", row + 1, path))?;
        samples.push(PoseSample {
            step: columns
                .integer(&record, &["step", "frame", "index"])
                .unwrap_or(row as i64),
            timestamp_sec: columns
                .number(&record, &["timestamp_sec", "t", "time_sec", "timestamp"])
                .unwrap_or(row as f64 / 30.0),
            x: columns
                .number(&record, &["x_m", "x_ft", "x", "tx"])
                .unwrap_or_default(),
            y: columns
                .number(&record, &["y_m", "y_ft", "y", "ty"])
                .unwrap_or_default(),
            z: columns
                .number(&record, &["z_m", "z_ft", "z", "tz"])
                .unwrap_or_default(),
            yaw: columns
                .number(&record, &["yaw_rad", "yaw_deg", "yaw"])
                .unwrap_or_default(),
        });
    }
    Ok(samples)
}

fn read_control_samples(path: &str) -> Result<Vec<ControlSample>> {
    let mut reader = trace_reader(path, "control")?;
    let columns = Columns::from_headers(reader.headers()?);
    let mut samples = Vec::new();
    for (row, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("read row {} from {}", row + 1, path))?;
        let sample = ControlSample {
            step: columns
                .integer(&record, &["step", "frame", "index"])
                .unwrap_or(row as i64),
            timestamp_sec: columns
                .number(&record, &["timestamp_sec", "t", "time_sec", "timestamp"])
                .unwrap_or(row as f64 / 30.0),
            throttle: columns
                .number(&record, &["throttle", "left_v", "thrust"])
                .unwrap_or_default(),
            yaw: columns
                .number(&record, &["yaw", "left_u"])
                .unwrap_or_default(),
            roll: columns
                .number(&record, &["roll", "right_u"])
                .unwrap_or_default(),
            pitch: columns
                .number(&record, &["pitch", "right_v"])
                .unwrap_or_default(),
        };
        validate_control_sample(path, row + 1, &sample)?;
        samples.push(sample);
    }
    Ok(samples)
}

fn read_gimbal_samples(path: &str) -> Result<Vec<GimbalSample>> {
    let mut reader = trace_reader(path, "gimbal")?;
    let columns = Columns::from_headers(reader.headers()?);
    let mut samples = Vec::new();
    for (row, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("read row {} from {}", row + 1, path))?;
        let sample = GimbalSample {
            step: columns
                .integer(&record, &["step", "frame", "index"])
                .unwrap_or(row as i64),
            timestamp_sec: columns
                .number(&record, &["timestamp_sec", "t", "time_sec", "timestamp"])
                .unwrap_or(row as f64 / 30.0),
            yaw: columns
                .number(&record, &["gimbal_yaw", "yaw"])
                .unwrap_or_default(),
            pitch: columns
                .number(&record, &["gimbal_pitch", "pitch"])
                .unwrap_or_default(),
            roll: columns
                .number(&record, &["gimbal_roll", "roll"])
                .unwrap_or_default(),
        };
        validate_gimbal_sample(path, row + 1, &sample)?;
        samples.push(sample);
    }
    Ok(samples)
}

fn validate_control_sample(path: &str, row_idx: usize, sample: &ControlSample) -> Result<()> {
    for (field, value) in [
        ("timestamp_sec", sample.timestamp_sec),
        ("throttle", sample.throttle),
        ("yaw", sample.yaw),
        ("roll", sample.roll),
        ("pitch", sample.pitch),
    ] {
        finite(path, row_idx, field, value)?;
    }
    Ok(())
}

fn validate_gimbal_sample(path: &str, row_idx: usize, sample: &GimbalSample) -> Result<()> {
    for (field, value) in [
        ("timestamp_sec", sample.timestamp_sec),
        ("yaw", sample.yaw),
        ("pitch", sample.pitch),
        ("roll", sample.roll),
    ] {
        finite(path, row_idx, field, value)?;
    }
    Ok(())
}

fn finite(path: &str, row_idx: usize, field: &str, value: f64) -> Result<()> {
    if value.is_finite() {
        return Ok(());
    }
    bail!(
        "{} row {} has non-finite {} value ({})",
        path,
        row_idx,
        field,
        value
    )
}

/// Grounded observation regimes over the pose trace.
fn pose_regime(samples: &[PoseSample]) -> Vec<AbstractStateSample> {
    let mut states = Vec::with_capacity(samples.len());
    let mut previous: Option<&PoseSample> = None;
    for sample in samples {
        let (speed, climb_rate, turn_rate) = match previous {
            Some(prev) => {
                let dt = (sample.timestamp_sec - prev.timestamp_sec).abs().max(1e-3);
                (
                    span(prev, sample) / dt,
                    (sample.z - prev.z) / dt,
                    (sample.yaw - prev.yaw) / dt,
                )
            }
            None => (0.0, 0.0, 0.0),
        };
        let symbol = pose_symbol(speed, climb_rate, turn_rate);
        states.push(AbstractStateSample {
            step: sample.step,
            timestamp_sec: sample.timestamp_sec,
            symbol_key: symbol.to_string(),
            payload_json: json!({
                "x": sample.x,
                "y": sample.y,
                "z": sample.z,
                "yaw": sample.yaw,
                "speed": speed,
                "vertical_rate": climb_rate,
                "turn_rate": turn_rate,
            })
            .to_string(),
            confidence: 0.9,
            sample_hash: digest(&[
                &sample.step.to_string(),
                &sample.timestamp_sec.to_string(),
                symbol,
                &sample.x.to_string(),
                &sample.y.to_string(),
                &sample.z.to_string(),
                &speed.to_string(),
                &turn_rate.to_string(),
            ]),
        });
        previous = Some(sample);
    }
    states
}

/// Grounded action regimes over the control trace.
fn control_regime(samples: &[ControlSample]) -> Vec<AbstractStateSample> {
    let mut states = Vec::with_capacity(samples.len());
    let mut previous: Option<[f64; 4]> = None;
    for sample in samples {
        let normalized = unit4([sample.throttle, sample.yaw, sample.roll, sample.pitch]);
        let delta = match previous {
            Some(prev) => [
                normalized[0] - prev[0],
                normalized[1] - prev[1],
                normalized[2] - prev[2],
                normalized[3] - prev[3],
            ],
            None => [0.0; 4],
        };
        let symbol = control_symbol(sample);
        let delta_hash = digest(&[
            &sample.step.to_string(),
            &sample.timestamp_sec.to_string(),
            &delta[0].to_string(),
            &delta[1].to_string(),
            &delta[2].to_string(),
            &delta[3].to_string(),
        ]);
        states.push(AbstractStateSample {
            step: sample.step,
            timestamp_sec: sample.timestamp_sec,
            symbol_key: symbol.to_string(),
            payload_json: json!({
                "throttle": sample.throttle,
                "yaw": sample.yaw,
                "roll": sample.roll,
                "pitch": sample.pitch,
                "normalized": {
                    "throttle": normalized[0],
                    "yaw": normalized[1],
                    "roll": normalized[2],
                    "pitch": normalized[3],
                },
                "delta": {
                    "throttle": delta[0],
                    "yaw": delta[1],
                    "roll": delta[2],
                    "pitch": delta[3],
                },
                "delta_hash": delta_hash,
                "action_summary": format!("{}:{:.3}", symbol, magnitude4(&normalized)),
            })
            .to_string(),
            confidence: 0.85,
            sample_hash: digest(&[
                &sample.step.to_string(),
                &sample.timestamp_sec.to_string(),
                symbol,
                &sample.throttle.to_string(),
                &sample.yaw.to_string(),
                &sample.roll.to_string(),
                &sample.pitch.to_string(),
            ]),
        });
        previous = Some(normalized);
    }
    states
}

/// Grounded action regimes over the gimbal trace.
fn gimbal_regime(samples: &[GimbalSample]) -> Vec<AbstractStateSample> {
    let mut states = Vec::with_capacity(samples.len());
    let mut previous: Option<[f64; 3]> = None;
    for sample in samples {
        let normalized = unit3([sample.yaw, sample.pitch, sample.roll]);
        let magnitude = sample
            .yaw
            .abs()
            .max(sample.pitch.abs())
            .max(sample.roll.abs());
        let delta = match previous {
            Some(prev) => [
                normalized[0] - prev[0],
                normalized[1] - prev[1],
                normalized[2] - prev[2],
            ],
            None => [0.0; 3],
        };
        let symbol = gimbal_symbol(sample.yaw, sample.pitch, magnitude);
        let delta_hash = digest(&[
            &sample.step.to_string(),
            &sample.timestamp_sec.to_string(),
            &delta[0].to_string(),
            &delta[1].to_string(),
            &delta[2].to_string(),
        ]);
        states.push(AbstractStateSample {
            step: sample.step,
            timestamp_sec: sample.timestamp_sec,
            symbol_key: symbol.to_string(),
            payload_json: json!({
                "yaw": sample.yaw,
                "pitch": sample.pitch,
                "roll": sample.roll,
                "magnitude": magnitude,
                "normalized": {
                    "yaw": normalized[0],
                    "pitch": normalized[1],
                    "roll": normalized[2],
                },
                "delta": {
                    "yaw": delta[0],
                    "pitch": delta[1],
                    "roll": delta[2],
                },
                "delta_hash": delta_hash,
                "action_summary": format!("{}:{:.3}", symbol, magnitude3(&normalized)),
            })
            .to_string(),
            confidence: 0.82,
            sample_hash: digest(&[
                &sample.step.to_string(),
                &sample.timestamp_sec.to_string(),
                symbol,
                &sample.yaw.to_string(),
                &sample.pitch.to_string(),
                &sample.roll.to_string(),
            ]),
        });
        previous = Some(normalized);
    }
    states
}

/// Grounded trajectory regimes over the pose trace.
fn route_regime(samples: &[PoseSample]) -> Vec<AbstractStateSample> {
    let total_length = samples
        .windows(2)
        .map(|pair| span(&pair[0], &pair[1]))
        .sum::<f64>()
        .max(1e-6);
    let mut travelled = 0.0;
    let mut states = Vec::with_capacity(samples.len());
    let mut previous: Option<&PoseSample> = None;
    for sample in samples {
        let (segment_length, turn_rate, heading_delta) = match previous {
            Some(prev) => {
                let dt = (sample.timestamp_sec - prev.timestamp_sec).abs().max(1e-3);
                let segment_length = span(prev, sample);
                travelled += segment_length;
                let heading_delta = sample.yaw - prev.yaw;
                (segment_length, heading_delta / dt, heading_delta)
            }
            None => (0.0, 0.0, 0.0),
        };
        let progress = (travelled / total_length).clamp(0.0, 1.0);
        let symbol = route_symbol(segment_length, turn_rate, progress);
        states.push(AbstractStateSample {
            step: sample.step,
            timestamp_sec: sample.timestamp_sec,
            symbol_key: symbol.to_string(),
            payload_json: json!({
                "progress": progress,
                "segment_length": segment_length,
                "turn_rate": turn_rate,
                "heading_delta": heading_delta,
                "x": sample.x,
                "y": sample.y,
                "z": sample.z,
            })
            .to_string(),
            confidence: 0.78,
            sample_hash: digest(&[
                &sample.step.to_string(),
                &sample.timestamp_sec.to_string(),
                symbol,
                &progress.to_string(),
                &segment_length.to_string(),
                &turn_rate.to_string(),
            ]),
        });
        previous = Some(sample);
    }
    states
}

/// The discrete pose symbol, in the order the thresholds are tested.
fn pose_symbol(speed: f64, climb_rate: f64, turn_rate: f64) -> &'static str {
    let turning = turn_rate.abs();
    if speed < 0.15 && turning < 0.15 {
        "hover"
    } else if climb_rate > 0.15 {
        "climb"
    } else if climb_rate < -0.15 {
        "descent"
    } else if turning > 0.75 && speed < 0.75 {
        "yaw_turn"
    } else if turning > 0.35 {
        "arc_turn"
    } else if speed > 1.25 {
        "transit_fast"
    } else {
        "transit"
    }
}

/// The discrete control symbol: the axis with the largest command, or a stall.
fn control_symbol(sample: &ControlSample) -> &'static str {
    let axes = [
        ("throttle", sample.throttle),
        ("yaw", sample.yaw),
        ("roll", sample.roll),
        ("pitch", sample.pitch),
    ];
    let (axis, value) = axes
        .iter()
        .copied()
        .max_by(|left, right| left.1.abs().partial_cmp(&right.1.abs()).unwrap())
        .unwrap();
    if value.abs() < 0.12 {
        return "neutral";
    }
    let positive = value >= 0.0;
    match axis {
        "throttle" => {
            if positive {
                "throttle_up"
            } else {
                "throttle_down"
            }
        }
        "yaw" => {
            if positive {
                "yaw_right"
            } else {
                "yaw_left"
            }
        }
        "roll" => {
            if positive {
                "roll_right"
            } else {
                "roll_left"
            }
        }
        _ => {
            if positive {
                "pitch_forward"
            } else {
                "pitch_back"
            }
        }
    }
}

/// The discrete gimbal symbol: centred, or whichever axis scans further.
fn gimbal_symbol(yaw: f64, pitch: f64, magnitude: f64) -> &'static str {
    if magnitude < 0.08 {
        "centered"
    } else if pitch.abs() > yaw.abs() {
        "pitch_scan"
    } else {
        "yaw_scan"
    }
}

/// The discrete trajectory symbol, in the order the thresholds are tested.
fn route_symbol(segment_length: f64, turn_rate: f64, progress: f64) -> &'static str {
    if segment_length < 0.05 {
        "station_keeping"
    } else if turn_rate.abs() > 0.35 {
        "turn_segment"
    } else if progress > 0.9 {
        "egress"
    } else {
        "corridor_follow"
    }
}

/// Euclidean distance between two poses.
fn span(left: &PoseSample, right: &PoseSample) -> f64 {
    let dx = right.x - left.x;
    let dy = right.y - left.y;
    let dz = right.z - left.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Scale four commands so no component exceeds one.
fn unit4(values: [f64; 4]) -> [f64; 4] {
    let scale = values
        .iter()
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    values.map(|value| value / scale)
}

/// Scale three commands so no component exceeds one.
fn unit3(values: [f64; 3]) -> [f64; 3] {
    let scale = values
        .iter()
        .fold(1.0_f64, |scale, value| scale.max(value.abs()));
    values.map(|value| value / scale)
}

fn magnitude4(values: &[f64; 4]) -> f64 {
    values.iter().fold(0.0_f64, |top, value| top.max(value.abs()))
}

fn magnitude3(values: &[f64; 3]) -> f64 {
    values.iter().fold(0.0_f64, |top, value| top.max(value.abs()))
}

/// SHA-256 over NUL-terminated parts; the store's sample identity.
fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    format!("{:x}", hasher.finalize())
}

/// Normalised header names mapped to their position in a CSV row.
struct Columns(HashMap<String, usize>);

impl Columns {
    fn from_headers(headers: &StringRecord) -> Self {
        let mut positions = HashMap::with_capacity(headers.len());
        for (index, name) in headers.iter().enumerate() {
            positions.insert(canonical(name), index);
        }
        Self(positions)
    }

    fn position(&self, aliases: &[&str]) -> Option<usize> {
        aliases
            .iter()
            .find_map(|alias| self.0.get(&canonical(alias)).copied())
    }

    fn number(&self, row: &StringRecord, aliases: &[&str]) -> Option<f64> {
        row.get(self.position(aliases)?)?.trim().parse().ok()
    }

    fn integer(&self, row: &StringRecord, aliases: &[&str]) -> Option<i64> {
        row.get(self.position(aliases)?)?.trim().parse().ok()
    }
}

/// Headers are matched case- and punctuation-blind.
fn canonical(header: &str) -> String {
    header
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}
