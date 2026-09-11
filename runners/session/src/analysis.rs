//! The pose-trace analysis chain: read the `drone-fg` CSV artifacts, normalise them into
//! abstraction samples, rebuild the derived control surfaces and persist the whole window.

use anyhow::{bail, Context, Result};
use qualia_session_store::{
    mission_types::{GraphKind, RegionKind},
    AbstractStateSample, AbstractionEpochUpsert, AbstractionSpaceUpsert, SessionStore,
    SessionStreamUpsert, SessionUpsert, WorldRegionUpsert,
};
use qualia_sync_types::{ArtifactFormat, SplineControlState, TensorArtifactRef};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::offline_inference::{run_offline_inference, InferenceJobSpec};
use crate::util::{current_hlc_timestamp, now_iso8601, stable_hash, probe_video_frame_geometry};

pub(crate) const METERS_PER_FOOT: f64 = 0.3048;

// ---------------------------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PoseTraceRow {
    pub step: i64,
    pub timestamp_sec: f64,
    pub tx_ft: f64,
    pub ty_ft: f64,
    pub tz_ft: f64,
    pub qw: f64,
    pub qx: f64,
    pub qy: f64,
    pub qz: f64,
    pub matches: i64,
    pub inliers: i64,
    pub motion_px: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ControllerCaptureRow {
    pub step: i64,
    pub timestamp_sec: f64,
    pub left_u: f64,
    pub left_v: f64,
    pub right_u: f64,
    pub right_v: f64,
    pub left_confidence: f64,
    pub right_confidence: f64,
    pub controller_confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneCloudPointRow {
    pub x_ft: f64,
    pub y_ft: f64,
    pub z_ft: f64,
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub observation_count: i64,
    pub pair_count: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct SceneCloudSummary {
    pub point_count: usize,
    pub min_x_m: f64,
    pub min_y_m: f64,
    pub min_z_m: f64,
    pub max_x_m: f64,
    pub max_y_m: f64,
    pub max_z_m: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct CameraGeometrySummary {
    pub frame_width_px: i64,
    pub frame_height_px: i64,
    pub fx_px: f64,
    pub fy_px: f64,
    pub cx_px: f64,
    pub cy_px: f64,
    pub hfov_deg: f64,
    pub vfov_deg: f64,
    pub camera_model: String,
    pub calibration_source: String,
    pub pose_frame: String,
    pub extrinsics_kind: String,
}

#[derive(Debug, Clone)]
struct ControlProxyRow {
    step: i64,
    timestamp_sec: f64,
    throttle_cmd: f64,
    yaw_cmd: f64,
    roll_cmd: f64,
    pitch_cmd: f64,
    throttle_active: bool,
    yaw_active: bool,
    roll_active: bool,
    pitch_active: bool,
}

#[derive(Debug, Clone)]
struct ControllerCaptureSamples {
    control_proxy: Vec<AbstractStateSample>,
    gimbal_form: Vec<AbstractStateSample>,
    flight_regime: Vec<AbstractStateSample>,
}

#[derive(Debug, Clone, Copy)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

impl Vec3 {
    fn minus(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Quaternion {
    w: f64,
    x: f64,
    y: f64,
    z: f64,
}

impl Quaternion {
    fn normalized(self) -> Self {
        let length = (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if length <= f64::EPSILON {
            return Self {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            };
        }
        Self {
            w: self.w / length,
            x: self.x / length,
            y: self.y / length,
            z: self.z / length,
        }
    }

    fn conjugated(self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    fn times(self, other: Self) -> Self {
        Self {
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
        }
    }

    fn rotated(self, vector: Vec3) -> Vec3 {
        let rotated = self
            .times(Quaternion {
                w: 0.0,
                x: vector.x,
                y: vector.y,
                z: vector.z,
            })
            .times(self.conjugated());
        Vec3 {
            x: rotated.x,
            y: rotated.y,
            z: rotated.z,
        }
    }

    /// Yaw, pitch and roll in degrees, the convention the control proxy is derived in.
    fn ypr_degrees(self) -> (f64, f64, f64) {
        let q = self.normalized();
        let yaw_numerator = 2.0 * (q.w * q.z + q.x * q.y);
        let yaw_denominator = 1.0 - 2.0 * (q.y * q.y + q.z * q.z);
        let yaw = yaw_numerator.atan2(yaw_denominator).to_degrees();

        let pitch_sine = 2.0 * (q.w * q.y - q.z * q.x);
        let pitch = if pitch_sine.abs() >= 1.0 {
            pitch_sine.signum() * std::f64::consts::FRAC_PI_2
        } else {
            pitch_sine.asin()
        }
        .to_degrees();

        let roll_numerator = 2.0 * (q.w * q.x + q.y * q.z);
        let roll_denominator = 1.0 - 2.0 * (q.x * q.x + q.y * q.y);
        let roll = roll_numerator.atan2(roll_denominator).to_degrees();

        (yaw, pitch, roll)
    }

    /// Row-major rotation matrix; column 2 is the body forward axis.
    fn rotation_matrix(self) -> [[f64; 3]; 3] {
        let q = self.normalized();
        let xx = q.x * q.x;
        let yy = q.y * q.y;
        let zz = q.z * q.z;
        let xy = q.x * q.y;
        let xz = q.x * q.z;
        let yz = q.y * q.z;
        let wx = q.w * q.x;
        let wy = q.w * q.y;
        let wz = q.w * q.z;
        [
            [1.0 - 2.0 * (yy + zz), 2.0 * (xy - wz), 2.0 * (xz + wy)],
            [2.0 * (xy + wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz - wx)],
            [2.0 * (xz - wy), 2.0 * (yz + wx), 1.0 - 2.0 * (xx + yy)],
        ]
    }
}

#[derive(Debug, Clone)]
struct TracePoint {
    x_m: f64,
    z_m: f64,
    motion_px: f64,
    confidence: f64,
}

// ---------------------------------------------------------------------------------------------
// CSV readers
// ---------------------------------------------------------------------------------------------

fn parse_column<T: std::str::FromStr>(text: &str, field: &str, line_no: usize) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse::<T>()
        .with_context(|| format!("parse {field} at line {line_no}"))
}

/// Split a data line and reject a row whose column count is not `expected`.
fn split_columns<'a>(line: &'a str, expected: usize, line_no: usize, what: &str, path: &Path) -> Result<Vec<&'a str>> {
    let columns = line.split(',').collect::<Vec<_>>();
    if columns.len() != expected {
        bail!(
            "malformed {what} row at line {} in {}: expected {expected} columns, got {}",
            line_no,
            path.display(),
            columns.len()
        );
    }
    Ok(columns)
}

pub(crate) fn read_pose_trace_csv(path: &Path) -> Result<Vec<PoseTraceRow>> {
    let reader = BufReader::new(
        std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
    );
    let mut rows = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line =
            line.with_context(|| format!("read line {} from {}", line_no + 1, path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_no == 0 && trimmed.starts_with("step,") {
            continue;
        }
        let columns = split_columns(trimmed, 12, line_no + 1, "pose trace", path)?;
        let at = line_no + 1;
        rows.push(PoseTraceRow {
            step: parse_column(columns[0], "step", at)?,
            timestamp_sec: parse_column(columns[1], "timestamp_sec", at)?,
            tx_ft: parse_column(columns[2], "tx", at)?,
            ty_ft: parse_column(columns[3], "ty", at)?,
            tz_ft: parse_column(columns[4], "tz", at)?,
            qw: parse_column(columns[5], "qw", at)?,
            qx: parse_column(columns[6], "qx", at)?,
            qy: parse_column(columns[7], "qy", at)?,
            qz: parse_column(columns[8], "qz", at)?,
            matches: parse_column(columns[9], "matches", at)?,
            inliers: parse_column(columns[10], "inliers", at)?,
            motion_px: parse_column(columns[11], "motion_px", at)?,
        });
    }

    if rows.is_empty() {
        bail!("pose trace csv {} had no rows", path.display());
    }
    Ok(rows)
}

pub(crate) fn read_controller_capture_csv(path: &Path) -> Result<Vec<ControllerCaptureRow>> {
    let reader = BufReader::new(
        std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
    );
    let mut rows = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line =
            line.with_context(|| format!("read line {} from {}", line_no + 1, path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_no == 0 && trimmed.starts_with("step,") {
            continue;
        }
        let columns = split_columns(trimmed, 9, line_no + 1, "controller capture", path)?;
        let at = line_no + 1;
        rows.push(ControllerCaptureRow {
            step: parse_column(columns[0], "step", at)?,
            timestamp_sec: parse_column(columns[1], "timestamp_sec", at)?,
            left_u: parse_column(columns[2], "left_u", at)?,
            left_v: parse_column(columns[3], "left_v", at)?,
            right_u: parse_column(columns[4], "right_u", at)?,
            right_v: parse_column(columns[5], "right_v", at)?,
            left_confidence: parse_column(columns[6], "left_confidence", at)?,
            right_confidence: parse_column(columns[7], "right_confidence", at)?,
            controller_confidence: parse_column(columns[8], "controller_confidence", at)?,
        });
    }

    if rows.is_empty() {
        bail!("controller capture csv {} had no rows", path.display());
    }
    Ok(rows)
}

pub(crate) fn read_scene_cloud_csv(path: &Path) -> Result<Vec<SceneCloudPointRow>> {
    let reader = BufReader::new(
        std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
    );
    let mut rows = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line =
            line.with_context(|| format!("read line {} from {}", line_no + 1, path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_no == 0 && trimmed.starts_with("x_ft,") {
            continue;
        }
        let columns = split_columns(trimmed, 8, line_no + 1, "scene cloud", path)?;
        let at = line_no + 1;
        rows.push(SceneCloudPointRow {
            x_ft: parse_column(columns[0], "x_ft", at)?,
            y_ft: parse_column(columns[1], "y_ft", at)?,
            z_ft: parse_column(columns[2], "z_ft", at)?,
            r: parse_column(columns[3], "r", at)?,
            g: parse_column(columns[4], "g", at)?,
            b: parse_column(columns[5], "b", at)?,
            observation_count: parse_column(columns[6], "observation_count", at)?,
            pair_count: parse_column(columns[7], "pair_count", at)?,
        });
    }

    if rows.is_empty() {
        bail!("scene cloud csv {} had no rows", path.display());
    }
    Ok(rows)
}

pub(crate) fn summarize_scene_cloud(rows: &[SceneCloudPointRow]) -> Option<SceneCloudSummary> {
    let first = rows.first()?;
    let mut summary = SceneCloudSummary {
        point_count: rows.len(),
        min_x_m: first.x_ft * METERS_PER_FOOT,
        min_y_m: first.y_ft * METERS_PER_FOOT,
        min_z_m: first.z_ft * METERS_PER_FOOT,
        max_x_m: first.x_ft * METERS_PER_FOOT,
        max_y_m: first.y_ft * METERS_PER_FOOT,
        max_z_m: first.z_ft * METERS_PER_FOOT,
    };
    for row in &rows[1..] {
        let x_m = row.x_ft * METERS_PER_FOOT;
        let y_m = row.y_ft * METERS_PER_FOOT;
        let z_m = row.z_ft * METERS_PER_FOOT;
        summary.min_x_m = summary.min_x_m.min(x_m);
        summary.min_y_m = summary.min_y_m.min(y_m);
        summary.min_z_m = summary.min_z_m.min(z_m);
        summary.max_x_m = summary.max_x_m.max(x_m);
        summary.max_y_m = summary.max_y_m.max(y_m);
        summary.max_z_m = summary.max_z_m.max(z_m);
    }
    Some(summary)
}

// ---------------------------------------------------------------------------------------------
// Camera geometry
// ---------------------------------------------------------------------------------------------

/// Derive a pinhole summary from the frame size, assuming a 0.95 width focal length.
pub(crate) fn derive_camera_geometry(
    frame_width_px: i64,
    frame_height_px: i64,
) -> CameraGeometrySummary {
    let width = frame_width_px as f64;
    let height = frame_height_px as f64;
    let fx_px = width * 0.95;
    let fy_px = fx_px;
    let hfov_deg = (2.0 * (width / (2.0 * fx_px)).atan()).to_degrees();
    let vfov_deg = (2.0 * (height / (2.0 * fy_px)).atan()).to_degrees();

    CameraGeometrySummary {
        frame_width_px,
        frame_height_px,
        fx_px,
        fy_px,
        cx_px: width * 0.5,
        cy_px: height * 0.5,
        hfov_deg,
        vfov_deg,
        camera_model: "pinhole".to_string(),
        calibration_source: "scene_cloud_heuristic_width_0p95".to_string(),
        pose_frame: "camera".to_string(),
        extrinsics_kind: "identity_camera_pose".to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Helper binary invocations
// ---------------------------------------------------------------------------------------------

fn input_file_has_substance(path: &str) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

fn trace_has_motion(rows: &[PoseTraceRow]) -> bool {
    rows.windows(2).any(|pair| {
        let dx = pair[1].tx_ft - pair[0].tx_ft;
        let dy = pair[1].ty_ft - pair[0].ty_ft;
        let dz = pair[1].tz_ft - pair[0].tz_ft;
        (dx * dx + dy * dy + dz * dz).sqrt() > 1e-4
    })
}

fn run_helper(helper: &Path, args: &[String], what: &str) -> Result<()> {
    let status = ProcessCommand::new(helper)
        .args(args)
        .status()
        .with_context(|| format!("launch {}", helper.display()))?;
    if !status.success() {
        bail!("{what} failed via {} with status {status}", helper.display());
    }
    Ok(())
}

fn frame_limit_args(max_frames: i64) -> Vec<String> {
    if max_frames > 0 {
        vec!["--max-frames".to_string(), max_frames.to_string()]
    } else {
        Vec::new()
    }
}

fn run_video_trace(
    helper: &Path,
    input_path: &str,
    output_path: &Path,
    max_frames: i64,
    frame_stride: i64,
    max_features: i64,
    min_motion_px: f64,
) -> Result<()> {
    let mut args = vec![
        "video-trace".to_string(),
        "--input".to_string(),
        input_path.to_string(),
        "--output".to_string(),
        output_path.to_string_lossy().into_owned(),
        "--frame-stride".to_string(),
        frame_stride.to_string(),
        "--max-features".to_string(),
        max_features.to_string(),
        "--min-motion".to_string(),
        min_motion_px.to_string(),
    ];
    args.extend(frame_limit_args(max_frames));
    run_helper(helper, &args, "video-trace")
}

fn run_controller_capture(
    helper: &Path,
    input_path: &str,
    output_path: &Path,
    max_frames: i64,
    frame_stride: i64,
) -> Result<()> {
    let mut args = vec![
        "controller-capture".to_string(),
        "--input".to_string(),
        input_path.to_string(),
        "--output".to_string(),
        output_path.to_string_lossy().into_owned(),
        "--frame-stride".to_string(),
        frame_stride.to_string(),
    ];
    args.extend(frame_limit_args(max_frames));
    run_helper(helper, &args, "controller-capture")
}

fn run_scene_cloud(
    helper: &Path,
    input_path: &str,
    trace_path: &Path,
    optimized_trace_path: Option<&Path>,
    output_path: &Path,
) -> Result<()> {
    let mut args = vec![
        "scene-cloud".to_string(),
        "--video".to_string(),
        input_path.to_string(),
        "--trace".to_string(),
        trace_path.to_string_lossy().into_owned(),
        "--output-prefix".to_string(),
        output_path.with_extension("").to_string_lossy().into_owned(),
    ];
    if let Some(optimized) = optimized_trace_path {
        args.push("--optimized".to_string());
        args.push(optimized.to_string_lossy().into_owned());
    }
    run_helper(helper, &args, "scene-cloud")
}

fn maybe_generate_scene_cloud(
    input_path: &str,
    video_trace_bin_override: Option<&str>,
    trace_path: &Path,
    optimized_trace_path: Option<&Path>,
    output_path: &Path,
    trace_rows: &[PoseTraceRow],
) -> Result<Option<(PathBuf, SceneCloudSummary)>> {
    if !input_file_has_substance(input_path) || !trace_has_motion(trace_rows) {
        return Ok(None);
    }

    let helper = crate::util::resolve_video_trace_bin(video_trace_bin_override)?;
    if let Err(error) = run_scene_cloud(
        &helper,
        input_path,
        trace_path,
        optimized_trace_path,
        output_path,
    ) {
        eprintln!("warn: scene-cloud skipped for {}: {error:#}", input_path);
        return Ok(None);
    }

    let rows = match read_scene_cloud_csv(output_path) {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!(
                "warn: scene-cloud parse skipped for {}: {error:#}",
                output_path.display()
            );
            return Ok(None);
        }
    };
    let Some(summary) = summarize_scene_cloud(&rows) else {
        return Ok(None);
    };
    Ok(Some((output_path.to_path_buf(), summary)))
}

// ---------------------------------------------------------------------------------------------
// Normalisation
// ---------------------------------------------------------------------------------------------

fn clamp_unit(value: f64) -> f64 {
    value.clamp(-1.0, 1.0)
}

/// 90th-percentile magnitude, never below `fallback`; the scale the control proxy divides by.
fn robust_abs_scale(values: &[f64], fallback: f64) -> f64 {
    let mut magnitudes = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .map(f64::abs)
        .collect::<Vec<_>>();
    if magnitudes.is_empty() {
        return fallback;
    }
    magnitudes.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((magnitudes.len() - 1) as f64 * 0.90).floor() as usize;
    fallback.max(magnitudes[index])
}

pub(crate) fn ypr_from_quaternion_rad(qw: f64, qx: f64, qy: f64, qz: f64) -> (f64, f64, f64) {
    let yaw = (2.0 * (qw * qz + qx * qy)).atan2(1.0 - 2.0 * (qy * qy + qz * qz));
    let pitch_sine = 2.0 * (qw * qy - qz * qx);
    let pitch = if pitch_sine.abs() >= 1.0 {
        pitch_sine.signum() * std::f64::consts::FRAC_PI_2
    } else {
        pitch_sine.asin()
    };
    let roll = (2.0 * (qw * qx + qy * qz)).atan2(1.0 - 2.0 * (qx * qx + qy * qy));
    (yaw, pitch, roll)
}

/// Confidence the pose trace carries per sample: a fixed opener, then a blend of inlier ratio and
/// image motion.
pub(crate) fn pose_confidence(matches: i64, inliers: i64, motion_px: f64, index: usize) -> f64 {
    if index == 0 {
        return 0.85;
    }
    if matches <= 0 {
        return 0.40;
    }
    let inlier_ratio = (inliers.max(0) as f64 / matches.max(1) as f64).clamp(0.0, 1.0);
    let motion_score = (motion_px / 12.0).clamp(0.0, 1.0);
    (0.35 + 0.45 * inlier_ratio + 0.20 * motion_score).clamp(0.0, 0.99)
}

pub(crate) fn normalize_pose_trace_rows(rows: &[PoseTraceRow]) -> Vec<AbstractStateSample> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let x_m = row.tx_ft * METERS_PER_FOOT;
            let y_m = row.ty_ft * METERS_PER_FOOT;
            let z_m = row.tz_ft * METERS_PER_FOOT;
            let (yaw_rad, pitch_rad, roll_rad) =
                ypr_from_quaternion_rad(row.qw, row.qx, row.qy, row.qz);
            let payload = json!({
                "x_m": x_m,
                "y_m": y_m,
                "z_m": z_m,
                "yaw_rad": yaw_rad,
                "pitch_rad": pitch_rad,
                "roll_rad": roll_rad,
                "qw": row.qw,
                "qx": row.qx,
                "qy": row.qy,
                "qz": row.qz,
                "matches": row.matches,
                "inliers": row.inliers,
                "motion_px": row.motion_px,
                "source_unit": "ft",
                "position_source": {
                    "tx_ft": row.tx_ft,
                    "ty_ft": row.ty_ft,
                    "tz_ft": row.tz_ft
                }
            });
            AbstractStateSample {
                step: row.step,
                timestamp_sec: row.timestamp_sec,
                symbol_key: "pose_observation".to_string(),
                payload_json: payload.to_string(),
                confidence: pose_confidence(row.matches, row.inliers, row.motion_px, index),
                sample_hash: stable_hash(&format!(
                    "pose_trace|{}|{:.6}|{:.6}|{:.6}|{:.6}|{:.6}",
                    row.step, row.timestamp_sec, x_m, y_m, z_m, yaw_rad
                )),
            }
        })
        .collect()
}

pub(crate) fn normalize_controller_capture_rows(
    rows: &[ControllerCaptureRow],
) -> Vec<AbstractStateSample> {
    rows.iter()
        .map(|row| {
            let payload = json!({
                "left_u": row.left_u,
                "left_v": row.left_v,
                "right_u": row.right_u,
                "right_v": row.right_v,
                "left_confidence": row.left_confidence,
                "right_confidence": row.right_confidence,
                "controller_confidence": row.controller_confidence,
            });
            AbstractStateSample {
                step: row.step,
                timestamp_sec: row.timestamp_sec,
                symbol_key: "rc_input_observation".to_string(),
                payload_json: payload.to_string(),
                confidence: row.controller_confidence.clamp(0.0, 1.0),
                sample_hash: stable_hash(&format!(
                    "rc_input_observation|{}|{:.6}|{}",
                    row.step, row.timestamp_sec, payload
                )),
            }
        })
        .collect()
}

fn position_vec_ft(row: &PoseTraceRow) -> Vec3 {
    Vec3 {
        x: row.tx_ft,
        y: row.ty_ft,
        z: row.tz_ft,
    }
}

fn orientation_quaternion(row: &PoseTraceRow) -> Quaternion {
    Quaternion {
        w: row.qw,
        x: row.qx,
        y: row.qy,
        z: row.qz,
    }
    .normalized()
}

fn flight_regime_label(row: &ControlProxyRow) -> &'static str {
    let throttle = row.throttle_cmd.abs();
    let yaw = row.yaw_cmd.abs();
    let roll = row.roll_cmd.abs();
    let pitch = row.pitch_cmd.abs();
    if throttle.max(yaw).max(roll).max(pitch) < 0.12 {
        return "hover";
    }
    if yaw > 0.45 && roll < 0.25 && pitch < 0.25 {
        return "yaw_turn";
    }
    if throttle > 0.5 && roll < 0.35 && pitch < 0.35 {
        return "climb_descent";
    }
    if pitch > 0.35 && roll < 0.35 {
        return "forward_back";
    }
    if roll > 0.35 && pitch < 0.35 {
        return "lateral_bank";
    }
    "maneuver"
}

/// Rebuild the command, gimbal and regime surfaces from pose deltas expressed in each frame's own
/// body axes.
fn build_controller_capture_samples(rows: &[PoseTraceRow]) -> ControllerCaptureSamples {
    if rows.is_empty() {
        return ControllerCaptureSamples {
            control_proxy: Vec::new(),
            gimbal_form: Vec::new(),
            flight_regime: Vec::new(),
        };
    }

    let mut body_forward = Vec::with_capacity(rows.len().saturating_sub(1));
    let mut body_vertical = Vec::with_capacity(rows.len().saturating_sub(1));
    let mut body_lateral = Vec::with_capacity(rows.len().saturating_sub(1));
    let mut body_yaw = Vec::with_capacity(rows.len().saturating_sub(1));
    for pair in rows.windows(2) {
        let (delta, yaw_deg) = local_delta(&pair[0], &pair[1]);
        body_lateral.push(delta.x);
        body_vertical.push(delta.y);
        body_forward.push(delta.z);
        body_yaw.push(yaw_deg);
    }

    let forward_scale = robust_abs_scale(&body_forward, 0.10);
    let vertical_scale = robust_abs_scale(&body_vertical, 0.08);
    let lateral_scale = robust_abs_scale(&body_lateral, 0.10);
    let yaw_scale = robust_abs_scale(&body_yaw, 4.0);

    let mut controls = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let mut control = ControlProxyRow {
            step: row.step,
            timestamp_sec: row.timestamp_sec,
            throttle_cmd: 0.0,
            yaw_cmd: 0.0,
            roll_cmd: 0.0,
            pitch_cmd: 0.0,
            throttle_active: false,
            yaw_active: false,
            roll_active: false,
            pitch_active: false,
        };
        if index > 0 {
            let (delta, yaw_deg) = local_delta(&rows[index - 1], row);
            control.roll_cmd = clamp_unit(delta.x / lateral_scale);
            control.throttle_cmd = clamp_unit(delta.y / vertical_scale);
            control.pitch_cmd = clamp_unit(delta.z / forward_scale);
            control.yaw_cmd = clamp_unit(yaw_deg / yaw_scale);
        }
        control.throttle_active = control.throttle_cmd.abs() >= 0.14;
        control.yaw_active = control.yaw_cmd.abs() >= 0.12;
        control.roll_active = control.roll_cmd.abs() >= 0.12;
        control.pitch_active = control.pitch_cmd.abs() >= 0.12;
        controls.push(control);
    }

    let control_proxy = controls
        .iter()
        .map(|control| {
            let payload = json!({
                "throttle_cmd": control.throttle_cmd,
                "yaw_cmd": control.yaw_cmd,
                "roll_cmd": control.roll_cmd,
                "pitch_cmd": control.pitch_cmd,
                "throttle_active": control.throttle_active,
                "yaw_active": control.yaw_active,
                "roll_active": control.roll_active,
                "pitch_active": control.pitch_active,
            });
            AbstractStateSample {
                step: control.step,
                timestamp_sec: control.timestamp_sec,
                symbol_key: "control_proxy".to_string(),
                confidence: control
                    .throttle_cmd
                    .abs()
                    .max(control.yaw_cmd.abs())
                    .max(control.roll_cmd.abs())
                    .max(control.pitch_cmd.abs()),
                sample_hash: stable_hash(&format!(
                    "control_proxy|{}|{:.6}|{}",
                    control.step, control.timestamp_sec, payload
                )),
                payload_json: payload.to_string(),
            }
        })
        .collect::<Vec<_>>();

    let gimbal_form = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let control = &controls[index];
            let orientation = orientation_quaternion(row);
            let (gimbal_yaw, gimbal_pitch, gimbal_roll) = orientation.ypr_degrees();
            let matrix = orientation.rotation_matrix();
            let payload = json!({
                "left_u": control.yaw_cmd,
                "left_v": control.throttle_cmd,
                "right_u": control.roll_cmd,
                "right_v": control.pitch_cmd,
                "gimbal_yaw": gimbal_yaw,
                "gimbal_pitch": gimbal_pitch,
                "gimbal_roll": gimbal_roll,
                "nx": matrix[0][2],
                "ny": matrix[1][2],
                "nz": matrix[2][2],
            });
            let confidence = control
                .yaw_cmd
                .abs()
                .max(control.throttle_cmd.abs())
                .max(control.roll_cmd.abs())
                .max(control.pitch_cmd.abs());
            AbstractStateSample {
                step: row.step,
                timestamp_sec: row.timestamp_sec,
                symbol_key: "gimbal_form".to_string(),
                payload_json: payload.to_string(),
                confidence,
                sample_hash: stable_hash(&format!(
                    "gimbal_form|{}|{:.6}|{}",
                    row.step, row.timestamp_sec, payload
                )),
            }
        })
        .collect::<Vec<_>>();

    let flight_regime = controls
        .iter()
        .map(|control| {
            let label = flight_regime_label(control);
            let confidence = control
                .throttle_cmd
                .abs()
                .max(control.yaw_cmd.abs())
                .max(control.roll_cmd.abs())
                .max(control.pitch_cmd.abs());
            let payload = json!({
                "label": label,
                "throttle_cmd": control.throttle_cmd,
                "yaw_cmd": control.yaw_cmd,
                "roll_cmd": control.roll_cmd,
                "pitch_cmd": control.pitch_cmd,
                "confidence": confidence,
            });
            AbstractStateSample {
                step: control.step,
                timestamp_sec: control.timestamp_sec,
                symbol_key: label.to_string(),
                payload_json: payload.to_string(),
                confidence,
                sample_hash: stable_hash(&format!(
                    "flight_regime|{}|{:.6}|{}",
                    control.step, control.timestamp_sec, payload
                )),
            }
        })
        .collect::<Vec<_>>();

    ControllerCaptureSamples {
        control_proxy,
        gimbal_form,
        flight_regime,
    }
}

/// Translation delta of `current` relative to `previous`, rotated into the previous body frame,
/// with the yaw change in degrees.
fn local_delta(previous: &PoseTraceRow, current: &PoseTraceRow) -> (Vec3, f64) {
    let global = position_vec_ft(current).minus(position_vec_ft(previous));
    let rotation = orientation_quaternion(previous);
    let local = rotation.conjugated().rotated(global);
    let delta_rotation = rotation
        .conjugated()
        .times(orientation_quaternion(current))
        .normalized();
    let (yaw_deg, _, _) = delta_rotation.ypr_degrees();
    (local, yaw_deg)
}

// ---------------------------------------------------------------------------------------------
// Controller capture discovery
// ---------------------------------------------------------------------------------------------

fn should_attempt_controller_capture(
    session: &qualia_session_store::SessionRow,
    input_path: &str,
) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        session.media_kind.to_ascii_lowercase(),
        session.analysis_kind.to_ascii_lowercase(),
        session.filename.to_ascii_lowercase(),
        input_path.to_ascii_lowercase()
    );
    [
        "controller",
        "sticks",
        "gimbal",
        "dji",
        "cleanshot",
        "desktop",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn load_or_extract_controller_capture(
    session: &qualia_session_store::SessionRow,
    input_path: &str,
    controller_csv_override: Option<&str>,
    video_trace_bin_override: Option<&str>,
    output_path: &Path,
    max_frames: i64,
    frame_stride: i64,
) -> Result<Option<Vec<ControllerCaptureRow>>> {
    if let Some(controller_csv) = controller_csv_override {
        if output_path != Path::new(controller_csv) {
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("create controller output directory {}", parent.display())
                })?;
            }
            std::fs::copy(controller_csv, output_path).with_context(|| {
                format!(
                    "copy controller csv from {} to {}",
                    controller_csv,
                    output_path.display()
                )
            })?;
        }
        return read_controller_capture_csv(output_path).map(Some);
    }

    if !should_attempt_controller_capture(session, input_path) {
        return Ok(None);
    }

    let helper = crate::util::resolve_video_trace_bin(video_trace_bin_override)?;
    if let Err(error) = run_controller_capture(
        &helper,
        input_path,
        output_path,
        max_frames,
        frame_stride,
    ) {
        eprintln!(
            "warn: controller-capture skipped for {}: {error:#}",
            input_path
        );
        return Ok(None);
    }

    let rows = match read_controller_capture_csv(output_path) {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!(
                "warn: controller-capture parse skipped for {}: {error:#}",
                output_path.display()
            );
            return Ok(None);
        }
    };
    let mean_confidence =
        rows.iter().map(|row| row.controller_confidence).sum::<f64>() / rows.len() as f64;
    if mean_confidence < 0.10 {
        eprintln!(
            "warn: controller-capture confidence too low for {} ({:.3}); ignoring",
            input_path, mean_confidence
        );
        return Ok(None);
    }
    Ok(Some(rows))
}

// ---------------------------------------------------------------------------------------------
// Trajectory spline artifact
// ---------------------------------------------------------------------------------------------

fn sample_position(sample: &AbstractStateSample) -> Result<(f64, f64, f64)> {
    let payload: Value = serde_json::from_str(&sample.payload_json)
        .with_context(|| format!("parse pose payload for sample {}", sample.step))?;
    Ok((
        payload.get("x_m").and_then(Value::as_f64).unwrap_or(0.0),
        payload.get("y_m").and_then(Value::as_f64).unwrap_or(0.0),
        payload.get("z_m").and_then(Value::as_f64).unwrap_or(0.0),
    ))
}

fn estimate_velocity(
    previous: Option<&AbstractStateSample>,
    current: &AbstractStateSample,
    next: Option<&AbstractStateSample>,
) -> Result<(f64, f64, f64)> {
    let here = sample_position(current)?;
    match (previous, next) {
        (Some(previous), Some(next)) => {
            let before = sample_position(previous)?;
            let after = sample_position(next)?;
            let dt = (next.timestamp_sec - previous.timestamp_sec).max(1e-6);
            Ok((
                (after.0 - before.0) / dt,
                (after.1 - before.1) / dt,
                (after.2 - before.2) / dt,
            ))
        }
        (Some(previous), None) => {
            let before = sample_position(previous)?;
            let dt = (current.timestamp_sec - previous.timestamp_sec).max(1e-6);
            Ok(((here.0 - before.0) / dt, (here.1 - before.1) / dt, (here.2 - before.2) / dt))
        }
        (None, Some(next)) => {
            let after = sample_position(next)?;
            let dt = (next.timestamp_sec - current.timestamp_sec).max(1e-6);
            Ok(((after.0 - here.0) / dt, (after.1 - here.1) / dt, (after.2 - here.2) / dt))
        }
        (None, None) => Ok((0.0, 0.0, 0.0)),
    }
}

/// Even knot indices across the sample list, at most 32, always including the first and last.
fn select_knot_indices(sample_count: usize) -> Vec<usize> {
    let target = 32usize.min(sample_count.max(2));
    if target == 1 {
        return vec![0];
    }
    let last = sample_count - 1;
    let mut selected = Vec::with_capacity(target + 1);
    for knot in 0..target {
        let ratio = knot as f64 / (target - 1) as f64;
        let index = (last as f64 * ratio).round() as usize;
        if selected.last().copied() != Some(index) {
            selected.push(index);
        }
    }
    if selected.last().copied() != Some(last) {
        selected.push(last);
    }
    selected
}

fn write_trajectory_spline_artifact(
    output_path: &Path,
    samples: &[AbstractStateSample],
) -> Result<(String, String, f32)> {
    if samples.is_empty() {
        bail!("cannot build trajectory spline artifact from empty samples");
    }

    let indices = select_knot_indices(samples.len());
    let mut knots = Vec::with_capacity(indices.len());
    let mut confidence_sum = 0.0f64;

    for &index in &indices {
        let sample = &samples[index];
        let payload: Value = serde_json::from_str(&sample.payload_json).with_context(|| {
            format!(
                "parse pose payload for trajectory spline at sample {}",
                sample.step
            )
        })?;
        let x_m = payload.get("x_m").and_then(Value::as_f64).unwrap_or(0.0);
        let y_m = payload.get("y_m").and_then(Value::as_f64).unwrap_or(0.0);
        let z_m = payload.get("z_m").and_then(Value::as_f64).unwrap_or(0.0);
        let qw = payload.get("qw").and_then(Value::as_f64).unwrap_or(1.0);
        let qx = payload.get("qx").and_then(Value::as_f64).unwrap_or(0.0);
        let qy = payload.get("qy").and_then(Value::as_f64).unwrap_or(0.0);
        let qz = payload.get("qz").and_then(Value::as_f64).unwrap_or(0.0);

        let previous = index.checked_sub(1).map(|before| &samples[before]);
        let next = samples.get(index + 1);
        let velocity = estimate_velocity(previous, sample, next)?;
        let weight = sample.confidence.clamp(0.0, 1.0);
        confidence_sum += weight;

        knots.push(json!({
            "sample_index": index,
            "step": sample.step,
            "timestamp_sec": sample.timestamp_sec,
            "position_m": [x_m, y_m, z_m],
            "velocity_mps": [velocity.0, velocity.1, velocity.2],
            "orientation_quat": [qw, qx, qy, qz],
            "weight": weight
        }));
    }

    let artifact = json!({
        "schema": "trajectory_spline.v1",
        "basis": "weighted_quintic_hermite_seed",
        "frame": "relative_monocular",
        "knot_profile": "nonuniform_time",
        "sample_count": samples.len(),
        "knot_count": knots.len(),
        "knots": knots
    });
    let artifact_text = serde_json::to_string_pretty(&artifact)?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create spline artifact directory {}", parent.display()))?;
    }
    std::fs::write(output_path, &artifact_text)
        .with_context(|| format!("write trajectory spline artifact {}", output_path.display()))?;

    Ok((
        output_path.to_string_lossy().into_owned(),
        stable_hash(&artifact_text),
        (confidence_sum / indices.len() as f64).clamp(0.0, 1.0) as f32,
    ))
}

// ---------------------------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------------------------

pub(crate) fn select_observation_video_stream(
    streams: &[qualia_session_store::SessionStreamRow],
) -> Option<qualia_session_store::SessionStreamRow> {
    streams
        .iter()
        .find(|stream| stream.stream_kind == "video" && stream.role == "observation")
        .or_else(|| streams.iter().find(|stream| stream.stream_kind == "video"))
        .cloned()
}

pub(crate) fn default_session_artifact_dir(store_path: &str, session_id: i64) -> PathBuf {
    Path::new(store_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("artifacts"))
        .join(format!("session_{session_id}"))
}

fn next_epoch_index(store: &SessionStore, session_id: i64) -> i64 {
    store
        .list_epochs(session_id)
        .ok()
        .and_then(|epochs| epochs.into_iter().map(|epoch| epoch.epoch_index).max())
        .map(|highest| highest + 1)
        .unwrap_or(0)
}

/// Per-epoch frames plus the two observation spaces and the three derived action/state spaces.
fn persist_epoch_samples(
    store: &SessionStore,
    epoch_id: i64,
    input_path: &str,
    observation_stream_key: Option<&str>,
    samples: &[AbstractStateSample],
    controller_capture_samples: Option<&[AbstractStateSample]>,
    trace_rows: &[PoseTraceRow],
    camera_geometry: Option<&CameraGeometrySummary>,
) -> Result<(i64, i64, Option<i64>)> {
    let frame_space_id = store.upsert_space(&AbstractionSpaceUpsert {
        epoch_id,
        space_family: "observation".to_string(),
        abstraction_name: "frame_observation".to_string(),
        source_kind: "video_trace".to_string(),
        representation_kind: "discrete".to_string(),
        uncertainty_kind: "reported".to_string(),
        dimensionality: 3,
        schema_json: json!({
            "fields": ["frame_index", "stream_key", "artifact_path"],
            "artifact_kind": "video_frame",
            "camera_geometry": camera_geometry
        })
        .to_string(),
    })?;
    let frame_samples = samples
        .iter()
        .map(|sample| {
            let payload = json!({
                "frame_index": sample.step,
                "stream_key": observation_stream_key,
                "artifact_path": input_path
            });
            AbstractStateSample {
                step: sample.step,
                timestamp_sec: sample.timestamp_sec,
                symbol_key: observation_stream_key
                    .unwrap_or("frame_observation")
                    .to_string(),
                payload_json: payload.to_string(),
                confidence: 1.0,
                sample_hash: stable_hash(&format!(
                    "frame_observation|{}|{:.6}|{}|{}",
                    sample.step,
                    sample.timestamp_sec,
                    observation_stream_key.unwrap_or(""),
                    input_path
                )),
            }
        })
        .collect::<Vec<_>>();
    store.replace_state_samples(epoch_id, frame_space_id, &frame_samples)?;

    let space_id = store.upsert_space(&AbstractionSpaceUpsert {
        epoch_id,
        space_family: "observation".to_string(),
        abstraction_name: "pose_observation".to_string(),
        source_kind: "video_trace".to_string(),
        representation_kind: "continuous".to_string(),
        uncertainty_kind: "estimated".to_string(),
        dimensionality: 12,
        schema_json: json!({
            "fields": [
                "x_m", "y_m", "z_m", "yaw_rad", "pitch_rad", "roll_rad",
                "qw", "qx", "qy", "qz", "matches", "inliers", "motion_px"
            ],
            "source_unit": "ft",
            "normalized_unit": "m"
        })
        .to_string(),
    })?;
    store.replace_state_samples(epoch_id, space_id, samples)?;

    let rc_input_space_id = if let Some(capture_samples) = controller_capture_samples {
        let rc_space_id = store.upsert_space(&AbstractionSpaceUpsert {
            epoch_id,
            space_family: "action".to_string(),
            abstraction_name: "rc_input_observation".to_string(),
            source_kind: "controller_capture".to_string(),
            representation_kind: "continuous".to_string(),
            uncertainty_kind: "estimated".to_string(),
            dimensionality: 7,
            schema_json: json!({
                "fields": [
                    "left_u", "left_v", "right_u", "right_v",
                    "left_confidence", "right_confidence", "controller_confidence"
                ],
                "capture_source": "controller_video"
            })
            .to_string(),
        })?;
        store.replace_state_samples(epoch_id, rc_space_id, capture_samples)?;
        Some(rc_space_id)
    } else {
        None
    };

    let derived = build_controller_capture_samples(trace_rows);
    if !derived.control_proxy.is_empty() {
        let control_space_id = store.upsert_space(&AbstractionSpaceUpsert {
            epoch_id,
            space_family: "action".to_string(),
            abstraction_name: "control_proxy".to_string(),
            source_kind: "video_trace_derived".to_string(),
            representation_kind: "continuous".to_string(),
            uncertainty_kind: "estimated".to_string(),
            dimensionality: 4,
            schema_json: json!({
                "fields": ["throttle_cmd", "yaw_cmd", "roll_cmd", "pitch_cmd"],
                "derivation": "pose_trace_proxy"
            })
            .to_string(),
        })?;
        store.replace_state_samples(epoch_id, control_space_id, &derived.control_proxy)?;
    }

    if !derived.gimbal_form.is_empty() {
        let gimbal_space_id = store.upsert_space(&AbstractionSpaceUpsert {
            epoch_id,
            space_family: "action".to_string(),
            abstraction_name: "gimbal_form".to_string(),
            source_kind: "video_trace_derived".to_string(),
            representation_kind: "continuous".to_string(),
            uncertainty_kind: "estimated".to_string(),
            dimensionality: 10,
            schema_json: json!({
                "fields": [
                    "left_u", "left_v", "right_u", "right_v",
                    "gimbal_yaw", "gimbal_pitch", "gimbal_roll", "nx", "ny", "nz"
                ],
                "derivation": "pose_trace_proxy"
            })
            .to_string(),
        })?;
        store.replace_state_samples(epoch_id, gimbal_space_id, &derived.gimbal_form)?;
    }

    if !derived.flight_regime.is_empty() {
        let regime_space_id = store.upsert_space(&AbstractionSpaceUpsert {
            epoch_id,
            space_family: "state".to_string(),
            abstraction_name: "flight_regime".to_string(),
            source_kind: "video_trace_derived".to_string(),
            representation_kind: "discrete".to_string(),
            uncertainty_kind: "estimated".to_string(),
            dimensionality: 1,
            schema_json: json!({
                "labels": [
                    "hover", "yaw_turn", "climb_descent",
                    "forward_back", "lateral_bank", "maneuver"
                ],
                "derivation": "pose_trace_proxy"
            })
            .to_string(),
        })?;
        store.replace_state_samples(epoch_id, regime_space_id, &derived.flight_regime)?;
    }

    Ok((frame_space_id, space_id, rc_input_space_id))
}

fn trace_point_from_sample(sample: &AbstractStateSample) -> Option<TracePoint> {
    let payload = serde_json::from_str::<Value>(&sample.payload_json).ok()?;
    Some(TracePoint {
        x_m: payload.get("x_m")?.as_f64()?,
        z_m: payload.get("z_m")?.as_f64()?,
        motion_px: payload
            .get("motion_px")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        confidence: sample.confidence,
    })
}

/// Run the offline graph inference over the fresh samples and wrap the trace in a local-space
/// region whose support polygon follows the path.
fn persist_trace_bootstrap_world(
    store: &SessionStore,
    session_id: i64,
    epoch_id: i64,
    epoch_index: i64,
    input_path: &str,
    samples: &[AbstractStateSample],
) -> Result<()> {
    if samples.is_empty() {
        return Ok(());
    }

    let window_start_sec = samples.first().map(|sample| sample.timestamp_sec).unwrap_or(0.0);
    let window_end_sec = samples
        .last()
        .map(|sample| sample.timestamp_sec)
        .unwrap_or(window_start_sec);
    let inference = run_offline_inference(
        store,
        InferenceJobSpec {
            session_id,
            environment_id: Some(session_id),
            window_start_sec: Some(window_start_sec),
            window_end_sec: Some(window_end_sec),
            graph_kind: GraphKind::Sensorimotor,
            require_exact: false,
        },
    )
    .context("bootstrap trace graph inference")?;

    let Some(&fragment_id) = inference.fragment_ids.first() else {
        return Ok(());
    };

    let points = samples
        .iter()
        .filter_map(trace_point_from_sample)
        .collect::<Vec<_>>();
    if points.is_empty() {
        return Ok(());
    }

    let min_x = points.iter().map(|point| point.x_m).fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|point| point.x_m)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_z = points.iter().map(|point| point.z_m).fold(f64::INFINITY, f64::min);
    let max_z = points
        .iter()
        .map(|point| point.z_m)
        .fold(f64::NEG_INFINITY, f64::max);

    let span = (max_x - min_x).abs().max((max_z - min_z).abs());
    let padding = 0.25_f64.max(span * 0.10);
    let bounds = vec![
        vec![min_x - padding, min_z - padding],
        vec![max_x + padding, min_z - padding],
        vec![max_x + padding, max_z + padding],
        vec![min_x - padding, max_z + padding],
    ];

    let centroid_x = points.iter().map(|point| point.x_m).sum::<f64>() / points.len() as f64;
    let centroid_z = points.iter().map(|point| point.z_m).sum::<f64>() / points.len() as f64;
    let avg_confidence =
        points.iter().map(|point| point.confidence).sum::<f64>() / points.len() as f64;
    let avg_motion_px = points.iter().map(|point| point.motion_px).sum::<f64>() / points.len() as f64;
    let path_length_m = points
        .windows(2)
        .map(|pair| {
            let dx = pair[1].x_m - pair[0].x_m;
            let dz = pair[1].z_m - pair[0].z_m;
            (dx * dx + dz * dz).sqrt()
        })
        .sum::<f64>();

    let stride = ((points.len() + 31) / 32).max(1);
    let support_points = points
        .iter()
        .step_by(stride)
        .map(|point| vec![point.x_m, point.z_m])
        .collect::<Vec<_>>();

    store
        .upsert_world_region(&WorldRegionUpsert {
            environment_id: Some(session_id),
            session_id,
            source_fragment_id: fragment_id,
            region_key: format!("trace_envelope_epoch_{epoch_index}"),
            region_kind: RegionKind::LocalSpace,
            support_point_count: points.len() as i64,
            confidence: avg_confidence.clamp(0.0, 1.0),
            centroid_json: json!([centroid_x, centroid_z]).to_string(),
            bounds_json: serde_json::to_string(&bounds)?,
            signature_hash: stable_hash(&format!(
                "trace_envelope|{session_id}|{epoch_index}|{:.6}|{:.6}|{:.6}|{:.6}|{}",
                min_x,
                min_z,
                max_x,
                max_z,
                points.len()
            )),
            metadata_json: json!({
                "source": "video_trace_bootstrap",
                "input_path": input_path,
                "epoch_id": epoch_id,
                "epoch_index": epoch_index,
                "sample_count": samples.len(),
                "window_start_sec": window_start_sec,
                "window_end_sec": window_end_sec,
                "path_length_m": path_length_m,
                "avg_confidence": avg_confidence,
                "avg_motion_px": avg_motion_px,
                "lidar_points": support_points,
            })
            .to_string(),
        })
        .context("persist trace bootstrap world region")?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn persist_pose_trace_analysis(
    store: &SessionStore,
    session: &qualia_session_store::SessionRow,
    input_path: &str,
    trace_path: &Path,
    controller_capture_path: Option<&Path>,
    optimized_trace_path: Option<&Path>,
    trace_rows: &[PoseTraceRow],
    samples: &[AbstractStateSample],
    scene_cloud: Option<&(PathBuf, SceneCloudSummary)>,
    trajectory_spline_path: &Path,
    controller_capture_samples: Option<&[AbstractStateSample]>,
    observation_stream_key: Option<&str>,
    epoch_index: i64,
    layer_name: &str,
    abstraction_family: &str,
    trace_stream_key: &str,
) -> Result<VideoAnalysisReport> {
    let now = now_iso8601();
    let frame_geometry = probe_video_frame_geometry(input_path);
    let camera_geometry = frame_geometry
        .as_ref()
        .map(|geometry| derive_camera_geometry(geometry.width_px, geometry.height_px));
    let duration_sec = samples
        .last()
        .map(|sample| sample.timestamp_sec)
        .unwrap_or(session.duration_sec);
    let analysis_kind = "monocular_pose_trace".to_string();
    let (spline_path_text, spline_checksum, spline_confidence) =
        write_trajectory_spline_artifact(trajectory_spline_path, samples)?;
    let spline_control_key = format!("spline/session_{}/epoch_{epoch_index}", session.id);

    store.upsert_session(&SessionUpsert {
        path: session.path.clone(),
        filename: session.filename.clone(),
        media_kind: session.media_kind.clone(),
        analysis_kind: analysis_kind.clone(),
        status: "ready".to_string(),
        duration_sec: duration_sec.max(session.duration_sec),
        imported_at: session.imported_at.clone(),
    })?;

    let optimized_path_text =
        optimized_trace_path.map(|path| path.to_string_lossy().into_owned());
    store.upsert_stream(&SessionStreamUpsert {
        session_id: session.id,
        stream_key: trace_stream_key.to_string(),
        stream_kind: "pose_trace_csv".to_string(),
        role: "execution".to_string(),
        path: trace_path.to_string_lossy().into_owned(),
        sync_group: "analysis".to_string(),
        metadata_json: json!({
            "source": "video-trace",
            "input_path": input_path,
            "optimized_trace_path": optimized_path_text,
            "sample_count": samples.len(),
            "controller_capture_path": controller_capture_path
                .map(|path| path.to_string_lossy().into_owned()),
            "camera_geometry": camera_geometry.as_ref()
        })
        .to_string(),
    })?;

    if let Some((scene_cloud_path, scene_cloud_summary)) = scene_cloud {
        store.upsert_stream(&SessionStreamUpsert {
            session_id: session.id,
            stream_key: "scene_cloud".to_string(),
            stream_kind: "scene_cloud_csv".to_string(),
            role: "world".to_string(),
            path: scene_cloud_path.to_string_lossy().into_owned(),
            sync_group: "analysis".to_string(),
            metadata_json: json!({
                "source": "scene-cloud",
                "input_path": input_path,
                "trace_path": trace_path.to_string_lossy().into_owned(),
                "optimized_trace_path": optimized_path_text,
                "point_count": scene_cloud_summary.point_count,
                "camera_geometry": camera_geometry.as_ref(),
                "bounds_m": {
                    "min": [
                        scene_cloud_summary.min_x_m,
                        scene_cloud_summary.min_y_m,
                        scene_cloud_summary.min_z_m
                    ],
                    "max": [
                        scene_cloud_summary.max_x_m,
                        scene_cloud_summary.max_y_m,
                        scene_cloud_summary.max_z_m
                    ]
                }
            })
            .to_string(),
        })?;
    }

    store.upsert_stream(&SessionStreamUpsert {
        session_id: session.id,
        stream_key: "trajectory_spline".to_string(),
        stream_kind: "trajectory_spline_json".to_string(),
        role: "world".to_string(),
        path: spline_path_text.clone(),
        sync_group: "analysis".to_string(),
        metadata_json: json!({
            "source": "weighted_quintic_hermite_seed",
            "input_path": input_path,
            "trace_path": trace_path.to_string_lossy().into_owned(),
            "sample_count": samples.len(),
            "knot_profile": "nonuniform_time",
            "frame": "relative_monocular",
            "checksum": spline_checksum,
            "camera_geometry": camera_geometry.as_ref()
        })
        .to_string(),
    })?;

    store.upsert_world_model_spline_control(
        &SplineControlState {
            key: spline_control_key.clone(),
            profile_id: "tensor/trajectory_spline".to_string(),
            coeffs: None,
            artifact_ref: Some(TensorArtifactRef {
                artifact_id: format!(
                    "artifact/session_{}/epoch_{epoch_index}/trajectory_spline",
                    session.id
                ),
                profile_id: "tensor/trajectory_spline".to_string(),
                artifact_format: ArtifactFormat::Json,
                uri: format!("file://{spline_path_text}"),
                checksum: spline_checksum,
            }),
            knot_profile: Some("weighted_quintic_hermite_seed".to_string()),
            confidence: spline_confidence,
            mission_relevance: if trace_has_motion(trace_rows) { 0.95 } else { 0.55 },
            frame: Some("relative_monocular".to_string()),
        },
        &current_hlc_timestamp(),
    )?;

    if let (Some(controller_path), Some(capture_samples)) =
        (controller_capture_path, controller_capture_samples)
    {
        store.upsert_stream(&SessionStreamUpsert {
            session_id: session.id,
            stream_key: "controller_capture".to_string(),
            stream_kind: "controller_capture_csv".to_string(),
            role: "observation".to_string(),
            path: controller_path.to_string_lossy().into_owned(),
            sync_group: "analysis".to_string(),
            metadata_json: json!({
                "source": "controller-capture",
                "input_path": input_path,
                "sample_count": capture_samples.len()
            })
            .to_string(),
        })?;
    }

    let epoch_id = store.upsert_epoch(&AbstractionEpochUpsert {
        session_id: session.id,
        epoch_index,
        layer_name: layer_name.to_string(),
        abstraction_family: abstraction_family.to_string(),
        status: "complete".to_string(),
        created_at: now.clone(),
        completed_at: Some(now),
        summary_json: json!({
            "source": "video-trace",
            "input_path": input_path,
            "trace_path": trace_path.to_string_lossy().into_owned(),
            "optimized_trace_path": optimized_path_text,
            "sample_count": samples.len(),
            "video_frame_geometry": frame_geometry.as_ref().map(|geometry| json!({
                "width_px": geometry.width_px,
                "height_px": geometry.height_px,
                "avg_frame_rate_hz": geometry.avg_frame_rate_hz
            })),
            "camera_geometry": camera_geometry.as_ref(),
            "scene_cloud_path": scene_cloud.map(|(path, _)| path.to_string_lossy().into_owned()),
            "scene_cloud_point_count": scene_cloud.map(|(_, summary)| summary.point_count).unwrap_or(0),
            "trajectory_spline_path": spline_path_text.clone(),
            "trajectory_spline_control_key": spline_control_key.clone(),
            "controller_capture_path": controller_capture_path
                .map(|path| path.to_string_lossy().into_owned()),
            "controller_capture_sample_count": controller_capture_samples.map(<[AbstractStateSample]>::len).unwrap_or(0)
        })
        .to_string(),
    })?;

    let (frame_space_id, space_id, rc_input_space_id) = persist_epoch_samples(
        store,
        epoch_id,
        input_path,
        observation_stream_key,
        samples,
        controller_capture_samples,
        trace_rows,
        camera_geometry.as_ref(),
    )?;

    persist_trace_bootstrap_world(store, session.id, epoch_id, epoch_index, input_path, samples)?;

    Ok(VideoAnalysisReport {
        session_id: session.id,
        input_path: input_path.to_string(),
        trace_path: trace_path.to_string_lossy().into_owned(),
        controller_capture_path: controller_capture_path
            .map(|path| path.to_string_lossy().into_owned()),
        scene_cloud_path: scene_cloud.map(|(path, _)| path.to_string_lossy().into_owned()),
        scene_cloud_point_count: scene_cloud.map(|(_, summary)| summary.point_count).unwrap_or(0),
        trajectory_spline_path: spline_path_text,
        trajectory_spline_control_key: spline_control_key,
        optimized_trace_path: optimized_path_text,
        trace_stream_key: trace_stream_key.to_string(),
        epoch_id,
        frame_space_id,
        space_id,
        rc_input_space_id,
        sample_count: samples.len(),
        rc_input_sample_count: controller_capture_samples
            .map(<[AbstractStateSample]>::len)
            .unwrap_or(0),
        duration_sec,
        analysis_kind,
    })
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Everything `analyze-session` was asked for, resolved from the CLI by the caller.
pub(crate) struct AnalyzeRequest<'a> {
    pub session_id: i64,
    pub input: Option<&'a str>,
    pub trace_csv: Option<&'a str>,
    pub controller_csv: Option<&'a str>,
    pub video_trace_bin: Option<&'a str>,
    pub output: Option<&'a str>,
    pub optimized_output: Option<&'a str>,
    pub max_frames: i64,
    pub frame_stride: i64,
    pub max_features: i64,
    pub min_motion_px: f64,
    pub epoch_index: Option<i64>,
    pub layer_name: &'a str,
    pub abstraction_family: &'a str,
    pub trace_stream_key: &'a str,
}

pub(crate) fn analyze_session(
    store: &SessionStore,
    store_path: &str,
    request: AnalyzeRequest<'_>,
) -> Result<()> {
    let AnalyzeRequest {
        session_id,
        input: input_override,
        trace_csv: trace_csv_override,
        controller_csv: controller_csv_override,
        video_trace_bin: video_trace_bin_override,
        output: output_override,
        optimized_output: optimized_output_override,
        max_frames,
        frame_stride,
        max_features,
        min_motion_px,
        epoch_index: epoch_index_override,
        layer_name,
        abstraction_family,
        trace_stream_key,
    } = request;
    let Some(session) = store.session_by_id(session_id)? else {
        bail!("session {session_id} not found");
    };

    let streams = store.list_streams(session_id)?;
    let observation_stream = match input_override {
        Some(requested) => streams
            .iter()
            .find(|stream| stream.stream_kind == "video" && stream.path == requested)
            .cloned()
            .or_else(|| select_observation_video_stream(&streams)),
        None => select_observation_video_stream(&streams),
    };
    let input_path = input_override
        .map(str::to_owned)
        .or_else(|| observation_stream.as_ref().map(|stream| stream.path.clone()))
        .unwrap_or_else(|| session.path.clone());

    let artifact_dir = default_session_artifact_dir(store_path, session_id);
    std::fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("create artifact directory {}", artifact_dir.display()))?;

    let controller_capture_path = artifact_dir.join("controller_capture.csv");
    let scene_cloud_path = artifact_dir.join("scene_cloud.csv");
    let trajectory_spline_path = artifact_dir.join("trajectory_spline.json");
    let trace_path = output_override
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_dir.join("pose_trace.csv"));
    let optimized_trace_path = optimized_output_override.map(PathBuf::from);

    if let Some(trace_csv) = trace_csv_override {
        if trace_path.as_path() != Path::new(trace_csv) {
            let parent = trace_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| artifact_dir.clone());
            std::fs::create_dir_all(&parent)
                .with_context(|| format!("create trace output directory {}", parent.display()))?;
            std::fs::copy(trace_csv, &trace_path).with_context(|| {
                format!(
                    "copy trace csv from {} to {}",
                    trace_csv,
                    trace_path.display()
                )
            })?;
        }
    } else {
        let helper = crate::util::resolve_video_trace_bin(video_trace_bin_override)?;
        run_video_trace(
            &helper,
            &input_path,
            &trace_path,
            max_frames,
            frame_stride,
            max_features,
            min_motion_px,
        )?;
    }

    let controller_rows = load_or_extract_controller_capture(
        &session,
        &input_path,
        controller_csv_override,
        video_trace_bin_override,
        &controller_capture_path,
        max_frames,
        frame_stride,
    )?;
    let raw_rows = read_pose_trace_csv(&trace_path)?;
    let normalized = normalize_pose_trace_rows(&raw_rows);
    let scene_cloud = maybe_generate_scene_cloud(
        &input_path,
        video_trace_bin_override,
        &trace_path,
        optimized_trace_path.as_deref(),
        &scene_cloud_path,
        &raw_rows,
    )?;
    let controller_samples = controller_rows
        .as_deref()
        .map(normalize_controller_capture_rows);
    let epoch_index = epoch_index_override.unwrap_or_else(|| next_epoch_index(store, session_id));

    let report = persist_pose_trace_analysis(
        store,
        &session,
        &input_path,
        &trace_path,
        controller_rows
            .as_ref()
            .map(|_| controller_capture_path.as_path()),
        optimized_trace_path.as_deref(),
        &raw_rows,
        &normalized,
        scene_cloud.as_ref(),
        &trajectory_spline_path,
        controller_samples.as_deref(),
        observation_stream
            .as_ref()
            .map(|stream| stream.stream_key.as_str()),
        epoch_index,
        layer_name,
        abstraction_family,
        trace_stream_key,
    )?;

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Report printed by `analyze-session`.
#[derive(Debug, serde::Serialize)]
pub(crate) struct VideoAnalysisReport {
    pub session_id: i64,
    pub input_path: String,
    pub trace_path: String,
    pub controller_capture_path: Option<String>,
    pub scene_cloud_path: Option<String>,
    pub scene_cloud_point_count: usize,
    pub trajectory_spline_path: String,
    pub trajectory_spline_control_key: String,
    pub optimized_trace_path: Option<String>,
    pub trace_stream_key: String,
    pub epoch_id: i64,
    pub frame_space_id: i64,
    pub space_id: i64,
    pub rc_input_space_id: Option<i64>,
    pub sample_count: usize,
    pub rc_input_sample_count: usize,
    pub duration_sec: f64,
    pub analysis_kind: String,
}
