//! `qualia-cuda-service`: the `compute.v1` request worker.
//!
//! The worker listens on one endpoint — a TCP loopback address on Windows, a
//! Unix socket elsewhere — and answers one newline-delimited JSON request per
//! connection. It owns the resident CUDA contexts (`SmokeContext` for
//! `cuda_smoke`, `CostmapStatsContext` for `costmap_stats`), falls back to the
//! host-side reduction when no device can be opened, and runs the grid planner
//! for `plan_path`.
//!
//! Environment:
//!
//! - `QUALIA_COMPUTE_SOCKET` — endpoint to bind (default above).
//! - `QUALIA_SERVICE_INSTANCE` — identity echoed on every response; when unset
//!   the host name is used.
//! - `QUALIA_CUDA_DEVICE_NAME` / `QUALIA_CUDA_SM` — override what is reported
//!   for the device when `nvidia-smi` is not the right oracle.

mod planner;

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[cfg(windows)]
use tokio::net::{TcpListener as Listener, TcpStream as Wire};
#[cfg(not(windows))]
use tokio::net::{UnixListener as Listener, UnixStream as Wire};

#[cfg(not(windows))]
use std::path::Path;

#[cfg(feature = "cuda")]
use qualia_cuda::{CostmapStatsContext, SmokeContext};

/// The request/response dialect this worker speaks.
pub(crate) const SCHEMA_VERSION: &str = "compute.v1";

/// The name this process calls itself when the host has no better one.
const SERVICE_NAME: &str = "qualia-cuda-service";

#[cfg(windows)]
const DEFAULT_ENDPOINT: &str = "127.0.0.1:46321";
#[cfg(not(windows))]
const DEFAULT_ENDPOINT: &str = "/tmp/qualia-compute.sock";

const PLANNER_MAX_GRID_WIDTH: u32 = 32;
const PLANNER_MAX_GRID_DEPTH: u32 = 32;
pub(crate) const PLANNER_MAX_PATH_POINTS: usize = 256;

const DEFAULT_MAX_ITERATIONS: u32 = 50_000;

#[tokio::main]
async fn main() {
    let endpoint =
        std::env::var("QUALIA_COMPUTE_SOCKET").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    // A Unix socket file outlives the process that made it; a fresh bind needs
    // the stale inode gone first.
    #[cfg(not(windows))]
    if Path::new(&endpoint).exists() {
        let _ = std::fs::remove_file(&endpoint);
    }

    // `TcpListener::bind` is async, the Unix one is not, so the bind differs by
    // platform while the failure line does not.
    #[cfg(windows)]
    let listener = Listener::bind(&endpoint)
        .await
        .unwrap_or_else(|_| panic!("bind compute socket: {endpoint}"));
    #[cfg(not(windows))]
    let listener =
        Listener::bind(&endpoint).unwrap_or_else(|_| panic!("bind compute socket: {endpoint}"));
    eprintln!("qualia-cuda-service: listening on {endpoint}");

    let worker = Worker::from_env();

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("qualia-cuda-service: accept error: {error}");
                continue;
            }
        };

        let worker = worker.clone();
        tokio::spawn(async move {
            if let Err(error) = serve(stream, worker).await {
                eprintln!("qualia-cuda-service: client error: {error}");
            }
        });
    }
}

/// Reads one request line, writes one response line. A caller that hangs up
/// before sending anything is not an error.
async fn serve(stream: Wire, worker: Worker) -> Result<(), String> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    let read = reader
        .read_line(&mut line)
        .await
        .map_err(|error| format!("read_line failed: {error}"))?;
    if read == 0 {
        return Ok(());
    }

    let request: Request = serde_json::from_str(line.trim())
        .map_err(|error| format!("bad request json: {error}"))?;
    let body = serde_json::to_vec(&worker.dispatch(request))
        .map_err(|error| format!("serialize response failed: {error}"))?;

    write_half
        .write_all(&body)
        .await
        .map_err(|error| format!("write failed: {error}"))?;
    write_half
        .write_all(b"\n")
        .await
        .map_err(|error| format!("newline write failed: {error}"))?;
    write_half
        .flush()
        .await
        .map_err(|error| format!("flush failed: {error}"))?;
    Ok(())
}

#[derive(Clone)]
struct Worker {
    instance: String,
    gpu: Arc<GpuRuntime>,
}

impl Worker {
    fn from_env() -> Self {
        let instance = non_empty(std::env::var("QUALIA_SERVICE_INSTANCE").ok())
            .unwrap_or_else(host_instance);
        let gpu = Arc::new(GpuRuntime::open());
        let status = gpu.snapshot();
        eprintln!(
            "qualia-cuda-service: cuda runtime status={} device='{}' sm_{}",
            status.status, status.device_name, status.sm
        );
        Self { instance, gpu }
    }

    fn dispatch(&self, request: Request) -> Response {
        match request.meta.request_type.as_str() {
            "capabilities" => Response::Capabilities(self.capabilities(&request)),
            "cuda_smoke" => Response::CudaSmoke(self.cuda_smoke(&request)),
            "costmap_stats" => {
                let request_id = request.meta.request_id.clone();
                match request.into_costmap_stats() {
                    Ok(spec) => match self.costmap_stats(&spec) {
                        Ok(body) => Response::CostmapStats(body),
                        Err(error) => Response::Error(self.failure(spec.request_id, error)),
                    },
                    Err(error) => Response::Error(self.failure(request_id, error)),
                }
            }
            "plan_path" => {
                let request_id = request.meta.request_id.clone();
                match request.into_plan_path() {
                    Ok(spec) => match planner::plan_path(&spec, &self.instance) {
                        Ok(body) => Response::Path(body),
                        Err(error) => Response::Error(self.failure(spec.request_id, error)),
                    },
                    Err(error) => Response::Error(self.failure(request_id, error)),
                }
            }
            other => Response::Error(self.failure(
                request.meta.request_id,
                ComputeError::plain(
                    "unsupported_version",
                    format!("unsupported request_type: {other}"),
                ),
            )),
        }
    }

    fn response_meta(&self, request: &Request, result_type: &str, status: &str) -> ResponseMeta {
        ResponseMeta {
            schema_version: request.meta.schema_version.clone(),
            service_instance: self.instance.clone(),
            request_id: request.meta.request_id.clone(),
            result_type: result_type.to_string(),
            status: status.to_string(),
        }
    }

    fn failure(&self, request_id: String, error: ComputeError) -> ErrorResponse {
        ErrorResponse {
            meta: ResponseMeta {
                schema_version: SCHEMA_VERSION.to_string(),
                service_instance: self.instance.clone(),
                request_id,
                result_type: "error".to_string(),
                status: "error".to_string(),
            },
            error,
        }
    }

    fn capabilities(&self, request: &Request) -> CapabilityResponse {
        let status = self.gpu.snapshot();
        CapabilityResponse {
            meta: self.response_meta(request, "capabilities", "ok"),
            healthy: status.healthy,
            planner_algorithms: planner::supported_algorithms(),
            max_grid_width: PLANNER_MAX_GRID_WIDTH,
            max_grid_depth: PLANNER_MAX_GRID_DEPTH,
            max_path_points: PLANNER_MAX_PATH_POINTS as u32,
            supports_object_footprints: true,
            supports_costmap_debug: true,
            cuda: CudaInfo {
                device_name: status.device_name,
                sm: status.sm,
                status: status.status,
                reason: status.reason,
            },
        }
    }

    fn cuda_smoke(&self, request: &Request) -> CudaSmokeResponse {
        let outcome = self.gpu.smoke();
        CudaSmokeResponse {
            meta: self.response_meta(request, "cuda_smoke", &outcome.status),
            reason: outcome.reason,
            compiler: outcome.compiler,
            binary_path: outcome.binary_path,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
        }
    }

    fn costmap_stats(&self, spec: &CostmapStatsRequest) -> Result<CostmapStatsResponse, ComputeError> {
        planner::check_grid(&spec.grid)?;
        let summary = self.gpu.reduce(&spec.grid.occupied, &spec.grid.cost)?;
        let total_cells = spec.grid.width * spec.grid.depth;

        Ok(CostmapStatsResponse {
            meta: ResponseMeta {
                schema_version: spec.schema_version.clone(),
                service_instance: self.instance.clone(),
                request_id: spec.request_id.clone(),
                result_type: "costmap_stats".to_string(),
                status: "ok".to_string(),
            },
            total_cells,
            occupied_count: summary.occupied_count,
            high_cost_count: summary.high_cost_count,
            blocked_count: summary.blocked_count,
            cost_sum: summary.cost_sum,
            mean_cost: if total_cells > 0 {
                summary.cost_sum as f32 / total_cells as f32
            } else {
                0.0
            },
        })
    }
}

#[cfg(windows)]
fn host_instance() -> String {
    non_empty(std::env::var("COMPUTERNAME").ok()).unwrap_or_else(|| SERVICE_NAME.to_string())
}

#[cfg(not(windows))]
fn host_instance() -> String {
    non_empty(
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|name| name.trim().to_string()),
    )
    .unwrap_or_else(|| SERVICE_NAME.to_string())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// What the CUDA runtime ended up being, as reported on `capabilities` and in
/// the operator log.
struct GpuSnapshot {
    healthy: bool,
    status: String,
    reason: Option<String>,
    device_name: String,
    sm: u32,
}

/// The result of one `cuda_smoke` request.
struct SmokeOutcome {
    status: String,
    reason: String,
    compiler: Option<String>,
    binary_path: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
}

impl SmokeOutcome {
    fn refused(status: &str, reason: &str) -> Self {
        Self {
            status: status.to_string(),
            reason: reason.to_string(),
            compiler: None,
            binary_path: None,
            stdout: None,
            stderr: None,
        }
    }
}

#[derive(Default)]
struct CostmapSummary {
    occupied_count: u32,
    high_cost_count: u32,
    blocked_count: u32,
    cost_sum: u32,
}

/// The resident device contexts, or the reason there are none.
struct GpuRuntime {
    device_name: String,
    sm: u32,
    backend: Backend,
}

enum Backend {
    #[cfg(feature = "cuda")]
    Resident {
        smoke: Arc<SmokeContext>,
        costmap: Arc<CostmapStatsContext>,
    },
    /// The `cuda` feature was compiled out.
    #[cfg(not(feature = "cuda"))]
    Absent { reason: String },
    /// The `cuda` feature is on but the driver or NVRTC would not initialize.
    #[cfg(feature = "cuda")]
    Failed { reason: String },
}

impl GpuRuntime {
    fn open() -> Self {
        let device_name = detect_device_name();
        let sm = detect_sm();

        #[cfg(feature = "cuda")]
        {
            match resident_contexts() {
                Ok((smoke, costmap, name)) => {
                    return Self {
                        device_name: name,
                        sm,
                        backend: Backend::Resident {
                            smoke: Arc::new(smoke),
                            costmap: Arc::new(costmap),
                        },
                    }
                }
                Err(reason) => {
                    return Self {
                        device_name,
                        sm,
                        backend: Backend::Failed { reason },
                    }
                }
            }
        }

        #[cfg(not(feature = "cuda"))]
        Self {
            device_name,
            sm,
            backend: Backend::Absent {
                reason: "cuda feature not enabled in qualia-cuda-service".to_string(),
            },
        }
    }

    fn snapshot(&self) -> GpuSnapshot {
        let reason = match &self.backend {
            #[cfg(feature = "cuda")]
            Backend::Resident { .. } => None,
            #[cfg(feature = "cuda")]
            Backend::Failed { reason } => Some(reason.clone()),
            #[cfg(not(feature = "cuda"))]
            Backend::Absent { reason } => Some(reason.clone()),
        };
        let healthy = reason.is_none();
        let status = match &self.backend {
            #[cfg(feature = "cuda")]
            Backend::Resident { .. } => "ready",
            #[cfg(feature = "cuda")]
            Backend::Failed { .. } => "init_failed",
            #[cfg(not(feature = "cuda"))]
            Backend::Absent { .. } => "disabled",
        };

        GpuSnapshot {
            healthy,
            status: status.to_string(),
            reason,
            device_name: self.device_name.clone(),
            sm: self.sm,
        }
    }

    fn smoke(&self) -> SmokeOutcome {
        match &self.backend {
            #[cfg(feature = "cuda")]
            Backend::Resident { smoke, .. } => match smoke.run_add_one([1.0, 2.0, 3.0, 4.0]) {
                Ok(out) => SmokeOutcome {
                    status: "ok".to_string(),
                    reason: "resident CUDA smoke kernel executed successfully".to_string(),
                    compiler: Some("qualia-cuda::SmokeContext".to_string()),
                    binary_path: None,
                    stdout: Some(format!(
                        "[{:.1},{:.1},{:.1},{:.1}]",
                        out[0], out[1], out[2], out[3]
                    )),
                    stderr: Some(format!("device={}", smoke.device_name())),
                },
                Err(error) => SmokeOutcome {
                    status: "run_failed".to_string(),
                    reason: format!("resident CUDA smoke kernel failed: {error}"),
                    compiler: Some("qualia-cuda::SmokeContext".to_string()),
                    binary_path: None,
                    stdout: None,
                    stderr: None,
                },
            },
            #[cfg(feature = "cuda")]
            Backend::Failed { reason } => SmokeOutcome::refused("error", reason),
            #[cfg(not(feature = "cuda"))]
            Backend::Absent { reason } => SmokeOutcome::refused("blocked", reason),
        }
    }

    fn reduce(&self, occupied: &[u8], cost: &[u8]) -> Result<CostmapSummary, ComputeError> {
        match &self.backend {
            #[cfg(feature = "cuda")]
            Backend::Resident { costmap, .. } => costmap
                .run(occupied, cost)
                .map(|stats| CostmapSummary {
                    occupied_count: stats.occupied_count,
                    high_cost_count: stats.high_cost_count,
                    blocked_count: stats.blocked_count,
                    cost_sum: stats.cost_sum,
                })
                .map_err(|error| {
                    ComputeError::retryable(
                        "cuda_failed",
                        format!("resident CUDA costmap_stats failed: {error}"),
                    )
                }),
            #[cfg(feature = "cuda")]
            Backend::Failed { .. } => Ok(host_costmap_summary(occupied, cost)),
            #[cfg(not(feature = "cuda"))]
            Backend::Absent { .. } => Ok(host_costmap_summary(occupied, cost)),
        }
    }
}

/// Host arithmetic with the same contract as the device reduction: a non-zero
/// occupancy flag blocks its cell, a cost of 200 or more is "high".
fn host_costmap_summary(occupied: &[u8], cost: &[u8]) -> CostmapSummary {
    let mut summary = CostmapSummary::default();
    for (flag, weight) in occupied.iter().zip(cost) {
        if *flag != 0 {
            summary.occupied_count += 1;
            summary.blocked_count += 1;
        }
        if *weight >= 200 {
            summary.high_cost_count += 1;
        }
        summary.cost_sum += u32::from(*weight);
    }
    summary
}

#[cfg(feature = "cuda")]
fn resident_contexts() -> Result<(SmokeContext, CostmapStatsContext, String), String> {
    let loaded = std::panic::catch_unwind(|| (SmokeContext::new(), CostmapStatsContext::new()))
        .map_err(|payload| format!("failed to load CUDA runtime: {}", panic_text(payload)))?;
    let (smoke, costmap) = loaded;

    let smoke = smoke.map_err(|error| {
        format!("failed to initialize resident CUDA smoke runtime: {error}")
    })?;
    let costmap = costmap.map_err(|error| {
        format!("failed to initialize resident CUDA costmap runtime: {error}")
    })?;
    let device_name = smoke.device_name();
    Ok((smoke, costmap, device_name))
}

#[cfg(feature = "cuda")]
fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "unknown panic".to_string()
}

fn detect_device_name() -> String {
    if let Ok(name) = std::env::var("QUALIA_CUDA_DEVICE_NAME") {
        return name;
    }
    if let Some((name, _)) = query_nvidia_smi() {
        return name;
    }
    #[cfg(not(windows))]
    if Path::new("/sys/devices/platform/gpu.0/load").exists() {
        return "Jetson Orin Nano GPU".to_string();
    }
    "generic-planner".to_string()
}

fn detect_sm() -> u32 {
    let from_env = std::env::var("QUALIA_CUDA_SM")
        .ok()
        .and_then(|value| value.parse().ok());
    let from_nvidia = query_nvidia_smi().and_then(|(_, sm)| sm);

    #[cfg(not(windows))]
    let from_board = Path::new("/sys/devices/platform/gpu.0/load")
        .exists()
        .then_some(87);

    #[cfg(windows)]
    let from_board = None;

    from_env.or(from_nvidia).or(from_board).unwrap_or(0)
}

fn query_nvidia_smi() -> Option<(String, Option<u32>)> {
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=name,compute_cap", "--format=csv,noheader"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .to_string();
    let mut parts = line.split(',').map(|part| part.trim());
    let name = parts.next()?.to_string();
    let sm = parts.next().and_then(parse_compute_capability);
    Some((name, sm))
}

fn parse_compute_capability(value: &str) -> Option<u32> {
    let digits = value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(flatten)]
    meta: RequestMeta,
    algorithm: Option<String>,
    goal: Option<PlannerPose>,
    start: Option<PlannerPose>,
    grid: Option<PlannerGrid>,
    constraints: Option<PlannerConstraints>,
    world_context: Option<WorldContext>,
    replan: Option<ReplanContext>,
    belief_risk: Option<BeliefRisk>,
}

impl Request {
    fn into_costmap_stats(self) -> Result<CostmapStatsRequest, ComputeError> {
        Ok(CostmapStatsRequest {
            schema_version: self.meta.schema_version,
            request_id: self.meta.request_id,
            grid: self
                .grid
                .ok_or_else(|| ComputeError::plain("invalid_request", "missing grid"))?,
            _timestamp_ns: self.meta.timestamp_ns,
        })
    }

    fn into_plan_path(self) -> Result<PathPlanRequest, ComputeError> {
        Ok(PathPlanRequest {
            schema_version: self.meta.schema_version,
            request_id: self.meta.request_id,
            algorithm: self.algorithm.unwrap_or_else(|| "grid_astar".to_string()),
            goal: self
                .goal
                .ok_or_else(|| ComputeError::plain("invalid_request", "missing goal"))?,
            start: self
                .start
                .ok_or_else(|| ComputeError::plain("invalid_request", "missing start"))?,
            grid: self
                .grid
                .ok_or_else(|| ComputeError::plain("invalid_request", "missing grid"))?,
            constraints: self.constraints.unwrap_or_default(),
            world_context: self.world_context.unwrap_or_default(),
            replan: self.replan,
            belief_risk: self.belief_risk,
            _timestamp_ns: self.meta.timestamp_ns,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RequestMeta {
    schema_version: String,
    request_id: String,
    request_type: String,
    #[serde(default)]
    timestamp_ns: u64,
}

#[derive(Debug, Deserialize)]
struct CostmapStatsRequest {
    schema_version: String,
    request_id: String,
    grid: PlannerGrid,
    #[serde(default, rename = "timestamp_ns")]
    _timestamp_ns: u64,
}

#[derive(Debug, Deserialize, Clone)]
struct PathPlanRequest {
    schema_version: String,
    request_id: String,
    algorithm: String,
    goal: PlannerPose,
    start: PlannerPose,
    grid: PlannerGrid,
    constraints: PlannerConstraints,
    world_context: WorldContext,
    replan: Option<ReplanContext>,
    belief_risk: Option<BeliefRisk>,
    #[serde(default, rename = "timestamp_ns")]
    _timestamp_ns: u64,
}

/// A pose on the grid and in the world frame.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
struct PlannerPose {
    cell_x: i32,
    cell_z: i32,
    x_m: f32,
    y_m: f32,
    z_m: f32,
    yaw_rad: f32,
}

#[derive(Debug, Deserialize, Clone)]
struct PlannerGrid {
    width: u32,
    depth: u32,
    resolution_m: f32,
    occupied: Vec<u8>,
    cost: Vec<u8>,
}

impl PlannerGrid {
    /// Row-major index of a cell known to be in bounds.
    fn index(&self, cell: crate::planner::Cell) -> usize {
        cell.z as usize * self.width as usize + cell.x as usize
    }

    /// True when the whole footprint centred on `cell` is in bounds and free.
    fn is_clear(&self, cell: crate::planner::Cell, radius: u32) -> bool {
        let span = radius as i32;
        for dz in -span..=span {
            for dx in -span..=span {
                let x = cell.x + dx;
                let z = cell.z + dz;
                if x < 0 || z < 0 || x >= self.width as i32 || z >= self.depth as i32 {
                    return false;
                }
                if self.occupied[z as usize * self.width as usize + x as usize] != 0 {
                    return false;
                }
            }
        }
        true
    }

    /// Centre of a cell in the world frame the grid is centred on.
    fn world_x(&self, cell_x: i32) -> f32 {
        let half = self.width as f32 * self.resolution_m * 0.5;
        -half + (cell_x as f32 + 0.5) * self.resolution_m
    }

    fn world_z(&self, cell_z: i32) -> f32 {
        let half = self.depth as f32 * self.resolution_m * 0.5;
        -half + (cell_z as f32 + 0.5) * self.resolution_m
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
struct PlannerConstraints {
    #[serde(default)]
    robot_radius_cells: u32,
    #[serde(default)]
    allow_unknown: bool,
    #[serde(default = "default_max_iterations")]
    max_iterations: u32,
}

impl Default for PlannerConstraints {
    fn default() -> Self {
        Self {
            robot_radius_cells: 0,
            allow_unknown: false,
            max_iterations: default_max_iterations(),
        }
    }
}

fn default_max_iterations() -> u32 {
    DEFAULT_MAX_ITERATIONS
}

/// The sequences the map, voxel and footprint layers had reached when the
/// request was made; `voxel_seq` is echoed as the planner's costmap sequence.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Default, Clone)]
struct WorldContext {
    #[serde(default)]
    nav_seq: u64,
    #[serde(default)]
    voxel_seq: u64,
    #[serde(default)]
    footprint_seq: u64,
}

#[derive(Debug, Deserialize, Clone)]
struct ReplanContext {
    previous_request_id: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct BeliefRisk {
    uncertainty_weight: f32,
    semantic_novelty: f32,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Response {
    Capabilities(CapabilityResponse),
    CudaSmoke(CudaSmokeResponse),
    CostmapStats(CostmapStatsResponse),
    Path(PathPlanResponse),
    Error(ErrorResponse),
}

#[derive(Debug, Serialize)]
struct CapabilityResponse {
    #[serde(flatten)]
    meta: ResponseMeta,
    healthy: bool,
    planner_algorithms: Vec<String>,
    max_grid_width: u32,
    max_grid_depth: u32,
    max_path_points: u32,
    supports_object_footprints: bool,
    supports_costmap_debug: bool,
    cuda: CudaInfo,
}

#[derive(Debug, Serialize)]
struct CudaInfo {
    device_name: String,
    sm: u32,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct CudaSmokeResponse {
    #[serde(flatten)]
    meta: ResponseMeta,
    reason: String,
    compiler: Option<String>,
    binary_path: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
}

#[derive(Debug, Serialize)]
struct CostmapStatsResponse {
    #[serde(flatten)]
    meta: ResponseMeta,
    total_cells: u32,
    occupied_count: u32,
    high_cost_count: u32,
    blocked_count: u32,
    cost_sum: u32,
    mean_cost: f32,
}

#[derive(Debug, Serialize)]
struct PathPlanResponse {
    #[serde(flatten)]
    meta: ResponseMeta,
    planning_ms: Option<f32>,
    path: Vec<PathPoint>,
    summary: Option<PathSummary>,
    debug: Option<PathDebug>,
    error: Option<ComputeError>,
}

#[derive(Debug, Serialize)]
struct PathPoint {
    cell_x: i32,
    cell_z: i32,
    x_m: f32,
    z_m: f32,
}

#[derive(Debug, Serialize)]
struct PathSummary {
    path_cost: f32,
    expanded_nodes: u32,
    reachable: bool,
}

#[derive(Debug, Serialize)]
struct PathDebug {
    costmap_seq: u64,
    algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    replan_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replan_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uncertainty_weight: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_novelty: Option<f32>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    #[serde(flatten)]
    meta: ResponseMeta,
    error: ComputeError,
}

#[derive(Debug, Serialize)]
struct ResponseMeta {
    schema_version: String,
    service_instance: String,
    request_id: String,
    result_type: String,
    status: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ComputeError {
    code: String,
    message: String,
    retryable: bool,
}

impl ComputeError {
    pub(crate) fn plain(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub(crate) fn retryable(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: true,
        }
    }
}
