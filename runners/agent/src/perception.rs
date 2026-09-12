//! What the camera and VSLAM lanes are publishing, and the frames themselves.
//!
//! Every reading comes from the shared-memory arena's seqlocked snapshots, so
//! the status and the frame an operator looks at are the same bytes the runners
//! wrote — never a re-rendered approximation of them.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use qualia_types::{CAMERA_THUMB_PIXELS, LIDAR_MAX_POINTS};
use serde::Serialize;

use crate::{AppState, CAMERA_STALE_MS, VSLAM_STALE_MS};

/// The camera lane.
#[derive(Debug, Serialize)]
pub struct CameraPerceptionStatus {
    pub available: bool,
    pub fresh: bool,
    pub source: String,
    pub frame_seq: u64,
    pub age_ms: Option<u64>,
    pub source_width: u32,
    pub source_height: u32,
    pub thumbnail_width: u32,
    pub thumbnail_height: u32,
    pub luminance_mean: f32,
    pub luminance_stddev: f32,
    pub quality: &'static str,
    pub retention: &'static str,
}

/// The VSLAM lane.
#[derive(Debug, Serialize)]
pub struct VslamPerceptionStatus {
    pub available: bool,
    pub fresh: bool,
    pub frame_seq: u64,
    pub age_ms: Option<u64>,
    pub tracking: bool,
    pub tracking_confidence: f32,
    pub pose_confidence: f32,
    pub feature_count: u32,
    pub keyframe_count: u32,
    pub loop_closure_count: u32,
    pub pose_writer_active: bool,
}

/// Both perception lanes plus the movement gate they feed.
#[derive(Debug, Serialize)]
pub struct PerceptionStatus {
    pub schema_version: &'static str,
    pub status: &'static str,
    pub camera: CameraPerceptionStatus,
    pub vslam: VslamPerceptionStatus,
    pub movement_input_ready: bool,
    pub movement_blocker: Option<String>,
    pub visual_mapping_ready: bool,
    pub visual_mapping_warning: Option<String>,
}

/// The source the camera lane names: the stream or snapshot URL the operator
/// configured, else the shared-memory camera frame the runners publish.
pub fn camera_source() -> String {
    ["QUALIA_CAMERA_STREAM_URL", "QUALIA_CAMERA_SNAPSHOT_URL"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "qualia-shm:camera_frame".to_string())
}

fn camera_quality(luminance_mean: f32, luminance_stddev: f32) -> &'static str {
    if !luminance_mean.is_finite() || !luminance_stddev.is_finite() {
        return "unavailable";
    }
    if luminance_mean < 12.0 {
        "underexposed"
    } else if luminance_mean > 243.0 {
        "overexposed"
    } else if luminance_stddev < 6.0 {
        "flat"
    } else {
        "usable"
    }
}

fn unavailable_status(reason: &str) -> PerceptionStatus {
    PerceptionStatus {
        schema_version: "qualia.perception-status.v1",
        status: "unavailable",
        camera: CameraPerceptionStatus {
            available: false,
            fresh: false,
            source: camera_source(),
            frame_seq: 0,
            age_ms: None,
            source_width: 0,
            source_height: 0,
            thumbnail_width: 0,
            thumbnail_height: 0,
            luminance_mean: 0.0,
            luminance_stddev: 0.0,
            quality: "unavailable",
            retention: "latest-color-frame-plus-luma-thumbnail",
        },
        vslam: VslamPerceptionStatus {
            available: false,
            fresh: false,
            frame_seq: 0,
            age_ms: None,
            tracking: false,
            tracking_confidence: 0.0,
            pose_confidence: 0.0,
            feature_count: 0,
            keyframe_count: 0,
            loop_closure_count: 0,
            pose_writer_active: false,
        },
        movement_input_ready: false,
        movement_blocker: Some(reason.to_string()),
        visual_mapping_ready: false,
        visual_mapping_warning: Some(reason.to_string()),
    }
}

/// `GET /perception/status`
pub async fn status_get(State(state): State<AppState>) -> (StatusCode, Json<PerceptionStatus>) {
    let Some(region) = state.shm_opt() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(unavailable_status("Qualia shared memory is unavailable")),
        );
    };

    let now = crate::now_ns();
    let camera = region.camera_frame().snapshot(8).unwrap_or_default();
    let camera_available = camera.valid && camera.seq != 0 && camera.timestamp_ns != 0;
    let camera_age_ms = now.checked_sub(camera.timestamp_ns).map(|age| age / 1_000_000);
    let camera_fresh = camera_available
        && camera_age_ms.is_some_and(|age| age <= CAMERA_STALE_MS);
    let quality = camera_quality(camera.luminance_mean, camera.luminance_stddev);

    let vslam = region.vslam_frontend();
    let vslam_seq = vslam.seq.load(std::sync::atomic::Ordering::Acquire);
    let vslam_available = vslam_seq != 0 && vslam.timestamp_ns != 0;
    let vslam_age_ms = now.checked_sub(vslam.timestamp_ns).map(|age| age / 1_000_000);
    let vslam_fresh = vslam_available && vslam_age_ms.is_some_and(|age| age <= VSLAM_STALE_MS);
    let tracking = vslam.tracking_ok.load(std::sync::atomic::Ordering::Acquire);

    let world = region.world_model();
    let pose_fresh = world.robot_pose.timestamp_ns != 0
        && now
            .checked_sub(world.robot_pose.timestamp_ns)
            .is_some_and(|age| age <= crate::POSE_STALE_NS);
    let lidar_fresh = region
        .lidar_scan()
        .snapshot(4)
        .map(|scan| {
            scan.scan_end_ns != 0
                && now
                    .checked_sub(scan.scan_end_ns)
                    .is_some_and(|age| age <= crate::LIDAR_STALE_NS)
        })
        .unwrap_or(false);

    let mut visual_warnings = Vec::new();
    if !camera_fresh {
        visual_warnings.push("camera frames are not fresh".to_string());
    } else if quality != "usable" {
        visual_warnings.push(format!("camera image is {quality}"));
    }
    if !vslam_fresh {
        visual_warnings.push("VSLAM has not processed a fresh camera frame".to_string());
    } else if !tracking {
        visual_warnings.push("VSLAM tracking confidence is low".to_string());
    }
    let mut blockers = Vec::new();
    if !pose_fresh {
        blockers.push("pose is not fresh".to_string());
    }
    if !lidar_fresh {
        blockers.push("lidar is not fresh".to_string());
    }
    let movement_input_ready = blockers.is_empty();
    let visual_mapping_ready = visual_warnings.is_empty();

    let status = PerceptionStatus {
        schema_version: "qualia.perception-status.v1",
        status: if movement_input_ready && visual_mapping_ready {
            "ready"
        } else {
            "degraded"
        },
        camera: CameraPerceptionStatus {
            available: camera_available,
            fresh: camera_fresh,
            source: camera_source(),
            frame_seq: camera.seq,
            age_ms: camera_age_ms,
            source_width: camera.source_width,
            source_height: camera.source_height,
            thumbnail_width: camera.thumb_width,
            thumbnail_height: camera.thumb_height,
            luminance_mean: camera.luminance_mean,
            luminance_stddev: camera.luminance_stddev,
            quality,
            retention: "latest-color-frame-plus-luma-thumbnail",
        },
        vslam: VslamPerceptionStatus {
            available: vslam_available,
            fresh: vslam_fresh,
            frame_seq: vslam.frame_seq,
            age_ms: vslam_age_ms,
            tracking,
            tracking_confidence: vslam.tracking_confidence,
            pose_confidence: vslam.pose_confidence,
            feature_count: vslam.feature_count,
            keyframe_count: vslam.keyframe_count,
            loop_closure_count: vslam.loop_closure_count,
            pose_writer_active: vslam.pose_writer_active != 0,
        },
        movement_input_ready,
        movement_blocker: (!blockers.is_empty()).then(|| blockers.join("; ")),
        visual_mapping_ready,
        visual_mapping_warning: (!visual_warnings.is_empty()).then(|| visual_warnings.join("; ")),
    };
    (StatusCode::OK, Json(status))
}

/// `GET /perception/frame.pgm` — the luma thumbnail as a portable graymap.
pub async fn frame_pgm_get(State(state): State<AppState>) -> Response {
    let Some(region) = state.shm_opt() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(camera) = region.camera_frame().snapshot(8) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !camera.valid || camera.seq == 0 || camera.thumb_width == 0 || camera.thumb_height == 0 {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let pixel_count = (camera.thumb_width as usize)
        .saturating_mul(camera.thumb_height as usize)
        .min(CAMERA_THUMB_PIXELS);
    let mut body = format!("P5\n{} {}\n255\n", camera.thumb_width, camera.thumb_height).into_bytes();
    body.extend_from_slice(&camera.thumbnail_luma[..pixel_count]);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/x-portable-graymap"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Bytes::from(body),
    )
        .into_response()
}

/// `GET /perception/frame` — the encoded preview the camera runner left behind.
pub async fn encoded_frame_get(State(state): State<AppState>) -> Response {
    let Some(region) = state.shm_opt() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // The slot's own reader takes the sequence, copies the payload, fences and
    // re-reads the sequence. A hand-rolled pair here used to close its window
    // before the copy, which is the torn read the fence prevents.
    let Ok(preview) = region.camera_preview().snapshot(8) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if preview.seq == 0 || preview.bytes.is_empty() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let content_type = match preview.format {
        1 => "image/jpeg",
        2 => "image/png",
        _ => return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response(),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Bytes::from(preview.bytes),
    )
        .into_response()
}

/// `GET /perception/lidar.pgm` — the polar scan drawn from above.
pub async fn lidar_frame_get(State(state): State<AppState>) -> Response {
    const SIDE: usize = 256;
    const METERS_PER_PIXEL: f32 = 0.05;
    let Some(region) = state.shm_opt() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(scan) = region.lidar_scan().snapshot(8) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let point_count = (scan.point_count as usize).min(LIDAR_MAX_POINTS);
    if scan.seq == 0 || point_count == 0 {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut pixels = vec![0u8; SIDE * SIDE];
    let center = (SIDE / 2) as isize;
    for dy in -2..=2 {
        for dx in -2..=2 {
            pixels[(center + dy) as usize * SIDE + (center + dx) as usize] = 160;
        }
    }
    for point in scan.points.iter().take(point_count) {
        if point.intensity == 0 || !point.distance_m.is_finite() || point.distance_m <= 0.0 {
            continue;
        }
        let x = center + (point.angle_rad.sin() * point.distance_m / METERS_PER_PIXEL) as isize;
        let y = center - (point.angle_rad.cos() * point.distance_m / METERS_PER_PIXEL) as isize;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (px, py) = (x + dx, y + dy);
                if (0..SIDE as isize).contains(&px) && (0..SIDE as isize).contains(&py) {
                    pixels[py as usize * SIDE + px as usize] = 255;
                }
            }
        }
    }
    let mut body = format!("P5\n{SIDE} {SIDE}\n255\n").into_bytes();
    body.extend_from_slice(&pixels);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/x-portable-graymap"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Bytes::from(body),
    )
        .into_response()
}
