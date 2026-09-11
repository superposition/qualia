//! Routes whose backing subsystem has not landed yet.
//!
//! The reference process defines these paths, and consumers call them, so they
//! stay registered in the router. Rather than answer 200 with a body this build
//! cannot substantiate, each one names the missing subsystem and returns
//! `503 Service Unavailable`. Nothing here is a placeholder to be filled in
//! later with the same shape: the response is the truth about this build.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// The response schema for a route whose subsystem is missing.
pub const UNAVAILABLE_SCHEMA_VERSION: &str = "qualia.unavailable.v1";

/// `503` naming `subsystem` as the thing this build does not carry.
pub fn unavailable(subsystem: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "schema_version": UNAVAILABLE_SCHEMA_VERSION,
            "error": format!("{subsystem} is not available in this build"),
            "subsystem": subsystem,
        })),
    )
        .into_response()
}

macro_rules! unlanded {
    ($($name:ident => $subsystem:literal),+ $(,)?) => {
        $(
            pub async fn $name() -> Response {
                unavailable($subsystem)
            }
        )+
    };
}

unlanded! {
    // Leash owns action authority; its evidence consumer is not part of this build.
    action_applied_post => "the Leash action-evidence consumer",
    // Lane fusion needs the visual-fusion and arena-tag runtimes.
    landmarks_post => "the visual landmark fusion runtime",
    arena_tags_get => "the arena tag perception runtime",
    multiview_landmarks_post => "the cross-view triangulation runtime",
    // The planner's costmap view is built by the compute service.
    costmap_get => "the compute service",
    compute_jobs_get => "the compute service",
    compute_jobs_post => "the compute service",
    compute_job_get => "the compute service",
    compute_job_cancel_post => "the compute service",
    targeted_jobs_get => "the targeted compute service",
    targeted_jobs_post => "the targeted compute service",
    targeted_job_get => "the targeted compute service",
    targeted_job_cancel_post => "the targeted compute service",
    // The spatial world model owns these projections.
    world_snapshot_get => "the spatial world model",
    world_export_get => "the spatial world model",
    proposals_get => "the spatial world model",
    proposals_post => "the spatial world model",
    decisions_get => "the spatial world model",
    decisions_post => "the spatial world model",
    canonical_get => "the spatial world model",
    operational_get => "the spatial world model",
    spatial_get => "the spatial world model",
    // Replica exchange needs the sync arena runtime.
    replicas_get => "the sync arena runtime",
    replicas_post => "the sync arena runtime",
    peers_get => "the sync arena runtime",
    watermarks_get => "the sync arena runtime",
    ops_post => "the sync arena runtime",
    audit_get => "the sync arena runtime",
    sync_snapshot_get => "the sync arena runtime",
    sync_state_get => "the sync arena runtime",
    sync_stream_get => "the sync arena runtime",
    // Sessions, epochs and missions live in the session store.
    sessions_get => "qualia-session-store",
    session_detail_get => "qualia-session-store",
    session_epochs_get => "qualia-session-store",
    session_pose_stream_get => "qualia-session-store",
    session_replay_slice_get => "qualia-session-store",
    session_mcap_window_get => "qualia-session-store",
    session_environment_get => "qualia-session-store",
    session_graph_fragments_get => "qualia-session-store",
    gate_reviews_get => "qualia-session-store",
    gate_reviews_post => "qualia-session-store",
    epoch_spaces_get => "qualia-session-store",
    space_samples_get => "qualia-session-store",
    fragment_beliefs_get => "qualia-session-store",
    session_merge_candidates_get => "qualia-session-store",
    merge_candidates_for_session_get => "qualia-session-store",
    environment_plans_get => "qualia-session-store",
    plan_run_regions_get => "qualia-session-store",
    coach_reviews_get => "qualia-session-store",
    // Fleet learning and the belief layers report through cognition.
    converged_beliefs_get => "the cognition fleet runtime",
    fleet_status_get => "the cognition fleet runtime",
    learning_candidates_get => "the cognition fleet runtime",
    learning_evaluations_get => "the cognition fleet runtime",
    learning_promotions_get => "the cognition fleet runtime",
    belief_sketch_get => "the cognition runtime",
    belief_status_get => "the cognition runtime",
    belief_checkpoint_post => "the cognition runtime",
    belief_shadow_start_post => "the cognition runtime",
    belief_shadow_stop_post => "the cognition runtime",
    belief_shadow_patch_post => "the cognition runtime",
    belief_shadow_publish_post => "the cognition runtime",
    // Arena sessions are a sync projection owned by the session store.
    arena_status_get => "the sync arena runtime",
    arena_start_post => "the sync arena runtime",
    arena_stop_post => "the sync arena runtime",
    guard_stop_post => "the Leash operator client",
    studio_control_ack_post => "the sync arena runtime",
}
