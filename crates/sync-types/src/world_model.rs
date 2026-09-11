use crate::{HlcTimestamp, ReplicaRole, ReplicaTrustState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORLD_MODEL_SCHEMA_VERSION: &str = "world.model.v1";
pub const DEFAULT_COMPACT_TENSOR_INLINE_FLOAT_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TypeProfileFamily {
    Tensor,
    Vector,
    Stream,
    Artifact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TensorDtype {
    Float32,
    Float16,
    Bfloat16,
    Int8,
    Uint8,
    Bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VectorMetric {
    Cosine,
    Dot,
    Euclidean,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Event,
    Sampled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamOrdering {
    Ordered,
    BestEffort,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamDeltaKind {
    Snapshot,
    Delta,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactFormat {
    Safetensors,
    Arrow,
    Parquet,
    Json,
    Npy,
    Onnx,
    Gguf,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypeProfile {
    pub profile_id: String,
    pub family: TypeProfileFamily,
    pub semantic_role: String,
    pub version: u32,
}

impl TypeProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.profile_id.trim().is_empty() {
            return Err("profile_id must not be empty".to_string());
        }
        if self.semantic_role.trim().is_empty() {
            return Err("semantic_role must not be empty".to_string());
        }
        if self.version == 0 {
            return Err("profile version must be >= 1".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TensorProfile {
    pub base: TypeProfile,
    pub rank: u8,
    pub shape: Vec<u32>,
    pub layout: String,
    pub axis_roles: Vec<String>,
    pub dtype: TensorDtype,
    pub coordinate_frame: Option<String>,
}

impl TensorProfile {
    pub fn validate(&self) -> Result<(), String> {
        self.base.validate()?;
        if self.base.family != TypeProfileFamily::Tensor {
            return Err("tensor profile base family must be tensor".to_string());
        }
        if self.rank == 0 {
            return Err("tensor profile rank must be > 0".to_string());
        }
        if self.shape.len() != self.rank as usize {
            return Err("tensor profile shape length must match rank".to_string());
        }
        if self.shape.contains(&0) {
            return Err("tensor profile shape dimensions must be > 0".to_string());
        }
        if self.layout.trim().is_empty() {
            return Err("tensor profile layout must not be empty".to_string());
        }
        if !self.axis_roles.is_empty() && self.axis_roles.len() != self.shape.len() {
            return Err("tensor profile axis_roles length must match rank".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorProfile {
    pub base: TypeProfile,
    pub dims: u32,
    pub element_dtype: TensorDtype,
    pub metric: VectorMetric,
    pub normalized: bool,
}

impl VectorProfile {
    pub fn validate(&self) -> Result<(), String> {
        self.base.validate()?;
        if self.base.family != TypeProfileFamily::Vector {
            return Err("vector profile base family must be vector".to_string());
        }
        if self.dims == 0 {
            return Err("vector profile dims must be > 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamProfile {
    pub base: TypeProfile,
    pub value_profile_id: String,
    pub stream_kind: StreamKind,
    pub ordering: StreamOrdering,
    pub delta_kind: StreamDeltaKind,
    pub replayable: bool,
}

impl StreamProfile {
    pub fn validate(&self) -> Result<(), String> {
        self.base.validate()?;
        if self.base.family != TypeProfileFamily::Stream {
            return Err("stream profile base family must be stream".to_string());
        }
        if self.value_profile_id.trim().is_empty() {
            return Err("stream value_profile_id must not be empty".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactProfile {
    pub base: TypeProfile,
    pub artifact_format: ArtifactFormat,
    pub mime_type: Option<String>,
    pub compression: Option<String>,
}

impl ArtifactProfile {
    pub fn validate(&self) -> Result<(), String> {
        self.base.validate()?;
        if self.base.family != TypeProfileFamily::Artifact {
            return Err("artifact profile base family must be artifact".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TensorArtifactRef {
    pub artifact_id: String,
    pub profile_id: String,
    pub artifact_format: ArtifactFormat,
    pub uri: String,
    pub checksum: String,
}

impl TensorArtifactRef {
    pub fn validate(&self) -> Result<(), String> {
        if self.artifact_id.trim().is_empty() {
            return Err("tensor artifact_ref artifact_id must not be empty".to_string());
        }
        if self.profile_id.trim().is_empty() {
            return Err("tensor artifact_ref profile_id must not be empty".to_string());
        }
        if self.uri.trim().is_empty() {
            return Err("tensor artifact_ref uri must not be empty".to_string());
        }
        if self.checksum.trim().is_empty() {
            return Err("tensor artifact_ref checksum must not be empty".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeliefVectorState {
    pub key: String,
    pub profile_id: String,
    pub values: Option<Vec<f32>>,
    pub artifact_ref: Option<TensorArtifactRef>,
    pub support_weight: f32,
    pub source_weight: f32,
    pub entropy: Option<f32>,
    pub freshness_ms: Option<u64>,
}

impl BeliefVectorState {
    pub fn validate(&self) -> Result<(), String> {
        validate_tensor_state_key(&self.key, "belief vector")?;
        validate_profile_id(&self.profile_id, "belief vector")?;
        validate_inline_or_reference(
            self.values
                .as_ref()
                .map(|values| !values.is_empty())
                .unwrap_or(false),
            self.artifact_ref.as_ref(),
            "belief vector",
        )?;
        validate_unit_interval(self.support_weight, "belief vector support_weight")?;
        validate_unit_interval(self.source_weight, "belief vector source_weight")?;
        if self.entropy.is_some_and(|entropy| entropy < 0.0) {
            return Err("belief vector entropy must be >= 0.0".to_string());
        }
        if let Some(artifact_ref) = &self.artifact_ref {
            artifact_ref.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SplineControlState {
    pub key: String,
    pub profile_id: String,
    pub coeffs: Option<Vec<f32>>,
    pub artifact_ref: Option<TensorArtifactRef>,
    pub knot_profile: Option<String>,
    pub confidence: f32,
    pub mission_relevance: f32,
    pub frame: Option<String>,
}

impl SplineControlState {
    pub fn validate(&self) -> Result<(), String> {
        validate_tensor_state_key(&self.key, "spline control")?;
        validate_profile_id(&self.profile_id, "spline control")?;
        validate_inline_or_reference(
            self.coeffs
                .as_ref()
                .map(|values| !values.is_empty())
                .unwrap_or(false),
            self.artifact_ref.as_ref(),
            "spline control",
        )?;
        validate_unit_interval(self.confidence, "spline control confidence")?;
        validate_unit_interval(self.mission_relevance, "spline control mission_relevance")?;
        if let Some(knot_profile) = &self.knot_profile {
            if knot_profile.trim().is_empty() {
                return Err(
                    "spline control knot_profile must not be empty when provided".to_string(),
                );
            }
        }
        if let Some(artifact_ref) = &self.artifact_ref {
            artifact_ref.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RelationFactorState {
    pub key: String,
    pub profile_id: String,
    pub factor_kind: String,
    pub node_refs: Vec<String>,
    pub params: Value,
    pub inline_tensor: Option<Vec<f32>>,
    pub artifact_ref: Option<TensorArtifactRef>,
    pub energy_weight: f32,
    pub confidence: f32,
    pub support_refs: Vec<String>,
}

impl RelationFactorState {
    pub fn validate(&self) -> Result<(), String> {
        validate_tensor_state_key(&self.key, "relation factor")?;
        validate_profile_id(&self.profile_id, "relation factor")?;
        if self.factor_kind.trim().is_empty() {
            return Err("relation factor factor_kind must not be empty".to_string());
        }
        if self.node_refs.len() < 2 {
            return Err("relation factor must reference at least two nodes".to_string());
        }
        validate_inline_or_reference(
            self.inline_tensor
                .as_ref()
                .map(|values| !values.is_empty())
                .unwrap_or(false),
            self.artifact_ref.as_ref(),
            "relation factor",
        )?;
        validate_unit_interval(self.energy_weight, "relation factor energy_weight")?;
        validate_unit_interval(self.confidence, "relation factor confidence")?;
        if let Some(artifact_ref) = &self.artifact_ref {
            artifact_ref.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProposalLineage {
    pub derived_from: Vec<String>,
    pub supersedes: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Object,
    Region,
    GraphNode,
    GraphFactor,
    PlannerAdvisory,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Proposed,
    Promoted,
    Rejected,
    Superseded,
    Withdrawn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectProposal {
    pub label: String,
    pub attributes: Value,
    pub profile_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegionProposal {
    pub region_kind: String,
    pub bounds: Value,
    pub attributes: Value,
    pub profile_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNodeProposal {
    pub node_kind: String,
    pub attributes: Value,
    pub profile_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphFactorProposal {
    pub factor_kind: String,
    pub left_node_id: String,
    pub right_node_id: String,
    pub parameters: Value,
    pub energy_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannerAdvisory {
    pub advisory_kind: String,
    pub payload: Value,
    pub target_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ProposalBody {
    Object(ObjectProposal),
    Region(RegionProposal),
    GraphNode(GraphNodeProposal),
    GraphFactor(GraphFactorProposal),
    PlannerAdvisory(PlannerAdvisory),
}

impl ProposalBody {
    pub fn proposal_kind(&self) -> ProposalKind {
        match self {
            ProposalBody::Object(_) => ProposalKind::Object,
            ProposalBody::Region(_) => ProposalKind::Region,
            ProposalBody::GraphNode(_) => ProposalKind::GraphNode,
            ProposalBody::GraphFactor(_) => ProposalKind::GraphFactor,
            ProposalBody::PlannerAdvisory(_) => ProposalKind::PlannerAdvisory,
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            ProposalBody::Object(body) => {
                if body.label.trim().is_empty() {
                    return Err("object proposal label must not be empty".to_string());
                }
            }
            ProposalBody::Region(body) => {
                if body.region_kind.trim().is_empty() {
                    return Err("region proposal kind must not be empty".to_string());
                }
            }
            ProposalBody::GraphNode(body) => {
                if body.node_kind.trim().is_empty() {
                    return Err("graph node proposal kind must not be empty".to_string());
                }
            }
            ProposalBody::GraphFactor(body) => {
                if body.factor_kind.trim().is_empty() {
                    return Err("graph factor proposal kind must not be empty".to_string());
                }
                if body.left_node_id.trim().is_empty() || body.right_node_id.trim().is_empty() {
                    return Err("graph factor proposal node ids must not be empty".to_string());
                }
                validate_unit_interval(body.energy_weight, "graph factor energy_weight")?;
            }
            ProposalBody::PlannerAdvisory(body) => {
                if body.advisory_kind.trim().is_empty() {
                    return Err("planner advisory kind must not be empty".to_string());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProposalEnvelope {
    pub schema_version: String,
    pub proposal_id: String,
    pub proposal_kind: ProposalKind,
    pub source_replica_id: String,
    pub source_replica_role: ReplicaRole,
    pub created_at_hlc: HlcTimestamp,
    pub status: ProposalStatus,
    pub belief_weight: f32,
    pub source_weight: f32,
    pub mission_relevance: f32,
    pub confidence: f32,
    pub lineage: ProposalLineage,
    pub body: ProposalBody,
}

impl ProposalEnvelope {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema_version != WORLD_MODEL_SCHEMA_VERSION {
            return Err(format!(
                "unsupported world-model schema_version: {}",
                self.schema_version
            ));
        }
        if self.proposal_id.trim().is_empty() {
            return Err("proposal_id must not be empty".to_string());
        }
        if self.source_replica_id.trim().is_empty() {
            return Err("source_replica_id must not be empty".to_string());
        }
        if self.proposal_kind != self.body.proposal_kind() {
            return Err("proposal_kind must match proposal body kind".to_string());
        }
        validate_unit_interval(self.belief_weight, "belief_weight")?;
        validate_unit_interval(self.source_weight, "source_weight")?;
        validate_unit_interval(self.mission_relevance, "mission_relevance")?;
        validate_unit_interval(self.confidence, "confidence")?;
        self.body.validate()
    }

    pub fn validate_authority(
        &self,
        registered_role: ReplicaRole,
        trust_state: ReplicaTrustState,
    ) -> Result<(), String> {
        validate_world_model_replica(
            self.source_replica_role,
            registered_role,
            trust_state,
            false,
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoachDecisionKind {
    Promote,
    Reject,
    Merge,
    Split,
    Link,
    Deprecate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoachDecision {
    pub schema_version: String,
    pub decision_id: String,
    pub decision_kind: CoachDecisionKind,
    pub curator_replica_id: String,
    pub curator_replica_role: ReplicaRole,
    pub created_at_hlc: HlcTimestamp,
    pub target_proposal_ids: Vec<String>,
    pub output_ids: Vec<String>,
    pub reason: Option<String>,
}

impl CoachDecision {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema_version != WORLD_MODEL_SCHEMA_VERSION {
            return Err(format!(
                "unsupported world-model schema_version: {}",
                self.schema_version
            ));
        }
        if self.decision_id.trim().is_empty() {
            return Err("decision_id must not be empty".to_string());
        }
        if self.curator_replica_id.trim().is_empty() {
            return Err("curator_replica_id must not be empty".to_string());
        }
        if self.target_proposal_ids.is_empty() {
            return Err("coach decision must target at least one proposal".to_string());
        }
        match self.decision_kind {
            CoachDecisionKind::Promote => {
                if self.target_proposal_ids.len() != 1 {
                    return Err("promote must target exactly one proposal".to_string());
                }
                if self.output_ids.len() > 1 {
                    return Err("promote may emit at most one canonical output id".to_string());
                }
            }
            CoachDecisionKind::Reject | CoachDecisionKind::Deprecate => {
                if !self.output_ids.is_empty() {
                    return Err("reject/deprecate decisions must not emit output ids".to_string());
                }
            }
            CoachDecisionKind::Merge => {
                if self.target_proposal_ids.len() < 2 {
                    return Err("merge must target at least two proposals".to_string());
                }
                if self.output_ids.len() > 1 {
                    return Err("merge may emit at most one canonical output id".to_string());
                }
            }
            CoachDecisionKind::Split => {
                if self.target_proposal_ids.len() != 1 {
                    return Err("split must target exactly one proposal".to_string());
                }
                if self.output_ids.is_empty() {
                    return Err("split must emit at least one output id".to_string());
                }
            }
            CoachDecisionKind::Link => {
                if self.target_proposal_ids.len() < 2 {
                    return Err("link must target at least two proposals".to_string());
                }
            }
        }
        Ok(())
    }

    pub fn validate_authority(
        &self,
        registered_role: ReplicaRole,
        trust_state: ReplicaTrustState,
    ) -> Result<(), String> {
        validate_world_model_replica(
            self.curator_replica_role,
            registered_role,
            trust_state,
            true,
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalKind {
    Object,
    Region,
    GraphNode,
    GraphFactor,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalStatus {
    Accepted,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalObject {
    pub label: String,
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalRegion {
    pub region_kind: String,
    pub bounds: Value,
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalGraphNode {
    pub node_kind: String,
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalGraphFactor {
    pub factor_kind: String,
    pub left_node_id: String,
    pub right_node_id: String,
    pub parameters: Value,
    pub energy_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CanonicalBody {
    Object(CanonicalObject),
    Region(CanonicalRegion),
    GraphNode(CanonicalGraphNode),
    GraphFactor(CanonicalGraphFactor),
}

impl CanonicalBody {
    pub fn canonical_kind(&self) -> CanonicalKind {
        match self {
            CanonicalBody::Object(_) => CanonicalKind::Object,
            CanonicalBody::Region(_) => CanonicalKind::Region,
            CanonicalBody::GraphNode(_) => CanonicalKind::GraphNode,
            CanonicalBody::GraphFactor(_) => CanonicalKind::GraphFactor,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CanonicalStateEnvelope {
    pub schema_version: String,
    pub canonical_id: String,
    pub canonical_kind: CanonicalKind,
    pub source_decision_id: String,
    pub source_proposal_ids: Vec<String>,
    pub accepted_at_hlc: HlcTimestamp,
    pub status: CanonicalStatus,
    pub body: CanonicalBody,
}

impl CanonicalStateEnvelope {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.schema_version != WORLD_MODEL_SCHEMA_VERSION {
            return Err(format!(
                "unsupported world-model schema_version: {}",
                self.schema_version
            ));
        }
        if self.canonical_id.trim().is_empty() {
            return Err("canonical_id must not be empty".to_string());
        }
        if self.source_decision_id.trim().is_empty() {
            return Err("source_decision_id must not be empty".to_string());
        }
        if self.source_proposal_ids.is_empty() {
            return Err("canonical state must retain at least one source proposal id".to_string());
        }
        if self.canonical_kind != self.body.canonical_kind() {
            return Err("canonical_kind must match canonical body kind".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalPose {
    pub canonical_id: String,
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalNavGoal {
    pub canonical_id: String,
    pub active: bool,
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalHazardState {
    pub canonical_id: String,
    pub kind: String,
    pub severity: Option<f32>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationalPlannerCompact {
    pub accepted_count: u32,
    pub nav_goal_present: bool,
    pub hazard_count: u32,
    pub route_factor_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalStateEnvelope {
    pub schema_version: String,
    pub pose: Option<OperationalPose>,
    pub nav_goal: Option<OperationalNavGoal>,
    pub hazards: Vec<OperationalHazardState>,
    pub planner: OperationalPlannerCompact,
    pub source_canonical_ids: Vec<String>,
}

/// Flattens accepted canonical entries into the compact operational view the
/// control runners consume. Later entries win for the single-valued pose and
/// nav-goal slots; hazards accumulate.
pub fn project_canonical_to_operational(
    canonical: &[CanonicalStateEnvelope],
) -> Result<OperationalStateEnvelope, String> {
    let mut pose = None;
    let mut nav_goal = None;
    let mut hazards = Vec::new();
    let mut route_factor_count = 0_u32;
    let mut source_canonical_ids = Vec::with_capacity(canonical.len());

    for entry in canonical {
        entry.validate_shape()?;
        if entry.status != CanonicalStatus::Accepted {
            continue;
        }
        source_canonical_ids.push(entry.canonical_id.clone());
        match &entry.body {
            CanonicalBody::Object(object) => {
                if let Some(attrs) = object.attributes.as_object() {
                    if let Some(candidate) =
                        extract_pose_candidate(&entry.canonical_id, Some(&object.label), attrs)
                    {
                        pose = Some(candidate);
                    }
                    if let Some(candidate) =
                        extract_nav_goal_candidate(&entry.canonical_id, Some(&object.label), attrs)
                    {
                        nav_goal = Some(candidate);
                    }
                    if let Some(candidate) =
                        extract_hazard_candidate(&entry.canonical_id, Some(&object.label), attrs)
                    {
                        hazards.push(candidate);
                    }
                }
            }
            CanonicalBody::Region(region) => {
                if let Some(candidate) = extract_region_nav_goal_candidate(
                    &entry.canonical_id,
                    &region.region_kind,
                    &region.bounds,
                    &region.attributes,
                ) {
                    nav_goal = Some(candidate);
                }
                if let Some(attrs) = region.attributes.as_object() {
                    if let Some(candidate) = extract_hazard_candidate(
                        &entry.canonical_id,
                        Some(&region.region_kind),
                        attrs,
                    ) {
                        hazards.push(candidate);
                    }
                }
            }
            CanonicalBody::GraphNode(node) => {
                if let Some(attrs) = node.attributes.as_object() {
                    if let Some(candidate) =
                        extract_pose_candidate(&entry.canonical_id, Some(&node.node_kind), attrs)
                    {
                        pose = Some(candidate);
                    }
                    if let Some(candidate) = extract_nav_goal_candidate(
                        &entry.canonical_id,
                        Some(&node.node_kind),
                        attrs,
                    ) {
                        nav_goal = Some(candidate);
                    }
                    if let Some(candidate) =
                        extract_hazard_candidate(&entry.canonical_id, Some(&node.node_kind), attrs)
                    {
                        hazards.push(candidate);
                    }
                }
            }
            CanonicalBody::GraphFactor(factor) => {
                if is_route_factor(&factor.factor_kind) {
                    route_factor_count += 1;
                }
                if let Some(params) = factor.parameters.as_object() {
                    if let Some(candidate) = extract_factor_hazard_candidate(
                        &entry.canonical_id,
                        &factor.factor_kind,
                        params,
                    ) {
                        hazards.push(candidate);
                    }
                }
            }
        }
    }

    let nav_goal_present = nav_goal.is_some();
    Ok(OperationalStateEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        pose,
        nav_goal,
        planner: OperationalPlannerCompact {
            accepted_count: source_canonical_ids.len() as u32,
            nav_goal_present,
            hazard_count: hazards.len() as u32,
            route_factor_count,
        },
        hazards,
        source_canonical_ids,
    })
}

/// Materializes one promoted proposal into canonical state. Only a promote
/// decision that targets exactly this proposal is accepted, and planner
/// advisories are never canonical.
pub fn promote_proposal_to_canonical(
    proposal: &ProposalEnvelope,
    decision: &CoachDecision,
) -> Result<CanonicalStateEnvelope, String> {
    decision.validate_shape()?;
    proposal.validate_shape()?;
    if decision.decision_kind != CoachDecisionKind::Promote {
        return Err("only promote decisions can materialize canonical state".to_string());
    }
    if decision.target_proposal_ids != vec![proposal.proposal_id.clone()] {
        return Err("promote decision must target the promoted proposal".to_string());
    }
    let canonical_id = decision
        .output_ids
        .first()
        .cloned()
        .unwrap_or_else(|| proposal.proposal_id.clone());
    let (canonical_kind, body) = match &proposal.body {
        ProposalBody::Object(object) => (
            CanonicalKind::Object,
            CanonicalBody::Object(CanonicalObject {
                label: object.label.clone(),
                attributes: object.attributes.clone(),
            }),
        ),
        ProposalBody::Region(region) => (
            CanonicalKind::Region,
            CanonicalBody::Region(CanonicalRegion {
                region_kind: region.region_kind.clone(),
                bounds: region.bounds.clone(),
                attributes: region.attributes.clone(),
            }),
        ),
        ProposalBody::GraphNode(node) => (
            CanonicalKind::GraphNode,
            CanonicalBody::GraphNode(CanonicalGraphNode {
                node_kind: node.node_kind.clone(),
                attributes: node.attributes.clone(),
            }),
        ),
        ProposalBody::GraphFactor(factor) => (
            CanonicalKind::GraphFactor,
            CanonicalBody::GraphFactor(CanonicalGraphFactor {
                factor_kind: factor.factor_kind.clone(),
                left_node_id: factor.left_node_id.clone(),
                right_node_id: factor.right_node_id.clone(),
                parameters: factor.parameters.clone(),
                energy_weight: factor.energy_weight,
            }),
        ),
        ProposalBody::PlannerAdvisory(_) => {
            return Err(
                "planner advisory proposals cannot be promoted into canonical state".to_string(),
            )
        }
    };
    let canonical = CanonicalStateEnvelope {
        schema_version: WORLD_MODEL_SCHEMA_VERSION.to_string(),
        canonical_id,
        canonical_kind,
        source_decision_id: decision.decision_id.clone(),
        source_proposal_ids: vec![proposal.proposal_id.clone()],
        accepted_at_hlc: decision.created_at_hlc,
        status: CanonicalStatus::Accepted,
        body,
    };
    canonical.validate_shape()?;
    Ok(canonical)
}

fn validate_world_model_replica(
    claimed_role: ReplicaRole,
    registered_role: ReplicaRole,
    trust_state: ReplicaTrustState,
    require_curator_role: bool,
) -> Result<(), String> {
    if !trust_state.allows_sync() {
        return Err("replica trust state does not allow world-model writes".to_string());
    }
    if claimed_role != registered_role {
        return Err(format!(
            "claimed replica role {:?} does not match registered role {:?}",
            claimed_role, registered_role
        ));
    }
    if require_curator_role && !matches!(registered_role, ReplicaRole::Host | ReplicaRole::Operator)
    {
        return Err("coach decisions require host or operator authority".to_string());
    }
    Ok(())
}

fn validate_unit_interval(value: f32, field: &str) -> Result<(), String> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} must be between 0.0 and 1.0"))
    }
}

fn validate_tensor_state_key(key: &str, kind: &str) -> Result<(), String> {
    if key.trim().is_empty() {
        Err(format!("{kind} key must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_profile_id(profile_id: &str, kind: &str) -> Result<(), String> {
    if profile_id.trim().is_empty() {
        Err(format!("{kind} profile_id must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_inline_or_reference(
    has_inline: bool,
    artifact_ref: Option<&TensorArtifactRef>,
    kind: &str,
) -> Result<(), String> {
    if has_inline || artifact_ref.is_some() {
        Ok(())
    } else {
        Err(format!(
            "{kind} must provide either inline values or an artifact_ref"
        ))
    }
}

pub fn compact_tensor_prefers_inline(value_count: usize) -> bool {
    value_count <= DEFAULT_COMPACT_TENSOR_INLINE_FLOAT_LIMIT
}

pub fn compact_tensor_should_reference(value_count: usize) -> bool {
    !compact_tensor_prefers_inline(value_count)
}

fn attr_flag(attrs: &serde_json::Map<String, Value>, key: &str) -> bool {
    attrs.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn attr_text_equals(attrs: &serde_json::Map<String, Value>, key: &str, expected: &str) -> bool {
    attrs
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn label_equals(label_or_kind: Option<&str>, expected: &str) -> bool {
    label_or_kind.is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn is_pose_role(label_or_kind: Option<&str>, attrs: &serde_json::Map<String, Value>) -> bool {
    attr_flag(attrs, "pose")
        || attr_text_equals(attrs, "operational_role", "pose")
        || attr_text_equals(attrs, "intent", "pose")
        || label_equals(label_or_kind, "pose")
        || label_equals(label_or_kind, "pose_summary")
}

fn is_nav_goal_role(label_or_kind: Option<&str>, attrs: &serde_json::Map<String, Value>) -> bool {
    attr_flag(attrs, "nav_goal")
        || attr_flag(attrs, "goal")
        || attr_text_equals(attrs, "operational_role", "nav_goal")
        || attr_text_equals(attrs, "intent", "nav_goal")
        || label_equals(label_or_kind, "nav_goal")
        || label_equals(label_or_kind, "goal")
}

fn is_hazard_role(label_or_kind: Option<&str>, attrs: &serde_json::Map<String, Value>) -> bool {
    attr_flag(attrs, "hazard")
        || attr_text_equals(attrs, "operational_role", "hazard")
        || attr_text_equals(attrs, "intent", "hazard")
        || label_or_kind.is_some_and(|value| value.to_ascii_lowercase().contains("hazard"))
}

fn extract_pose_candidate(
    canonical_id: &str,
    label_or_kind: Option<&str>,
    attrs: &serde_json::Map<String, Value>,
) -> Option<OperationalPose> {
    if !is_pose_role(label_or_kind, attrs) {
        return None;
    }
    Some(OperationalPose {
        canonical_id: canonical_id.to_string(),
        x_m: get_number(attrs, "x_m")?,
        y_m: get_number_or_default(attrs, "y_m", 0.0),
        z_m: get_number(attrs, "z_m")?,
        yaw_rad: get_number_or_default(attrs, "yaw_rad", 0.0),
        confidence: attrs
            .get("confidence")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
    })
}

fn extract_nav_goal_candidate(
    canonical_id: &str,
    label_or_kind: Option<&str>,
    attrs: &serde_json::Map<String, Value>,
) -> Option<OperationalNavGoal> {
    if !is_nav_goal_role(label_or_kind, attrs) {
        return None;
    }
    Some(OperationalNavGoal {
        canonical_id: canonical_id.to_string(),
        active: true,
        x_m: get_number(attrs, "x_m")?,
        y_m: get_number_or_default(attrs, "y_m", 0.0),
        z_m: get_number(attrs, "z_m")?,
        yaw_rad: get_number_or_default(attrs, "yaw_rad", 0.0),
    })
}

fn extract_region_nav_goal_candidate(
    canonical_id: &str,
    region_kind: &str,
    bounds: &Value,
    attributes: &Value,
) -> Option<OperationalNavGoal> {
    let attrs = attributes.as_object()?;
    if !is_nav_goal_role(Some(region_kind), attrs) {
        return None;
    }
    let bounds = bounds.as_object()?;
    let yaw_rad = get_number_or_default(attrs, "yaw_rad", 0.0);
    if let Some(center) = bounds.get("center").and_then(Value::as_object) {
        return Some(OperationalNavGoal {
            canonical_id: canonical_id.to_string(),
            active: true,
            x_m: get_number(center, "x_m")?,
            y_m: get_number_or_default(center, "y_m", 0.0),
            z_m: get_number(center, "z_m")?,
            yaw_rad,
        });
    }
    Some(OperationalNavGoal {
        canonical_id: canonical_id.to_string(),
        active: true,
        x_m: get_number(bounds, "x_m")?,
        y_m: get_number_or_default(bounds, "y_m", 0.0),
        z_m: get_number(bounds, "z_m")?,
        yaw_rad,
    })
}

fn extract_hazard_candidate(
    canonical_id: &str,
    label_or_kind: Option<&str>,
    attrs: &serde_json::Map<String, Value>,
) -> Option<OperationalHazardState> {
    if !is_hazard_role(label_or_kind, attrs) {
        return None;
    }
    Some(OperationalHazardState {
        canonical_id: canonical_id.to_string(),
        kind: attrs
            .get("hazard_kind")
            .and_then(Value::as_str)
            .or(label_or_kind)
            .unwrap_or("hazard")
            .to_string(),
        severity: attrs
            .get("severity")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
        summary: attrs.get("summary").and_then(Value::as_str).map(str::to_string),
    })
}

fn extract_factor_hazard_candidate(
    canonical_id: &str,
    factor_kind: &str,
    params: &serde_json::Map<String, Value>,
) -> Option<OperationalHazardState> {
    if !(factor_kind.to_ascii_lowercase().contains("hazard") || attr_flag(params, "hazard")) {
        return None;
    }
    Some(OperationalHazardState {
        canonical_id: canonical_id.to_string(),
        kind: factor_kind.to_string(),
        severity: params
            .get("severity")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
        summary: params.get("summary").and_then(Value::as_str).map(str::to_string),
    })
}

fn is_route_factor(factor_kind: &str) -> bool {
    let lowered = factor_kind.to_ascii_lowercase();
    lowered.contains("route") || lowered.contains("path") || lowered.contains("nav")
}

fn get_number(map: &serde_json::Map<String, Value>, key: &str) -> Option<f32> {
    map.get(key).and_then(Value::as_f64).map(|value| value as f32)
}

fn get_number_or_default(map: &serde_json::Map<String, Value>, key: &str, default: f32) -> f32 {
    get_number(map, key).unwrap_or(default)
}
