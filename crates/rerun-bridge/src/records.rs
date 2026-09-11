//! Projection inputs and the record streams derived from them.
//!
//! Everything here is pure: a projection in, an ordered list of
//! [`WorldModelProjectionRecord`] values out. The bridge is the only place
//! that touches a Rerun recording.

use qualia_session_store::{SyncAppendEntryRow, SyncOpRow, SyncReplicaRow, SyncStateRow};
use qualia_sync_types::{
    CanonicalBody, CanonicalStateEnvelope, CanonicalStatus, CoachDecision, OperationalStateEnvelope,
    ProposalBody, ProposalEnvelope, ProposalStatus, SyncApplyStatus,
};
use rerun::{Color, TextLogLevel};
use std::collections::{BTreeSet, HashMap};

use crate::render::{
    canonical_body_text, canonical_geometry_point, canonical_lineage_markdown, canonical_summary_text,
    coach_decision_level_and_color, coach_decision_kind_label, coach_decision_markdown,
    coach_output_markdown, coach_summary_markdown, coach_target_markdown, coach_timeline_text,
    extract_point_from_value, graph_factor_markdown, graph_layer_markdown, graph_relation_label,
    operational_consequence_markdown, operational_summary_markdown, point_text,
    proposal_body_text, proposal_geometry_point, proposal_summary_text, replay_frame_markdown,
    sync_append_entry_summary_text, sync_op_summary_text, sync_replica_summary_text,
    sync_state_summary_text,
};
use crate::taxonomy::{ReplayEntityTaxonomy, SyncEntityTaxonomy, WorldModelEntityTaxonomy};

/// One world-model slice: proposals, coach decisions, accepted canonical
/// state, and the flattened operational view.
pub struct WorldModelProjection<'a> {
    pub proposals: &'a [ProposalEnvelope],
    pub decisions: &'a [CoachDecision],
    pub canonical: &'a [CanonicalStateEnvelope],
    pub operational: &'a OperationalStateEnvelope,
}

/// One sync slice: the replica registry, materialized state, append log, and
/// the op log.
pub struct SyncProjection<'a> {
    pub replicas: &'a [SyncReplicaRow],
    pub states: &'a [SyncStateRow],
    pub append_entries: &'a [SyncAppendEntryRow],
    pub ops: &'a [SyncOpRow],
}

/// One timeline entry for the replay export.
pub struct SessionReplayFrame<'a> {
    pub tick: u64,
    pub label: Option<&'a str>,
    pub world_model: Option<WorldModelProjection<'a>>,
    pub sync: Option<SyncProjection<'a>>,
}

/// The payload written at one entity path.
#[derive(Debug, Clone, PartialEq)]
pub enum WorldModelProjectionContent {
    Text(String),
    Document(String),
    LogEvent {
        text: String,
        level: TextLogLevel,
        color: Color,
    },
    Points2D(Vec<(f32, f32)>),
    LineStrip2D(Vec<(f32, f32)>),
}

/// One entity path plus what to write there.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldModelProjectionRecord {
    pub entity_path: String,
    pub content: WorldModelProjectionContent,
}

fn text_record(entity_path: String, text: String) -> WorldModelProjectionRecord {
    WorldModelProjectionRecord {
        entity_path,
        content: WorldModelProjectionContent::Text(text),
    }
}

fn document_record(entity_path: String, markdown: String) -> WorldModelProjectionRecord {
    WorldModelProjectionRecord {
        entity_path,
        content: WorldModelProjectionContent::Document(markdown),
    }
}

fn points_record(entity_path: String, points: Vec<(f32, f32)>) -> WorldModelProjectionRecord {
    WorldModelProjectionRecord {
        entity_path,
        content: WorldModelProjectionContent::Points2D(points),
    }
}

fn line_record(entity_path: String, points: Vec<(f32, f32)>) -> WorldModelProjectionRecord {
    WorldModelProjectionRecord {
        entity_path,
        content: WorldModelProjectionContent::LineStrip2D(points),
    }
}

/// Builds the full record stream for one world-model slice.
pub fn collect_world_model_projection_records(
    tick: u64,
    projection: &WorldModelProjection<'_>,
) -> Vec<WorldModelProjectionRecord> {
    let mut records = Vec::new();

    records.push(text_record(
        WorldModelEntityTaxonomy::summary().to_string(),
        format!(
            "tick={} proposals={} canonical={} accepted={} nav_goal_present={} hazards={}",
            tick,
            projection.proposals.len(),
            projection.canonical.len(),
            projection.operational.planner.accepted_count,
            projection.operational.planner.nav_goal_present,
            projection.operational.hazards.len()
        ),
    ));

    let promoted = projection
        .proposals
        .iter()
        .filter(|proposal| proposal.status == ProposalStatus::Promoted)
        .count();
    records.push(text_record(
        WorldModelEntityTaxonomy::proposals_root().to_string(),
        format!("count={} accepted={promoted}", projection.proposals.len()),
    ));

    let accepted = projection
        .canonical
        .iter()
        .filter(|entry| entry.status == CanonicalStatus::Accepted)
        .count();
    records.push(text_record(
        WorldModelEntityTaxonomy::canonical_root().to_string(),
        format!("count={} accepted={accepted}", projection.canonical.len()),
    ));

    for proposal in projection.proposals {
        records.push(text_record(
            WorldModelEntityTaxonomy::proposal_summary(&proposal.proposal_id),
            proposal_summary_text(proposal),
        ));
        records.push(text_record(
            WorldModelEntityTaxonomy::proposal_body(&proposal.proposal_id),
            proposal_body_text(proposal),
        ));
        records.push(match proposal_geometry_point(proposal) {
            Some(point) => points_record(
                WorldModelEntityTaxonomy::proposal_geometry(&proposal.proposal_id),
                vec![point],
            ),
            None => text_record(
                format!(
                    "{}/geometry",
                    WorldModelEntityTaxonomy::proposal(&proposal.proposal_id)
                ),
                "geometry=unavailable".to_string(),
            ),
        });
    }

    for entry in projection.canonical {
        records.push(text_record(
            WorldModelEntityTaxonomy::canonical_summary(&entry.canonical_id),
            canonical_summary_text(entry),
        ));
        records.push(text_record(
            WorldModelEntityTaxonomy::canonical_body(&entry.canonical_id),
            canonical_body_text(entry),
        ));
        records.push(match canonical_geometry_point(entry) {
            Some(point) => points_record(
                WorldModelEntityTaxonomy::canonical_geometry(&entry.canonical_id),
                vec![point],
            ),
            None => text_record(
                format!(
                    "{}/geometry",
                    WorldModelEntityTaxonomy::canonical(&entry.canonical_id)
                ),
                "geometry=unavailable".to_string(),
            ),
        });
    }

    records.extend(collect_operational_records(projection.operational));
    records.extend(collect_proposal_graph_records(projection.proposals));
    records.extend(collect_canonical_graph_records(projection.canonical));
    records.extend(collect_canonical_spatial_records(projection.canonical));
    records.extend(collect_coach_records(projection.decisions));

    records
}

fn collect_operational_records(
    operational: &OperationalStateEnvelope,
) -> Vec<WorldModelProjectionRecord> {
    let mut records = Vec::new();

    records.push(text_record(
        WorldModelEntityTaxonomy::operational_root().to_string(),
        format!(
            "pose_present={} nav_goal_present={} hazards={} route_factor_count={}",
            operational.pose.is_some(),
            operational.nav_goal.is_some(),
            operational.hazards.len(),
            operational.planner.route_factor_count
        ),
    ));
    records.push(document_record(
        WorldModelEntityTaxonomy::operational_summary().to_string(),
        operational_summary_markdown(operational),
    ));

    if let Some(pose) = &operational.pose {
        records.push(points_record(
            WorldModelEntityTaxonomy::operational_pose().to_string(),
            vec![(pose.x_m, pose.y_m)],
        ));
    }
    if let Some(nav_goal) = &operational.nav_goal {
        records.push(points_record(
            WorldModelEntityTaxonomy::operational_nav_goal().to_string(),
            vec![(nav_goal.x_m, nav_goal.y_m)],
        ));
    }
    if let (Some(pose), Some(nav_goal)) = (&operational.pose, &operational.nav_goal) {
        records.push(line_record(
            WorldModelEntityTaxonomy::operational_route().to_string(),
            vec![(pose.x_m, pose.y_m), (nav_goal.x_m, nav_goal.y_m)],
        ));
    }

    for hazard in &operational.hazards {
        records.push(text_record(
            WorldModelEntityTaxonomy::operational_hazard(&hazard.canonical_id),
            format!(
                "kind={} severity={:?} summary={:?}",
                hazard.kind, hazard.severity, hazard.summary
            ),
        ));
    }

    records.push(text_record(
        WorldModelEntityTaxonomy::operational_planner().to_string(),
        format!(
            "accepted_count={} nav_goal_present={} hazard_count={} route_factor_count={}",
            operational.planner.accepted_count,
            operational.planner.nav_goal_present,
            operational.planner.hazard_count,
            operational.planner.route_factor_count
        ),
    ));

    for source_canonical_id in &operational.source_canonical_ids {
        records.push(document_record(
            WorldModelEntityTaxonomy::operational_consequence(source_canonical_id),
            operational_consequence_markdown(operational, source_canonical_id),
        ));
    }

    records
}

fn collect_proposal_graph_records(
    proposals: &[ProposalEnvelope],
) -> Vec<WorldModelProjectionRecord> {
    let mut records = Vec::new();
    let mut node_positions = HashMap::new();
    let mut node_descriptors = Vec::new();
    let mut factor_descriptors = Vec::new();

    for proposal in proposals {
        let ProposalBody::GraphNode(node) = &proposal.body else {
            continue;
        };
        let position = extract_point_from_value(&node.attributes);
        if let Some(point) = position {
            node_positions.insert(proposal.proposal_id.clone(), point);
            records.push(points_record(
                WorldModelEntityTaxonomy::proposal_graph_node(&proposal.proposal_id),
                vec![point],
            ));
        }
        node_descriptors.push(format!(
            "- `{}` kind=`{}` position={} profile_refs={} confidence={:.2}",
            proposal.proposal_id,
            node.node_kind,
            point_text(position),
            node.profile_refs.len(),
            proposal.confidence
        ));
    }

    for proposal in proposals {
        let ProposalBody::GraphFactor(factor) = &proposal.body else {
            continue;
        };
        let relation = graph_relation_label(&factor.factor_kind, &factor.parameters);
        let endpoints = node_positions
            .get(&factor.left_node_id)
            .copied()
            .zip(node_positions.get(&factor.right_node_id).copied());
        if let Some((left, right)) = endpoints {
            records.push(line_record(
                WorldModelEntityTaxonomy::proposal_graph_factor_edge(&proposal.proposal_id),
                vec![left, right],
            ));
        }
        records.push(document_record(
            WorldModelEntityTaxonomy::proposal_graph_factor_summary(&proposal.proposal_id),
            graph_factor_markdown(
                &proposal.proposal_id,
                &factor.factor_kind,
                &factor.left_node_id,
                &factor.right_node_id,
                factor.energy_weight,
                relation,
                &factor.parameters,
            ),
        ));
        factor_descriptors.push(format!(
            "- `{}` relation=`{}` factor_kind=`{}` left=`{}` right=`{}` energy_weight={:.2}",
            proposal.proposal_id,
            relation,
            factor.factor_kind,
            factor.left_node_id,
            factor.right_node_id,
            factor.energy_weight
        ));
    }

    records.push(document_record(
        WorldModelEntityTaxonomy::proposal_graph_summary_document().to_string(),
        graph_layer_markdown(
            "Proposal Graph",
            WorldModelEntityTaxonomy::proposal_graph_root(),
            &node_descriptors,
            &factor_descriptors,
        ),
    ));

    records
}

fn collect_canonical_graph_records(
    canonical: &[CanonicalStateEnvelope],
) -> Vec<WorldModelProjectionRecord> {
    let mut records = Vec::new();
    let mut node_positions = HashMap::new();
    let mut node_descriptors = Vec::new();
    let mut factor_descriptors = Vec::new();

    for entry in canonical {
        let CanonicalBody::GraphNode(node) = &entry.body else {
            continue;
        };
        let position = extract_point_from_value(&node.attributes);
        if let Some(point) = position {
            node_positions.insert(entry.canonical_id.clone(), point);
            records.push(points_record(
                WorldModelEntityTaxonomy::canonical_graph_node(&entry.canonical_id),
                vec![point],
            ));
        }
        node_descriptors.push(format!(
            "- `{}` kind=`{}` position={} source_decision=`{}`",
            entry.canonical_id,
            node.node_kind,
            point_text(position),
            entry.source_decision_id
        ));
    }

    for entry in canonical {
        let CanonicalBody::GraphFactor(factor) = &entry.body else {
            continue;
        };
        let relation = graph_relation_label(&factor.factor_kind, &factor.parameters);
        let endpoints = node_positions
            .get(&factor.left_node_id)
            .copied()
            .zip(node_positions.get(&factor.right_node_id).copied());
        if let Some((left, right)) = endpoints {
            records.push(line_record(
                WorldModelEntityTaxonomy::canonical_graph_factor_edge(&entry.canonical_id),
                vec![left, right],
            ));
        }
        records.push(document_record(
            WorldModelEntityTaxonomy::canonical_graph_factor_summary(&entry.canonical_id),
            graph_factor_markdown(
                &entry.canonical_id,
                &factor.factor_kind,
                &factor.left_node_id,
                &factor.right_node_id,
                factor.energy_weight,
                relation,
                &factor.parameters,
            ),
        ));
        factor_descriptors.push(format!(
            "- `{}` relation=`{}` factor_kind=`{}` left=`{}` right=`{}` energy_weight={:.2}",
            entry.canonical_id,
            relation,
            factor.factor_kind,
            factor.left_node_id,
            factor.right_node_id,
            factor.energy_weight
        ));
    }

    records.push(document_record(
        WorldModelEntityTaxonomy::canonical_graph_summary_document().to_string(),
        graph_layer_markdown(
            "Accepted Graph",
            WorldModelEntityTaxonomy::canonical_graph_root(),
            &node_descriptors,
            &factor_descriptors,
        ),
    ));

    records
}

fn collect_canonical_spatial_records(
    canonical: &[CanonicalStateEnvelope],
) -> Vec<WorldModelProjectionRecord> {
    let mut records = Vec::new();
    let mut node_positions = HashMap::new();

    for entry in canonical {
        records.push(document_record(
            WorldModelEntityTaxonomy::canonical_lineage(&entry.canonical_id),
            canonical_lineage_markdown(entry),
        ));

        match &entry.body {
            CanonicalBody::Object(body) => {
                if let Some(point) = extract_point_from_value(&body.attributes) {
                    records.push(points_record(
                        WorldModelEntityTaxonomy::canonical_spatial_object(&entry.canonical_id),
                        vec![point],
                    ));
                }
            }
            CanonicalBody::Region(body) => {
                if let Some(point) = extract_point_from_value(&body.bounds)
                    .or_else(|| extract_point_from_value(&body.attributes))
                {
                    records.push(points_record(
                        WorldModelEntityTaxonomy::canonical_spatial_region(&entry.canonical_id),
                        vec![point],
                    ));
                }
            }
            CanonicalBody::GraphNode(body) => {
                if let Some(point) = extract_point_from_value(&body.attributes) {
                    node_positions.insert(entry.canonical_id.clone(), point);
                    records.push(points_record(
                        WorldModelEntityTaxonomy::canonical_spatial_node(&entry.canonical_id),
                        vec![point],
                    ));
                }
            }
            CanonicalBody::GraphFactor(body) => {
                if let Some(point) = extract_point_from_value(&body.parameters) {
                    records.push(points_record(
                        WorldModelEntityTaxonomy::canonical_spatial_factor(&entry.canonical_id),
                        vec![point],
                    ));
                }
            }
        }
    }

    for entry in canonical {
        let CanonicalBody::GraphFactor(factor) = &entry.body else {
            continue;
        };
        let endpoints = node_positions
            .get(&factor.left_node_id)
            .copied()
            .zip(node_positions.get(&factor.right_node_id).copied());
        if let Some((left, right)) = endpoints {
            records.push(line_record(
                WorldModelEntityTaxonomy::canonical_spatial_factor_edge(&entry.canonical_id),
                vec![left, right],
            ));
        }
    }

    records
}

fn collect_coach_records(decisions: &[CoachDecision]) -> Vec<WorldModelProjectionRecord> {
    let mut records = Vec::new();
    let mut counts = HashMap::<&'static str, usize>::new();

    for decision in decisions {
        *counts
            .entry(coach_decision_kind_label(decision.decision_kind))
            .or_default() += 1;
        let (level, color) = coach_decision_level_and_color(decision.decision_kind);
        records.push(WorldModelProjectionRecord {
            entity_path: WorldModelEntityTaxonomy::coach_timeline_kind(decision.decision_kind)
                .to_string(),
            content: WorldModelProjectionContent::LogEvent {
                text: coach_timeline_text(decision),
                level,
                color,
            },
        });
        records.push(document_record(
            WorldModelEntityTaxonomy::coach_decision(&decision.decision_id),
            coach_decision_markdown(decision),
        ));
        for proposal_id in &decision.target_proposal_ids {
            records.push(document_record(
                WorldModelEntityTaxonomy::proposal_coach_event(proposal_id, &decision.decision_id),
                coach_target_markdown(decision, proposal_id),
            ));
        }
        for output_id in &decision.output_ids {
            records.push(document_record(
                WorldModelEntityTaxonomy::canonical_coach_event(output_id, &decision.decision_id),
                coach_output_markdown(decision, output_id),
            ));
        }
    }

    records.push(document_record(
        WorldModelEntityTaxonomy::coach_summary().to_string(),
        coach_summary_markdown(&counts, decisions),
    ));

    records
}

/// Builds the full record stream for one sync slice.
pub fn collect_sync_projection_records(
    tick: u64,
    projection: &SyncProjection<'_>,
) -> Vec<WorldModelProjectionRecord> {
    let namespaces = projection
        .states
        .iter()
        .map(|state| state.namespace.to_string())
        .collect::<BTreeSet<_>>();
    let applied = projection
        .ops
        .iter()
        .filter(|op| op.status == SyncApplyStatus::Applied)
        .count();
    let duplicated = projection
        .ops
        .iter()
        .filter(|op| op.status == SyncApplyStatus::Duplicate)
        .count();
    let rejected = projection
        .ops
        .iter()
        .filter(|op| {
            !matches!(
                op.status,
                SyncApplyStatus::Applied | SyncApplyStatus::Duplicate
            )
        })
        .count();

    let mut records = Vec::new();

    records.push(text_record(
        SyncEntityTaxonomy::summary().to_string(),
        format!(
            "tick={} replicas={} states={} append_entries={} ops={} applied_ops={} namespaces={}",
            tick,
            projection.replicas.len(),
            projection.states.len(),
            projection.append_entries.len(),
            projection.ops.len(),
            applied,
            namespaces.len()
        ),
    ));

    let enabled = projection
        .replicas
        .iter()
        .filter(|replica| replica.enabled)
        .count();
    let trusted = projection
        .replicas
        .iter()
        .filter(|replica| {
            replica.trust_state == qualia_sync_types::ReplicaTrustState::Trusted
        })
        .count();
    records.push(text_record(
        SyncEntityTaxonomy::replicas_root().to_string(),
        format!(
            "count={} enabled={enabled} trusted={trusted}",
            projection.replicas.len()
        ),
    ));
    for replica in projection.replicas {
        records.push(text_record(
            SyncEntityTaxonomy::replica(&replica.replica_id),
            sync_replica_summary_text(replica),
        ));
    }

    records.push(text_record(
        SyncEntityTaxonomy::states_root().to_string(),
        format!(
            "count={} namespaces={}",
            projection.states.len(),
            namespaces.len()
        ),
    ));
    for state in projection.states {
        records.push(text_record(
            SyncEntityTaxonomy::state(state.namespace, &state.key),
            sync_state_summary_text(state),
        ));
    }

    records.push(text_record(
        SyncEntityTaxonomy::append_root().to_string(),
        format!("count={}", projection.append_entries.len()),
    ));
    for entry in projection.append_entries {
        records.push(text_record(
            SyncEntityTaxonomy::append_entry(entry.namespace, &entry.key, &entry.entry_id),
            sync_append_entry_summary_text(entry),
        ));
    }

    records.push(text_record(
        SyncEntityTaxonomy::ops_root().to_string(),
        format!(
            "count={} applied={applied} duplicates={duplicated} rejected={rejected}",
            projection.ops.len()
        ),
    ));
    for op in projection.ops {
        records.push(text_record(
            SyncEntityTaxonomy::op(&op.op_id),
            sync_op_summary_text(op),
        ));
    }

    records
}

/// Builds the markdown document for one replay frame.
pub(crate) fn replay_frame_record(frame: &SessionReplayFrame<'_>) -> WorldModelProjectionRecord {
    document_record(
        ReplayEntityTaxonomy::summary().to_string(),
        replay_frame_markdown(frame),
    )
}
