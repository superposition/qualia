//! Operator actions and the small media surface: pose and goal writes into the
//! arena, posted snapshots, entity profiles, the explore process and the TLS
//! certificate the listener presents.

use std::net::IpAddr;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use qualia_types::{NavGoal, NavPose};
use rcgen::{CertificateParams, KeyPair, SanType};
use serde::{Deserialize, Serialize};

use crate::{AppState, ServiceStatusMeta, NAV_GRID_RESOLUTION_M, SNAPSHOT_MAX_AGE_SECS};

/// The rooms the grid spans, matching the shared memory layout.
const ROOM_HALF_W_M: f32 = qualia_types::VOXEL_W as f32 * NAV_GRID_RESOLUTION_M * 0.5;
const ROOM_HALF_D_M: f32 = qualia_types::VOXEL_D as f32 * NAV_GRID_RESOLUTION_M * 0.5;

/// Where the explore runner writes its log.
pub fn explore_log_path() -> String {
    "/tmp/qualia-explore.log".to_string()
}

/// A goal in both grid and world coordinates.
#[derive(Debug, Deserialize)]
pub struct NavGoalRequest {
    cell_x: i32,
    cell_z: i32,
    x_m: f32,
    z_m: f32,
    #[serde(default)]
    y_m: f32,
    #[serde(default)]
    yaw_rad: f32,
}

/// A pose from a producer that names itself.
#[derive(Debug, Deserialize)]
pub struct NavPoseRequest {
    x_m: f32,
    y_m: f32,
    z_m: f32,
    yaw_rad: f32,
    #[serde(default = "default_confidence")]
    confidence: f32,
    #[serde(default)]
    source: Option<String>,
}

fn default_confidence() -> f32 {
    1.0
}

/// Which producer last wrote the pose, and how authoritative it is.
#[derive(Debug, Clone)]
pub struct PoseAuthority {
    pub source: String,
    pub priority: u8,
    pub updated_at_ns: u64,
}

impl Default for PoseAuthority {
    fn default() -> Self {
        Self {
            source: "none".to_string(),
            priority: 0,
            updated_at_ns: 0,
        }
    }
}

/// The status envelope of a supervised helper process.
#[derive(Debug, Serialize)]
pub struct ExploreStatus {
    #[serde(flatten)]
    pub meta: ServiceStatusMeta,
    pub available: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub log_path: String,
}

/// `GET /entities`
pub async fn entities_get(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "schema_version": "qualia.entities.v1",
        "entities": (*state.entity_profiles).clone(),
    }))
}

/// `GET /thought-theater/config`
pub async fn thought_theater_get(State(state): State<AppState>) -> Json<crate::ThoughtTheaterConfig> {
    Json((*state.thought_theater).clone())
}

/// `POST /snapshot` — the board posts one JPEG, the operator page reads it back.
pub async fn snapshot_post(body: Bytes) -> StatusCode {
    if body.is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    match std::fs::write(crate::SNAPSHOT_PATH, &body) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `GET /orin-snap` — the freshest posted capture, or nothing.
pub async fn orin_snapshot_get() -> Response {
    for path in [crate::ORIN_SNAPSHOT_PATH, crate::SNAPSHOT_PATH] {
        if is_fresh_snapshot(path, SNAPSHOT_MAX_AGE_SECS) {
            if let Ok(bytes) = std::fs::read(path) {
                return (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "image/jpeg")],
                    bytes,
                )
                    .into_response();
            }
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn is_fresh_snapshot(path: &str, max_age_secs: u64) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(age) = modified.elapsed() else {
        return false;
    };
    age <= Duration::from_secs(max_age_secs)
}

/// `POST /nav/goal` — write the goal the drive runner follows.
pub async fn nav_goal_post(
    State(state): State<AppState>,
    Json(request): Json<NavGoalRequest>,
) -> StatusCode {
    let region = match state.shm() {
        Ok(region) => region,
        Err(response) => return response.status(),
    };
    let goal = match canonical_nav_goal(
        request.cell_x,
        request.cell_z,
        request.x_m,
        request.y_m,
        request.z_m,
        request.yaw_rad,
        crate::now_ns(),
    ) {
        Ok(goal) => goal,
        Err(()) => return StatusCode::BAD_REQUEST,
    };
    region.set_nav_goal(goal);
    StatusCode::NO_CONTENT
}

/// `POST /nav/cancel` — clear the goal and stop the drive lane.
pub async fn nav_cancel_post(State(state): State<AppState>) -> StatusCode {
    let region = match state.shm() {
        Ok(region) => region,
        Err(response) => return response.status(),
    };
    region.set_nav_goal(NavGoal {
        active: 0,
        _pad0: [0; 3],
        cell_x: 0,
        cell_z: 0,
        x_m: 0.0,
        y_m: 0.0,
        z_m: 0.0,
        yaw_rad: 0.0,
        timestamp_ns: crate::now_ns(),
    });
    StatusCode::NO_CONTENT
}

/// `POST /nav/pose` — a producer publishes the robot's pose.
///
/// A fresh authority outranks a lower-priority one, so a VSLAM pose cannot be
/// displaced by an odometry pose that arrives a moment later.
pub async fn nav_pose_post(
    State(state): State<AppState>,
    Json(request): Json<NavPoseRequest>,
) -> StatusCode {
    let region = match state.shm() {
        Ok(region) => region,
        Err(response) => return response.status(),
    };
    let now_ns = crate::now_ns();
    let source = request
        .source
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let priority = nav_pose_priority(&source);
    {
        let mut authority = state.pose_authority.lock().expect("pose authority lock");
        let authority_is_fresh = now_ns.saturating_sub(authority.updated_at_ns) <= 2_000_000_000;
        if authority_is_fresh && priority < authority.priority {
            return StatusCode::ACCEPTED;
        }
        authority.source = source;
        authority.priority = priority;
        authority.updated_at_ns = now_ns;
    }
    region.set_robot_pose(NavPose {
        x_m: request.x_m,
        y_m: request.y_m,
        z_m: request.z_m,
        yaw_rad: request.yaw_rad,
        pitch_rad: 0.0,
        roll_rad: 0.0,
        confidence: request.confidence,
        _pad0: 0.0,
        timestamp_ns: now_ns,
    });
    StatusCode::NO_CONTENT
}

/// Poses from a visual estimator outrank a transform, which outranks odometry.
fn nav_pose_priority(source: &str) -> u8 {
    let source = source.to_ascii_lowercase();
    let visual = [
        "vslam",
        "visual_slam",
        "visual-odometry",
        "visual_odometry",
        "orbslam",
        "rtabmap",
        "zed",
        "zed2",
        "stereo",
        "vio",
    ];
    if visual.iter().any(|marker| source.contains(marker)) {
        3
    } else if source.contains("tf") {
        2
    } else if source.contains("odom") {
        1
    } else {
        0
    }
}

/// Resolve a goal to both coordinate frames, rejecting a cell off the grid.
///
/// World coordinates win when the caller supplied them, exactly as the planner
/// resolves a target; otherwise the cell centre is the goal.
fn canonical_nav_goal(
    cell_x: i32,
    cell_z: i32,
    x_m: f32,
    y_m: f32,
    z_m: f32,
    yaw_rad: f32,
    timestamp_ns: u64,
) -> Result<NavGoal, ()> {
    let (cell_x, cell_z, x_m, z_m) = if x_m.is_finite() && z_m.is_finite() && (x_m != 0.0 || z_m != 0.0)
    {
        let (cell_x, cell_z) = world_to_cell(x_m, z_m)?;
        (cell_x, cell_z, x_m, z_m)
    } else {
        let (x_m, z_m) = cell_to_world(cell_x, cell_z);
        (cell_x, cell_z, x_m, z_m)
    };
    validate_cell(cell_x, cell_z)?;
    Ok(NavGoal {
        active: 1,
        _pad0: [0; 3],
        cell_x,
        cell_z,
        x_m,
        y_m,
        z_m,
        yaw_rad,
        timestamp_ns,
    })
}

fn world_to_cell(x_m: f32, z_m: f32) -> Result<(i32, i32), ()> {
    let cell_x = ((x_m + ROOM_HALF_W_M) / NAV_GRID_RESOLUTION_M).floor() as i32;
    let cell_z = ((z_m + ROOM_HALF_D_M) / NAV_GRID_RESOLUTION_M).floor() as i32;
    validate_cell(cell_x, cell_z)?;
    Ok((cell_x, cell_z))
}

fn cell_to_world(cell_x: i32, cell_z: i32) -> (f32, f32) {
    (
        -ROOM_HALF_W_M + (cell_x as f32 + 0.5) * NAV_GRID_RESOLUTION_M,
        -ROOM_HALF_D_M + (cell_z as f32 + 0.5) * NAV_GRID_RESOLUTION_M,
    )
}

fn validate_cell(cell_x: i32, cell_z: i32) -> Result<(), ()> {
    if (0..qualia_types::VOXEL_W as i32).contains(&cell_x)
        && (0..qualia_types::VOXEL_D as i32).contains(&cell_z)
    {
        Ok(())
    } else {
        Err(())
    }
}

/// `GET /explore/status`
pub async fn explore_status_get(State(state): State<AppState>) -> Json<ExploreStatus> {
    Json(explore_status(&state))
}

/// `POST /explore/start`
pub async fn explore_start_post(State(state): State<AppState>) -> Response {
    match start_explore_process() {
        Ok(_) => (StatusCode::OK, Json(explore_status(&state))).into_response(),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ExploreStatus {
                meta: ServiceStatusMeta {
                    status: "error".to_string(),
                    ..crate::status_meta(&state.config.compute.service_instance, "explore_status")
                },
                available: false,
                running: false,
                pid: None,
                log_path: format!("{} ({message})", explore_log_path()),
            }),
        )
            .into_response(),
    }
}

/// `POST /explore/stop`
pub async fn explore_stop_post(State(state): State<AppState>) -> Response {
    match stop_explore_process() {
        Ok(_) => (StatusCode::OK, Json(explore_status(&state))).into_response(),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ExploreStatus {
                meta: ServiceStatusMeta {
                    status: "error".to_string(),
                    ..crate::status_meta(&state.config.compute.service_instance, "explore_status")
                },
                available: false,
                running: false,
                pid: None,
                log_path: format!("{} ({message})", explore_log_path()),
            }),
        )
            .into_response(),
    }
}

fn explore_status(state: &AppState) -> ExploreStatus {
    let (available, running, pid) = explore_process_state();
    ExploreStatus {
        meta: crate::status_meta(&state.config.compute.service_instance, "explore_status"),
        available,
        running,
        pid,
        log_path: explore_log_path(),
    }
}

#[cfg(target_os = "linux")]
fn explore_process_state() -> (bool, bool, Option<u32>) {
    const BINARY: &str = "./target/release/qualia-explore";
    let available = std::path::Path::new(BINARY).exists();
    let pid = std::fs::read_dir("/proc")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .find(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|name| name.trim() == "qualia-explore")
                .unwrap_or(false)
        });
    (available, pid.is_some(), pid)
}

#[cfg(not(target_os = "linux"))]
fn explore_process_state() -> (bool, bool, Option<u32>) {
    (false, false, None)
}

#[cfg(target_os = "linux")]
fn start_explore_process() -> Result<(), String> {
    use std::process::{Command, Stdio};
    if explore_process_state().1 {
        return Ok(());
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(explore_log_path())
        .map_err(|error| format!("open explore log: {error}"))?;
    let stdout = log.try_clone().map_err(|error| format!("clone log fd: {error}"))?;
    Command::new("./target/release/qualia-explore")
        .current_dir(".")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("spawn qualia-explore: {error}"))?;
    std::thread::sleep(Duration::from_millis(250));
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn start_explore_process() -> Result<(), String> {
    Err("explore control unsupported on this platform".to_string())
}

#[cfg(target_os = "linux")]
fn stop_explore_process() -> Result<(), String> {
    use std::process::Command;
    Command::new("pkill")
        .args(["-x", "qualia-explore"])
        .status()
        .map_err(|error| format!("pkill qualia-explore: {error}"))?;
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn stop_explore_process() -> Result<(), String> {
    Err("explore control unsupported on this platform".to_string())
}

/// Ensure a self-signed certificate exists at `cert_path`, generating one with
/// localhost, loopback and the board's address as SANs when it does not.
///
/// Reusing the file keeps the pinned fingerprint an operator recorded stable
/// across restarts.
pub fn ensure_tls_cert(tls_dir: &str, cert_path: &str, key_path: &str, jetson_ip: &str) {
    if std::path::Path::new(cert_path).exists() && std::path::Path::new(key_path).exists() {
        eprintln!("qualia-agent: reusing TLS cert at {cert_path}");
        return;
    }
    std::fs::create_dir_all(tls_dir).expect("cannot create TLS dir");

    let board_ip: IpAddr = jetson_ip.parse().unwrap_or(IpAddr::V4([192, 168, 0, 221].into()));
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).expect("cert params");
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::V4([127, 0, 0, 1].into())));
    params.subject_alt_names.push(SanType::IpAddress(board_ip));

    let key_pair = KeyPair::generate().expect("keygen");
    let certificate = params.self_signed(&key_pair).expect("self-sign");
    std::fs::write(cert_path, certificate.pem()).expect("write cert");
    std::fs::write(key_path, key_pair.serialize_pem()).expect("write key");
    eprintln!("qualia-agent: generated new TLS cert at {cert_path}");
}
