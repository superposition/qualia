//! Small helpers shared by the session binary's command modules: hashing, timestamps, video
//! probing and the `drone-fg` lookup.

use anyhow::{bail, Result};
use chrono::Utc;
use serde_json::{json, Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

/// Deterministic 64-bit digest of a text blob, rendered as fixed-width lowercase hex.
pub(crate) fn stable_hash(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn current_hlc_timestamp() -> String {
    let physical_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    format!("{physical_ns}:0")
}

pub(crate) fn infer_filename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_owned())
}

/// Path rendered relative to `base` with forward slashes, or the full path when it escapes `base`.
pub(crate) fn relative_path_or_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Locate the video-trace helper: an explicit path, then `QUALIA_DRONE_FG_BIN`, then the bundled
/// build locations.
pub(crate) fn resolve_video_trace_bin(explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(PathBuf::from(path));
    }
    if let Ok(from_env) = std::env::var("QUALIA_DRONE_FG_BIN") {
        if !from_env.trim().is_empty() {
            return Ok(PathBuf::from(from_env));
        }
    }

    for candidate in [
        PathBuf::from("studio/build/drone-fg"),
        PathBuf::from("studio/build/qualia-studio.app/Contents/Resources/drone-fg"),
        PathBuf::from("drone-fg"),
    ] {
        if candidate.components().count() == 1 || candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!("unable to locate drone-fg; pass --video-trace-bin or set QUALIA_DRONE_FG_BIN");
}

/// Frame geometry reported by `ffprobe` for the first video stream.
#[derive(Debug, Clone)]
pub(crate) struct VideoFrameGeometry {
    pub width_px: i64,
    pub height_px: i64,
    pub avg_frame_rate_hz: Option<f64>,
}

/// Accepts ffprobe's `num/den` rationals as well as a bare decimal frame rate.
pub(crate) fn parse_ffprobe_frame_rate(raw: &str) -> Option<f64> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    if let Some((numerator, denominator)) = text.split_once('/') {
        let top = numerator.trim().parse::<f64>().ok()?;
        let bottom = denominator.trim().parse::<f64>().ok()?;
        if bottom.abs() <= f64::EPSILON {
            return None;
        }
        return Some(top / bottom);
    }
    text.parse::<f64>().ok()
}

/// Probe width, height and average frame rate of `input_path`; `None` when ffprobe is absent or
/// reports nothing usable.
pub(crate) fn probe_video_frame_geometry(input_path: &str) -> Option<VideoFrameGeometry> {
    let output = ProcessCommand::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate",
            "-of",
            "json",
            input_path,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let parsed: Value = serde_json::from_slice(&output.stdout).ok()?;
    let stream = parsed.get("streams")?.as_array()?.first()?;
    let width_px = stream.get("width")?.as_i64()?;
    let height_px = stream.get("height")?.as_i64()?;
    if width_px <= 0 || height_px <= 0 {
        return None;
    }

    Some(VideoFrameGeometry {
        width_px,
        height_px,
        avg_frame_rate_hz: stream
            .get("avg_frame_rate")
            .and_then(Value::as_str)
            .and_then(parse_ffprobe_frame_rate),
    })
}

/// Serialise stream metadata, enriching `video` streams with probed frame timing.
pub(crate) fn stream_metadata_json(
    stream_kind: &str,
    path: &str,
    metadata: Value,
) -> Result<String> {
    let mut object = match metadata {
        Value::Object(map) => map,
        scalar => return Ok(serde_json::to_string(&scalar)?),
    };
    if stream_kind == "video" {
        annotate_video_timing(&mut object, path);
    }
    Ok(serde_json::to_string(&Value::Object(object))?)
}

/// Add frame geometry, fps and average frame rate to `metadata` when ffprobe can read `path`.
/// Existing keys win: an explicit manifest value is never overwritten.
fn annotate_video_timing(metadata: &mut Map<String, Value>, path: &str) {
    let Some(geometry) = probe_video_frame_geometry(path) else {
        return;
    };

    metadata
        .entry("video_frame_geometry".to_owned())
        .or_insert_with(|| {
            json!({
                "width_px": geometry.width_px,
                "height_px": geometry.height_px,
                "avg_frame_rate_hz": geometry.avg_frame_rate_hz
            })
        });
    if let Some(fps) = geometry.avg_frame_rate_hz {
        metadata.entry("fps".to_owned()).or_insert(json!(fps));
        metadata
            .entry("avg_frame_rate_hz".to_owned())
            .or_insert(json!(fps));
    }
}
