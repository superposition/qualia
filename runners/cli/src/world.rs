//! World-model payloads and the wire documents exchanged with the agent.
//!
//! The CLI never invents a second representation of a proposal or a decision:
//! it builds the same envelopes the sync layer validates, and it reads back the
//! agent's responses into the same shapes.

use qualia_sync_types::{
    CanonicalBody, CanonicalStateEnvelope, CoachDecision, CoachDecisionKind, HlcTimestamp,
    ObjectProposal, OperationalStateEnvelope, ProposalBody, ProposalEnvelope, ProposalKind,
    ProposalLineage, ProposalStatus, ReplicaRole, WORLD_MODEL_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::{Decision, Proposal};
use crate::platform;

/// The identity the CLI assumes when it writes into the world model.
#[derive(Debug, Clone)]
pub struct ReplicaIdentity {
    pub replica_id: String,
    pub role: ReplicaRole,
    pub display_name: String,
}

/// Resolve the operator identity from a flag, the environment, or the host.
pub fn replica_identity(explicit: Option<String>) -> ReplicaIdentity {
    let host = platform::host_name();
    let replica_id = explicit
        .or_else(|| std::env::var("QUALIA_CLI_REPLICA_ID").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("qualia-cli-{}", slug(host.as_str())));
    let role = std::env::var("QUALIA_CLI_REPLICA_ROLE")
        .ok()
        .and_then(|value| value.parse::<ReplicaRole>().ok())
        .unwrap_or(ReplicaRole::Operator);

    ReplicaIdentity {
        replica_id,
        role,
        display_name: format!("qualia-cli ({host})"),
    }
}

/// Turn a parsed proposal into the envelope the agent accepts.
pub fn proposal_envelope(proposal: Proposal, identity: &ReplicaIdentity) -> ProposalEnvelope {
    match proposal {
        Proposal::NavGoal {
            x_m,
            y_m,
            z_m,
            yaw_rad,
            shared,
        } => ProposalEnvelope {
            schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
            proposal_id: shared
                .proposal_id
                .unwrap_or_else(|| generate_id("proposal", "nav-goal")),
            proposal_kind: ProposalKind::Object,
            source_replica_id: identity.replica_id.clone(),
            source_replica_role: identity.role,
            created_at_hlc: HlcTimestamp::new(platform::now_ns(), 0),
            status: ProposalStatus::Proposed,
            belief_weight: shared.belief_weight,
            source_weight: shared.source_weight,
            mission_relevance: shared.mission_relevance,
            confidence: shared.confidence,
            lineage: ProposalLineage::default(),
            body: ProposalBody::Object(ObjectProposal {
                label: "nav_goal".to_string(),
                attributes: json!({
                    "nav_goal": true,
                    "x_m": x_m,
                    "y_m": y_m,
                    "z_m": z_m,
                    "yaw_rad": yaw_rad,
                    "confidence": shared.confidence,
                }),
                profile_refs: vec!["vector/goal".to_string()],
            }),
        },
        Proposal::Object {
            label,
            x_m,
            y_m,
            z_m,
            yaw_rad,
            marks,
            severity,
            summary,
            shared,
        } => {
            let mut attributes = serde_json::Map::new();
            attributes.insert("x_m".to_string(), json!(x_m));
            attributes.insert("y_m".to_string(), json!(y_m));
            attributes.insert("z_m".to_string(), json!(z_m));
            attributes.insert("yaw_rad".to_string(), json!(yaw_rad));
            attributes.insert("confidence".to_string(), json!(shared.confidence));
            if marks.pose {
                attributes.insert("pose".to_string(), json!(true));
            }
            if marks.nav_goal {
                attributes.insert("nav_goal".to_string(), json!(true));
            }
            if marks.hazard {
                attributes.insert("hazard".to_string(), json!(true));
            }
            if let Some(severity) = severity {
                attributes.insert("severity".to_string(), json!(severity));
            }
            if let Some(summary) = summary {
                attributes.insert("summary".to_string(), json!(summary));
            }

            ProposalEnvelope {
                schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
                proposal_id: shared
                    .proposal_id
                    .unwrap_or_else(|| generate_id("proposal", &label)),
                proposal_kind: ProposalKind::Object,
                source_replica_id: identity.replica_id.clone(),
                source_replica_role: identity.role,
                created_at_hlc: HlcTimestamp::new(platform::now_ns(), 0),
                status: ProposalStatus::Proposed,
                belief_weight: shared.belief_weight,
                source_weight: shared.source_weight,
                mission_relevance: shared.mission_relevance,
                confidence: shared.confidence,
                lineage: ProposalLineage::default(),
                body: ProposalBody::Object(ObjectProposal {
                    label,
                    attributes: Value::Object(attributes),
                    profile_refs: Vec::new(),
                }),
            }
        }
    }
}

/// Turn a parsed decision into the envelope the agent accepts.
pub fn decision_envelope(decision: Decision, identity: &ReplicaIdentity) -> CoachDecision {
    match decision {
        Decision::Promote {
            proposal_id,
            shared,
        } => CoachDecision {
            schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
            decision_id: shared
                .decision_id
                .unwrap_or_else(|| generate_id("decision", "promote")),
            decision_kind: CoachDecisionKind::Promote,
            curator_replica_id: identity.replica_id.clone(),
            curator_replica_role: identity.role,
            created_at_hlc: HlcTimestamp::new(platform::now_ns(), 0),
            target_proposal_ids: vec![proposal_id.clone()],
            output_ids: shared.output_id.into_iter().collect(),
            reason: shared
                .reason
                .or_else(|| Some(format!("promote {proposal_id} from qualia-cli"))),
        },
        Decision::Reject {
            proposal_id,
            shared,
        } => CoachDecision {
            schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
            decision_id: shared
                .decision_id
                .unwrap_or_else(|| generate_id("decision", "reject")),
            decision_kind: CoachDecisionKind::Reject,
            curator_replica_id: identity.replica_id.clone(),
            curator_replica_role: identity.role,
            created_at_hlc: HlcTimestamp::new(platform::now_ns(), 0),
            target_proposal_ids: vec![proposal_id.clone()],
            output_ids: Vec::new(),
            reason: shared
                .reason
                .or_else(|| Some(format!("reject {proposal_id} from qualia-cli"))),
        },
    }
}

/// Enroll this CLI as a loopback operator replica so the agent accepts its
/// writes.
pub fn register_replica(agent_url: &str, identity: &ReplicaIdentity) -> Result<(), String> {
    let registration = ReplicaRegistration {
        replica_id: identity.replica_id.clone(),
        replica_role: identity.role,
        display_name: Some(identity.display_name.clone()),
        endpoint: Some(String::new()),
        trust_state: None,
        enabled: Some(true),
        capabilities: Some(json!({
            "tool": "qualia-cli",
            "commands": ["propose", "decide", "world"],
        })),
        metadata: Some(json!({
            "host": platform::host_name(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        })),
    };
    let _: Value = crate::http::post_json(agent_url, "/sync/replicas", &registration)?;
    Ok(())
}

/// One-line summary of a proposal body, for the non-JSON report.
pub fn describe_proposal(body: &ProposalBody) -> String {
    match body {
        ProposalBody::Object(object) => {
            format!("label={}{}", object.label, position_suffix(&object.attributes))
        }
        ProposalBody::Region(region) => format!(
            "region_kind={}{}",
            region.region_kind,
            position_suffix(&region.bounds)
        ),
        ProposalBody::GraphNode(node) => format!("node_kind={}", node.node_kind),
        ProposalBody::GraphFactor(factor) => format!(
            "factor_kind={} left={} right={}",
            factor.factor_kind, factor.left_node_id, factor.right_node_id
        ),
        ProposalBody::PlannerAdvisory(advisory) => {
            format!("advisory_kind={}", advisory.advisory_kind)
        }
    }
}

/// One-line summary of a canonical body, for the non-JSON report.
pub fn describe_canonical(body: &CanonicalBody) -> String {
    match body {
        CanonicalBody::Object(object) => {
            format!("label={}{}", object.label, position_suffix(&object.attributes))
        }
        CanonicalBody::Region(region) => format!(
            "region_kind={}{}",
            region.region_kind,
            position_suffix(&region.bounds)
        ),
        CanonicalBody::GraphNode(node) => format!("node_kind={}", node.node_kind),
        CanonicalBody::GraphFactor(factor) => format!(
            "factor_kind={} left={} right={}",
            factor.factor_kind, factor.left_node_id, factor.right_node_id
        ),
    }
}

/// Render an `x_m`/`y_m`/`z_m` triple when the attributes carry one.
fn position_suffix(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return String::new();
    };
    let Some(x_m) = map.get("x_m").and_then(Value::as_f64) else {
        return String::new();
    };
    let y_m = map.get("y_m").and_then(Value::as_f64).unwrap_or(0.0);
    let z_m = map.get("z_m").and_then(Value::as_f64).unwrap_or(0.0);
    format!(" @ ({x_m:.2}, {y_m:.2}, {z_m:.2})")
}

/// Build a fresh entity id from a prefix, a human seed and the wall clock.
pub fn generate_id(prefix: &str, seed: &str) -> String {
    format!("{prefix}/{}-{}", slug(seed), platform::now_ns())
}

/// Fold arbitrary text into a lowercase, dash-separated path segment.
fn slug(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            pending_dash = false;
        } else if !pending_dash {
            out.push('-');
            pending_dash = true;
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "item".to_string()
    } else {
        out.to_string()
    }
}

// ---------------------------------------------------------------------------
// Wire documents
// ---------------------------------------------------------------------------

/// The capability request the compute service answers.
#[derive(Debug, Serialize)]
pub struct ComputeRequestMeta {
    pub schema_version: String,
    pub request_id: String,
    pub request_type: String,
    pub timestamp_ns: u64,
}

#[derive(Debug, Deserialize)]
pub struct ComputeCapabilities {
    pub service_instance: String,
    pub healthy: bool,
    pub planner_algorithms: Vec<String>,
    pub cuda: CudaInfo,
}

#[derive(Debug, Deserialize)]
pub struct CudaInfo {
    pub device_name: String,
    pub sm: u32,
    pub status: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReplicaRegistration {
    replica_id: String,
    replica_role: ReplicaRole,
    display_name: Option<String>,
    endpoint: Option<String>,
    trust_state: Option<String>,
    enabled: Option<bool>,
    capabilities: Option<Value>,
    metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProposalResponse {
    pub proposal: ProposalEnvelope,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub decision: CoachDecision,
    pub canonical: Option<CanonicalStateEnvelope>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProposalsResponse {
    pub proposals: Vec<ProposalEnvelope>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CanonicalResponse {
    pub canonical: Vec<CanonicalStateEnvelope>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperationalResponse {
    pub operational: OperationalStateEnvelope,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecisionsResponse {
    pub decisions: Vec<CoachDecision>,
}

/// The `world --json` document.
#[derive(Debug, Serialize)]
pub struct WorldSnapshot {
    pub agent_url: String,
    pub proposals: Vec<ProposalEnvelope>,
    pub canonical: Vec<CanonicalStateEnvelope>,
    pub operational: OperationalStateEnvelope,
}

/// The `decisions --json` document.
#[derive(Debug, Serialize)]
pub struct DecisionList {
    pub agent_url: String,
    pub decisions: Vec<CoachDecision>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ReplicaIdentity {
        ReplicaIdentity {
            replica_id: "operator-a".to_string(),
            role: ReplicaRole::Operator,
            display_name: "operator-a".to_string(),
        }
    }

    #[test]
    fn slug_collapses_separators_and_falls_back() {
        assert_eq!(slug("Nav Goal"), "nav-goal");
        assert_eq!(slug("  spaced//out  "), "spaced-out");
        assert_eq!(slug("crate"), "crate");
        assert_eq!(slug("!!!"), "item");
    }

    #[test]
    fn generated_ids_are_prefixed_and_time_suffixed() {
        let proposal = generate_id("proposal", "nav-goal");
        assert!(proposal.starts_with("proposal/nav-goal-"), "{proposal}");
        assert!(
            proposal["proposal/nav-goal-".len()..].parse::<u64>().is_ok(),
            "{proposal}"
        );

        let decision = generate_id("decision", "Promote");
        assert!(decision.starts_with("decision/promote-"), "{decision}");
        assert!(
            decision["decision/promote-".len()..].parse::<u64>().is_ok(),
            "{decision}"
        );
    }

    #[test]
    fn nav_goal_is_an_operational_object_proposal() {
        let envelope = proposal_envelope(
            Proposal::NavGoal {
                x_m: 4.0,
                y_m: 0.0,
                z_m: 2.0,
                yaw_rad: 0.1,
                shared: crate::cli::ProposalShared {
                    proposal_id: Some("proposal/nav-goal-1".to_string()),
                    ..Default::default()
                },
            },
            &identity(),
        );

        assert_eq!(envelope.proposal_id, "proposal/nav-goal-1");
        assert_eq!(envelope.proposal_kind, ProposalKind::Object);
        assert_eq!(envelope.status, ProposalStatus::Proposed);
        assert_eq!(envelope.source_replica_role, ReplicaRole::Operator);
        envelope.validate_shape().expect("envelope is well formed");

        let ProposalBody::Object(body) = envelope.body else {
            panic!("nav goal must travel as an object proposal");
        };
        assert_eq!(body.label, "nav_goal");
        assert_eq!(body.profile_refs, vec!["vector/goal".to_string()]);
        let attributes = body.attributes.as_object().expect("attributes");
        assert_eq!(attributes.get("nav_goal").and_then(Value::as_bool), Some(true));
        assert_eq!(attributes.get("x_m").and_then(Value::as_f64), Some(4.0));
        assert_eq!(attributes.get("z_m").and_then(Value::as_f64), Some(2.0));
    }

    #[test]
    fn object_proposal_keeps_the_scene_marks_it_was_given() {
        let envelope = proposal_envelope(
            Proposal::Object {
                label: "crate".to_string(),
                x_m: 1.0,
                y_m: 0.0,
                z_m: 3.0,
                yaw_rad: 0.0,
                marks: crate::cli::ObjectMarks {
                    pose: false,
                    nav_goal: false,
                    hazard: true,
                },
                severity: Some(0.7),
                summary: Some("blocked aisle".to_string()),
                shared: crate::cli::ProposalShared {
                    proposal_id: Some("proposal/crate-1".to_string()),
                    ..Default::default()
                },
            },
            &identity(),
        );

        let ProposalBody::Object(body) = envelope.body else {
            panic!("object proposal expected");
        };
        let attributes = body.attributes.as_object().expect("attributes");
        assert_eq!(attributes.get("hazard").and_then(Value::as_bool), Some(true));
        assert_eq!(attributes.get("pose"), None);
        assert_eq!(attributes.get("nav_goal"), None);
        assert_eq!(
            attributes.get("summary").and_then(Value::as_str),
            Some("blocked aisle")
        );
        let severity = attributes
            .get("severity")
            .and_then(Value::as_f64)
            .expect("severity");
        assert!((severity - 0.7).abs() < 1e-3);
    }

    #[test]
    fn decisions_default_their_reason_to_the_verb_and_id() {
        let promote = decision_envelope(
            Decision::Promote {
                proposal_id: "proposal/nav-goal-1".to_string(),
                shared: Default::default(),
            },
            &identity(),
        );
        assert_eq!(promote.decision_kind, CoachDecisionKind::Promote);
        assert_eq!(promote.target_proposal_ids, vec!["proposal/nav-goal-1"]);
        assert_eq!(
            promote.reason.as_deref(),
            Some("promote proposal/nav-goal-1 from qualia-cli")
        );
        promote.validate_shape().expect("decision is well formed");

        let reject = decision_envelope(
            Decision::Reject {
                proposal_id: "proposal/crate-1".to_string(),
                shared: Default::default(),
            },
            &identity(),
        );
        assert_eq!(reject.decision_kind, CoachDecisionKind::Reject);
        assert!(reject.output_ids.is_empty());
        assert_eq!(
            reject.reason.as_deref(),
            Some("reject proposal/crate-1 from qualia-cli")
        );
    }

    #[test]
    fn explicit_ids_and_reasons_are_preserved() {
        let decision = decision_envelope(
            Decision::Reject {
                proposal_id: "proposal/crate-1".to_string(),
                shared: crate::cli::DecisionShared {
                    decision_id: Some("decision/fixed".to_string()),
                    reason: Some("superseded".to_string()),
                    ..Default::default()
                },
            },
            &identity(),
        );
        assert_eq!(decision.decision_id, "decision/fixed");
        assert_eq!(decision.reason.as_deref(), Some("superseded"));
    }

    #[test]
    fn body_summaries_render_positions_when_present() {
        let object = ProposalBody::Object(ObjectProposal {
            label: "crate".to_string(),
            attributes: json!({"x_m": 1.5, "y_m": 2.0, "z_m": 3.25}),
            profile_refs: Vec::new(),
        });
        assert_eq!(describe_proposal(&object), "label=crate @ (1.50, 2.00, 3.25)");

        let flat = ProposalBody::Object(ObjectProposal {
            label: "crate".to_string(),
            attributes: json!({"summary": "no position"}),
            profile_refs: Vec::new(),
        });
        assert_eq!(describe_proposal(&flat), "label=crate");
    }

    #[test]
    fn explicit_replica_id_wins_over_the_host_default() {
        let identity = replica_identity(Some("operator-a".to_string()));
        assert_eq!(identity.replica_id, "operator-a");
        assert!(identity.display_name.starts_with("qualia-cli ("));
    }
}
