//! `qualia-agent` — the stack's mission broker and HTTP surface.
//!
//! The process has one job: be the place every strand can look at and report
//! through. It serves the operator page, publishes the braid view the console
//! and the browser poll, brokers bounded missions whose envelopes arrive from
//! the mission broker, and streams the shared-memory view to the studio over a
//! websocket.
//!
//! What it does *not* do is own any of the records it shows. The braid folds
//! events and holds no storage; missions are journalled so a restart cannot
//! lose a decision; evidence and generations stay in the crates that own them.
//!
//! Route paths, JSON field names, environment keys, shared-memory offsets and
//! emitted log lines are the interface this crate keeps, so the console, the
//! web page and the deployment manifests keep working across the rewrite.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use qualia_shm::ShmRegion;
use serde::Serialize;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

pub mod auth;
pub mod braid;
pub mod compute;
pub mod config;
pub mod health;
pub mod mcp;
pub mod media;
pub mod mission_control;
pub mod pending;
pub mod perception;
pub mod realtime;
pub mod shm;

pub use config::AgentConfig;

/// Status envelope schema, shared by every `*_status` body.
pub const SERVICE_STATUS_SCHEMA_VERSION: &str = "qualia.service-status.v1";
/// A pose older than this is not a planner input.
pub const POSE_STALE_NS: u64 = 1_500_000_000;
/// A lidar scan older than this is not a planner input.
pub const LIDAR_STALE_NS: u64 = 1_500_000_000;
/// A camera frame older than this is shown as stale.
pub const CAMERA_STALE_MS: u64 = 4_000;
/// A VSLAM update older than this is shown as stale.
pub const VSLAM_STALE_MS: u64 = 4_000;
/// The websocket stream cadence.
pub const SHM_POLL_INTERVAL_MS: u64 = 100;
/// A snapshot posted to `/snapshot` is only served while it is this fresh.
pub const SNAPSHOT_MAX_AGE_SECS: u64 = 10;
/// The planner grid pitch, in metres; shared with the costmap view.
pub const NAV_GRID_RESOLUTION_M: f32 = 0.4;
/// Where a posted camera snapshot lands on the host.
pub const SNAPSHOT_PATH: &str = "/tmp/qualia_snapshot.jpg";
/// Orin capture fallback the snapshot route serves when the camera one is stale.
pub const ORIN_SNAPSHOT_PATH: &str = "/tmp/qualia_orin_snap.jpg";

/// Signalling peers, keyed by the id they register with.
pub type PeerMap = Arc<Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<String>>>>;

/// Nanoseconds since the Unix epoch — the unit every clock in the stack uses.
pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

/// Milliseconds since the Unix epoch, for the mission wire types.
pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// The `meta` block every service-status body starts with.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatusMeta {
    pub schema_version: String,
    pub service_instance: String,
    pub status_type: String,
    pub status: String,
}

/// Build the status envelope for `status_type` with the configured instance.
pub fn status_meta(service_instance: &str, status_type: &str) -> ServiceStatusMeta {
    ServiceStatusMeta {
        schema_version: SERVICE_STATUS_SCHEMA_VERSION.to_string(),
        service_instance: service_instance.to_string(),
        status_type: status_type.to_string(),
        status: "ok".to_string(),
    }
}

/// The thinking-theater configuration the studio reads before connecting.
#[derive(Debug, Clone, Serialize)]
pub struct ThoughtTheaterConfig {
    pub enabled: bool,
    pub viewer_url: String,
    pub source_url: String,
    pub blueprint_name: String,
}

/// Everything a handler needs. Cheap to clone: every field is behind an `Arc`
/// or is itself a handle.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AgentConfig>,
    pub auth: auth::AuthConfig,
    pub braid: braid::BraidRuntime,
    pub mission_control: mission_control::MissionControlRuntime,
    pub thought_theater: Arc<ThoughtTheaterConfig>,
    pub entity_profiles: Arc<Vec<qualia_types::EntityProfile>>,
    pub certificate_sha256: Arc<String>,
    /// Which producer last published the robot pose.
    pub pose_authority: Arc<Mutex<media::PoseAuthority>>,
    pub peers: PeerMap,
}

impl AppState {
    /// The shared-memory region, or the response an operator should see when it
    /// is not mapped.
    pub fn shm(&self) -> Result<ShmRegion, Response> {
        shm::open_region(&self.config.shm_name, self.config.shm_autocreate).map_err(|error| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": format!("shared memory unavailable: {error}") })),
            )
                .into_response()
        })
    }

    /// Read the shared-memory region if it is mapped, for handlers that stay
    /// available when it is not.
    pub fn shm_opt(&self) -> Option<ShmRegion> {
        shm::open_region(&self.config.shm_name, self.config.shm_autocreate).ok()
    }
}

/// Assemble the process state: load entity profiles, open or seed the mission
/// journal, and bind the braid to the run.
pub fn build_state(config: AgentConfig) -> Result<AppState, String> {
    let entity_profiles = config::load_entity_profiles(&config)?;
    let braid = braid::BraidRuntime::new();
    let mission_control = mission_control::MissionControlRuntime::from_config(&config, braid.clone());
    Ok(AppState {
        thought_theater: Arc::new(config.thought_theater.clone()),
        certificate_sha256: Arc::new(String::new()),
        auth: config.auth.clone(),
        braid,
        mission_control,
        entity_profiles: Arc::new(entity_profiles),
        pose_authority: Arc::new(Mutex::new(media::PoseAuthority::default())),
        peers: Arc::new(Mutex::new(HashMap::new())),
        config: Arc::new(config),
    })
}

/// Start the background loops the process needs: the mission supervisor and the
/// mission broker adapter when a broker is configured.
pub fn start_background(state: &AppState) {
    mission_control::start(state.clone());
}

/// The whole HTTP surface, in one place.
///
/// Every path the reference process answered is registered here, in the same
/// method. Routes whose backing subsystem has not landed yet answer
/// [`pending::unavailable`] naming that subsystem instead of inventing a body.
pub fn app(state: AppState) -> Router {
    let index_file = format!("{}/index.html", state.config.web_dir);
    let serve_dir = ServeDir::new(state.config.web_dir.clone());

    Router::new()
        .route("/braid", get(braid::view_get))
        .route("/mcp", post(mcp::protocol_post))
        .route("/mcp/status", get(mcp::status_get))
        .route("/mcp/tools", get(mcp::tools_get))
        .route("/mcp/call", post(mcp::call_post))
        .route("/arena/status", get(pending::arena_status_get))
        .route("/arena/session/start", post(pending::arena_start_post))
        .route("/arena/session/stop", post(pending::arena_stop_post))
        .route("/arena/guard/stop", post(pending::guard_stop_post))
        .route("/studio/control/ack", post(pending::studio_control_ack_post))
        .route("/belief/status", get(pending::belief_status_get))
        .route("/jepa/status", get(mcp::jepa_status_get))
        .route("/belief/sketch", get(pending::belief_sketch_get))
        .route("/belief/checkpoint", post(pending::belief_checkpoint_post))
        .route("/belief/shadow/start", post(pending::belief_shadow_start_post))
        .route("/belief/shadow/stop", post(pending::belief_shadow_stop_post))
        .route("/belief/shadow/patch", post(pending::belief_shadow_patch_post))
        .route(
            "/belief/shadow/publish",
            post(pending::belief_shadow_publish_post),
        )
        .route("/beliefs/converged", get(pending::converged_beliefs_get))
        .route("/fleet/status", get(pending::fleet_status_get))
        .route("/learning/candidates", get(pending::learning_candidates_get))
        .route(
            "/learning/evaluations",
            get(pending::learning_evaluations_get),
        )
        .route("/learning/promotions", get(pending::learning_promotions_get))
        .route("/entities", get(media::entities_get))
        .route("/ws", get(realtime::ws_handler))
        .route("/signal", get(realtime::signal_handler))
        .route("/snapshot", post(media::snapshot_post))
        .route("/orin-snap", get(media::orin_snapshot_get))
        .route("/nav/goal", post(media::nav_goal_post))
        .route("/nav/cancel", post(media::nav_cancel_post))
        .route("/nav/pose", post(media::nav_pose_post))
        .route("/evidence/action/applied", post(pending::action_applied_post))
        .route("/integration/status", get(health::integration_get))
        .route("/perception/status", get(perception::status_get))
        .route("/perception/frame.pgm", get(perception::frame_pgm_get))
        .route("/perception/frame", get(perception::encoded_frame_get))
        .route("/perception/lidar.pgm", get(perception::lidar_frame_get))
        .route("/perception/landmarks", post(pending::landmarks_post))
        .route("/perception/arena-tags", get(pending::arena_tags_get))
        .route(
            "/perception/multiview/landmarks",
            post(pending::multiview_landmarks_post),
        )
        .route("/planner/status", get(health::planner_status_get))
        .route("/planner/costmap", get(pending::costmap_get))
        .route("/health/ready", get(health::ready_get))
        .route("/compute/capabilities", get(compute::capabilities_get))
        .route("/compute/cuda-smoke", get(compute::cuda_smoke_get))
        .route("/compute/costmap-stats", get(compute::costmap_stats_get))
        .route("/compute/path", post(compute::path_post))
        .route(
            "/compute/jobs",
            get(pending::compute_jobs_get).post(pending::compute_jobs_post),
        )
        .route("/compute/jobs/{job_id}", get(pending::compute_job_get))
        .route(
            "/compute/jobs/{job_id}/cancel",
            post(pending::compute_job_cancel_post),
        )
        .route(
            "/compute/v2/jobs",
            get(pending::targeted_jobs_get).post(pending::targeted_jobs_post),
        )
        .route(
            "/compute/v2/jobs/{job_id}",
            get(pending::targeted_job_get),
        )
        .route(
            "/compute/v2/jobs/{job_id}/cancel",
            post(pending::targeted_job_cancel_post),
        )
        .route("/thought-theater/config", get(media::thought_theater_get))
        .route("/world/snapshot", get(pending::world_snapshot_get))
        .route("/world/export", get(pending::world_export_get))
        .route(
            "/world-model/proposals",
            get(pending::proposals_get).post(pending::proposals_post),
        )
        .route(
            "/world-model/decisions",
            get(pending::decisions_get).post(pending::decisions_post),
        )
        .route("/world-model/canonical", get(pending::canonical_get))
        .route("/world-model/operational", get(pending::operational_get))
        .route("/world-model/spatial", get(pending::spatial_get))
        .route(
            "/mission-control/envelopes",
            post(mission_control::envelope_post),
        )
        .route(
            "/mission-control/missions",
            get(mission_control::missions_get),
        )
        .route("/mission-control/events", get(mission_control::events_get))
        .route(
            "/mission-control/missions/{mission_id}/authorize",
            post(mission_control::authorize_post),
        )
        .route(
            "/sync/replicas",
            get(pending::replicas_get).post(pending::replicas_post),
        )
        .route("/sync/peers", get(pending::peers_get))
        .route("/sync/watermarks", get(pending::watermarks_get))
        .route("/sync/ops", post(pending::ops_post))
        .route("/sync/audit", get(pending::audit_get))
        .route("/sync/snapshot", get(pending::sync_snapshot_get))
        .route(
            "/sync/state/{namespace}",
            get(pending::sync_state_get),
        )
        .route("/sync/stream", get(pending::sync_stream_get))
        .route("/sessions", get(pending::sessions_get))
        .route("/sessions/{session_id}", get(pending::session_detail_get))
        .route(
            "/sessions/{session_id}/epochs",
            get(pending::session_epochs_get),
        )
        .route(
            "/sessions/{session_id}/pose-stream",
            get(pending::session_pose_stream_get),
        )
        .route(
            "/sessions/{session_id}/replay-slice",
            get(pending::session_replay_slice_get),
        )
        .route(
            "/sessions/{session_id}/mcap-window",
            get(pending::session_mcap_window_get),
        )
        .route(
            "/sessions/{session_id}/environment",
            get(pending::session_environment_get),
        )
        .route(
            "/sessions/{session_id}/graph-fragments",
            get(pending::session_graph_fragments_get),
        )
        .route(
            "/sessions/{session_id}/gate-reviews",
            get(pending::gate_reviews_get).post(pending::gate_reviews_post),
        )
        .route("/epochs/{epoch_id}/spaces", get(pending::epoch_spaces_get))
        .route("/spaces/{space_id}/samples", get(pending::space_samples_get))
        .route(
            "/graph-fragments/{fragment_id}/beliefs",
            get(pending::fragment_beliefs_get),
        )
        .route(
            "/session-merges/{left_session_id}/{right_session_id}/candidates",
            get(pending::session_merge_candidates_get),
        )
        .route(
            "/sessions/{session_id}/merge-candidates",
            get(pending::merge_candidates_for_session_get),
        )
        .route(
            "/environments/{environment_id}/plans",
            get(pending::environment_plans_get),
        )
        .route(
            "/plan-runs/{plan_run_id}/regions",
            get(pending::plan_run_regions_get),
        )
        .route(
            "/environments/{environment_id}/coach-reviews",
            get(pending::coach_reviews_get),
        )
        .route("/explore/status", get(media::explore_status_get))
        .route("/explore/start", post(media::explore_start_post))
        .route("/explore/stop", post(media::explore_stop_post))
        .route_service("/", ServeFile::new(index_file.clone()))
        .route_service("/index.html", ServeFile::new(index_file))
        .fallback_service(serve_dir)
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Serve the surface over TLS until the process is asked to stop.
///
/// The certificate is generated on first start and reused afterwards, so a
/// restart does not invalidate the pinned fingerprint an operator has.
pub async fn run(config: AgentConfig) -> Result<(), String> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| "install the rustls crypto provider".to_string())?;

    let cert_path = config::cert_path(&config.tls_dir);
    let key_path = config::key_path(&config.tls_dir);
    media::ensure_tls_cert(&config.tls_dir, &cert_path, &key_path, &config.jetson_ip);

    let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
        .await
        .expect("Failed to load TLS cert");

    let state = build_state(config)?;
    start_background(&state);

    let app = app(state.clone());
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], state.config.web_port));
    eprintln!(
        "qualia-agent: IGL dashboard at https://0.0.0.0:{}  (web dir: {})",
        state.config.web_port, state.config.web_dir
    );
    eprintln!("qualia-agent: SHM stream at  wss://0.0.0.0:{}/ws", state.config.web_port);
    eprintln!("qualia-agent: WebRTC signal  wss://0.0.0.0:{}/signal", state.config.web_port);
    eprintln!("qualia-agent: TLS cert at    {cert_path}");

    let server_handle = axum_server::Handle::new();
    let shutdown_handle = server_handle.clone();
    let shutdown_state = state.clone();
    tokio::spawn(async move {
        match shutdown_signal().await {
            Ok(()) => {
                if let Err(error) = mission_control::shutdown(&shutdown_state).await {
                    eprintln!("qualia-agent: mission shutdown failure: {error}");
                }
                shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(3)));
            }
            Err(error) => eprintln!("qualia-agent: unable to install shutdown signal: {error}"),
        }
    });

    axum_server::bind_rustls(addr, tls_config)
        .handle(server_handle)
        .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await
        .map_err(|error| format!("serve the HTTP surface: {error}"))
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<(), String> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| format!("install SIGTERM handler: {error}"))?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.map_err(|error| format!("install Ctrl-C handler: {error}")),
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<(), String> {
    tokio::signal::ctrl_c()
        .await
        .map_err(|error| format!("install Ctrl-C handler: {error}"))
}
