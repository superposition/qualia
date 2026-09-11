//! Stable entity paths for everything the bridge logs.

use qualia_sync_types::{CoachDecisionKind, SyncNamespace};

/// Entity ids that keep the recording tree navigable: anything outside
/// `[A-Za-z0-9._-]` collapses to `-` so paths never split on user data.
pub(crate) fn sanitize_entity_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// World-model entity tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldModelEntityTaxonomy;

impl WorldModelEntityTaxonomy {
    pub const ROOT: &'static str = "qualia/world_model";
    pub const PROPOSALS_ROOT: &'static str = "qualia/world_model/proposals";
    pub const COACH_ROOT: &'static str = "qualia/world_model/coach";
    pub const CANONICAL_ROOT: &'static str = "qualia/world_model/canonical";
    pub const OPERATIONAL_ROOT: &'static str = "qualia/world_model/operational";
    pub const PROPOSAL_GRAPH_ROOT: &'static str = "qualia/world_model/proposals/graph";
    pub const CANONICAL_GRAPH_ROOT: &'static str = "qualia/world_model/canonical/graph";
    pub const CANONICAL_SPATIAL_ROOT: &'static str = "qualia/world_model/canonical/spatial";

    pub fn summary() -> &'static str {
        "qualia/world_model/summary"
    }

    pub fn proposals_root() -> &'static str {
        Self::PROPOSALS_ROOT
    }

    pub fn coach_root() -> &'static str {
        Self::COACH_ROOT
    }

    pub fn coach_summary() -> &'static str {
        "qualia/world_model/coach/summary"
    }

    pub fn coach_timeline_root() -> &'static str {
        "qualia/world_model/coach/timeline"
    }

    pub fn coach_timeline_kind(kind: CoachDecisionKind) -> &'static str {
        match kind {
            CoachDecisionKind::Promote => "qualia/world_model/coach/timeline/promote",
            CoachDecisionKind::Reject => "qualia/world_model/coach/timeline/reject",
            CoachDecisionKind::Merge => "qualia/world_model/coach/timeline/merge",
            CoachDecisionKind::Split => "qualia/world_model/coach/timeline/split",
            CoachDecisionKind::Link => "qualia/world_model/coach/timeline/link",
            CoachDecisionKind::Deprecate => "qualia/world_model/coach/timeline/deprecate",
        }
    }

    pub fn coach_decision(id: &str) -> String {
        format!("{}/decisions/{}", Self::COACH_ROOT, sanitize_entity_id(id))
    }

    pub fn proposal_coach_event(proposal_id: &str, decision_id: &str) -> String {
        format!(
            "{}/coach/{}",
            Self::proposal(proposal_id),
            sanitize_entity_id(decision_id)
        )
    }

    pub fn canonical_root() -> &'static str {
        Self::CANONICAL_ROOT
    }

    pub fn operational_root() -> &'static str {
        Self::OPERATIONAL_ROOT
    }

    pub fn proposal(id: &str) -> String {
        format!("{}/{}", Self::PROPOSALS_ROOT, sanitize_entity_id(id))
    }

    pub fn proposal_graph_root() -> &'static str {
        Self::PROPOSAL_GRAPH_ROOT
    }

    pub fn proposal_graph_nodes_root() -> &'static str {
        "qualia/world_model/proposals/graph/nodes"
    }

    pub fn proposal_graph_node(id: &str) -> String {
        format!(
            "{}/{}",
            Self::proposal_graph_nodes_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn proposal_graph_factors_root() -> &'static str {
        "qualia/world_model/proposals/graph/factors"
    }

    pub fn proposal_graph_factor(id: &str) -> String {
        format!(
            "{}/{}",
            Self::proposal_graph_factors_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn proposal_graph_factor_edge(id: &str) -> String {
        format!("{}/edge", Self::proposal_graph_factor(id))
    }

    pub fn proposal_graph_factor_summary(id: &str) -> String {
        format!("{}/summary", Self::proposal_graph_factor(id))
    }

    pub fn proposal_graph_summary_document() -> &'static str {
        "qualia/world_model/proposals/graph/summary"
    }

    pub fn proposal_summary(id: &str) -> String {
        format!("{}/summary", Self::proposal(id))
    }

    pub fn proposal_body(id: &str) -> String {
        format!("{}/body", Self::proposal(id))
    }

    pub fn proposal_geometry(id: &str) -> String {
        format!("{}/geometry", Self::proposal(id))
    }

    pub fn canonical(id: &str) -> String {
        format!("{}/{}", Self::CANONICAL_ROOT, sanitize_entity_id(id))
    }

    pub fn canonical_coach_event(canonical_id: &str, decision_id: &str) -> String {
        format!(
            "{}/coach/{}",
            Self::canonical(canonical_id),
            sanitize_entity_id(decision_id)
        )
    }

    pub fn canonical_graph_root() -> &'static str {
        Self::CANONICAL_GRAPH_ROOT
    }

    pub fn canonical_spatial_root() -> &'static str {
        Self::CANONICAL_SPATIAL_ROOT
    }

    pub fn canonical_spatial_objects_root() -> &'static str {
        "qualia/world_model/canonical/spatial/objects"
    }

    pub fn canonical_spatial_object(id: &str) -> String {
        format!(
            "{}/{}",
            Self::canonical_spatial_objects_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn canonical_spatial_regions_root() -> &'static str {
        "qualia/world_model/canonical/spatial/regions"
    }

    pub fn canonical_spatial_region(id: &str) -> String {
        format!(
            "{}/{}",
            Self::canonical_spatial_regions_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn canonical_spatial_nodes_root() -> &'static str {
        "qualia/world_model/canonical/spatial/nodes"
    }

    pub fn canonical_spatial_node(id: &str) -> String {
        format!(
            "{}/{}",
            Self::canonical_spatial_nodes_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn canonical_spatial_factors_root() -> &'static str {
        "qualia/world_model/canonical/spatial/factors"
    }

    pub fn canonical_spatial_factor(id: &str) -> String {
        format!(
            "{}/{}",
            Self::canonical_spatial_factors_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn canonical_spatial_factor_edge(id: &str) -> String {
        format!("{}/edge", Self::canonical_spatial_factor(id))
    }

    pub fn canonical_lineage(id: &str) -> String {
        format!("{}/lineage", Self::canonical(id))
    }

    pub fn canonical_graph_nodes_root() -> &'static str {
        "qualia/world_model/canonical/graph/nodes"
    }

    pub fn canonical_graph_node(id: &str) -> String {
        format!(
            "{}/{}",
            Self::canonical_graph_nodes_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn canonical_graph_factors_root() -> &'static str {
        "qualia/world_model/canonical/graph/factors"
    }

    pub fn canonical_graph_factor(id: &str) -> String {
        format!(
            "{}/{}",
            Self::canonical_graph_factors_root(),
            sanitize_entity_id(id)
        )
    }

    pub fn canonical_graph_factor_edge(id: &str) -> String {
        format!("{}/edge", Self::canonical_graph_factor(id))
    }

    pub fn canonical_graph_factor_summary(id: &str) -> String {
        format!("{}/summary", Self::canonical_graph_factor(id))
    }

    pub fn canonical_graph_summary_document() -> &'static str {
        "qualia/world_model/canonical/graph/summary"
    }

    pub fn canonical_summary(id: &str) -> String {
        format!("{}/summary", Self::canonical(id))
    }

    pub fn canonical_body(id: &str) -> String {
        format!("{}/body", Self::canonical(id))
    }

    pub fn canonical_geometry(id: &str) -> String {
        format!("{}/geometry", Self::canonical(id))
    }

    pub fn operational_pose() -> &'static str {
        "qualia/world_model/operational/pose"
    }

    pub fn operational_nav_goal() -> &'static str {
        "qualia/world_model/operational/nav_goal"
    }

    pub fn operational_route() -> &'static str {
        "qualia/world_model/operational/route"
    }

    pub fn operational_summary() -> &'static str {
        "qualia/world_model/operational/summary"
    }

    pub fn operational_hazards() -> &'static str {
        "qualia/world_model/operational/hazards"
    }

    pub fn operational_hazard(id: &str) -> String {
        format!("{}/{}", Self::operational_hazards(), sanitize_entity_id(id))
    }

    pub fn operational_planner() -> &'static str {
        "qualia/world_model/operational/planner"
    }

    pub fn operational_consequences_root() -> &'static str {
        "qualia/world_model/operational/consequences"
    }

    pub fn operational_consequence(id: &str) -> String {
        format!(
            "{}/{}",
            Self::operational_consequences_root(),
            sanitize_entity_id(id)
        )
    }
}

/// Sync entity tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncEntityTaxonomy;

impl SyncEntityTaxonomy {
    pub const ROOT: &'static str = "qualia/sync";
    pub const REPLICAS_ROOT: &'static str = "qualia/sync/replicas";
    pub const STATES_ROOT: &'static str = "qualia/sync/states";
    pub const APPEND_ROOT: &'static str = "qualia/sync/append";
    pub const OPS_ROOT: &'static str = "qualia/sync/ops";

    pub fn summary() -> &'static str {
        "qualia/sync/summary"
    }

    pub fn replicas_root() -> &'static str {
        Self::REPLICAS_ROOT
    }

    pub fn replica(replica_id: &str) -> String {
        format!("{}/{}", Self::REPLICAS_ROOT, sanitize_entity_id(replica_id))
    }

    pub fn states_root() -> &'static str {
        Self::STATES_ROOT
    }

    pub fn state(namespace: SyncNamespace, key: &str) -> String {
        format!(
            "{}/{}/{}",
            Self::STATES_ROOT,
            sanitize_entity_id(&namespace.to_string()),
            sanitize_entity_id(key)
        )
    }

    pub fn append_root() -> &'static str {
        Self::APPEND_ROOT
    }

    pub fn append_entry(namespace: SyncNamespace, key: &str, entry_id: &str) -> String {
        format!(
            "{}/{}/{}/{}",
            Self::APPEND_ROOT,
            sanitize_entity_id(&namespace.to_string()),
            sanitize_entity_id(key),
            sanitize_entity_id(entry_id)
        )
    }

    pub fn ops_root() -> &'static str {
        Self::OPS_ROOT
    }

    pub fn op(op_id: &str) -> String {
        format!("{}/{}", Self::OPS_ROOT, sanitize_entity_id(op_id))
    }
}

/// Replay entity tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayEntityTaxonomy;

impl ReplayEntityTaxonomy {
    pub const ROOT: &'static str = "qualia/replay";

    pub fn summary() -> &'static str {
        "qualia/replay/summary"
    }
}
