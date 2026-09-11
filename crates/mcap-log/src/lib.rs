//! Crash-safe evidence logging for the Qualia arena.
//!
//! Every session is written to `<session>.mcap.partial` and is promoted to
//! `<session>.mcap` only after the writer has flushed a summary a reader can
//! use. A process that stops therefore leaves one of exactly two things behind:
//! a sealed segment that replays, or a partial that [`quarantine_partials`]
//! moves aside on the next boot. A reader never sees a half-written file under
//! a name that claims to be a session.
//!
//! Message payloads are JSON envelopes, and the channel set is fixed so a
//! silent subsystem is still visible in the sealed segment's inventory.

use mcap::{Channel, Compression, Message, MessageStream, Schema, Summary, WriteOptions, Writer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Result alias for every fallible operation in this crate.
pub type McapResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Profile tag written into the MCAP header of every Qualia segment.
pub const MCAP_PROFILE: &str = "qualia.arena.v1";
/// Name of the JSON schema registered for the evidence envelope.
pub const JSON_SCHEMA_NAME: &str = "qualia.evidence.v1";
pub const TOPIC_CAMERA: &str = "/qualia/camera";
pub const TOPIC_LIDAR: &str = "/qualia/lidar";
pub const TOPIC_POSE: &str = "/qualia/pose";
pub const TOPIC_VSLAM: &str = "/qualia/vslam";
pub const TOPIC_ACTION_REQUESTED: &str = "/qualia/action/requested";
pub const TOPIC_ACTION_CLAMPED: &str = "/qualia/action/clamped";
pub const TOPIC_ACTION_APPLIED: &str = "/qualia/action/applied";
pub const TOPIC_BELIEF: &str = "/qualia/belief";
pub const TOPIC_PRIOR: &str = "/qualia/prior";
pub const TOPIC_JEPA: &str = "/qualia/jepa";
pub const TOPIC_PLANNER: &str = "/qualia/planner";
pub const TOPIC_HEALTH: &str = "/qualia/health";

/// Topics every segment declares, so silence is distinguishable from absence.
pub const REQUIRED_TOPICS: [&str; 12] = [
    TOPIC_CAMERA,
    TOPIC_LIDAR,
    TOPIC_POSE,
    TOPIC_VSLAM,
    TOPIC_ACTION_REQUESTED,
    TOPIC_ACTION_CLAMPED,
    TOPIC_ACTION_APPLIED,
    TOPIC_BELIEF,
    TOPIC_PRIOR,
    TOPIC_JEPA,
    TOPIC_PLANNER,
    TOPIC_HEALTH,
];

/// Why a partial segment was moved out of the live directory.
const QUARANTINE_REASON: &str = "writer did not publish an atomic completed MCAP";

/// Envelope contract every evidence payload is expected to satisfy.
const JSON_SCHEMA: &[u8] = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","title":"Qualia evidence event","type":"object","required":["schema_version","producer_epoch","source_sequence","timestamp_ns"]}"#;

/// Per-topic summary of a sealed segment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelInventory {
    pub topic: String,
    pub schema: String,
    pub message_count: u64,
    pub min_log_time_ns: u64,
    pub max_log_time_ns: u64,
}

/// The receipt for a sealed segment: what it contains and how to check it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McapReference {
    pub schema_version: String,
    pub path: String,
    pub sha256: String,
    pub byte_length: u64,
    pub min_log_time_ns: u64,
    pub max_log_time_ns: u64,
    pub channels: Vec<ChannelInventory>,
    pub recovery_state: String,
}

/// One replayed message, with its payload decoded into an owned buffer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoggedMessage {
    pub topic: String,
    pub sequence: u32,
    pub log_time_ns: u64,
    pub publish_time_ns: u64,
    pub data: Vec<u8>,
}

/// Record of a partial segment that was moved aside during recovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuarantinedPartial {
    pub original_path: String,
    pub quarantine_path: String,
    pub reason: String,
}

/// Append-only writer for one arena session.
///
/// The writer owns the partial file for its whole lifetime and only publishes
/// the sealed name from [`McapSessionWriter::finish`].
pub struct McapSessionWriter {
    partial_path: PathBuf,
    final_path: PathBuf,
    writer: Writer<File>,
    schema: Arc<Schema<'static>>,
    channels: HashMap<String, Arc<Channel<'static>>>,
}

impl McapSessionWriter {
    /// Open a new session under `root`.
    ///
    /// Fails rather than overwrite an existing partial: that file belongs to a
    /// run that did not finish, and its evidence must be quarantined first.
    pub fn create(root: impl AsRef<Path>, session_id: &str) -> McapResult<Self> {
        let session_id = safe_session_id(session_id)?;
        fs::create_dir_all(root.as_ref())?;
        let final_path = root.as_ref().join(format!("{session_id}.mcap"));
        let partial_path = root
            .as_ref()
            .join(format!("{session_id}.mcap.partial"));
        if partial_path.exists() {
            return Err(format!("partial MCAP already exists: {}", partial_path.display()).into());
        }
        let options = WriteOptions::new()
            .compression(Some(Compression::Zstd))
            .compression_level(1)
            .compression_threads(1)
            .profile(MCAP_PROFILE)
            .library(format!("qualia-mcap/{}", env!("CARGO_PKG_VERSION")));
        let mut writer = Writer::with_options(File::create(&partial_path)?, options)?;
        let schema_id = writer.add_schema(JSON_SCHEMA_NAME, "jsonschema", JSON_SCHEMA)?;
        let schema = Arc::new(Schema {
            id: schema_id,
            name: JSON_SCHEMA_NAME.to_string(),
            encoding: "jsonschema".to_string(),
            data: Cow::Owned(JSON_SCHEMA.to_vec()),
        });
        Ok(Self {
            partial_path,
            final_path,
            writer,
            schema,
            channels: HashMap::new(),
        })
    }

    /// Serialize `value` as JSON and append it to `topic`.
    pub fn write_json<T: Serialize>(
        &mut self,
        topic: &str,
        sequence: u32,
        log_time_ns: u64,
        publish_time_ns: u64,
        value: &T,
    ) -> McapResult<()> {
        self.write_bytes(
            topic,
            sequence,
            log_time_ns,
            publish_time_ns,
            serde_json::to_vec(value)?,
        )
    }

    /// Declare every topic in [`REQUIRED_TOPICS`].
    pub fn register_required_topics(&mut self) -> McapResult<()> {
        for topic in REQUIRED_TOPICS {
            self.register_topic(topic)?;
        }
        Ok(())
    }

    /// Declare `topic` if it has not been declared yet.
    pub fn register_topic(&mut self, topic: &str) -> McapResult<()> {
        if self.channels.contains_key(topic) {
            return Ok(());
        }
        let metadata = BTreeMap::from([
            ("provenance".to_string(), "qualia-live".to_string()),
            (
                "schema_version".to_string(),
                JSON_SCHEMA_NAME.to_string(),
            ),
        ]);
        let id = self
            .writer
            .add_channel(self.schema.id, topic, "json", &metadata)?;
        self.channels.insert(
            topic.to_string(),
            Arc::new(Channel {
                id,
                topic: topic.to_string(),
                schema: Some(Arc::clone(&self.schema)),
                message_encoding: "json".to_string(),
                metadata,
            }),
        );
        Ok(())
    }

    /// Append a raw payload, declaring its topic on first use.
    pub fn write_bytes(
        &mut self,
        topic: &str,
        sequence: u32,
        log_time_ns: u64,
        publish_time_ns: u64,
        data: Vec<u8>,
    ) -> McapResult<()> {
        self.register_topic(topic)?;
        let channel = Arc::clone(
            self.channels
                .get(topic)
                .expect("a registered topic always has a channel"),
        );
        self.writer.write(&Message {
            channel,
            sequence,
            log_time: log_time_ns,
            publish_time: publish_time_ns,
            data: Cow::Owned(data),
        })?;
        Ok(())
    }

    /// Seal the session: flush the summary, verify it, then publish the name.
    ///
    /// The rename is the commit point. Everything before it leaves only the
    /// partial name behind, so a crash at any earlier instant is recoverable.
    pub fn finish(mut self) -> McapResult<McapReference> {
        self.writer.finish()?;
        self.writer.into_inner().sync_all()?;

        let bytes = fs::read(&self.partial_path)?;
        if Summary::read(&bytes)?.is_none() {
            return Err("completed MCAP has no readable summary".into());
        }
        let channels = inventory_bytes(&bytes)?;
        let (min_log_time_ns, max_log_time_ns) = time_bounds(&channels);
        let sha256 = format!("{:x}", Sha256::digest(&bytes));

        fs::rename(&self.partial_path, &self.final_path)?;
        sync_parent(&self.final_path)?;

        Ok(McapReference {
            schema_version: "qualia.mcap-reference.v1".to_string(),
            path: self.final_path.to_string_lossy().into_owned(),
            sha256,
            byte_length: bytes.len() as u64,
            min_log_time_ns,
            max_log_time_ns,
            channels,
            recovery_state: "complete".to_string(),
        })
    }
}

/// Summarize the channels of the segment at `path`.
pub fn inventory(path: impl AsRef<Path>) -> McapResult<Vec<ChannelInventory>> {
    inventory_bytes(&fs::read(path)?)
}

/// Replay the messages on `topic` whose log time falls in `start_ns..=end_ns`.
///
/// `topic` of `None` accepts every channel.
pub fn read_window(
    path: impl AsRef<Path>,
    topic: Option<&str>,
    start_ns: u64,
    end_ns: u64,
) -> McapResult<Vec<LoggedMessage>> {
    read_window_matching(path, start_ns, end_ns, |candidate| {
        topic.is_none_or(|wanted| wanted == candidate)
    })
}

/// Replay a time window, copying only the payloads of the requested topics.
///
/// Chunks are still decoded, but messages on other channels stay borrowed and
/// are dropped immediately rather than copied. Dataset construction leans on
/// this: it reads a handful of physical streams out of a much denser log.
pub fn read_window_topics(
    path: impl AsRef<Path>,
    topics: &[&str],
    start_ns: u64,
    end_ns: u64,
) -> McapResult<Vec<LoggedMessage>> {
    if topics.is_empty() {
        return Ok(Vec::new());
    }
    read_window_matching(path, start_ns, end_ns, |candidate| {
        topics.contains(&candidate)
    })
}

fn read_window_matching(
    path: impl AsRef<Path>,
    start_ns: u64,
    end_ns: u64,
    accepts_topic: impl Fn(&str) -> bool,
) -> McapResult<Vec<LoggedMessage>> {
    let bytes = fs::read(path)?;
    let mut messages = Vec::new();
    for message in MessageStream::new(&bytes)? {
        let message = message?;
        if message.log_time < start_ns || message.log_time > end_ns {
            continue;
        }
        if !accepts_topic(&message.channel.topic) {
            continue;
        }
        messages.push(LoggedMessage {
            topic: message.channel.topic.clone(),
            sequence: message.sequence,
            log_time_ns: message.log_time,
            publish_time_ns: message.publish_time,
            data: message.data.into_owned(),
        });
    }
    Ok(messages)
}

/// Move every `*.partial` segment in `root` out of the live namespace.
///
/// The bytes are never deleted, only renamed; a partial is evidence that the
/// previous run stopped early, and it is the caller's decision whether to read
/// it. Missing directories are not an error: a stack that never logged has
/// nothing to recover.
pub fn quarantine_partials(root: impl AsRef<Path>) -> McapResult<Vec<QuarantinedPartial>> {
    let mut quarantined = Vec::new();
    if !root.as_ref().exists() {
        return Ok(quarantined);
    }
    for entry in fs::read_dir(root.as_ref())? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("partial") {
            continue;
        }
        let quarantine = quarantine_path(&path);
        fs::rename(&path, &quarantine)?;
        quarantined.push(QuarantinedPartial {
            original_path: path.to_string_lossy().into_owned(),
            quarantine_path: quarantine.to_string_lossy().into_owned(),
            reason: QUARANTINE_REASON.to_string(),
        });
    }
    Ok(quarantined)
}

/// Pick a quarantine path that does not clobber an earlier recovery.
fn quarantine_path(partial: &Path) -> PathBuf {
    let mut candidate = partial.with_extension("quarantine");
    let mut suffix = 1u32;
    while candidate.exists() {
        candidate = partial.with_extension(format!("quarantine.{suffix}"));
        suffix += 1;
    }
    candidate
}

fn inventory_bytes(bytes: &[u8]) -> McapResult<Vec<ChannelInventory>> {
    let summary = Summary::read(bytes)?
        .ok_or_else(|| "MCAP is incomplete or missing its summary".to_string())?;
    let mut channels: BTreeMap<String, ChannelInventory> = BTreeMap::new();
    for channel in summary.channels.values() {
        let message_count = summary
            .stats
            .as_ref()
            .and_then(|stats| stats.channel_message_counts.get(&channel.id))
            .copied()
            .unwrap_or(0);
        channels.insert(
            channel.topic.clone(),
            ChannelInventory {
                topic: channel.topic.clone(),
                schema: channel_schema_name(channel),
                message_count,
                min_log_time_ns: 0,
                max_log_time_ns: 0,
            },
        );
    }

    // The summary knows how many messages each channel carries; only a replay
    // reveals when they happened, so the spans are filled in as we stream.
    let mut spans: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for message in MessageStream::new(bytes)? {
        let message = message?;
        if !channels.contains_key(&message.channel.topic) {
            channels.insert(
                message.channel.topic.clone(),
                ChannelInventory {
                    topic: message.channel.topic.clone(),
                    schema: channel_schema_name(&message.channel),
                    message_count: 0,
                    min_log_time_ns: 0,
                    max_log_time_ns: 0,
                },
            );
        }
        let span = spans
            .entry(message.channel.topic.clone())
            .or_insert((message.log_time, message.log_time));
        span.0 = span.0.min(message.log_time);
        span.1 = span.1.max(message.log_time);
    }

    Ok(channels
        .into_values()
        .map(|mut channel| {
            if let Some((min, max)) = spans.get(&channel.topic) {
                channel.min_log_time_ns = *min;
                channel.max_log_time_ns = *max;
            }
            channel
        })
        .collect())
}

fn channel_schema_name(channel: &Channel<'_>) -> String {
    channel
        .schema
        .as_ref()
        .map(|schema| schema.name.clone())
        .unwrap_or_default()
}

/// Bounds spanned by channels that actually carried messages.
///
/// A channel that was declared but never written must not pin the segment to
/// log time zero, which would look like evidence from the epoch.
fn time_bounds(channels: &[ChannelInventory]) -> (u64, u64) {
    let mut min = None;
    let mut max = None;
    for channel in channels.iter().filter(|channel| channel.message_count > 0) {
        min = Some(min.map_or(channel.min_log_time_ns, |value: u64| {
            value.min(channel.min_log_time_ns)
        }));
        max = Some(max.map_or(channel.max_log_time_ns, |value: u64| {
            value.max(channel.max_log_time_ns)
        }));
    }
    (min.unwrap_or(0), max.unwrap_or(0))
}

fn safe_session_id(session_id: &str) -> McapResult<&str> {
    let clean = !session_id.is_empty()
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !clean {
        return Err("session id must contain only ASCII letters, digits, '-' or '_'".into());
    }
    Ok(session_id)
}

/// Flush the directory entry that the sealing rename created.
fn sync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_bounds_ignore_declared_but_silent_channels() {
        let silent = ChannelInventory {
            topic: TOPIC_HEALTH.to_string(),
            schema: JSON_SCHEMA_NAME.to_string(),
            message_count: 0,
            min_log_time_ns: 0,
            max_log_time_ns: 0,
        };
        let spoken = ChannelInventory {
            topic: TOPIC_POSE.to_string(),
            schema: JSON_SCHEMA_NAME.to_string(),
            message_count: 2,
            min_log_time_ns: 4_000,
            max_log_time_ns: 9_000,
        };
        assert_eq!(time_bounds(&[silent.clone(), spoken.clone()]), (4_000, 9_000));
        assert_eq!(time_bounds(&[silent]), (0, 0));
        assert_eq!(time_bounds(&[]), (0, 0));
    }

    #[test]
    fn session_ids_are_restricted_to_path_safe_ascii() {
        for accepted in ["arena-1", "session_2", "A0"] {
            assert_eq!(safe_session_id(accepted).unwrap(), accepted);
        }
        for rejected in ["", "a.b", "a/b", "a b", "..", "séance", "a\\b"] {
            assert!(safe_session_id(rejected).is_err(), "{rejected:?} must fail");
        }
    }

    #[test]
    fn quarantine_paths_never_clobber_an_earlier_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let partial = temp.path().join("s.mcap.partial");
        assert_eq!(
            quarantine_path(&partial),
            temp.path().join("s.mcap.quarantine")
        );
        fs::write(temp.path().join("s.mcap.quarantine"), b"first").unwrap();
        assert_eq!(
            quarantine_path(&partial),
            temp.path().join("s.mcap.quarantine.1")
        );
        fs::write(temp.path().join("s.mcap.quarantine.1"), b"second").unwrap();
        assert_eq!(
            quarantine_path(&partial),
            temp.path().join("s.mcap.quarantine.2")
        );
    }
}
