mod cognition;
mod mission;
mod spatial;
mod world_model;
pub use cognition::*;
pub use mission::*;
pub use spatial::*;
pub use world_model::*;

use qualia_types::STATE_DIM;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub const SYNC_SCHEMA_VERSION: &str = "sync.v1";
pub const WEIGHT_TILE_SIZE: usize = 32;
pub const WEIGHT_TILE_VALUE_COUNT: usize = WEIGHT_TILE_SIZE * WEIGHT_TILE_SIZE;

/// Hybrid logical clock stamp: wall-clock nanoseconds plus a logical counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HlcTimestamp {
    pub physical_ns: u64,
    pub logical: u32,
}

impl HlcTimestamp {
    pub fn new(physical_ns: u64, logical: u32) -> Self {
        Self {
            physical_ns,
            logical,
        }
    }

    pub fn add_ms(self, lease_duration_ms: u64) -> Self {
        Self {
            physical_ns: self
                .physical_ns
                .saturating_add(lease_duration_ms.saturating_mul(1_000_000)),
            logical: self.logical,
        }
    }
}

impl Display for HlcTimestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.physical_ns, self.logical)
    }
}

impl FromStr for HlcTimestamp {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split(':');
        let physical_ns = parts
            .next()
            .ok_or_else(|| "missing physical_ns".to_string())?
            .parse::<u64>()
            .map_err(|_| "invalid physical_ns".to_string())?;
        let logical = parts
            .next()
            .ok_or_else(|| "missing logical".to_string())?
            .parse::<u32>()
            .map_err(|_| "invalid logical".to_string())?;
        if parts.next().is_some() {
            return Err("unexpected trailing timestamp components".to_string());
        }
        Ok(Self {
            physical_ns,
            logical,
        })
    }
}

impl Serialize for HlcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for HlcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplicaRole {
    Jetson,
    Host,
    Sensor,
    Operator,
    Unknown,
}

impl FromStr for ReplicaRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "jetson" => Ok(Self::Jetson),
            "host" => Ok(Self::Host),
            "sensor" => Ok(Self::Sensor),
            "operator" => Ok(Self::Operator),
            "unknown" => Ok(Self::Unknown),
            _ => Err(format!("unknown replica role: {value}")),
        }
    }
}

impl Display for ReplicaRole {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ReplicaRole::Jetson => "jetson",
            ReplicaRole::Host => "host",
            ReplicaRole::Sensor => "sensor",
            ReplicaRole::Operator => "operator",
            ReplicaRole::Unknown => "unknown",
        };
        write!(f, "{text}")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplicaTrustState {
    Trusted,
    Disabled,
}

impl ReplicaTrustState {
    pub fn allows_sync(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

impl Display for ReplicaTrustState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ReplicaTrustState::Trusted => "trusted",
            ReplicaTrustState::Disabled => "disabled",
        };
        write!(f, "{text}")
    }
}

impl FromStr for ReplicaTrustState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "trusted" => Ok(Self::Trusted),
            "disabled" => Ok(Self::Disabled),
            _ => Err(format!("unknown replica trust state: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SyncNamespace {
    Directive,
    Activity,
    SceneText,
    Pose,
    NavGoalAcceptance,
    Object,
    WorldRegion,
    GraphNode,
    GraphFactor,
    MergeCandidate,
    BeliefSnapshot,
    MessageSnapshot,
    Thought,
    Lore,
    PlannerAdvisory,
    SceneEmbedding,
    WeightTile,
    WorldModelProposal,
    WorldModelDecision,
    WorldModelCanonical,
    BeliefEvidence,
    BeliefAssertion,
    CognitionSnapshot,
    CognitionCandidate,
    CognitionEvaluation,
    CognitionPromotion,
    ComputeAdvisory,
}

impl Display for SyncNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            SyncNamespace::Directive => "directive",
            SyncNamespace::Activity => "activity",
            SyncNamespace::SceneText => "scene_text",
            SyncNamespace::Pose => "pose",
            SyncNamespace::NavGoalAcceptance => "nav_goal_acceptance",
            SyncNamespace::Object => "object",
            SyncNamespace::WorldRegion => "world_region",
            SyncNamespace::GraphNode => "graph_node",
            SyncNamespace::GraphFactor => "graph_factor",
            SyncNamespace::MergeCandidate => "merge_candidate",
            SyncNamespace::BeliefSnapshot => "belief_snapshot",
            SyncNamespace::MessageSnapshot => "message_snapshot",
            SyncNamespace::Thought => "thought",
            SyncNamespace::Lore => "lore",
            SyncNamespace::PlannerAdvisory => "planner_advisory",
            SyncNamespace::SceneEmbedding => "scene_embedding",
            SyncNamespace::WeightTile => "weight_tile",
            SyncNamespace::WorldModelProposal => "world_model_proposal",
            SyncNamespace::WorldModelDecision => "world_model_decision",
            SyncNamespace::WorldModelCanonical => "world_model_canonical",
            SyncNamespace::BeliefEvidence => "belief_evidence",
            SyncNamespace::BeliefAssertion => "belief_assertion",
            SyncNamespace::CognitionSnapshot => "cognition_snapshot",
            SyncNamespace::CognitionCandidate => "cognition_candidate",
            SyncNamespace::CognitionEvaluation => "cognition_evaluation",
            SyncNamespace::CognitionPromotion => "cognition_promotion",
            SyncNamespace::ComputeAdvisory => "compute_advisory",
        };
        write!(f, "{text}")
    }
}

impl FromStr for SyncNamespace {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "directive" => Ok(Self::Directive),
            "activity" => Ok(Self::Activity),
            "scene_text" => Ok(Self::SceneText),
            "pose" => Ok(Self::Pose),
            "nav_goal_acceptance" => Ok(Self::NavGoalAcceptance),
            "object" => Ok(Self::Object),
            "world_region" => Ok(Self::WorldRegion),
            "graph_node" => Ok(Self::GraphNode),
            "graph_factor" => Ok(Self::GraphFactor),
            "merge_candidate" => Ok(Self::MergeCandidate),
            "belief_snapshot" => Ok(Self::BeliefSnapshot),
            "message_snapshot" => Ok(Self::MessageSnapshot),
            "thought" => Ok(Self::Thought),
            "lore" => Ok(Self::Lore),
            "planner_advisory" => Ok(Self::PlannerAdvisory),
            "scene_embedding" => Ok(Self::SceneEmbedding),
            "weight_tile" => Ok(Self::WeightTile),
            "world_model_proposal" => Ok(Self::WorldModelProposal),
            "world_model_decision" => Ok(Self::WorldModelDecision),
            "world_model_canonical" => Ok(Self::WorldModelCanonical),
            "belief_evidence" => Ok(Self::BeliefEvidence),
            "belief_assertion" => Ok(Self::BeliefAssertion),
            "cognition_snapshot" => Ok(Self::CognitionSnapshot),
            "cognition_candidate" => Ok(Self::CognitionCandidate),
            "cognition_evaluation" => Ok(Self::CognitionEvaluation),
            "cognition_promotion" => Ok(Self::CognitionPromotion),
            "compute_advisory" => Ok(Self::ComputeAdvisory),
            _ => Err(format!("unknown namespace: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStateKind {
    Register,
    OrMap,
    AppendOnly,
    Embedding,
    WeightTile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncShmTarget {
    None,
    Directive,
    Activity,
    SceneText,
    Pose,
    NavGoal,
    Objects,
    SceneEmbedding,
    ThoughtBuffer,
    LoreBuffer,
    WeightTile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NamespaceAuthority {
    AnyTrusted,
    JetsonOnly,
    CuratorOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamespacePolicy {
    pub state_kind: SyncStateKind,
    pub shm_target: SyncShmTarget,
    pub authority: NamespaceAuthority,
}

impl NamespacePolicy {
    pub fn validate_authority(
        self,
        replica_role: ReplicaRole,
        trust_state: ReplicaTrustState,
    ) -> SyncResult<()> {
        if !trust_state.allows_sync() {
            return Err(SyncError::rejected_authority(
                "replica is not trusted for sync.v1 writes",
            ));
        }
        match self.authority {
            NamespaceAuthority::AnyTrusted => Ok(()),
            NamespaceAuthority::JetsonOnly if replica_role == ReplicaRole::Jetson => Ok(()),
            NamespaceAuthority::JetsonOnly => Err(SyncError::rejected_authority(
                "namespace is Jetson-authoritative",
            )),
            NamespaceAuthority::CuratorOnly
                if matches!(replica_role, ReplicaRole::Host | ReplicaRole::Operator) =>
            {
                Ok(())
            }
            NamespaceAuthority::CuratorOnly => Err(SyncError::rejected_authority(
                "namespace requires host or operator authority",
            )),
        }
    }
}

pub fn namespace_policy(namespace: SyncNamespace) -> NamespacePolicy {
    match namespace {
        SyncNamespace::Directive => NamespacePolicy {
            state_kind: SyncStateKind::Register,
            shm_target: SyncShmTarget::Directive,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::Activity => NamespacePolicy {
            state_kind: SyncStateKind::Register,
            shm_target: SyncShmTarget::Activity,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::SceneText => NamespacePolicy {
            state_kind: SyncStateKind::Register,
            shm_target: SyncShmTarget::SceneText,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::Pose => NamespacePolicy {
            state_kind: SyncStateKind::Register,
            shm_target: SyncShmTarget::Pose,
            authority: NamespaceAuthority::JetsonOnly,
        },
        SyncNamespace::NavGoalAcceptance => NamespacePolicy {
            state_kind: SyncStateKind::Register,
            shm_target: SyncShmTarget::NavGoal,
            authority: NamespaceAuthority::JetsonOnly,
        },
        SyncNamespace::Object => NamespacePolicy {
            state_kind: SyncStateKind::OrMap,
            shm_target: SyncShmTarget::Objects,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::WorldRegion
        | SyncNamespace::GraphNode
        | SyncNamespace::GraphFactor
        | SyncNamespace::MergeCandidate
        | SyncNamespace::BeliefSnapshot
        | SyncNamespace::BeliefAssertion
        | SyncNamespace::CognitionSnapshot => NamespacePolicy {
            state_kind: SyncStateKind::OrMap,
            shm_target: SyncShmTarget::None,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::MessageSnapshot
        | SyncNamespace::PlannerAdvisory
        | SyncNamespace::BeliefEvidence
        | SyncNamespace::CognitionCandidate
        | SyncNamespace::CognitionEvaluation
        | SyncNamespace::CognitionPromotion
        | SyncNamespace::ComputeAdvisory => NamespacePolicy {
            state_kind: SyncStateKind::AppendOnly,
            shm_target: SyncShmTarget::None,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::Thought => NamespacePolicy {
            state_kind: SyncStateKind::AppendOnly,
            shm_target: SyncShmTarget::ThoughtBuffer,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::Lore => NamespacePolicy {
            state_kind: SyncStateKind::AppendOnly,
            shm_target: SyncShmTarget::LoreBuffer,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::SceneEmbedding => NamespacePolicy {
            state_kind: SyncStateKind::Embedding,
            shm_target: SyncShmTarget::SceneEmbedding,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::WeightTile => NamespacePolicy {
            state_kind: SyncStateKind::WeightTile,
            shm_target: SyncShmTarget::WeightTile,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::WorldModelProposal => NamespacePolicy {
            state_kind: SyncStateKind::OrMap,
            shm_target: SyncShmTarget::None,
            authority: NamespaceAuthority::AnyTrusted,
        },
        SyncNamespace::WorldModelDecision => NamespacePolicy {
            state_kind: SyncStateKind::AppendOnly,
            shm_target: SyncShmTarget::None,
            authority: NamespaceAuthority::CuratorOnly,
        },
        SyncNamespace::WorldModelCanonical => NamespacePolicy {
            state_kind: SyncStateKind::OrMap,
            shm_target: SyncShmTarget::None,
            authority: NamespaceAuthority::CuratorOnly,
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncEnvelope {
    pub schema_version: String,
    pub replica_id: String,
    pub replica_role: ReplicaRole,
    pub run_id: String,
    pub counter: u64,
    pub timestamp_hlc: HlcTimestamp,
    pub namespace: SyncNamespace,
    pub key: String,
    pub body: SyncBody,
}

impl SyncEnvelope {
    pub fn op_id(&self) -> String {
        format!("{}/{}/{}", self.replica_id, self.run_id, self.counter)
    }

    pub fn validate_shape(&self) -> SyncResult<()> {
        if self.schema_version != SYNC_SCHEMA_VERSION {
            return Err(SyncError::validation(format!(
                "unsupported schema_version: {}",
                self.schema_version
            )));
        }
        for (field, value) in [
            ("replica_id", self.replica_id.as_str()),
            ("run_id", self.run_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(SyncError::validation(format!("{field} must not be empty")));
            }
            if value.contains('/') {
                return Err(SyncError::validation(format!(
                    "{field} must not contain '/'"
                )));
            }
        }
        if self.key.trim().is_empty() {
            return Err(SyncError::validation("key must not be empty"));
        }
        let policy = namespace_policy(self.namespace);
        if policy.state_kind != self.body.state_kind() {
            return Err(SyncError::validation(format!(
                "body kind {:?} does not match namespace {}",
                self.body.state_kind(),
                self.namespace
            )));
        }
        self.body.validate(self.namespace)
    }

    pub fn validate_authority(
        &self,
        replica_role: ReplicaRole,
        trust_state: ReplicaTrustState,
    ) -> SyncResult<()> {
        namespace_policy(self.namespace).validate_authority(replica_role, trust_state)
    }

    pub fn validate(&self) -> SyncResult<()> {
        self.validate_shape()?;
        self.validate_authority(self.replica_role, ReplicaTrustState::Trusted)
    }
}

pub fn validate_namespace_authority(
    namespace: SyncNamespace,
    replica_role: ReplicaRole,
    trust_state: ReplicaTrustState,
) -> SyncResult<()> {
    namespace_policy(namespace).validate_authority(replica_role, trust_state)
}

pub fn validate_registered_replica(
    namespace: SyncNamespace,
    claimed_role: ReplicaRole,
    registered_role: ReplicaRole,
    trust_state: ReplicaTrustState,
) -> SyncResult<()> {
    if !trust_state.allows_sync() {
        return Err(SyncError::rejected_authority(format!(
            "replica trust state for namespace {} is {}",
            namespace, trust_state
        )));
    }
    if claimed_role != registered_role {
        return Err(SyncError::rejected_authority(format!(
            "claimed replica role {} does not match registered role {}",
            claimed_role, registered_role
        )));
    }
    validate_namespace_authority(namespace, registered_role, trust_state)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum SyncBody {
    Register(SyncRegisterValue),
    OrMap(SyncOrMapOp),
    AppendOnly(SyncAppendOnlyEntry),
    Embedding(SyncEmbeddingOp),
    WeightTile(SyncWeightTileDelta),
}

impl SyncBody {
    pub fn state_kind(&self) -> SyncStateKind {
        match self {
            SyncBody::Register(_) => SyncStateKind::Register,
            SyncBody::OrMap(_) => SyncStateKind::OrMap,
            SyncBody::AppendOnly(_) => SyncStateKind::AppendOnly,
            SyncBody::Embedding(_) => SyncStateKind::Embedding,
            SyncBody::WeightTile(_) => SyncStateKind::WeightTile,
        }
    }

    pub fn validate(&self, namespace: SyncNamespace) -> SyncResult<()> {
        match self {
            SyncBody::Register(body) => body.validate(namespace),
            SyncBody::OrMap(body) => body.validate(namespace),
            SyncBody::AppendOnly(body) => body.validate(namespace),
            SyncBody::Embedding(body) => body.validate(),
            SyncBody::WeightTile(body) => body.validate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncRegisterValue {
    pub value: Value,
    pub lease_duration_ms: Option<u64>,
}

impl SyncRegisterValue {
    fn validate(&self, namespace: SyncNamespace) -> SyncResult<()> {
        match namespace {
            SyncNamespace::Directive | SyncNamespace::Activity | SyncNamespace::SceneText => {
                if self.value.as_str().is_none() {
                    return Err(SyncError::validation(format!(
                        "{} register value must be a string",
                        namespace
                    )));
                }
            }
            SyncNamespace::Pose => {
                serde_json::from_value::<SyncNavPose>(self.value.clone()).map_err(|error| {
                    SyncError::validation(format!("invalid pose payload: {error}"))
                })?;
            }
            SyncNamespace::NavGoalAcceptance => {
                serde_json::from_value::<SyncNavGoal>(self.value.clone()).map_err(|error| {
                    SyncError::validation(format!("invalid nav goal payload: {error}"))
                })?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OrMapAction {
    Upsert {
        value: Value,
        replaces_tags: Vec<String>,
    },
    Remove {
        remove_tags: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncOrMapOp {
    pub action: OrMapAction,
}

impl SyncOrMapOp {
    fn validate(&self, namespace: SyncNamespace) -> SyncResult<()> {
        match &self.action {
            OrMapAction::Upsert { value, .. } => {
                if value.is_null() {
                    return Err(SyncError::validation(
                        "or-map upsert value must not be null",
                    ));
                }
                match namespace {
                    SyncNamespace::WorldModelProposal => {
                        serde_json::from_value::<ProposalEnvelope>(value.clone())
                            .map_err(|error| {
                                SyncError::validation(format!(
                                    "invalid world-model proposal payload: {error}"
                                ))
                            })?
                            .validate_shape()
                            .map_err(SyncError::validation)?
                    }
                    SyncNamespace::WorldModelCanonical => {
                        serde_json::from_value::<CanonicalStateEnvelope>(value.clone())
                            .map_err(|error| {
                                SyncError::validation(format!(
                                    "invalid world-model canonical payload: {error}"
                                ))
                            })?
                            .validate_shape()
                            .map_err(SyncError::validation)?
                    }
                    SyncNamespace::BeliefAssertion => {
                        serde_json::from_value::<BeliefAssertionV1>(value.clone())
                            .map_err(|error| {
                                SyncError::validation(format!(
                                    "invalid belief assertion payload: {error}"
                                ))
                            })?
                            .validate()
                            .map_err(SyncError::validation)?
                    }
                    SyncNamespace::CognitionSnapshot => {
                        serde_json::from_value::<CognitionPeerSnapshotV1>(value.clone())
                            .map_err(|error| {
                                SyncError::validation(format!(
                                    "invalid cognition snapshot payload: {error}"
                                ))
                            })?
                            .validate()
                            .map_err(SyncError::validation)?
                    }
                    _ => {}
                }
            }
            OrMapAction::Remove { remove_tags } => {
                if remove_tags.is_empty() {
                    return Err(SyncError::validation(
                        "or-map remove must include at least one observed tag",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncAppendOnlyEntry {
    pub entry_id: String,
    pub value: Value,
}

impl SyncAppendOnlyEntry {
    fn validate(&self, namespace: SyncNamespace) -> SyncResult<()> {
        if self.entry_id.trim().is_empty() {
            return Err(SyncError::validation(
                "append-only entry_id must not be empty",
            ));
        }
        let payload_error = |error: serde_json::Error, label: &str| {
            SyncError::validation(format!("invalid {label} payload: {error}"))
        };
        match namespace {
            SyncNamespace::Thought => {
                serde_json::from_value::<SyncThoughtValue>(self.value.clone())
                    .map_err(|error| payload_error(error, "thought"))?;
            }
            SyncNamespace::Lore => {
                serde_json::from_value::<SyncLoreValue>(self.value.clone())
                    .map_err(|error| payload_error(error, "lore"))?;
            }
            SyncNamespace::WorldModelDecision => {
                serde_json::from_value::<CoachDecision>(self.value.clone())
                    .map_err(|error| payload_error(error, "world-model decision"))?
                    .validate_shape()
                    .map_err(SyncError::validation)?;
            }
            SyncNamespace::BeliefEvidence => {
                serde_json::from_value::<BeliefEvidenceV1>(self.value.clone())
                    .map_err(|error| payload_error(error, "belief evidence"))?
                    .validate()
                    .map_err(SyncError::validation)?;
            }
            SyncNamespace::CognitionCandidate => {
                serde_json::from_value::<CognitionCandidateDeltaV1>(self.value.clone())
                    .map_err(|error| payload_error(error, "cognition candidate"))?
                    .validate()
                    .map_err(SyncError::validation)?;
            }
            SyncNamespace::CognitionEvaluation => {
                serde_json::from_value::<CognitionCandidateEvaluationV1>(self.value.clone())
                    .map_err(|error| payload_error(error, "cognition evaluation"))?
                    .validate()
                    .map_err(SyncError::validation)?;
            }
            SyncNamespace::CognitionPromotion => {
                serde_json::from_value::<CognitionPromotionReceiptV1>(self.value.clone())
                    .map_err(|error| payload_error(error, "cognition promotion"))?
                    .validate()
                    .map_err(SyncError::validation)?;
            }
            SyncNamespace::ComputeAdvisory => {
                serde_json::from_value::<ComputeAdvisoryV1>(self.value.clone())
                    .map_err(|error| payload_error(error, "compute advisory"))?
                    .validate()
                    .map_err(SyncError::validation)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingMergeMode {
    AdditiveDelta,
    WeightedAverage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncEmbeddingOp {
    pub values: Vec<f32>,
    pub merge_mode: EmbeddingMergeMode,
    pub weight: Option<f32>,
}

impl SyncEmbeddingOp {
    fn validate(&self) -> SyncResult<()> {
        if self.values.len() != STATE_DIM {
            return Err(SyncError::validation(format!(
                "scene embedding must contain {} values",
                STATE_DIM
            )));
        }
        if matches!(self.merge_mode, EmbeddingMergeMode::WeightedAverage)
            && self.weight.unwrap_or_default() <= 0.0
        {
            return Err(SyncError::validation(
                "weighted-average scene embedding requires weight > 0",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WeightTileMergeMode {
    Additive,
    WeightedAverage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncWeightTileDelta {
    pub layer_id: u8,
    pub tile_row: u16,
    pub tile_col: u16,
    pub tile_size: u16,
    pub base_checkpoint: String,
    pub merge_mode: WeightTileMergeMode,
    pub delta: Vec<f32>,
    pub weight: Option<f32>,
}

impl SyncWeightTileDelta {
    fn validate(&self) -> SyncResult<()> {
        if self.tile_size as usize != WEIGHT_TILE_SIZE {
            return Err(SyncError::validation(format!(
                "tile_size must equal {}",
                WEIGHT_TILE_SIZE
            )));
        }
        if self.delta.len() != WEIGHT_TILE_VALUE_COUNT {
            return Err(SyncError::validation(format!(
                "weight tile delta must contain {} values",
                WEIGHT_TILE_VALUE_COUNT
            )));
        }
        if self.base_checkpoint.trim().is_empty() {
            return Err(SyncError::validation(
                "weight tile delta must include base_checkpoint",
            ));
        }
        if matches!(self.merge_mode, WeightTileMergeMode::WeightedAverage)
            && self.weight.unwrap_or_default() <= 0.0
        {
            return Err(SyncError::validation(
                "weighted-average weight tile requires weight > 0",
            ));
        }
        Ok(())
    }

    pub fn effective_tile_key(&self) -> String {
        format!(
            "layer/{}/tile/{}/{}",
            self.layer_id, self.tile_row, self.tile_col
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncNavPose {
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub pitch_rad: f32,
    pub roll_rad: f32,
    pub confidence: f32,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncNavGoal {
    pub active: bool,
    pub cell_x: i32,
    pub cell_z: i32,
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncSemanticObject {
    pub name: String,
    pub confidence: f32,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncThoughtValue {
    pub text: String,
    pub layer: u8,
    pub kind: u8,
    pub vfe: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncLoreValue {
    pub question: String,
    pub answer: String,
    pub layer: u8,
    pub reason: u8,
    pub embedding_delta: f32,
    pub effectiveness: f32,
}

pub const BELIEF_EVIDENCE_SCHEMA_VERSION: &str = "qualia.belief-evidence.v1";
pub const BELIEF_ASSERTION_SCHEMA_VERSION: &str = "qualia.belief-assertion.v1";
pub const COGNITION_PEER_SNAPSHOT_SCHEMA_VERSION: &str = "qualia.cognition-peer-snapshot.v1";
pub const COGNITION_CANDIDATE_SCHEMA_VERSION: &str = "qualia.cognition-candidate.v1";
pub const COGNITION_EVALUATION_SCHEMA_VERSION: &str = "qualia.cognition-evaluation.v1";
pub const COGNITION_PROMOTION_SCHEMA_VERSION: &str = "qualia.cognition-promotion.v1";
pub const COMPUTE_ADVISORY_SCHEMA_VERSION: &str = "qualia.compute-advisory.v1";
pub const MAX_COGNITION_CANDIDATE_PATCHES: usize = 256;
pub const MAX_COGNITION_PATCH_DELTA: f32 = 0.25;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BeliefEvidenceV1 {
    pub schema_version: String,
    pub evidence_id: String,
    pub source_replica_id: String,
    pub captured_at_ms: u128,
    pub modality: String,
    pub payload_digest: String,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub expires_at_ms: Option<u128>,
}

impl BeliefEvidenceV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, BELIEF_EVIDENCE_SCHEMA_VERSION)?;
        validate_identifier("evidence_id", &self.evidence_id)?;
        validate_identifier("source_replica_id", &self.source_replica_id)?;
        validate_identifier("modality", &self.modality)?;
        validate_sha256("payload_digest", &self.payload_digest)?;
        if self.captured_at_ms == 0 {
            return Err("captured_at_ms must be greater than zero".to_string());
        }
        if self
            .expires_at_ms
            .is_some_and(|expires_at_ms| expires_at_ms <= self.captured_at_ms)
        {
            return Err("expires_at_ms must be newer than captured_at_ms".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BeliefAssertionV1 {
    pub schema_version: String,
    pub belief_id: String,
    pub source_replica_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
    pub evidence_ids: Vec<String>,
    pub asserted_at_ms: u128,
}

impl BeliefAssertionV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, BELIEF_ASSERTION_SCHEMA_VERSION)?;
        validate_identifier("belief_id", &self.belief_id)?;
        validate_identifier("source_replica_id", &self.source_replica_id)?;
        validate_text("subject", &self.subject, 256)?;
        validate_text("predicate", &self.predicate, 128)?;
        validate_text("object", &self.object, 512)?;
        validate_unit_interval("confidence", self.confidence)?;
        validate_evidence_ids(&self.evidence_ids)?;
        if self.asserted_at_ms == 0 {
            return Err("asserted_at_ms must be greater than zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CognitionPeerSnapshotV1 {
    pub schema_version: String,
    pub replica_id: String,
    pub backend: String,
    pub parameter_digest: String,
    pub checkpoint_digest: String,
    pub layer_sequences: Vec<u64>,
    pub prediction_error_l2: Vec<f32>,
    pub precision: Vec<f32>,
    pub evidence_ids: Vec<String>,
    pub captured_at_ms: u128,
}

impl CognitionPeerSnapshotV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, COGNITION_PEER_SNAPSHOT_SCHEMA_VERSION)?;
        validate_identifier("replica_id", &self.replica_id)?;
        validate_text("backend", &self.backend, 160)?;
        validate_sha256("parameter_digest", &self.parameter_digest)?;
        validate_sha256("checkpoint_digest", &self.checkpoint_digest)?;
        if self.layer_sequences.len() != 4
            || self.prediction_error_l2.len() != 4
            || self.precision.len() != 4
        {
            return Err("cognition snapshot must contain exactly four L3-L6 values".to_string());
        }
        if self
            .prediction_error_l2
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("prediction_error_l2 values must be finite and non-negative".to_string());
        }
        for precision in &self.precision {
            validate_unit_interval("precision", *precision)?;
        }
        validate_evidence_ids(&self.evidence_ids)?;
        if self.captured_at_ms == 0 {
            return Err("captured_at_ms must be greater than zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CognitionDeltaPatchV1 {
    pub layer: u8,
    pub row: usize,
    pub column: usize,
    pub delta: f32,
}

impl CognitionDeltaPatchV1 {
    fn validate(&self) -> Result<(), String> {
        if !(3..=6).contains(&self.layer) {
            return Err("cognition patch layer must be L3-L6".to_string());
        }
        if self.row >= COGNITION_STATE_DIM || self.column >= COGNITION_STATE_DIM {
            return Err(format!(
                "cognition patch coordinates must fit {}x{}",
                COGNITION_STATE_DIM, COGNITION_STATE_DIM
            ));
        }
        if !self.delta.is_finite() || self.delta.abs() > MAX_COGNITION_PATCH_DELTA {
            return Err(format!(
                "cognition patch delta must be finite and within +/-{}",
                MAX_COGNITION_PATCH_DELTA
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CognitionCandidateDeltaV1 {
    pub schema_version: String,
    pub candidate_id: String,
    pub source_replica_id: String,
    pub source_backend: String,
    pub base_checkpoint_digest: String,
    pub parameter_digest: String,
    pub evidence_ids: Vec<String>,
    pub patches: Vec<CognitionDeltaPatchV1>,
    pub created_at_ms: u128,
}

impl CognitionCandidateDeltaV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, COGNITION_CANDIDATE_SCHEMA_VERSION)?;
        validate_identifier("candidate_id", &self.candidate_id)?;
        validate_identifier("source_replica_id", &self.source_replica_id)?;
        validate_text("source_backend", &self.source_backend, 160)?;
        validate_sha256("base_checkpoint_digest", &self.base_checkpoint_digest)?;
        validate_sha256("parameter_digest", &self.parameter_digest)?;
        validate_evidence_ids(&self.evidence_ids)?;
        if self.patches.is_empty() || self.patches.len() > MAX_COGNITION_CANDIDATE_PATCHES {
            return Err(format!(
                "candidate must contain 1..={} patches",
                MAX_COGNITION_CANDIDATE_PATCHES
            ));
        }
        for patch in &self.patches {
            patch.validate()?;
        }
        if self.created_at_ms == 0 {
            return Err("created_at_ms must be greater than zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CognitionEvaluationDecision {
    Promote,
    Reject,
    Incompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CognitionCandidateEvaluationV1 {
    pub schema_version: String,
    pub evaluation_id: String,
    pub candidate_id: String,
    pub evaluator_replica_id: String,
    pub checkpoint_digest: String,
    pub evidence_ids: Vec<String>,
    pub canonical_error_l2: f32,
    pub shadow_error_l2: f32,
    pub relative_improvement: f32,
    pub decision: CognitionEvaluationDecision,
    pub evaluated_at_ms: u128,
    #[serde(default)]
    pub reason: Option<String>,
}

impl CognitionCandidateEvaluationV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, COGNITION_EVALUATION_SCHEMA_VERSION)?;
        validate_identifier("evaluation_id", &self.evaluation_id)?;
        validate_identifier("candidate_id", &self.candidate_id)?;
        validate_identifier("evaluator_replica_id", &self.evaluator_replica_id)?;
        validate_sha256("checkpoint_digest", &self.checkpoint_digest)?;
        validate_evidence_ids(&self.evidence_ids)?;
        if !self.canonical_error_l2.is_finite()
            || !self.shadow_error_l2.is_finite()
            || !self.relative_improvement.is_finite()
            || self.canonical_error_l2 < 0.0
            || self.shadow_error_l2 < 0.0
        {
            return Err("evaluation errors and improvement must be finite".to_string());
        }
        if matches!(self.decision, CognitionEvaluationDecision::Promote)
            && self.shadow_error_l2 >= self.canonical_error_l2
        {
            return Err("promoted candidate must improve aggregate prediction error".to_string());
        }
        if let Some(reason) = self.reason.as_deref() {
            validate_text("reason", reason, 512)?;
        }
        if self.evaluated_at_ms == 0 {
            return Err("evaluated_at_ms must be greater than zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CognitionPromotionOutcome {
    Promoted,
    Rejected,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CognitionPromotionReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub candidate_id: String,
    pub evaluation_id: String,
    pub replica_id: String,
    pub previous_checkpoint_digest: String,
    pub resulting_checkpoint_digest: String,
    pub outcome: CognitionPromotionOutcome,
    pub recorded_at_ms: u128,
    #[serde(default)]
    pub reason: Option<String>,
}

impl CognitionPromotionReceiptV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, COGNITION_PROMOTION_SCHEMA_VERSION)?;
        validate_identifier("receipt_id", &self.receipt_id)?;
        validate_identifier("candidate_id", &self.candidate_id)?;
        validate_identifier("evaluation_id", &self.evaluation_id)?;
        validate_identifier("replica_id", &self.replica_id)?;
        validate_sha256("previous_checkpoint_digest", &self.previous_checkpoint_digest)?;
        validate_sha256(
            "resulting_checkpoint_digest",
            &self.resulting_checkpoint_digest,
        )?;
        if let Some(reason) = self.reason.as_deref() {
            validate_text("reason", reason, 512)?;
        }
        if self.recorded_at_ms == 0 {
            return Err("recorded_at_ms must be greater than zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComputeAdvisoryV1 {
    pub schema_version: String,
    pub job_id: String,
    pub requester_replica_id: String,
    pub worker_replica_id: String,
    pub job_type: String,
    pub status: String,
    pub input_digest: String,
    pub output_digest: String,
    pub queued_ms: u64,
    pub execution_ms: u64,
    pub completed_at_ms: u128,
    pub expires_at_ms: u128,
    pub result: Value,
}

impl ComputeAdvisoryV1 {
    pub fn validate(&self) -> Result<(), String> {
        expect_schema(&self.schema_version, COMPUTE_ADVISORY_SCHEMA_VERSION)?;
        validate_identifier("job_id", &self.job_id)?;
        validate_identifier("requester_replica_id", &self.requester_replica_id)?;
        validate_identifier("worker_replica_id", &self.worker_replica_id)?;
        validate_identifier("job_type", &self.job_type)?;
        if !matches!(self.status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(
                "compute advisory status must be completed, failed, or cancelled".to_string(),
            );
        }
        validate_sha256("input_digest", &self.input_digest)?;
        validate_sha256("output_digest", &self.output_digest)?;
        if self.completed_at_ms == 0 || self.expires_at_ms <= self.completed_at_ms {
            return Err("compute advisory must have a future expiration".to_string());
        }
        if self.result.is_null() {
            return Err("compute advisory result must not be null".to_string());
        }
        Ok(())
    }
}

fn expect_schema(actual: &str, expected: &str) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("schema_version must be {expected}"))
    }
}

fn validate_identifier(field: &str, value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 160 {
        return Err(format!("{field} must contain 1..=160 characters"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, max: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max {
        return Err(format!("{field} must contain 1..={max} characters"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(())
}

fn validate_sha256(field: &str, digest: &str) -> Result<(), String> {
    if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!(
            "{field} must be a 64-character hexadecimal SHA-256 digest"
        ))
    }
}

fn validate_unit_interval(field: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} must be finite and within 0..=1"))
    }
}

fn validate_evidence_ids(evidence_ids: &[String]) -> Result<(), String> {
    if evidence_ids.is_empty() || evidence_ids.len() > 64 {
        return Err("evidence_ids must contain 1..=64 entries".to_string());
    }
    for evidence_id in evidence_ids {
        validate_identifier("evidence_id", evidence_id)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncApplyStatus {
    Applied,
    Duplicate,
    StoredOnly,
    RejectedAuthority,
    BlockedLease,
    ValidationFailed,
    CheckpointMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncError {
    pub status: SyncApplyStatus,
    pub detail: String,
}

impl SyncError {
    pub fn validation(detail: impl Into<String>) -> Self {
        Self {
            status: SyncApplyStatus::ValidationFailed,
            detail: detail.into(),
        }
    }

    pub fn rejected_authority(detail: impl Into<String>) -> Self {
        Self {
            status: SyncApplyStatus::RejectedAuthority,
            detail: detail.into(),
        }
    }

    pub fn blocked_lease(detail: impl Into<String>) -> Self {
        Self {
            status: SyncApplyStatus::BlockedLease,
            detail: detail.into(),
        }
    }

    pub fn checkpoint_mismatch(detail: impl Into<String>) -> Self {
        Self {
            status: SyncApplyStatus::CheckpointMismatch,
            detail: detail.into(),
        }
    }
}

pub type SyncResult<T> = Result<T, SyncError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncRegisterState {
    pub value: Value,
    pub winner_timestamp_hlc: HlcTimestamp,
    pub winner_replica_id: String,
    pub last_op_id: String,
    pub lease_holder: Option<String>,
    pub lease_expires_hlc: Option<HlcTimestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncOrMapEntry {
    pub tag: String,
    pub timestamp_hlc: HlcTimestamp,
    pub replica_id: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncOrMapState {
    pub entries: Vec<SyncOrMapEntry>,
    pub removed_tags: Vec<String>,
    pub visible_tag: Option<String>,
    pub visible_value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncEmbeddingState {
    pub merge_mode: EmbeddingMergeMode,
    pub accumulator: Vec<f32>,
    pub total_weight: f32,
}

impl SyncEmbeddingState {
    pub fn effective_values(&self) -> Vec<f32> {
        match self.merge_mode {
            EmbeddingMergeMode::AdditiveDelta => self.accumulator.clone(),
            EmbeddingMergeMode::WeightedAverage => {
                if self.total_weight > 0.0 {
                    self.accumulator
                        .iter()
                        .map(|value| *value / self.total_weight)
                        .collect()
                } else {
                    vec![0.0; self.accumulator.len()]
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncWeightTileState {
    pub layer_id: u8,
    pub tile_row: u16,
    pub tile_col: u16,
    pub tile_size: u16,
    pub base_checkpoint: String,
    pub merge_mode: WeightTileMergeMode,
    pub base_values: Vec<f32>,
    pub accumulator: Vec<f32>,
    pub total_weight: f32,
}

impl SyncWeightTileState {
    pub fn effective_values(&self) -> Vec<f32> {
        let delta = match self.merge_mode {
            WeightTileMergeMode::Additive => self.accumulator.clone(),
            WeightTileMergeMode::WeightedAverage => {
                if self.total_weight > 0.0 {
                    self.accumulator
                        .iter()
                        .map(|value| *value / self.total_weight)
                        .collect()
                } else {
                    vec![0.0; self.accumulator.len()]
                }
            }
        };
        self.base_values
            .iter()
            .zip(delta.iter())
            .map(|(base, change)| base + change)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum SyncMaterializedState {
    Register(SyncRegisterState),
    OrMap(SyncOrMapState),
    Embedding(SyncEmbeddingState),
    WeightTile(SyncWeightTileState),
}

impl SyncMaterializedState {
    pub fn state_kind(&self) -> SyncStateKind {
        match self {
            SyncMaterializedState::Register(_) => SyncStateKind::Register,
            SyncMaterializedState::OrMap(_) => SyncStateKind::OrMap,
            SyncMaterializedState::Embedding(_) => SyncStateKind::Embedding,
            SyncMaterializedState::WeightTile(_) => SyncStateKind::WeightTile,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeOutcome<T> {
    pub state: T,
    pub changed: bool,
}

/// Last-writer-wins register keyed by `(hlc, op_id)`, with an optional lease
/// that blocks foreign writers while it is active.
pub fn merge_register(
    current: Option<&SyncRegisterState>,
    envelope: &SyncEnvelope,
    body: &SyncRegisterValue,
) -> SyncResult<MergeOutcome<SyncRegisterState>> {
    if let Some(current) = current {
        let lease_until = current.lease_expires_hlc;
        let lease_active = lease_until
            .map(|expires| envelope.timestamp_hlc < expires)
            .unwrap_or(false);
        if lease_active && current.lease_holder.as_deref() != Some(envelope.replica_id.as_str()) {
            return Err(SyncError::blocked_lease(format!(
                "register {} is leased by {} until {}",
                envelope.key,
                current.lease_holder.as_deref().unwrap_or("unknown"),
                lease_until
                    .map(|expires| expires.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            )));
        }
        if (envelope.timestamp_hlc, envelope.op_id())
            < (current.winner_timestamp_hlc, current.last_op_id.clone())
        {
            return Ok(MergeOutcome {
                state: current.clone(),
                changed: false,
            });
        }
    }

    Ok(MergeOutcome {
        state: SyncRegisterState {
            value: body.value.clone(),
            winner_timestamp_hlc: envelope.timestamp_hlc,
            winner_replica_id: envelope.replica_id.clone(),
            last_op_id: envelope.op_id(),
            lease_holder: body.lease_duration_ms.map(|_| envelope.replica_id.clone()),
            lease_expires_hlc: body
                .lease_duration_ms
                .map(|duration_ms| envelope.timestamp_hlc.add_ms(duration_ms)),
        },
        changed: true,
    })
}

/// Observed-remove map: upserts are keyed by op id, removes are recorded as
/// observed tombstones, and the visible entry is the newest surviving one.
pub fn merge_or_map(
    current: Option<&SyncOrMapState>,
    envelope: &SyncEnvelope,
    body: &SyncOrMapOp,
) -> SyncResult<MergeOutcome<SyncOrMapState>> {
    let mut state = current.cloned().unwrap_or(SyncOrMapState {
        entries: Vec::new(),
        removed_tags: Vec::new(),
        visible_tag: None,
        visible_value: None,
    });
    let mut changed = false;

    match &body.action {
        OrMapAction::Upsert {
            value,
            replaces_tags,
        } => {
            let tag = envelope.op_id();
            if !state.entries.iter().any(|entry| entry.tag == tag) {
                state.entries.push(SyncOrMapEntry {
                    tag,
                    timestamp_hlc: envelope.timestamp_hlc,
                    replica_id: envelope.replica_id.clone(),
                    value: value.clone(),
                });
                changed = true;
            }
            for replaced in replaces_tags {
                if !state.removed_tags.contains(replaced) {
                    state.removed_tags.push(replaced.clone());
                    changed = true;
                }
            }
        }
        OrMapAction::Remove { remove_tags } => {
            for removed in remove_tags {
                if !state.removed_tags.contains(removed) {
                    state.removed_tags.push(removed.clone());
                    changed = true;
                }
            }
        }
    }

    state.removed_tags.sort();
    state.removed_tags.dedup();
    state
        .entries
        .sort_by(|left, right| merge_rank(left).cmp(&merge_rank(right)));

    let visible = state
        .entries
        .iter()
        .filter(|entry| !state.removed_tags.iter().any(|removed| removed == &entry.tag))
        .max_by(|left, right| merge_rank(left).cmp(&merge_rank(right)));

    state.visible_tag = visible.map(|entry| entry.tag.clone());
    state.visible_value = visible.map(|entry| entry.value.clone());

    Ok(MergeOutcome { state, changed })
}

fn merge_rank(entry: &SyncOrMapEntry) -> (HlcTimestamp, &str) {
    (entry.timestamp_hlc, entry.tag.as_str())
}

/// Accumulating merge for scene embeddings; the merge mode is fixed by the
/// first operation observed for a key.
pub fn merge_embedding(
    current: Option<&SyncEmbeddingState>,
    body: &SyncEmbeddingOp,
) -> SyncResult<MergeOutcome<SyncEmbeddingState>> {
    let mut state = current.cloned().unwrap_or(SyncEmbeddingState {
        merge_mode: body.merge_mode,
        accumulator: vec![0.0; STATE_DIM],
        total_weight: 0.0,
    });
    if state.merge_mode != body.merge_mode {
        return Err(SyncError::validation(
            "embedding merge_mode must stay stable per key",
        ));
    }

    let weight = match body.merge_mode {
        EmbeddingMergeMode::AdditiveDelta => 1.0,
        EmbeddingMergeMode::WeightedAverage => body.weight.unwrap_or(1.0),
    };
    for (slot, value) in state.accumulator.iter_mut().zip(body.values.iter()) {
        *slot += *value * weight;
    }
    state.total_weight += weight;

    Ok(MergeOutcome {
        state,
        changed: true,
    })
}

/// Accumulating merge for weight tiles; the base checkpoint and merge mode must
/// match the materialized tile before any delta is applied.
pub fn merge_weight_tile<F>(
    current: Option<&SyncWeightTileState>,
    body: &SyncWeightTileDelta,
    mut read_base_values: F,
) -> SyncResult<MergeOutcome<SyncWeightTileState>>
where
    F: FnMut() -> Vec<f32>,
{
    let mut state = current.cloned().unwrap_or_else(|| SyncWeightTileState {
        layer_id: body.layer_id,
        tile_row: body.tile_row,
        tile_col: body.tile_col,
        tile_size: body.tile_size,
        base_checkpoint: body.base_checkpoint.clone(),
        merge_mode: body.merge_mode,
        base_values: read_base_values(),
        accumulator: vec![0.0; WEIGHT_TILE_VALUE_COUNT],
        total_weight: 0.0,
    });

    if state.base_checkpoint != body.base_checkpoint {
        return Err(SyncError::checkpoint_mismatch(format!(
            "tile {} base checkpoint mismatch: {} != {}",
            body.effective_tile_key(),
            state.base_checkpoint,
            body.base_checkpoint
        )));
    }
    if state.merge_mode != body.merge_mode {
        return Err(SyncError::checkpoint_mismatch(format!(
            "tile {} merge_mode mismatch",
            body.effective_tile_key()
        )));
    }

    let weight = match body.merge_mode {
        WeightTileMergeMode::Additive => 1.0,
        WeightTileMergeMode::WeightedAverage => body.weight.unwrap_or(1.0),
    };
    for (slot, delta) in state.accumulator.iter_mut().zip(body.delta.iter()) {
        *slot += *delta * weight;
    }
    state.total_weight += weight;

    Ok(MergeOutcome {
        state,
        changed: true,
    })
}
