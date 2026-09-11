//! What the planner and the accelerator lanes can do.
//!
//! The heavy planning lives behind a compute service on a socket. When that
//! service has not answered, the process reports the documented *unavailable*
//! capability block rather than a healthy one, so an operator can tell the
//! difference between "fast" and "silent".

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use qualia_types::{VOXEL_D, VOXEL_W};
use serde::Serialize;

/// `QUALIA_COMPUTE_SOCKET`, default `/tmp/qualia-compute.sock`.
#[cfg(not(windows))]
pub const DEFAULT_COMPUTE_SOCKET: &str = "/tmp/qualia-compute.sock";
/// `QUALIA_COMPUTE_SOCKET` has no default on Windows: there is no socket.
#[cfg(windows)]
pub const DEFAULT_COMPUTE_SOCKET: &str = "";
/// `QUALIA_COMPUTE_TIMEOUT_MS`, default `250`.
pub const DEFAULT_TIMEOUT_MS: u64 = 250;
/// The service instance every compute response names.
pub const COMPUTE_SCHEMA_VERSION: &str = "compute.v1";

/// Where the compute service listens, and how long a request may take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeConfig {
    pub socket_path: String,
    pub timeout_ms: u64,
    pub service_instance: String,
}

impl Default for ComputeConfig {
    fn default() -> Self {
        Self {
            socket_path: DEFAULT_COMPUTE_SOCKET.to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            service_instance: default_service_instance(),
        }
    }
}

impl ComputeConfig {
    pub fn from_env() -> Self {
        Self {
            socket_path: std::env::var("QUALIA_COMPUTE_SOCKET")
                .unwrap_or_else(|_| DEFAULT_COMPUTE_SOCKET.to_string()),
            timeout_ms: std::env::var("QUALIA_COMPUTE_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_TIMEOUT_MS),
            service_instance: std::env::var("QUALIA_SERVICE_INSTANCE")
                .unwrap_or_else(|_| default_service_instance()),
        }
    }
}

/// The host name this process reports as its instance.
pub fn default_service_instance() -> String {
    #[cfg(windows)]
    let host = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let host = std::fs::read_to_string("/etc/hostname").ok();
    host.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "qualia-agent".to_string())
}

/// Which accelerator the compute service reported.
#[derive(Debug, Clone, Serialize)]
pub struct CudaInfo {
    pub device_name: String,
    pub sm: u32,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The planner's whole capability block.
#[derive(Debug, Clone, Serialize)]
pub struct ComputeCapabilities {
    pub schema_version: String,
    pub service_instance: String,
    pub healthy: bool,
    pub planner_algorithms: Vec<String>,
    pub max_grid_width: u32,
    pub max_grid_depth: u32,
    pub max_path_points: u32,
    pub supports_object_footprints: bool,
    pub supports_costmap_debug: bool,
    pub cuda: CudaInfo,
}

/// A compute request that could not be served, in the service's own words.
#[derive(Debug, Clone, Serialize)]
pub struct ComputeErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// The error envelope a compute route returns.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub schema_version: String,
    pub request_id: Option<String>,
    pub result_type: String,
    pub status: String,
    pub error: ComputeErrorBody,
}

/// The capability block for a service that has not answered.
pub fn unavailable_capabilities(config: &ComputeConfig) -> ComputeCapabilities {
    ComputeCapabilities {
        schema_version: COMPUTE_SCHEMA_VERSION.to_string(),
        service_instance: config.service_instance.clone(),
        healthy: false,
        planner_algorithms: Vec::new(),
        max_grid_width: VOXEL_W as u32,
        max_grid_depth: VOXEL_D as u32,
        max_path_points: 0,
        supports_object_footprints: false,
        supports_costmap_debug: false,
        cuda: CudaInfo {
            device_name: "unavailable".to_string(),
            sm: 0,
            status: Some("unavailable".to_string()),
            reason: Some("compute service unavailable".to_string()),
        },
    }
}

/// Build the error envelope for `code`.
pub fn error_response(status: StatusCode, code: &str, message: String) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            schema_version: COMPUTE_SCHEMA_VERSION.to_string(),
            request_id: None,
            result_type: "error".to_string(),
            status: "error".to_string(),
            error: ComputeErrorBody {
                code: code.to_string(),
                message,
                retryable: false,
            },
        }),
    )
        .into_response()
}

/// The compute service is not part of this build: nothing on the socket ever
/// answers, so every consumer gets the documented unavailable block.
fn compute_service_error() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "the compute service is not available in this build".to_string(),
    )
}

pub async fn capabilities_get(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
) -> Json<ComputeCapabilities> {
    Json(unavailable_capabilities(&state.config.compute))
}

pub async fn cuda_smoke_get() -> Response {
    compute_service_error()
}

pub async fn costmap_stats_get() -> Response {
    compute_service_error()
}

pub async fn path_post() -> Response {
    compute_service_error()
}
