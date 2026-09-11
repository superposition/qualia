//! `qualia-camera` — camera ingest for the qualia body bus.
//!
//! The runner turns whatever the body's camera publishes — a JPEG/PNG snapshot
//! file a device daemon rewrites in place, an HTTP snapshot endpoint, or a
//! `multipart/x-mixed-replace` MJPEG stream — into the two shared-memory slots
//! the rest of the stack reads:
//!
//! * the [`CameraFrame`] thumbnail and its luma statistics, which the floor,
//!   JEPA and VSLAM runners derive from; and
//! * the bounded [`CameraPreview`], the encoded frame the console and the agent
//!   hand to an operator.
//!
//! Both are seqlocked in `qualia-types`, so a reader either takes a whole frame
//! or retries; nothing here buffers a second copy of the arena. A capture that
//! fails — a missing file, an HTTP error, an oversized body, bytes that do not
//! decode — leaves the last readable frame and preview exactly as they were,
//! which is what lets the stack keep running while the camera is unplugged.
//!
//! Everything the process needs comes from the environment; [`CameraConfig`]
//! reads and defaults the keys, so the binary is a thin driver over the logic
//! collected here, and the tests drive the same code the runner does.

use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::imageops::FilterType;
use qualia_shm::ShmRegion;
use qualia_types::{
    CameraFrame, CameraFrameSnapshot, CameraPreview, CAMERA_PREVIEW_MAX_BYTES, CAMERA_THUMB_H,
    CAMERA_THUMB_PIXELS, CAMERA_THUMB_W,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The environment key naming the arena to attach to.
pub const SHM_NAME_ENV: &str = "QUALIA_SHM_NAME";
/// The environment key carrying the MJPEG stream URL.
pub const STREAM_URL_ENV: &str = "QUALIA_CAMERA_STREAM_URL";
/// The environment key carrying the single-snapshot HTTP URL.
pub const SNAPSHOT_URL_ENV: &str = "QUALIA_CAMERA_SNAPSHOT_URL";
/// The environment key carrying the snapshot file path.
pub const SNAPSHOT_PATH_ENV: &str = "QUALIA_CAMERA_SNAPSHOT_PATH";
/// The environment key carrying the poll interval in milliseconds.
pub const POLL_MS_ENV: &str = "QUALIA_CAMERA_POLL_MS";
/// The environment key carrying the HTTP timeout in milliseconds.
pub const HTTP_TIMEOUT_MS_ENV: &str = "QUALIA_CAMERA_HTTP_TIMEOUT_MS";

/// The region `qualia-init` creates when the supervisor names no other.
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";
/// Where the on-board camera daemon writes a snapshot when no path is named.
pub const DEFAULT_SNAPSHOT_PATH: &str = "/tmp/qualia_orin_snap.jpg";
/// Poll interval for a file source when no interval is named.
pub const DEFAULT_POLL_MS: u64 = 100;
/// Poll interval for an HTTP source when no interval is named: a camera that
/// answers one request at a time must not be hammered.
pub const DEFAULT_HTTP_POLL_MS: u64 = 500;
/// HTTP timeout when none is named.
pub const DEFAULT_HTTP_TIMEOUT_MS: u64 = 2_000;
/// The slowest poll interval the runner will honour, in milliseconds.
pub const MIN_POLL_MS: u64 = 100;
/// Largest snapshot the runner will load, in bytes.
pub const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
/// Frames between summary lines once the first frame has been logged.
pub const LOG_EVERY_FRAMES: u64 = 30;

/// [`CameraPreview::format`] for a JPEG preview.
pub const PREVIEW_FORMAT_JPEG: u8 = 1;
/// [`CameraPreview::format`] for a PNG preview.
pub const PREVIEW_FORMAT_PNG: u8 = 2;
/// [`CameraPreview::format`] while the preview slot holds no usable frame.
pub const PREVIEW_FORMAT_NONE: u8 = 0;

/// Where the next encoded snapshot comes from.
pub enum SnapshotSource {
    /// A file the camera daemon rewrites; its modification time is the gate.
    File(String),
    /// An HTTP endpoint serving one encoded snapshot per request.
    Http { url: String, agent: ureq::Agent },
}

impl SnapshotSource {
    /// A file source at `path`.
    pub fn file(path: impl Into<String>) -> Self {
        Self::File(path.into())
    }

    /// An HTTP source whose requests time out after `timeout`.
    pub fn http(url: impl Into<String>, timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        Self::Http {
            url: url.into(),
            agent,
        }
    }
}

impl std::fmt::Debug for SnapshotSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => formatter.debug_tuple("File").field(path).finish(),
            Self::Http { url, .. } => formatter
                .debug_struct("Http")
                .field("url", url)
                .finish_non_exhaustive(),
        }
    }
}

/// The runner's environment-derived settings.
#[derive(Debug)]
pub struct CameraConfig {
    /// The arena to attach to; never created here.
    pub shm_name: String,
    /// A live MJPEG stream to consume instead of polling, when one is named.
    pub stream_url: Option<String>,
    /// What to poll when no stream is named.
    pub source: SnapshotSource,
    /// Time between two capture attempts.
    pub poll: Duration,
    /// Connect/read/write timeout for every HTTP request.
    pub http_timeout: Duration,
}

impl CameraConfig {
    /// Reads every key from the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads every key through `lookup`, substituting the documented default
    /// whenever a key is absent, blank or unparseable. A malformed value must
    /// never stop the runner from starting.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let shm_name = lookup(SHM_NAME_ENV).unwrap_or_else(|| DEFAULT_SHM_NAME.to_string());
        let stream_url = endpoint(lookup(STREAM_URL_ENV));
        let snapshot_url = endpoint(lookup(SNAPSHOT_URL_ENV));
        let snapshot_path = lookup(SNAPSHOT_PATH_ENV).unwrap_or_else(|| DEFAULT_SNAPSHOT_PATH.to_string());
        let default_poll_ms = if snapshot_url.is_some() {
            DEFAULT_HTTP_POLL_MS
        } else {
            DEFAULT_POLL_MS
        };
        let poll_ms = number(&mut lookup, POLL_MS_ENV)
            .unwrap_or(default_poll_ms)
            .max(MIN_POLL_MS);
        let timeout_ms =
            number(&mut lookup, HTTP_TIMEOUT_MS_ENV).unwrap_or(DEFAULT_HTTP_TIMEOUT_MS);
        let http_timeout = Duration::from_millis(timeout_ms);
        let source = match snapshot_url {
            Some(url) => SnapshotSource::http(url, http_timeout),
            None => SnapshotSource::File(snapshot_path),
        };

        Self {
            shm_name,
            stream_url,
            source,
            poll: Duration::from_millis(poll_ms),
            http_timeout,
        }
    }
}

/// A URL is only an endpoint once it carries something that is not whitespace.
fn endpoint(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// A numeric environment value; blank and malformed values read as absent.
fn number(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str) -> Option<u64> {
    lookup(key)?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// One encoded snapshot on its way to the arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSnapshot {
    /// The encoded JPEG or PNG bytes exactly as the source served them.
    pub bytes: Vec<u8>,
    /// The source's modification time in nanoseconds, or zero when the source
    /// has no such notion (HTTP).
    pub mtime_ns: u128,
}

/// Why a source produced no snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// The file source has not changed since the caller's watermark, so there
    /// is nothing new to publish.
    NotChanged,
    /// The source could not deliver bytes at all.
    Unavailable(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotChanged => write!(formatter, "the snapshot source has not changed"),
            Self::Unavailable(reason) => write!(formatter, "snapshot source unavailable: {reason}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Loads the next encoded snapshot, or says why there is none.
///
/// A file source is gated on its modification time: a file still carrying
/// `last_mtime_ns` reports [`SnapshotError::NotChanged`] without being read.
pub fn load_snapshot(
    source: &SnapshotSource,
    last_mtime_ns: u128,
) -> Result<LoadedSnapshot, SnapshotError> {
    match source {
        SnapshotSource::File(path) => {
            let metadata = std::fs::metadata(path).map_err(|error| {
                SnapshotError::Unavailable(format!("stat snapshot file {path}: {error}"))
            })?;
            let modified = metadata.modified().map_err(|error| {
                SnapshotError::Unavailable(format!("modification time of {path}: {error}"))
            })?;
            let mtime_ns = modified
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            if mtime_ns == 0 || mtime_ns == last_mtime_ns {
                return Err(SnapshotError::NotChanged);
            }
            let length = metadata.len();
            if length > MAX_SNAPSHOT_BYTES {
                return Err(SnapshotError::Unavailable(format!(
                    "snapshot file {path} is {length} bytes, over the {MAX_SNAPSHOT_BYTES} byte cap"
                )));
            }
            let bytes = std::fs::read(path).map_err(|error| {
                SnapshotError::Unavailable(format!("read snapshot file {path}: {error}"))
            })?;
            Ok(LoadedSnapshot { bytes, mtime_ns })
        }
        SnapshotSource::Http { url, agent } => {
            let response = agent
                .get(url)
                .set("Accept", "image/jpeg,image/png")
                .set("Cache-Control", "no-cache")
                .call()
                .map_err(|error| SnapshotError::Unavailable(format!("GET {url}: {error}")))?;
            let mut reader = response.into_reader().take(MAX_SNAPSHOT_BYTES + 1);
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).map_err(|error| {
                SnapshotError::Unavailable(format!("read snapshot from {url}: {error}"))
            })?;
            if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
                return Err(SnapshotError::Unavailable(format!(
                    "snapshot from {url} is over the {MAX_SNAPSHOT_BYTES} byte cap"
                )));
            }
            if bytes.is_empty() {
                return Err(SnapshotError::Unavailable(format!(
                    "{url} answered with an empty snapshot"
                )));
            }
            Ok(LoadedSnapshot {
                bytes,
                mtime_ns: 0,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Publication
// ---------------------------------------------------------------------------

/// Why a snapshot that arrived could not be published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestError {
    /// The bytes are not an image this build can decode.
    Decode(String),
    /// The decoded frame could not be written to the arena.
    Publish(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(reason) => write!(formatter, "decode snapshot: {reason}"),
            Self::Publish(reason) => write!(formatter, "publish snapshot: {reason}"),
        }
    }
}

impl std::error::Error for IngestError {}

/// Decodes one encoded snapshot and publishes both halves of it.
///
/// The source is decoded to luma, downscaled to the shared thumbnail geometry,
/// and published with its luma mean and standard deviation; the original
/// encoded bytes are offered to the operator preview when the slot can hold
/// them. Returns the new frame sequence number.
pub fn ingest_snapshot_bytes(shm: &ShmRegion, bytes: &[u8]) -> Result<u64, IngestError> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|error| IngestError::Decode(error.to_string()))?
        .to_luma8();
    let source_width = decoded.width();
    let source_height = decoded.height();
    let thumbnail = image::imageops::resize(
        &decoded,
        CAMERA_THUMB_W as u32,
        CAMERA_THUMB_H as u32,
        FilterType::Triangle,
    );

    let mut thumbnail_luma = [0u8; CAMERA_THUMB_PIXELS];
    thumbnail_luma.copy_from_slice(thumbnail.as_raw().as_slice());

    let mut sum = 0.0f64;
    let mut sum_of_squares = 0.0f64;
    for &pixel in &thumbnail_luma {
        let value = f64::from(pixel) / 255.0;
        sum += value;
        sum_of_squares += value * value;
    }
    let count = CAMERA_THUMB_PIXELS as f64;
    let mean = sum / count;
    let variance = (sum_of_squares / count - mean * mean).max(0.0);

    let snapshot = CameraFrameSnapshot {
        timestamp_ns: now_ns(),
        source_width,
        source_height,
        thumb_width: CAMERA_THUMB_W as u32,
        thumb_height: CAMERA_THUMB_H as u32,
        luminance_mean: mean as f32,
        luminance_stddev: variance.sqrt() as f32,
        valid: true,
        thumbnail_luma,
        ..CameraFrameSnapshot::default()
    };
    let seq = shm
        .camera_frame_mut()
        .publish(&snapshot)
        .map_err(|error| IngestError::Publish(error.to_string()))?;
    publish_camera_preview(shm, bytes, source_width, source_height);
    Ok(seq)
}

/// Publishes the encoded frame for the operator preview.
///
/// The preview is a seqlock: the sequence is odd while the bytes are copied and
/// even when a reader may take them. An encoding the console cannot display
/// (neither JPEG nor PNG, identified by its leading bytes), or one larger than
/// the slot, clears the preview instead of publishing a half-usable one; the
/// thumbnail frame is unaffected either way.
pub fn publish_camera_preview(shm: &ShmRegion, bytes: &[u8], width: u32, height: u32) {
    let format = encoded_format(bytes);
    let publishable = format != PREVIEW_FORMAT_NONE && bytes.len() <= CAMERA_PREVIEW_MAX_BYTES;
    let preview = shm.camera_preview_mut();
    let opening = preview.seq.load(Ordering::Acquire).wrapping_add(1) | 1;
    preview.seq.store(opening, Ordering::Release);
    preview.len.store(0, Ordering::Release);
    if publishable {
        preview.timestamp_ns = now_ns();
        preview.width = width;
        preview.height = height;
        preview.format = format;
        preview.bytes[..bytes.len()].copy_from_slice(bytes);
        preview.len.store(bytes.len(), Ordering::Release);
    } else {
        preview.timestamp_ns = 0;
        preview.width = 0;
        preview.height = 0;
        preview.format = PREVIEW_FORMAT_NONE;
    }
    preview.seq.store(opening.wrapping_add(1), Ordering::Release);
}

/// The preview format code the slot records for `bytes`.
fn encoded_format(bytes: &[u8]) -> u8 {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        PREVIEW_FORMAT_JPEG
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        PREVIEW_FORMAT_PNG
    } else {
        PREVIEW_FORMAT_NONE
    }
}

/// Clears the thumbnail slot to the state a reader sees before the first frame.
pub fn init_camera_frame(frame: &mut CameraFrame) {
    frame.timestamp_ns = 0;
    frame.source_width = 0;
    frame.source_height = 0;
    frame.thumb_width = CAMERA_THUMB_W as u32;
    frame.thumb_height = CAMERA_THUMB_H as u32;
    frame.luminance_mean = 0.0;
    frame.luminance_stddev = 0.0;
    frame.valid.store(false, Ordering::Release);
    frame.thumbnail_luma.fill(0);
    frame.seq.store(0, Ordering::Release);
}

/// Clears the preview slot to the state a reader sees before the first frame.
pub fn init_camera_preview(preview: &mut CameraPreview) {
    preview.timestamp_ns = 0;
    preview.width = 0;
    preview.height = 0;
    preview.format = PREVIEW_FORMAT_NONE;
    preview.len.store(0, Ordering::Release);
    preview.seq.store(0, Ordering::Release);
}

/// Wall-clock time in nanoseconds since the Unix epoch, or zero if the clock is
/// set before it.
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Exposure
// ---------------------------------------------------------------------------

/// How usable a frame's exposure looks to an operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameQuality {
    /// The frame is at or above the blown-highlight threshold.
    Blown,
    /// The frame is at or below the crushed-shadow threshold.
    Dark,
    /// The frame carries too little contrast to be worth mapping.
    Flat,
    /// The frame has usable exposure and contrast.
    Usable,
}

impl FrameQuality {
    /// The one-word label the runner logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Blown => "overexposed",
            Self::Dark => "underexposed",
            Self::Flat => "low-contrast",
            Self::Usable => "usable",
        }
    }
}

/// Classifies a thumbnail's luma mean and standard deviation.
///
/// Exposure is judged before contrast: a frame that is both blown and flat is
/// reported as blown, because that is the fault the operator has to fix.
pub fn frame_quality(mean: f32, stddev: f32) -> FrameQuality {
    if mean >= 0.90 {
        FrameQuality::Blown
    } else if mean <= 0.08 {
        FrameQuality::Dark
    } else if stddev <= 0.04 {
        FrameQuality::Flat
    } else {
        FrameQuality::Usable
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// What one capture attempt did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// A new snapshot reached both slots; `seq` is its frame sequence number
    /// and `mtime_ns` the watermark the caller passes back next time.
    Published { seq: u64, mtime_ns: u128 },
    /// The source had nothing new.
    Unchanged,
    /// The source produced no bytes.
    Unavailable(String),
    /// Bytes arrived but were not a decodable image.
    Corrupt(String),
}

/// Loads and publishes one snapshot.
///
/// A failed attempt publishes nothing: the last frame and preview stay exactly
/// as the previous attempt left them, so a camera that drops out does not blank
/// the stack's view of the world.
pub fn capture_once(
    shm: &ShmRegion,
    source: &SnapshotSource,
    last_mtime_ns: u128,
) -> CaptureOutcome {
    let snapshot = match load_snapshot(source, last_mtime_ns) {
        Ok(snapshot) => snapshot,
        Err(SnapshotError::NotChanged) => return CaptureOutcome::Unchanged,
        Err(SnapshotError::Unavailable(reason)) => return CaptureOutcome::Unavailable(reason),
    };
    match ingest_snapshot_bytes(shm, &snapshot.bytes) {
        Ok(seq) => CaptureOutcome::Published {
            seq,
            mtime_ns: snapshot.mtime_ns,
        },
        Err(IngestError::Decode(reason)) => CaptureOutcome::Corrupt(reason),
        Err(IngestError::Publish(reason)) => CaptureOutcome::Unavailable(reason),
    }
}

// ---------------------------------------------------------------------------
// MJPEG framing
// ---------------------------------------------------------------------------

/// Start-of-image marker of a JPEG frame.
const JPEG_SOI: [u8; 2] = [0xff, 0xd8];
/// End-of-image marker of a JPEG frame.
const JPEG_EOI: [u8; 2] = [0xff, 0xd9];
/// Bytes one read pulls from an MJPEG stream.
const MJPEG_CHUNK_BYTES: usize = 64 * 1024;
/// Buffered bytes discarded once they cannot open a frame.
const MJPEG_BUFFER_LIMIT: usize = 128 * 1024;

/// Cuts whole JPEG frames out of an MJPEG response body.
///
/// A `multipart/x-mixed-replace` body is a sequence of part headers and JPEG
/// frames, and the socket may hand it over in any alignment: the reader keeps
/// whatever it has buffered and looks for the next start-of-image, then for the
/// matching end-of-image, so a frame split across reads is reassembled and part
/// headers are skipped.
pub struct MjpegFrameReader<R> {
    reader: R,
    buffer: Vec<u8>,
}

impl<R: Read> MjpegFrameReader<R> {
    /// Wraps `reader` as an MJPEG frame source.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(MJPEG_CHUNK_BYTES * 2),
        }
    }

    /// Returns the next complete `FF D8 ... FF D9` frame.
    ///
    /// Fails once a frame that is still incomplete has grown past
    /// [`MAX_SNAPSHOT_BYTES`] — a frame completed within the same read that
    /// crosses the cap is still returned, at most one read over it — when the
    /// stream ends mid-frame, or when it ends without another frame at all.
    pub fn next_frame(&mut self) -> Result<Vec<u8>, String> {
        loop {
            if let Some(start) = marker_position(&self.buffer, JPEG_SOI) {
                if start > 0 {
                    self.buffer.drain(..start);
                }
                if let Some(relative_end) = marker_position(&self.buffer[2..], JPEG_EOI) {
                    let end = relative_end + 4;
                    return Ok(self.buffer.drain(..end).collect());
                }
                if self.buffer.len() > MAX_SNAPSHOT_BYTES as usize {
                    return Err(format!(
                        "frame exceeds the {MAX_SNAPSHOT_BYTES} byte snapshot cap"
                    ));
                }
            } else if self.buffer.len() > MJPEG_BUFFER_LIMIT {
                // Nothing here can open a frame, but a trailing 0xFF may be the
                // first half of the next start-of-image.
                let trailing_marker_byte = self.buffer.last() == Some(&JPEG_SOI[0]);
                self.buffer.clear();
                if trailing_marker_byte {
                    self.buffer.push(JPEG_SOI[0]);
                }
            }

            let mut chunk = [0u8; MJPEG_CHUNK_BYTES];
            let read = self
                .reader
                .read(&mut chunk)
                .map_err(|error| format!("read MJPEG stream: {error}"))?;
            if read == 0 {
                return Err("MJPEG stream ended before the next frame was complete".to_string());
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

/// The first offset in `bytes` at which `marker` appears.
pub fn marker_position(bytes: &[u8], marker: [u8; 2]) -> Option<usize> {
    bytes.windows(2).position(|pair| pair == marker)
}
