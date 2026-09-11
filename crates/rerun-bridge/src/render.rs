//! Text and markdown rendering for projection records.

use qualia_session_store::{SyncAppendEntryRow, SyncOpRow, SyncReplicaRow, SyncStateRow};
use qualia_sync_types::{
    CanonicalBody, CanonicalStateEnvelope, CoachDecision, CoachDecisionKind, OperationalStateEnvelope,
    ProposalBody, ProposalEnvelope, SyncBody, SyncMaterializedState,
};
use rerun::{Color, TextLogLevel};
use std::collections::HashMap;

use crate::records::SessionReplayFrame;
use crate::taxonomy::WorldModelEntityTaxonomy;

pub(crate) fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| format!("<json-error:{error}>"))
}

pub(crate) fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| format!("<json-error:{error}>"))
}

fn get_number(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f32> {
    map.get(key)
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32)
}

fn get_number_or_default(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: f32,
) -> f32 {
    get_number(map, key).unwrap_or(default)
}

/// Pulls a 2D point out of one of the attribute shapes the world model uses:
/// a nested `center`/`position` map, or flat `x_m`/`y_m` (falling back to
/// `x`/`y`).
pub(crate) fn extract_point_from_value(value: &serde_json::Value) -> Option<(f32, f32)> {
    let object = value.as_object()?;
    for nested in ["center", "position"] {
        if let Some(inner) = object.get(nested).and_then(serde_json::Value::as_object) {
            let x = get_number(inner, "x_m").or_else(|| get_number(inner, "x"))?;
            let y = get_number(inner, "y_m")
                .or_else(|| get_number(inner, "y"))
                .unwrap_or(0.0);
            return Some((x, y));
        }
    }
    if object.contains_key("x_m") {
        return Some((
            get_number(object, "x_m")?,
            get_number_or_default(object, "y_m", 0.0),
        ));
    }
    if object.contains_key("x") {
        return Some((
            get_number(object, "x")?,
            get_number_or_default(object, "y", 0.0),
        ));
    }
    None
}

pub(crate) fn point_text(point: Option<(f32, f32)>) -> String {
    match point {
        Some((x, y)) => format!("({x:.2}, {y:.2})"),
        None => "unavailable".to_string(),
    }
}

pub(crate) fn proposal_summary_text(proposal: &ProposalEnvelope) -> String {
    format!(
        "kind={:?} status={:?} confidence={:.2} mission_relevance={:.2} source_replica={}",
        proposal.proposal_kind,
        proposal.status,
        proposal.confidence,
        proposal.mission_relevance,
        proposal.source_replica_id
    )
}

pub(crate) fn proposal_body_text(proposal: &ProposalEnvelope) -> String {
    match &proposal.body {
        ProposalBody::Object(body) => format!(
            "label={} profile_refs={} attributes={}",
            body.label,
            body.profile_refs.len(),
            compact_json(&body.attributes)
        ),
        ProposalBody::Region(body) => format!(
            "region_kind={} profile_refs={} bounds={} attributes={}",
            body.region_kind,
            body.profile_refs.len(),
            compact_json(&body.bounds),
            compact_json(&body.attributes)
        ),
        ProposalBody::GraphNode(body) => format!(
            "node_kind={} profile_refs={} attributes={}",
            body.node_kind,
            body.profile_refs.len(),
            compact_json(&body.attributes)
        ),
        ProposalBody::GraphFactor(body) => format!(
            "factor_kind={} left={} right={} energy_weight={:.2} parameters={}",
            body.factor_kind,
            body.left_node_id,
            body.right_node_id,
            body.energy_weight,
            compact_json(&body.parameters)
        ),
        ProposalBody::PlannerAdvisory(body) => format!(
            "advisory_kind={} target_refs={} payload={}",
            body.advisory_kind,
            body.target_refs.len(),
            compact_json(&body.payload)
        ),
    }
}

pub(crate) fn proposal_geometry_point(proposal: &ProposalEnvelope) -> Option<(f32, f32)> {
    match &proposal.body {
        ProposalBody::Object(body) => extract_point_from_value(&body.attributes),
        ProposalBody::Region(body) => extract_point_from_value(&body.bounds)
            .or_else(|| extract_point_from_value(&body.attributes)),
        ProposalBody::GraphNode(body) => extract_point_from_value(&body.attributes),
        ProposalBody::GraphFactor(body) => extract_point_from_value(&body.parameters),
        ProposalBody::PlannerAdvisory(body) => extract_point_from_value(&body.payload),
    }
}

pub(crate) fn canonical_summary_text(canonical: &CanonicalStateEnvelope) -> String {
    format!(
        "kind={:?} status={:?} source_decision={} source_proposals={}",
        canonical.canonical_kind,
        canonical.status,
        canonical.source_decision_id,
        canonical.source_proposal_ids.len()
    )
}

pub(crate) fn canonical_body_text(canonical: &CanonicalStateEnvelope) -> String {
    match &canonical.body {
        CanonicalBody::Object(body) => format!(
            "label={} attributes={}",
            body.label,
            compact_json(&body.attributes)
        ),
        CanonicalBody::Region(body) => format!(
            "region_kind={} bounds={} attributes={}",
            body.region_kind,
            compact_json(&body.bounds),
            compact_json(&body.attributes)
        ),
        CanonicalBody::GraphNode(body) => format!(
            "node_kind={} attributes={}",
            body.node_kind,
            compact_json(&body.attributes)
        ),
        CanonicalBody::GraphFactor(body) => format!(
            "factor_kind={} left={} right={} energy_weight={:.2} parameters={}",
            body.factor_kind,
            body.left_node_id,
            body.right_node_id,
            body.energy_weight,
            compact_json(&body.parameters)
        ),
    }
}

pub(crate) fn canonical_geometry_point(canonical: &CanonicalStateEnvelope) -> Option<(f32, f32)> {
    match &canonical.body {
        CanonicalBody::Object(body) => extract_point_from_value(&body.attributes),
        CanonicalBody::Region(body) => extract_point_from_value(&body.bounds)
            .or_else(|| extract_point_from_value(&body.attributes)),
        CanonicalBody::GraphNode(body) => extract_point_from_value(&body.attributes),
        CanonicalBody::GraphFactor(body) => extract_point_from_value(&body.parameters),
    }
}

/// Which way a graph factor pulls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphRelationClass {
    Support,
    Contradiction,
    Neutral,
}

impl GraphRelationClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Support => "support",
            Self::Contradiction => "contradiction",
            Self::Neutral => "neutral",
        }
    }
}

fn keyword_relation(text: &str, negative: &[&str], positive: &[&str]) -> Option<GraphRelationClass> {
    let lowered = text.to_ascii_lowercase();
    if negative.iter().any(|needle| lowered.contains(needle)) {
        return Some(GraphRelationClass::Contradiction);
    }
    if positive.iter().any(|needle| lowered.contains(needle)) {
        return Some(GraphRelationClass::Support);
    }
    None
}

const NEGATIVE_FACTOR_KINDS: &[&str] = &["contradiction", "conflict", "exclude", "inhibit"];
const POSITIVE_FACTOR_KINDS: &[&str] = &["support", "route", "link", "evidence"];
const NEGATIVE_KEYS: &[&str] = &["contradiction", "conflict"];
const POSITIVE_KEYS: &[&str] = &["support", "evidence"];

fn flag_is_set(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    object
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn graph_relation_class(factor_kind: &str, value: &serde_json::Value) -> GraphRelationClass {
    if let Some(relation) =
        keyword_relation(factor_kind, NEGATIVE_FACTOR_KINDS, POSITIVE_FACTOR_KINDS)
    {
        return relation;
    }
    let Some(object) = value.as_object() else {
        return GraphRelationClass::Neutral;
    };
    if flag_is_set(object, "contradiction") || flag_is_set(object, "conflict") {
        return GraphRelationClass::Contradiction;
    }
    if flag_is_set(object, "support") {
        return GraphRelationClass::Support;
    }
    for key in ["relation", "edge_kind", "factor_kind", "kind"] {
        if let Some(text) = object.get(key).and_then(serde_json::Value::as_str) {
            if let Some(relation) = keyword_relation(text, NEGATIVE_KEYS, POSITIVE_KEYS) {
                return relation;
            }
        }
    }
    GraphRelationClass::Neutral
}

pub(crate) fn graph_relation_label(factor_kind: &str, value: &serde_json::Value) -> &'static str {
    graph_relation_class(factor_kind, value).as_str()
}

pub(crate) fn graph_factor_markdown(
    id: &str,
    factor_kind: &str,
    left_node_id: &str,
    right_node_id: &str,
    energy_weight: f32,
    relation: &str,
    parameters: &serde_json::Value,
) -> String {
    format!(
        "# Graph Factor `{id}`\n\n- relation: `{}`\n- factor_kind: `{}`\n- left: `{}`\n- right: `{}`\n- energy_weight: `{:.2}`\n\n```json\n{}\n```",
        relation,
        factor_kind,
        left_node_id,
        right_node_id,
        energy_weight,
        pretty_json(parameters)
    )
}

pub(crate) fn graph_layer_markdown(
    title: &str,
    root: &str,
    node_descriptors: &[String],
    factor_descriptors: &[String],
) -> String {
    let nodes = if node_descriptors.is_empty() {
        "- none".to_string()
    } else {
        node_descriptors.join("\n")
    };
    let factors = if factor_descriptors.is_empty() {
        "- none".to_string()
    } else {
        factor_descriptors.join("\n")
    };
    format!(
        "# {title}\n\n- root: `{root}`\n- graph_nodes: {}\n- graph_factors: {}\n\n## Nodes\n{}\n\n## Factors\n{}",
        node_descriptors.len(),
        factor_descriptors.len(),
        nodes,
        factors
    )
}

pub(crate) fn replay_frame_markdown(frame: &SessionReplayFrame<'_>) -> String {
    let world_model = frame
        .world_model
        .as_ref()
        .map(|projection| {
            format!(
                "- proposals: {}\n- decisions: {}\n- canonical: {}\n- operational_nav_goal: {}\n- operational_hazards: {}",
                projection.proposals.len(),
                projection.decisions.len(),
                projection.canonical.len(),
                projection.operational.nav_goal.is_some(),
                projection.operational.hazards.len()
            )
        })
        .unwrap_or_else(|| "- none".to_string());
    let sync = frame
        .sync
        .as_ref()
        .map(|projection| {
            format!(
                "- replicas: {}\n- states: {}\n- append_entries: {}\n- ops: {}",
                projection.replicas.len(),
                projection.states.len(),
                projection.append_entries.len(),
                projection.ops.len()
            )
        })
        .unwrap_or_else(|| "- none".to_string());
    let label = frame.label.unwrap_or("unnamed");

    format!(
        "# Replay Frame `{}`\n\n- tick: {}\n\n## World Model\n{}\n\n## Sync\n{}",
        label, frame.tick, world_model, sync
    )
}

pub(crate) fn coach_decision_kind_label(kind: CoachDecisionKind) -> &'static str {
    match kind {
        CoachDecisionKind::Promote => "promote",
        CoachDecisionKind::Reject => "reject",
        CoachDecisionKind::Merge => "merge",
        CoachDecisionKind::Split => "split",
        CoachDecisionKind::Link => "link",
        CoachDecisionKind::Deprecate => "deprecate",
    }
}

pub(crate) fn coach_decision_level_and_color(kind: CoachDecisionKind) -> (TextLogLevel, Color) {
    match kind {
        CoachDecisionKind::Promote => (TextLogLevel::INFO.into(), Color::from_rgb(56, 168, 83)),
        CoachDecisionKind::Reject => (TextLogLevel::WARN.into(), Color::from_rgb(214, 69, 65)),
        CoachDecisionKind::Merge => (TextLogLevel::INFO.into(), Color::from_rgb(53, 115, 210)),
        CoachDecisionKind::Split => (TextLogLevel::INFO.into(), Color::from_rgb(184, 118, 29)),
        CoachDecisionKind::Link => (TextLogLevel::INFO.into(), Color::from_rgb(95, 90, 220)),
        CoachDecisionKind::Deprecate => (TextLogLevel::WARN.into(), Color::from_rgb(128, 128, 128)),
    }
}

pub(crate) fn coach_timeline_text(decision: &CoachDecision) -> String {
    format!(
        "{} targets={} outputs={} curator={} reason={}",
        coach_decision_kind_label(decision.decision_kind),
        decision.target_proposal_ids.len(),
        decision.output_ids.len(),
        decision.curator_replica_id,
        decision.reason.as_deref().unwrap_or("n/a")
    )
}

fn bullet_list(entries: &[String]) -> String {
    if entries.is_empty() {
        "- none".to_string()
    } else {
        entries.join("\n")
    }
}

pub(crate) fn coach_decision_markdown(decision: &CoachDecision) -> String {
    let targets = bullet_list(
        &decision
            .target_proposal_ids
            .iter()
            .map(|target| format!("- `{target}`"))
            .collect::<Vec<_>>(),
    );
    let outputs = bullet_list(
        &decision
            .output_ids
            .iter()
            .map(|output| format!("- `{output}`"))
            .collect::<Vec<_>>(),
    );
    format!(
        "# Coach Decision `{}`\n\n- kind: `{}`\n- curator: `{}`\n- role: `{:?}`\n- created_at_hlc: `{}`\n- reason: {}\n\n## Targets\n{}\n\n## Outputs\n{}",
        decision.decision_id,
        coach_decision_kind_label(decision.decision_kind),
        decision.curator_replica_id,
        decision.curator_replica_role,
        decision.created_at_hlc,
        decision.reason.as_deref().unwrap_or("n/a"),
        targets,
        outputs
    )
}

pub(crate) fn coach_target_markdown(decision: &CoachDecision, proposal_id: &str) -> String {
    format!(
        "# Proposal Coach Effect\n\n- proposal: `{}`\n- decision: `{}`\n- kind: `{}`\n- reason: {}",
        proposal_id,
        decision.decision_id,
        coach_decision_kind_label(decision.decision_kind),
        decision.reason.as_deref().unwrap_or("n/a")
    )
}

pub(crate) fn coach_output_markdown(decision: &CoachDecision, output_id: &str) -> String {
    format!(
        "# Canonical Coach Effect\n\n- output: `{}`\n- decision: `{}`\n- kind: `{}`\n- source_targets: {}",
        output_id,
        decision.decision_id,
        coach_decision_kind_label(decision.decision_kind),
        decision.target_proposal_ids.join(", ")
    )
}

pub(crate) fn coach_summary_markdown(
    counts: &HashMap<&'static str, usize>,
    decisions: &[CoachDecision],
) -> String {
    let kinds = [
        CoachDecisionKind::Promote,
        CoachDecisionKind::Reject,
        CoachDecisionKind::Merge,
        CoachDecisionKind::Split,
        CoachDecisionKind::Link,
        CoachDecisionKind::Deprecate,
    ];
    let counts_text = kinds
        .iter()
        .map(|kind| {
            let label = coach_decision_kind_label(*kind);
            format!("- {}: {}", label, counts.get(label).copied().unwrap_or(0))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let events = bullet_list(
        &decisions
            .iter()
            .map(|decision| {
                format!(
                    "- `{}` `{}` targets={} outputs={}",
                    decision.decision_id,
                    coach_decision_kind_label(decision.decision_kind),
                    decision.target_proposal_ids.len(),
                    decision.output_ids.len()
                )
            })
            .collect::<Vec<_>>(),
    );

    format!(
        "# Coach Decisions\n\n## Counts\n{}\n\n## Timeline Paths\n- `{}`\n- `{}`\n\n## Events\n{}",
        counts_text,
        WorldModelEntityTaxonomy::coach_timeline_root(),
        WorldModelEntityTaxonomy::coach_root(),
        events
    )
}

fn optional_number(value: Option<f32>) -> String {
    value
        .map(|number| format!("{number:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

pub(crate) fn canonical_lineage_markdown(canonical: &CanonicalStateEnvelope) -> String {
    let source_decision = if canonical.source_decision_id.is_empty() {
        "- none".to_string()
    } else {
        format!(
            "- `{}` -> `{}`\n- coach_effect_path: `{}`",
            canonical.source_decision_id,
            WorldModelEntityTaxonomy::coach_decision(&canonical.source_decision_id),
            WorldModelEntityTaxonomy::canonical_coach_event(
                &canonical.canonical_id,
                &canonical.source_decision_id,
            )
        )
    };
    let source_proposals = bullet_list(
        &canonical
            .source_proposal_ids
            .iter()
            .map(|proposal_id| {
                format!(
                    "- `{}` -> `{}`",
                    proposal_id,
                    WorldModelEntityTaxonomy::proposal(proposal_id)
                )
            })
            .collect::<Vec<_>>(),
    );
    let accepted_views = match &canonical.body {
        CanonicalBody::Object(_) => vec![format!(
            "- accepted_spatial: `{}`",
            WorldModelEntityTaxonomy::canonical_spatial_object(&canonical.canonical_id)
        )],
        CanonicalBody::Region(_) => vec![format!(
            "- accepted_spatial: `{}`",
            WorldModelEntityTaxonomy::canonical_spatial_region(&canonical.canonical_id)
        )],
        CanonicalBody::GraphNode(_) => vec![
            format!(
                "- accepted_graph: `{}`",
                WorldModelEntityTaxonomy::canonical_graph_node(&canonical.canonical_id)
            ),
            format!(
                "- accepted_spatial: `{}`",
                WorldModelEntityTaxonomy::canonical_spatial_node(&canonical.canonical_id)
            ),
        ],
        CanonicalBody::GraphFactor(_) => vec![
            format!(
                "- accepted_graph: `{}`",
                WorldModelEntityTaxonomy::canonical_graph_factor_summary(&canonical.canonical_id)
            ),
            format!(
                "- accepted_spatial: `{}`",
                WorldModelEntityTaxonomy::canonical_spatial_factor(&canonical.canonical_id)
            ),
        ],
    }
    .join("\n");

    format!(
        "# Accepted Lineage `{}`\n\n- canonical_kind: `{:?}`\n- status: `{:?}`\n- accepted_at_hlc: `{}`\n- canonical_root: `{}`\n\n## Source Decision\n{}\n\n## Source Proposals\n{}\n\n## Inspect Paths\n- summary: `{}`\n- body: `{}`\n- geometry: `{}`\n{}\n",
        canonical.canonical_id,
        canonical.canonical_kind,
        canonical.status,
        canonical.accepted_at_hlc,
        WorldModelEntityTaxonomy::canonical(&canonical.canonical_id),
        source_decision,
        source_proposals,
        WorldModelEntityTaxonomy::canonical_summary(&canonical.canonical_id),
        WorldModelEntityTaxonomy::canonical_body(&canonical.canonical_id),
        WorldModelEntityTaxonomy::canonical_geometry(&canonical.canonical_id),
        accepted_views,
    )
}

pub(crate) fn operational_summary_markdown(operational: &OperationalStateEnvelope) -> String {
    let pose = operational
        .pose
        .as_ref()
        .map(|pose| {
            format!(
                "- canonical_id: `{}`\n- xy: `({:.2}, {:.2})`\n- z_m: `{:.2}`\n- yaw_rad: `{:.2}`\n- confidence: {}",
                pose.canonical_id,
                pose.x_m,
                pose.y_m,
                pose.z_m,
                pose.yaw_rad,
                optional_number(pose.confidence)
            )
        })
        .unwrap_or_else(|| "- none".to_string());
    let nav_goal = operational
        .nav_goal
        .as_ref()
        .map(|goal| {
            format!(
                "- canonical_id: `{}`\n- active: `{}`\n- xy: `({:.2}, {:.2})`\n- z_m: `{:.2}`\n- yaw_rad: `{:.2}`",
                goal.canonical_id, goal.active, goal.x_m, goal.y_m, goal.z_m, goal.yaw_rad
            )
        })
        .unwrap_or_else(|| "- none".to_string());
    let hazards = bullet_list(
        &operational
            .hazards
            .iter()
            .map(|hazard| {
                format!(
                    "- `{}` kind=`{}` severity={} summary={}",
                    hazard.canonical_id,
                    hazard.kind,
                    optional_number(hazard.severity),
                    hazard.summary.as_deref().unwrap_or("n/a")
                )
            })
            .collect::<Vec<_>>(),
    );
    let accepted_inputs = bullet_list(
        &operational
            .source_canonical_ids
            .iter()
            .map(|canonical_id| {
                format!(
                    "- `{}` -> `{}`",
                    canonical_id,
                    WorldModelEntityTaxonomy::operational_consequence(canonical_id)
                )
            })
            .collect::<Vec<_>>(),
    );

    format!(
        "# Operational Projection\n\n## Hot Path\n### Pose\n{}\n\n### Nav Goal\n{}\n\n### Hazards\n{}\n\n### Planner\n- accepted_count: `{}`\n- nav_goal_present: `{}`\n- hazard_count: `{}`\n- route_factor_count: `{}`\n\n## Accepted Inputs\n{}",
        pose,
        nav_goal,
        hazards,
        operational.planner.accepted_count,
        operational.planner.nav_goal_present,
        operational.planner.hazard_count,
        operational.planner.route_factor_count,
        accepted_inputs,
    )
}

pub(crate) fn operational_consequence_markdown(
    operational: &OperationalStateEnvelope,
    source_canonical_id: &str,
) -> String {
    let mut outputs = Vec::new();
    if operational
        .pose
        .as_ref()
        .is_some_and(|pose| pose.canonical_id == source_canonical_id)
    {
        outputs.push(format!(
            "- pose -> `{}`",
            WorldModelEntityTaxonomy::operational_pose()
        ));
    }
    if operational
        .nav_goal
        .as_ref()
        .is_some_and(|goal| goal.canonical_id == source_canonical_id)
    {
        outputs.push(format!(
            "- nav_goal -> `{}`",
            WorldModelEntityTaxonomy::operational_nav_goal()
        ));
    }
    for hazard in operational
        .hazards
        .iter()
        .filter(|hazard| hazard.canonical_id == source_canonical_id)
    {
        outputs.push(format!(
            "- hazard `{}` -> `{}`",
            hazard.kind,
            WorldModelEntityTaxonomy::operational_hazard(&hazard.canonical_id)
        ));
    }
    if outputs.is_empty() {
        outputs.push("- planner-only influence".to_string());
    }

    format!(
        "# Operational Consequence `{}`\n\n## Outputs\n{}\n\n## Shared Planner State\n- accepted_count: `{}`\n- nav_goal_present: `{}`\n- hazard_count: `{}`\n- route_factor_count: `{}`",
        source_canonical_id,
        outputs.join("\n"),
        operational.planner.accepted_count,
        operational.planner.nav_goal_present,
        operational.planner.hazard_count,
        operational.planner.route_factor_count,
    )
}

pub(crate) fn sync_replica_summary_text(replica: &SyncReplicaRow) -> String {
    format!(
        "role={:?} trust_state={:?} enabled={} endpoint={} last_sync_seq={} last_seen_hlc={:?}",
        replica.replica_role,
        replica.trust_state,
        replica.enabled,
        replica.endpoint,
        replica.last_sync_seq,
        replica.last_seen_hlc
    )
}

pub(crate) fn sync_state_summary_text(state: &SyncStateRow) -> String {
    let detail = match &state.state {
        SyncMaterializedState::Register(register) => format!(
            "kind=register winner_replica={} lease_holder={:?} value={}",
            register.winner_replica_id,
            register.lease_holder,
            compact_json(&register.value)
        ),
        SyncMaterializedState::OrMap(or_map) => format!(
            "kind=or_map entries={} removed_tags={} visible={}",
            or_map.entries.len(),
            or_map.removed_tags.len(),
            or_map.visible_value.is_some()
        ),
        SyncMaterializedState::Embedding(embedding) => format!(
            "kind=embedding merge_mode={:?} dims={} total_weight={:.2}",
            embedding.merge_mode,
            embedding.accumulator.len(),
            embedding.total_weight
        ),
        SyncMaterializedState::WeightTile(tile) => format!(
            "kind=weight_tile layer_id={} tile_row={} tile_col={} tile_size={} checkpoint={} total_weight={:.2}",
            tile.layer_id,
            tile.tile_row,
            tile.tile_col,
            tile.tile_size,
            tile.base_checkpoint,
            tile.total_weight
        ),
    };
    format!(
        "namespace={} key={} updated_hlc={} {}",
        state.namespace, state.key, state.updated_hlc, detail
    )
}

pub(crate) fn sync_append_entry_summary_text(entry: &SyncAppendEntryRow) -> String {
    format!(
        "namespace={} key={} entry_id={} replica={} timestamp_hlc={} value={}",
        entry.namespace,
        entry.key,
        entry.entry_id,
        entry.replica_id,
        entry.timestamp_hlc,
        compact_json(&entry.value)
    )
}

fn sync_body_kind(body: &SyncBody) -> &'static str {
    match body {
        SyncBody::Register(_) => "register",
        SyncBody::OrMap(_) => "or_map",
        SyncBody::AppendOnly(_) => "append_only",
        SyncBody::Embedding(_) => "embedding",
        SyncBody::WeightTile(_) => "weight_tile",
    }
}

pub(crate) fn sync_op_summary_text(op: &SyncOpRow) -> String {
    format!(
        "seq={} namespace={} key={} replica={} role={:?} status={:?} body_kind={} received_at={}",
        op.seq,
        op.namespace,
        op.key,
        op.replica_id,
        op.replica_role,
        op.status,
        sync_body_kind(&op.body),
        op.received_at
    )
}

/// Broad visual bucket used to style points and edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorldModelVisualClass {
    Proposal,
    Accepted,
    Operational,
    Neutral,
}

fn world_model_visual_class(entity_path: &str) -> WorldModelVisualClass {
    if [
        WorldModelEntityTaxonomy::canonical_spatial_root(),
        WorldModelEntityTaxonomy::canonical_graph_root(),
        WorldModelEntityTaxonomy::canonical_root(),
    ]
    .iter()
    .any(|root| entity_path.starts_with(root))
    {
        WorldModelVisualClass::Accepted
    } else if [
        WorldModelEntityTaxonomy::proposal_graph_root(),
        WorldModelEntityTaxonomy::proposals_root(),
    ]
    .iter()
    .any(|root| entity_path.starts_with(root))
    {
        WorldModelVisualClass::Proposal
    } else if entity_path.starts_with(WorldModelEntityTaxonomy::operational_root()) {
        WorldModelVisualClass::Operational
    } else {
        WorldModelVisualClass::Neutral
    }
}

pub(crate) fn world_model_point_style(entity_path: &str) -> (f32, Color) {
    match world_model_visual_class(entity_path) {
        WorldModelVisualClass::Proposal => (0.06, Color::from_rgb(70, 184, 255)),
        WorldModelVisualClass::Accepted => (0.075, Color::from_rgb(56, 168, 83)),
        WorldModelVisualClass::Operational => (0.07, Color::from_rgb(255, 190, 88)),
        WorldModelVisualClass::Neutral => (0.06, Color::from_rgb(181, 181, 181)),
    }
}

pub(crate) fn world_model_line_style(entity_path: &str) -> (f32, Color) {
    match world_model_visual_class(entity_path) {
        WorldModelVisualClass::Proposal => (0.02, Color::from_rgb(255, 190, 88)),
        WorldModelVisualClass::Accepted => (0.028, Color::from_rgb(56, 168, 83)),
        WorldModelVisualClass::Operational => (0.03, Color::from_rgb(255, 190, 88)),
        WorldModelVisualClass::Neutral => (0.02, Color::from_rgb(181, 181, 181)),
    }
}
