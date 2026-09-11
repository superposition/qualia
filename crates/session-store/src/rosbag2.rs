//! rosbag2 interop.
//!
//! Ingesting a recorded bag copies its `topics`/`messages` sqlite3 rows into a
//! fresh abstraction epoch: one abstraction space per topic, one abstract state
//! sample per message. Exporting goes the other way, rebuilding a rosbag2
//! directory (a `*_0.db3` file plus `metadata.yaml`) from an epoch's spaces.

use crate::{AbstractStateSample, AbstractionEpochUpsert, AbstractionSpaceUpsert, SessionStore, SessionStreamUpsert};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use std::fs;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rosbag2ImportOptions {
    pub epoch_index: i64, pub layer_name: String, pub abstraction_family: String, pub stream_key: String,
}

impl Default for Rosbag2ImportOptions {
    fn default() -> Self {
        Self { epoch_index: 0, layer_name: "rosbag2_ingest".to_string(), abstraction_family: "rosbag2_topics".to_string(), stream_key: "rosbag2".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rosbag2ImportReport {
    pub session_id: i64, pub epoch_id: i64, pub db_path: String, pub topics: usize, pub messages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Rosbag2ExportOptions { pub epoch_id: Option<i64>, pub bag_name: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rosbag2ExportReport {
    pub session_id: i64, pub epoch_id: i64, pub bag_dir: String, pub db_path: String, pub topics: usize, pub messages: usize,
}

impl SessionStore {
    pub fn import_rosbag2(&mut self, session_id: i64, bag_path: &str, options: &Rosbag2ImportOptions) -> rusqlite::Result<Rosbag2ImportReport> {
        let db_path = resolve_rosbag2_db_path(Path::new(bag_path))?;
        let source = Connection::open(&db_path)?;

        let stream = SessionStreamUpsert { session_id, stream_key: options.stream_key.clone(), stream_kind: "rosbag2".into(), role: "observation".into(), path: bag_path.to_owned(), sync_group: "rosbag2".into(), metadata_json: json!({ "db_path": db_path.display().to_string() }).to_string() };
        self.upsert_stream(&stream)?;

        let imported_at = now_iso8601();
        let epoch = AbstractionEpochUpsert { session_id, epoch_index: options.epoch_index, layer_name: options.layer_name.clone(), abstraction_family: options.abstraction_family.clone(), status: "complete".into(), created_at: imported_at.clone(), completed_at: Some(imported_at), summary_json: json!({ "source": "rosbag2", "path": bag_path }).to_string() };
        let epoch_id = self.upsert_epoch(&epoch)?;

        let mut topic_statement = source.prepare("SELECT id, name, type, serialization_format, offered_qos_profiles FROM topics ORDER BY id ASC")?;
        let topic_rows = topic_statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, Option<String>>(4)?))
        })?;
        let topics_by_id: HashMap<i64, (String, String, String, Option<String>)> = topic_rows.map(|row| {
            let (id, name, message_type, serialization, qos) = row?;
            Ok((id, (name, message_type, serialization, qos)))
        }).collect::<rusqlite::Result<_>>()?;

        let mut message_statement = source.prepare("SELECT id, topic_id, timestamp, data FROM messages ORDER BY timestamp ASC, id ASC")?;
        let message_rows = message_statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, Vec<u8>>(3)?))
        })?;

        let mut next_step: HashMap<i64, i64> = HashMap::new();
        let mut samples_by_topic: HashMap<i64, Vec<AbstractStateSample>> = HashMap::new();
        let mut imported_messages = 0usize;

        for message in message_rows {
            let (message_id, topic_id, timestamp_ns, data) = message?;
            let Some((topic_name, _, _, _)) = topics_by_id.get(&topic_id) else { continue };

            // Messags are ordered globally by timestamp; each topic keeps its own
            // running sample index.
            let cursor = next_step.entry(topic_id).or_insert(0);
            let step = *cursor;
            *cursor += 1;

            let frame_id = extract_frame_id_from_payload(&data);
            let timestamp_sec = timestamp_ns as f64 / 1_000_000_000.0;
            let symbol_key = frame_id.clone().unwrap_or_else(|| topic_name.clone());
            let payload_json = json!({ "topic": topic_name, "timestamp_ns": timestamp_ns, "frame_id": frame_id, "source_message_id": message_id, "encoding": "hex", "data_hex": hex_encode(&data) }).to_string();
            let sample_hash = format!("{topic_id}:{message_id}:{timestamp_ns}");
            samples_by_topic.entry(topic_id).or_default().push(AbstractStateSample { step, timestamp_sec, symbol_key, payload_json, confidence: 1.0, sample_hash });
            imported_messages += 1;
        }

        for (topic_id, topic_name, message_type, serialization, qos) in topics_by_id.iter().map(|(id, entry)| (*id, entry.0.as_str(), entry.1.as_str(), entry.2.as_str(), entry.3.as_deref())) {
            let space = AbstractionSpaceUpsert { epoch_id, space_family: "state".into(), abstraction_name: topic_to_abstraction_name(topic_name), source_kind: "rosbag2".into(), representation_kind: "message_blob".into(), uncertainty_kind: "reported".into(), dimensionality: 1, schema_json: json!({ "topic_name": topic_name, "message_type": message_type, "serialization_format": serialization, "offered_qos_profiles": qos }).to_string() };
            let space_id = self.upsert_space(&space)?;
            let samples = samples_by_topic.remove(&topic_id).unwrap_or_default();
            self.replace_state_samples(epoch_id, space_id, &samples)?;
        }

        let reported_topics = topics_by_id.len();
        Ok(Rosbag2ImportReport { session_id, epoch_id, db_path: db_path.display().to_string(), topics: reported_topics, messages: imported_messages })
    }

    pub fn export_rosbag2(&self, session_id: i64, output_dir: &str, options: &Rosbag2ExportOptions) -> rusqlite::Result<Rosbag2ExportReport> {
        let epoch_id = match options.epoch_id {
            Some(chosen) => chosen,
            None => self.list_epochs(session_id)?.last().map(|epoch| epoch.id).ok_or(rusqlite::Error::QueryReturnedNoRows)?,
        };
        let session = self.session_by_id(session_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;

        let bag_dir = Path::new(output_dir);
        ensure_dir(bag_dir)?;

        let bag_stem = match options.bag_name.clone() {
            Some(name) => name,
            None => sanitize_stem(&session.filename),
        };
        let db_path = bag_dir.join(format!("{}_0.db3", bag_stem));
        if db_path.exists() { fs::remove_file(&db_path).map_err(to_sql_err)?; }

        let destination = Connection::open(&db_path)?;
        destination.execute_batch(
            r#"
            CREATE TABLE topics (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                type TEXT NOT NULL,
                serialization_format TEXT NOT NULL,
                offered_qos_profiles TEXT NOT NULL
            );
            CREATE TABLE messages (
                id INTEGER PRIMARY KEY,
                topic_id INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                data BLOB NOT NULL
            );
            CREATE INDEX timestamp_idx ON messages (timestamp ASC);
            "#,
        )?;

        let mut topic_ids: HashMap<String, i64> = HashMap::new();
        let mut queued: Vec<(i64, i64, Vec<u8>)> = Vec::new();

        for space in self.list_spaces(epoch_id)?.into_iter().filter(|space| space.source_kind == "rosbag2") {
            let schema = serde_json::from_str::<Value>(&space.schema_json).unwrap_or(Value::Null);
            let field = |key: &str| schema.get(key).and_then(Value::as_str);
            let topic_name = field("topic_name").unwrap_or(space.abstraction_name.as_str());
            let topic_type = field("message_type").unwrap_or("unknown_msgs/Unknown");
            let serialization = field("serialization_format").unwrap_or("cdr");
            let qos = field("offered_qos_profiles").unwrap_or("");

            let topic_id = match topic_ids.get(topic_name).copied() {
                Some(existing) => existing,
                None => {
                    let assigned = i64::try_from(topic_ids.len() + 1).unwrap_or(i64::MAX);
                    destination.execute("INSERT INTO topics (id, name, type, serialization_format, offered_qos_profiles) VALUES (?1, ?2, ?3, ?4, ?5)", params![assigned, topic_name, topic_type, serialization, qos])?;
                    topic_ids.insert(topic_name.to_owned(), assigned);
                    assigned
                }
            };

            for sample in self.list_state_samples(space.id, None, None, None)? {
                let payload = serde_json::from_str::<Value>(&sample.payload_json).unwrap_or(Value::Null);
                let fallback_ns = (sample.timestamp_sec * 1_000_000_000.0) as i64;
                let timestamp_ns = payload.get("timestamp_ns").and_then(Value::as_i64).unwrap_or(fallback_ns);
                let data_hex = payload.get("data_hex").and_then(Value::as_str).unwrap_or("");
                queued.push((topic_id, timestamp_ns, hex_decode(data_hex).unwrap_or_default()));
            }
        }

        queued.sort_by_key(|(_, timestamp_ns, _)| *timestamp_ns);
        for (offset, (topic_id, timestamp_ns, bytes)) in queued.iter().enumerate() {
            let message_id = i64::try_from(offset + 1).unwrap_or(i64::MAX);
            destination.execute("INSERT INTO messages (id, topic_id, timestamp, data) VALUES (?1, ?2, ?3, ?4)", params![message_id, topic_id, timestamp_ns, bytes])?;
        }

        let duration_ns = queued.first().zip(queued.last()).map(|(first, last)| last.1.saturating_sub(first.1)).unwrap_or(0);
        let start_ns = queued.first().map(|entry| entry.1).unwrap_or(0);
        let metadata = format!("rosbag2_bagfile_information:\n  version: 5\n  storage_identifier: sqlite3\n  duration:\n    nanoseconds: {duration_ns}\n  starting_time:\n    nanoseconds_since_epoch: {start_ns}\n  message_count: {}\n", queued.len());
        fs::write(bag_dir.join("metadata.yaml"), metadata).map_err(to_sql_err)?;

        let reported_count = queued.len();
        Ok(Rosbag2ExportReport { session_id, epoch_id, bag_dir: bag_dir.display().to_string(), db_path: db_path.display().to_string(), topics: topic_ids.len(), messages: reported_count })
    }
}

fn resolve_rosbag2_db_path(path: &Path) -> rusqlite::Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    for entry in fs::read_dir(path).map_err(to_sql_err)? {
        let candidate = entry.map_err(to_sql_err)?.path();
        if candidate.extension().and_then(|value| value.to_str()) == Some("db3") {
            return Ok(candidate);
        }
    }
    Err(rusqlite::Error::InvalidPath(path.to_path_buf()))
}

fn sanitize_stem(value: &str) -> String {
    let stem = Path::new(value).file_stem().and_then(|text| text.to_str()).unwrap_or("session");
    let mut sanitized = String::with_capacity(stem.len());
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() { "session".to_string() } else { sanitized }
}

fn topic_to_abstraction_name(topic: &str) -> String {
    let trimmed = topic.trim_start_matches('/');
    if trimmed.is_empty() { "topic_root".to_string() } else { trimmed.replace('/', "__") }
}

fn extract_frame_id_from_payload(data: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(data).ok()?;
    let nested = value.get("header").and_then(|header| header.get("frame_id"));
    let flat = value.get("frame_id");
    nested.or(flat).and_then(Value::as_str).map(str::to_owned)
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    hex.as_bytes().chunks(2).map(|pair| -> Option<u8> {
        let text = std::str::from_utf8(pair).ok()?;
        u8::from_str_radix(text, 16).ok()
    }).collect()
}

fn now_iso8601() -> String {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    seconds.to_string()
}

fn ensure_dir(path: &Path) -> rusqlite::Result<()> {
    fs::create_dir_all(path).map_err(to_sql_err)
}

fn to_sql_err(err: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(err))
}
