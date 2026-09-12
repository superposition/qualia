//! The envelopes: the model's draft becomes the repository's own types, and the
//! repository's own validators are what accept or refuse it.
//!
//! Two conversions, both grounded in `qualia-sync-types`:
//!
//! - the draft becomes a [`CoachDecision`] and is checked by
//!   [`CoachDecision::validate_shape`]; a `promote` is then run through
//!   [`promote_proposal_to_canonical`], which is the rule that a promote must
//!   target exactly its proposal and that a planner advisory can never be
//!   canonical (`crates/sync-types/src/world_model.rs:862-880`);
//! - the decision's mission draft becomes a [`MissionEnvelopeV1`], whose
//!   evidence comes from the proposal's own lineage and whose numbers are
//!   checked by [`MissionEnvelopeV1::validate`] before anything is posted.
//!
//! Nothing here invents a value the agent would have to trust: a decision whose
//! targets are not the proposals the broker holds, or a promote whose proposal
//! carries no lineage evidence, is refused with the reason rather than posted.

use qualia_sync_types::{
    promote_proposal_to_canonical, CanonicalStateEnvelope, CoachDecision, CoachDecisionKind,
    HlcTimestamp, MissionCommandV1, MissionConstraintsV1, MissionEnvelopeV1, MissionObjectiveKindV1,
    MissionObjectiveV1, ProposalEnvelope, ReplicaRole, MISSION_ENVELOPE_SCHEMA_VERSION,
    WORLD_MODEL_SCHEMA_VERSION,
};

use crate::config::{
    Area, DEFAULT_EVIDENCE_MAX_AGE_MS, DEFAULT_MAX_DISTANCE_M, DEFAULT_MAX_REPLANS,
    DEFAULT_MAX_RUNTIME_MS, DEFAULT_SPEED_CEILING_MPS,
};
use crate::coach::DecisionDraft;

/// Everything the broker, not the model, owns in a mission envelope.
#[derive(Debug, Clone)]
pub struct MissionRequest<'a> {
    pub broker_id: &'a str,
    pub producer_epoch: u64,
    pub sequence: u64,
    pub mission_id: &'a str,
    pub idempotency_key: &'a str,
    pub fly_governed: bool,
    pub area: &'a Area,
    pub now_ms: u128,
}

impl MissionRequest<'_> {
    /// The constraint member the model may bound but not exceed.
    pub fn constraints(&self, draft: &crate::coach::MissionDraft) -> MissionConstraintsV1 {
        MissionConstraintsV1 {
            operating_area: self.area.envelope(),
            speed_ceiling_mps: draft.speed_ceiling_mps.unwrap_or(DEFAULT_SPEED_CEILING_MPS),
            max_distance_m: draft.max_distance_m.unwrap_or(DEFAULT_MAX_DISTANCE_M),
            max_runtime_ms: draft.max_runtime_ms.unwrap_or(DEFAULT_MAX_RUNTIME_MS),
            max_replans: draft.max_replans.unwrap_or(DEFAULT_MAX_REPLANS),
            evidence_max_age_ms: draft
                .evidence_max_age_ms
                .unwrap_or(DEFAULT_EVIDENCE_MAX_AGE_MS),
        }
    }
}

/// The broker's identity inside the world model: a curator decision is made
/// with host or operator authority (`world_model.rs:952-955`).
pub const CURATOR_ROLE: ReplicaRole = ReplicaRole::Operator;

/// Build the repository's decision from the model's draft, refusing any target
/// the broker does not hold.
pub fn decision_from_draft(
    draft: &DecisionDraft,
    curator_id: &str,
    held_proposal_ids: &[String],
    decision_id: &str,
    now_ms: u128,
) -> Result<CoachDecision, String> {
    for target in &draft.target_proposal_ids {
        if !held_proposal_ids.iter().any(|held| held == target) {
            return Err(format!(
                "the model targeted {target}, which is not a proposal this broker holds"
            ));
        }
    }
    let decision = CoachDecision {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        decision_id: decision_id.to_string(),
        decision_kind: draft.decision_kind,
        curator_replica_id: curator_id.to_string(),
        curator_replica_role: CURATOR_ROLE,
        created_at_hlc: HlcTimestamp::new(now_ms as u64 * 1_000_000, 0),
        target_proposal_ids: draft.target_proposal_ids.clone(),
        output_ids: draft.output_ids.clone(),
        reason: draft.reason.clone(),
    };
    decision.validate_shape()?;
    Ok(decision)
}

/// Materialize a promote through the repository's own rule.
pub fn materialize(
    proposal: &ProposalEnvelope,
    decision: &CoachDecision,
) -> Result<CanonicalStateEnvelope, String> {
    promote_proposal_to_canonical(proposal, decision)
}

/// Build the mission envelope one promote decision justifies.
///
/// Evidence is the proposal's own lineage: a promote with no evidence does not
/// open a mission, because a `start` without an evidence reference is not one
/// the agent accepts (`crates/sync-types/src/mission.rs`).
pub fn mission_from_decision(
    decision: &CoachDecision,
    draft: &DecisionDraft,
    proposal: &ProposalEnvelope,
    request: &MissionRequest<'_>,
) -> Result<MissionEnvelopeV1, String> {
    if decision.decision_kind != CoachDecisionKind::Promote {
        return Err(format!(
            "the broker opens a mission only from a promote decision; {:?} needs none",
            decision.decision_kind
        ));
    }
    let mission = draft
        .mission
        .as_ref()
        .ok_or_else(|| "the promote decision carried no mission draft".to_string())?;
    let mut evidence_refs: Vec<String> = proposal
        .lineage
        .evidence_refs
        .iter()
        .map(|reference| reference.trim().to_string())
        .filter(|reference| !reference.is_empty() && reference.len() <= 256)
        .collect();
    evidence_refs.dedup();
    if evidence_refs.is_empty() {
        return Err(format!(
            "promote requires lineage evidence: proposal {} carries no lineage.evidence_refs",
            proposal.proposal_id
        ));
    }
    evidence_refs.truncate(64);

    let kind = objective_kind(mission.objective_kind.as_deref());
    let summary = mission
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(str::to_string)
        .or_else(|| decision.reason.as_deref().map(str::trim).filter(|reason| !reason.is_empty()).map(str::to_string))
        .ok_or_else(|| "the promote decision carried neither a mission summary nor a reason".to_string())?;
    let summary = bounded(&summary, 512);

    let constraints = request.constraints(mission);
    let envelope = MissionEnvelopeV1 {
        schema_version: MISSION_ENVELOPE_SCHEMA_VERSION.to_string(),
        broker_id: request.broker_id.to_string(),
        producer_epoch: request.producer_epoch,
        sequence: request.sequence,
        mission_id: request.mission_id.to_string(),
        idempotency_key: request.idempotency_key.to_string(),
        command: MissionCommandV1::Start,
        issued_at_ms: request.now_ms,
        // The deadline is the mission's own runtime bound, read from the
        // broker's clock at build time; the agent refuses one already past and
        // one more than 120 s out.
        deadline_ms: request.now_ms + constraints.max_runtime_ms as u128,
        objective: MissionObjectiveV1 {
            kind,
            summary,
            target_x_m: mission.target_x_m,
            target_y_m: mission.target_y_m,
            tolerance_m: mission.tolerance_m,
        },
        constraints,
        evidence_refs,
        fly_governed: request.fly_governed,
    };
    envelope.validate()?;
    Ok(envelope)
}

fn objective_kind(kind: Option<&str>) -> MissionObjectiveKindV1 {
    match kind.unwrap_or_default() {
        "observe_point" => MissionObjectiveKindV1::ObservePoint,
        "navigate_to" => MissionObjectiveKindV1::NavigateTo,
        _ => MissionObjectiveKindV1::ExploreFrontier,
    }
}

/// Truncate on a character boundary, so a multi-byte summary cannot panic.
fn bounded(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}
