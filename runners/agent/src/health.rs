//! Stack health: what is fresh, what is ready, and what is missing.
//!
//! Everything here is read from the shared-memory arena and the compute
//! configuration at request time. Nothing is cached: a stale reading must show
//! up as stale on the very next poll, or an operator cannot trust the page.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::compute::{unavailable_capabilities, ComputeCapabilities};
use crate::{status_meta, AppState, ServiceStatusMeta, LIDAR_STALE_NS, POSE_STALE_NS};

/// One planner input, with its age.
#[derive(Debug, Serialize)]
pub struct IntegrationInputStatus {
    pub source: String,
    pub fresh: bool,
    pub age_ms: Option<u64>,
}

/// What the motion authority has confirmed lately.
#[derive(Debug, Serialize)]
pub struct IntegrationActionStatus {
    pub authority: &'static str,
    pub synced: bool,
    pub fresh: bool,
    pub producer_epoch: u64,
    pub source_sequence: u64,
    pub latest_sequence: u64,
    pub oldest_sequence: u64,
    pub qualia_history_sequence: u64,
    pub age_ms: Option<u64>,
    pub consecutive_failures: u64,
    pub last_error: Option<String>,
}

/// The stack's input readiness.
#[derive(Debug, Serialize)]
pub struct IntegrationStatus {
    #[serde(flatten)]
    pub meta: ServiceStatusMeta,
    pub ros_connected: bool,
    pub pose: IntegrationInputStatus,
    pub lidar: IntegrationInputStatus,
    pub action: IntegrationActionStatus,
    pub planner_ready: bool,
}

/// Where the planner's accelerator stands.
#[derive(Debug, Serialize)]
pub struct RobotAcceleratorView {
    pub source: &'static str,
    pub fresh: bool,
    pub age_ms: Option<u64>,
    pub requested: String,
    pub active: String,
    pub available: bool,
    pub required: bool,
    pub message: String,
}

/// The planner's readiness and the inputs it would plan from.
#[derive(Debug, Serialize)]
pub struct PlannerStatus {
    #[serde(flatten)]
    pub meta: ServiceStatusMeta,
    pub planner_ready: bool,
    pub planner_algorithms: Vec<String>,
    pub compute: ComputeCapabilities,
    pub robot_accelerator: RobotAcceleratorView,
    pub belief_features: Option<serde_json::Value>,
    pub last_result: Option<serde_json::Value>,
}

/// The whole stack's readiness.
#[derive(Debug, Serialize)]
pub struct StackHealth {
    #[serde(flatten)]
    pub meta: ServiceStatusMeta,
    pub healthy: bool,
    pub ready: bool,
    pub integration: IntegrationStatus,
    pub planner: PlannerStatus,
    pub compute: StackComputeStatus,
}

/// The compute slice of the stack health.
#[derive(Debug, Serialize)]
pub struct StackComputeStatus {
    pub healthy: bool,
    pub planner_algorithms: Vec<String>,
    pub cuda: crate::compute::CudaInfo,
}

/// Pose and lidar freshness, read from the arena the runners publish into.
struct InputReadings {
    pose: IntegrationInputStatus,
    lidar: IntegrationInputStatus,
}

fn input_readings(state: &AppState) -> InputReadings {
    let now = crate::now_ns();
    let (pose_ns, lidar_ns) = match state.shm_opt() {
        Some(region) => {
            let world = region.world_model();
            let pose_ns = world.robot_pose.timestamp_ns;
            let lidar_ns = region
                .lidar_scan()
                .snapshot(4)
                .map(|scan| scan.scan_end_ns)
                .unwrap_or(0);
            (pose_ns, lidar_ns)
        }
        None => (0, 0),
    };
    let reading = |timestamp_ns: u64, stale_ns: u64| IntegrationInputStatus {
        source: "shm".to_string(),
        fresh: timestamp_ns != 0
            && now.checked_sub(timestamp_ns).is_some_and(|age| age <= stale_ns),
        age_ms: (timestamp_ns != 0).then(|| now.saturating_sub(timestamp_ns) / 1_000_000),
    };
    InputReadings {
        pose: reading(pose_ns, POSE_STALE_NS),
        lidar: reading(lidar_ns, LIDAR_STALE_NS),
    }
}

/// The action channel is Leash's; until its evidence consumer lands nothing
/// has been confirmed, which is what these zeros say.
fn action_status() -> IntegrationActionStatus {
    IntegrationActionStatus {
        authority: "leash",
        synced: false,
        fresh: false,
        producer_epoch: 0,
        source_sequence: 0,
        latest_sequence: 0,
        oldest_sequence: 0,
        qualia_history_sequence: 0,
        age_ms: None,
        consecutive_failures: 0,
        last_error: None,
    }
}

/// The accelerator Leash reports on its own health route; not yet consumed.
fn accelerator_view() -> RobotAcceleratorView {
    RobotAcceleratorView {
        source: "leash:/health",
        fresh: false,
        age_ms: None,
        requested: String::new(),
        active: String::new(),
        available: false,
        required: false,
        message: String::new(),
    }
}

fn integration_of(state: &AppState) -> IntegrationStatus {
    let inputs = input_readings(state);
    let planner_ready = inputs.pose.fresh && inputs.lidar.fresh;
    IntegrationStatus {
        meta: status_meta(&state.config.compute.service_instance, "integration_status"),
        ros_connected: false,
        pose: inputs.pose,
        lidar: inputs.lidar,
        action: action_status(),
        planner_ready,
    }
}

fn planner_of(state: &AppState) -> PlannerStatus {
    let inputs = input_readings(state);
    let capabilities = unavailable_capabilities(&state.config.compute);
    let planner_ready = inputs.pose.fresh && inputs.lidar.fresh && !capabilities.planner_algorithms.is_empty();
    PlannerStatus {
        meta: status_meta(&state.config.compute.service_instance, "planner_status"),
        planner_ready,
        planner_algorithms: capabilities.planner_algorithms.clone(),
        compute: capabilities,
        robot_accelerator: accelerator_view(),
        belief_features: None,
        last_result: None,
    }
}

/// `GET /integration/status`
pub async fn integration_get(State(state): State<AppState>) -> (StatusCode, Json<IntegrationStatus>) {
    (StatusCode::OK, Json(integration_of(&state)))
}

/// `GET /planner/status`
pub async fn planner_status_get(State(state): State<AppState>) -> (StatusCode, Json<PlannerStatus>) {
    (StatusCode::OK, Json(planner_of(&state)))
}

/// `GET /health/ready`
pub async fn ready_get(State(state): State<AppState>) -> (StatusCode, Json<StackHealth>) {
    let integration = integration_of(&state);
    let planner = planner_of(&state);
    let compute = StackComputeStatus {
        healthy: planner.compute.healthy,
        planner_algorithms: planner.compute.planner_algorithms.clone(),
        cuda: planner.compute.cuda.clone(),
    };
    let healthy = compute.healthy;
    let ready = planner.planner_ready && compute.healthy;
    (
        StatusCode::OK,
        Json(StackHealth {
            meta: status_meta(&state.config.compute.service_instance, "stack_health"),
            healthy,
            ready,
            integration,
            planner,
            compute,
        }),
    )
}
