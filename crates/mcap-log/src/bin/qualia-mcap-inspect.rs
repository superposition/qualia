//! `qualia-mcap-inspect`: read one sealed Qualia segment.
//!
//! With just a path it prints a per-stream timing report: message counts, the
//! log-time span, integer gap percentiles and payload sizes, grouped by topic
//! and by the `entity` field of each JSON envelope. With
//! `--require-zero-applied <entities>` it instead audits that the named
//! entities have a complete, contiguous and genuinely zero applied-action
//! history, which is how a zero-motion run is proved rather than asserted.

use qualia_mcap::{inventory, read_window, LoggedMessage, TOPIC_ACTION_APPLIED};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: qualia-mcap-inspect <session.mcap> \
                     [--require-zero-applied <entity,entity>]";
const INSPECTION_SCHEMA: &str = "qualia.mcap-inspection.v1";
const ACTION_SCHEMA: &str = "qualia.action-evidence.v1";
const AUDIT_SCHEMA: &str = "qualia.zero-applied-action-audit.v1";

type InspectResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Timing summary for one `(topic, entity)` stream.
#[derive(Debug, Serialize)]
struct StreamTiming {
    topic: String,
    entity: String,
    messages: usize,
    min_timestamp_ns: u64,
    max_timestamp_ns: u64,
    mean_gap_ns: u64,
    p50_gap_ns: u64,
    p95_gap_ns: u64,
    max_gap_ns: u64,
    payload_bytes: u64,
    mean_payload_bytes: u64,
    max_payload_bytes: usize,
}

/// Raw per-stream samples, before the timing report is computed.
#[derive(Default)]
struct StreamSamples {
    timestamps: Vec<u64>,
    payload_sizes: Vec<usize>,
}

/// Contiguous zero applied-action history for one entity.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct ZeroActionEntityAudit {
    entity: String,
    messages: usize,
    first_history_sequence: u64,
    last_history_sequence: u64,
    first_timestamp_ns: u64,
    last_timestamp_ns: u64,
}

/// Verdict on every required entity's applied-action history.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct ZeroActionAudit {
    schema_version: &'static str,
    required_entities: Vec<String>,
    total_messages: usize,
    entities: Vec<ZeroActionEntityAudit>,
    verified_zero: bool,
}

/// What the operator asked the inspector to do.
enum Request {
    Summary(PathBuf),
    ZeroApplied {
        path: PathBuf,
        entities: BTreeSet<String>,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qualia-mcap-inspect: {error}");
        std::process::exit(1);
    }
}

fn run() -> InspectResult<()> {
    let request = match parse_args()? {
        Some(request) => request,
        None => {
            println!("{USAGE}");
            return Ok(());
        }
    };
    match request {
        Request::ZeroApplied { path, entities } => {
            let messages = read_window(&path, Some(TOPIC_ACTION_APPLIED), 0, u64::MAX)?;
            let audit = audit_zero_applied_actions(&messages, &entities)?;
            println!("{}", serde_json::to_string_pretty(&audit)?);
        }
        Request::Summary(path) => {
            let channels = inventory(&path)?;
            let streams = stream_timings(&path)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "schema_version": INSPECTION_SCHEMA,
                    "path": path,
                    "channels": channels,
                    "streams": streams,
                }))?
            );
        }
    }
    Ok(())
}

/// Parse argv. `Ok(None)` means the usage text should be printed.
fn parse_args() -> Result<Option<Request>, String> {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else {
        return Ok(None);
    };
    if first == "--help" || first == "-h" {
        return Ok(None);
    }
    let path = PathBuf::from(first);
    match args.next() {
        None => Ok(Some(Request::Summary(path))),
        Some(flag) if flag == "--require-zero-applied" => {
            let list = args
                .next()
                .ok_or("--require-zero-applied requires a comma-separated entity list")?;
            if args.next().is_some() {
                return Err("unexpected argument after zero-action entity list".to_string());
            }
            Ok(Some(Request::ZeroApplied {
                path,
                entities: parse_required_entities(&list.to_string_lossy())?,
            }))
        }
        Some(_) => Err("unknown qualia-mcap-inspect argument".to_string()),
    }
}

fn parse_required_entities(value: &str) -> Result<BTreeSet<String>, String> {
    let entities = value
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let malformed = entities.is_empty()
        || entities.iter().any(|entity| {
            entity.is_empty()
                || !entity
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
    if malformed {
        return Err("zero-action entity list is empty or malformed".to_string());
    }
    Ok(entities)
}

fn stream_timings(path: &Path) -> InspectResult<Vec<StreamTiming>> {
    let mut streams = BTreeMap::<(String, String), StreamSamples>::new();
    for message in read_window(path, None, 0, u64::MAX)? {
        let value = serde_json::from_slice::<Value>(&message.data)?;
        let entity = value
            .get("entity")
            .and_then(Value::as_str)
            .unwrap_or("unscoped")
            .to_string();
        let timestamp_ns = value
            .get("timestamp_ns")
            .and_then(Value::as_u64)
            .unwrap_or(message.log_time_ns);
        let samples = streams.entry((message.topic, entity)).or_default();
        samples.timestamps.push(timestamp_ns);
        samples.payload_sizes.push(message.data.len());
    }
    Ok(streams
        .into_iter()
        .map(|((topic, entity), samples)| stream_timing(topic, entity, samples))
        .collect())
}

fn stream_timing(topic: String, entity: String, mut samples: StreamSamples) -> StreamTiming {
    samples.timestamps.sort_unstable();
    let mut gaps = samples
        .timestamps
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .collect::<Vec<_>>();
    gaps.sort_unstable();
    let payload_bytes = samples.payload_sizes.iter().map(|size| *size as u64).sum::<u64>();
    let messages = samples.timestamps.len();
    StreamTiming {
        topic,
        entity,
        messages,
        min_timestamp_ns: samples.timestamps.first().copied().unwrap_or(0),
        max_timestamp_ns: samples.timestamps.last().copied().unwrap_or(0),
        mean_gap_ns: if gaps.is_empty() {
            0
        } else {
            gaps.iter().sum::<u64>() / gaps.len() as u64
        },
        p50_gap_ns: percentile(&gaps, 0.50),
        p95_gap_ns: percentile(&gaps, 0.95),
        max_gap_ns: gaps.last().copied().unwrap_or(0),
        payload_bytes,
        mean_payload_bytes: if messages == 0 {
            0
        } else {
            payload_bytes / messages as u64
        },
        max_payload_bytes: samples.payload_sizes.iter().copied().max().unwrap_or(0),
    }
}

/// Nearest-rank percentile over an ascending slice.
fn percentile(sorted: &[u64], quantile: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}

/// Require that every named entity has a complete, nonzero-free applied-action
/// history, and report the span each one covers.
fn audit_zero_applied_actions(
    messages: &[LoggedMessage],
    required_entities: &BTreeSet<String>,
) -> Result<ZeroActionAudit, String> {
    let mut states = BTreeMap::<String, ZeroActionEntityAudit>::new();
    for message in messages {
        if message.topic != TOPIC_ACTION_APPLIED {
            return Err("zero-action audit received a non-applied-action topic".to_string());
        }
        let value: Value = serde_json::from_slice(&message.data)
            .map_err(|error| format!("malformed applied-action JSON: {error}"))?;
        let entity = string_field(&value, "entity")?;
        let schema = string_field(&value, "schema_version")?;
        let stage = string_field(&value, "stage")?;
        let history_sequence = u64_field(&value, "history_sequence")?;
        let source_sequence = u64_field(&value, "source_sequence")?;
        let producer_epoch = u64_field(&value, "producer_epoch")?;
        let timestamp_ns = u64_field(&value, "timestamp_ns")?;
        let interval_start_ns = u64_field(&value, "interval_start_ns")?;
        let interval_end_ns = u64_field(&value, "interval_end_ns")?;
        let left = f64_field(&value, "left")?;
        let right = f64_field(&value, "right")?;
        let applied_left = f64_field(&value, "applied_left")?;
        let applied_right = f64_field(&value, "applied_right")?;
        let speed_scale = f64_field(&value, "speed_scale")?;
        let valid = bool_field(&value, "valid")?;
        let leash_authority = match value.get("authority") {
            Some(Value::Number(number)) => number.as_u64() == Some(1),
            Some(Value::String(authority)) => authority == "leash",
            _ => false,
        };
        if schema != ACTION_SCHEMA
            || stage != "transport_accepted"
            || history_sequence == 0
            || source_sequence == 0
            || producer_epoch == 0
            || interval_start_ns == 0
            || interval_end_ns <= interval_start_ns
            || timestamp_ns != interval_end_ns
            || message.log_time_ns != interval_end_ns
            || message.publish_time_ns < message.log_time_ns
            || !speed_scale.is_finite()
            || !(0.0..=1.0).contains(&speed_scale)
            || !valid
            || !leash_authority
            || left != applied_left
            || right != applied_right
        {
            return Err(format!(
                "{entity} applied-action interval {history_sequence} violates the evidence contract"
            ));
        }
        if left != 0.0 || right != 0.0 || applied_left != 0.0 || applied_right != 0.0 {
            return Err(format!(
                "{entity} applied-action interval {history_sequence} is nonzero"
            ));
        }
        let state = states.entry(entity.to_string()).or_insert_with(|| ZeroActionEntityAudit {
            entity: entity.to_string(),
            messages: 0,
            first_history_sequence: 0,
            last_history_sequence: 0,
            first_timestamp_ns: 0,
            last_timestamp_ns: 0,
        });
        if state.messages != 0 && history_sequence != state.last_history_sequence + 1 {
            return Err(format!(
                "{entity} applied-action history is not contiguous at {history_sequence}"
            ));
        }
        if state.messages == 0 {
            state.first_history_sequence = history_sequence;
            state.first_timestamp_ns = timestamp_ns;
        }
        state.messages += 1;
        state.last_history_sequence = history_sequence;
        state.last_timestamp_ns = timestamp_ns;
    }
    for entity in required_entities {
        if !states.contains_key(entity) {
            return Err(format!(
                "required entity {entity} has no applied-action history"
            ));
        }
    }
    let entities = states.into_values().collect::<Vec<_>>();
    Ok(ZeroActionAudit {
        schema_version: AUDIT_SCHEMA,
        required_entities: required_entities.iter().cloned().collect(),
        total_messages: entities.iter().map(|entity| entity.messages).sum(),
        entities,
        verified_zero: true,
    })
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("applied-action {field} is missing or malformed"))
}

fn u64_field(value: &Value, field: &str) -> Result<u64, String> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("applied-action {field} is missing or malformed"))
}

fn f64_field(value: &Value, field: &str) -> Result<f64, String> {
    value
        .get(field)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("applied-action {field} is missing or malformed"))
}

fn bool_field(value: &Value, field: &str) -> Result<bool, String> {
    value
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("applied-action {field} is missing or malformed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_message(entity: &str, history_sequence: u64, left: f64) -> LoggedMessage {
        let timestamp_ns = 1_000 + history_sequence;
        LoggedMessage {
            topic: TOPIC_ACTION_APPLIED.to_string(),
            sequence: history_sequence as u32,
            log_time_ns: timestamp_ns,
            publish_time_ns: timestamp_ns,
            data: serde_json::to_vec(&serde_json::json!({
                "schema_version": ACTION_SCHEMA,
                "producer_epoch": 1,
                "entity": entity,
                "history_sequence": history_sequence,
                "source_sequence": history_sequence,
                "timestamp_ns": timestamp_ns,
                "interval_start_ns": timestamp_ns - 1,
                "interval_end_ns": timestamp_ns,
                "stage": "transport_accepted",
                "left": left,
                "right": 0.0,
                "applied_left": left,
                "applied_right": 0.0,
                "speed_scale": 1.0,
                "authority": "leash",
                "valid": true
            }))
            .unwrap(),
        }
    }

    #[test]
    fn gap_percentiles_are_nearest_rank() {
        let timing = stream_timing(
            "/qualia/camera".to_string(),
            "guard".to_string(),
            StreamSamples {
                timestamps: vec![100, 200, 400, 800],
                payload_sizes: vec![10, 20, 30, 40],
            },
        );
        assert_eq!(timing.messages, 4);
        assert_eq!(timing.mean_gap_ns, 700 / 3);
        assert_eq!(timing.p50_gap_ns, 200);
        assert_eq!(timing.p95_gap_ns, 400);
        assert_eq!(timing.max_gap_ns, 400);
        assert_eq!(timing.payload_bytes, 100);
        assert_eq!(timing.mean_payload_bytes, 25);
        assert_eq!(timing.max_payload_bytes, 40);
    }

    #[test]
    fn zero_action_audit_requires_every_named_entity() {
        let messages = vec![
            action_message("guard", 7, 0.0),
            action_message("pinkie", 11, 0.0),
            action_message("guard", 8, 0.0),
            action_message("pinkie", 12, 0.0),
        ];
        let audit =
            audit_zero_applied_actions(&messages, &parse_required_entities("guard,pinkie").unwrap())
                .unwrap();
        assert!(audit.verified_zero);
        assert_eq!(audit.total_messages, 4);
        assert_eq!(audit.entities.len(), 2);

        let missing = parse_required_entities("guard,pinkie,courier").unwrap();
        assert!(audit_zero_applied_actions(&messages, &missing).is_err());
    }

    #[test]
    fn zero_action_audit_catches_transient_motion_and_broken_contiguity() {
        let motion = vec![
            action_message("guard", 7, 0.0),
            action_message("guard", 8, 0.1),
            action_message("guard", 9, 0.0),
        ];
        let error = audit_zero_applied_actions(
            &motion,
            &parse_required_entities("guard").unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("is nonzero"), "{error}");

        let gap = vec![
            action_message("guard", 7, 0.0),
            action_message("guard", 9, 0.0),
        ];
        let error = audit_zero_applied_actions(&gap, &parse_required_entities("guard").unwrap())
            .unwrap_err();
        assert!(error.contains("not contiguous"), "{error}");
    }
}
