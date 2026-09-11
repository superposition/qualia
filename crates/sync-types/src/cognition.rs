use serde::{Deserialize, Serialize};

pub const COGNITION_CONTRACT_VERSION: &str = "qualia.cognition.v1";
pub const COGNITION_BOUNDARY_VERSION: &str = "qualia.cognition-boundary.v2";
pub const COGNITION_LEGACY_BOUNDARY_VERSION: &str = "qualia.cognition.v1";
pub const COGNITION_STATE_DIM: usize = 1_024;
pub const COGNITION_BOUNDARY_TIMEOUT_MS: u128 = 500;
pub const COGNITION_CHECKPOINT_INTERVAL_MS: u128 = 60_000;
pub const SEMANTIC_PRIOR_VERSION: &str = "qualia.semantic-prior.v2";
pub const SEMANTIC_PROJECTION_VERSION: &str = "qualia.l6-to-l3.fixed.v1";
pub const SEMANTIC_PRIOR_MAX_LIFETIME_MS: u128 = 30_000;
pub const SEMANTIC_PRIOR_EVIDENCE_MAX_AGE_MS: u128 = 500;
pub const SEMANTIC_PRIOR_FEATURE_DIM: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionCapabilitiesV1 {
    pub schema_version: String,
    pub runtime: String,
    pub owner: String,
    pub state_dim: usize,
    pub owned_layers: Vec<u8>,
    pub sensor_plane: u8,
    pub backend: String,
    pub cadences_hz: Vec<f32>,
    pub cross_boundary_timeout_ms: u128,
    pub checkpoint_interval_ms: u128,
    pub semantic_prior_target_layer: u8,
    pub motor_authority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionLayerSnapshotV1 {
    pub schema_version: String,
    pub ts_ms: u128,
    pub layer: u8,
    pub owner: String,
    pub cadence_hz: f32,
    pub sequence: u64,
    pub precision: f32,
    pub prediction_error_l2: f32,
    pub activation_mean: f32,
    pub activation_rms: f32,
    pub fresh: bool,
    pub source_ts_ms: Option<u128>,
    pub source_age_ms: Option<u128>,
    pub source_epoch: Option<String>,
    pub source_sequence: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionBoundaryFrameV2 {
    pub schema_version: String,
    pub ts_ms: u128,
    pub expires_at_ms: u128,
    pub source: String,
    pub destination: String,
    pub layer: u8,
    /// Written once per producer boot. `sequence` is monotonic inside one
    /// epoch and starts again from zero when the producer restarts.
    #[serde(default)]
    pub producer_epoch: String,
    pub sequence: u64,
    pub precision: f32,
    pub state_digest: String,
    pub latent: Vec<f32>,
}

/// Kept so callers written against the earlier name still compile. The JSON is
/// the v2 envelope either way; a reader tells the two apart by `schema_version`
/// and `producer_epoch`, and v1 documents keep parsing through the migration.
pub type CognitionBoundaryFrameV1 = CognitionBoundaryFrameV2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticPriorRuntime {
    Hermes,
    Kimi,
    LocalSlm,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticClass {
    Obstacle,
    Traversable,
    ObjectIdentity,
    Person,
    GoalPreference,
    Motion,
    Location,
    Uncertainty,
}

impl SemanticClass {
    fn feature_index(self) -> usize {
        match self {
            Self::Obstacle => 0,
            Self::Traversable => 1,
            Self::ObjectIdentity => 2,
            Self::Person => 3,
            Self::GoalPreference => 4,
            Self::Motion => 5,
            Self::Location => 6,
            Self::Uncertainty => 7,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRelation {
    Is,
    Near,
    LeftOf,
    RightOf,
    AheadOf,
    Behind,
    Blocks,
    Supports,
}

impl SemanticRelation {
    fn feature_index(self) -> usize {
        match self {
            Self::Is => 8,
            Self::Near => 9,
            Self::LeftOf => 10,
            Self::RightOf => 11,
            Self::AheadOf => 12,
            Self::Behind => 13,
            Self::Blocks => 14,
            Self::Supports => 15,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticPolarity {
    Supports,
    Contradicts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TypedSemanticPropositionV2 {
    pub class: SemanticClass,
    pub subject: String,
    pub relation: SemanticRelation,
    pub object: Option<String>,
    pub polarity: SemanticPolarity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SemanticPriorSourceV2 {
    pub runtime: SemanticPriorRuntime,
    pub model_id: String,
    pub model_version: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SemanticEvidenceRefV2 {
    pub producer_epoch: u64,
    pub runner_epoch: u64,
    pub inference_seq: u64,
    pub timestamp_ms: u128,
    pub camera_seq: u64,
    pub lidar_seq: u64,
    pub pose_seq: u64,
    pub action_seq: u64,
    pub physical_model_id: String,
    pub checkpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalGoalPreferenceV2 {
    pub canonical_goal_id: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SemanticPriorV2 {
    pub schema_version: String,
    pub prior_id: String,
    pub proposition: TypedSemanticPropositionV2,
    pub evidence_refs: Vec<SemanticEvidenceRefV2>,
    pub confidence: f32,
    pub created_at_ms: u128,
    pub expires_at_ms: u128,
    pub target_layer: u8,
    pub projection_version: String,
    pub source: SemanticPriorSourceV2,
    pub goal_preference: Option<CanonicalGoalPreferenceV2>,
}

/// Exact physical observation against which a language prior is admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticPriorValidationContext {
    pub now_ms: u128,
    pub producer_epoch: u64,
    pub runner_epoch: u64,
    pub inference_seq: u64,
    pub timestamp_ms: u128,
    pub camera_seq: u64,
    pub lidar_seq: u64,
    pub pose_seq: u64,
    pub action_seq: u64,
    pub physical_model_id: String,
    pub checkpoint_id: String,
    pub known_canonical_goal_ids: Vec<String>,
}

impl SemanticPriorV2 {
    pub fn validate(&self, context: &SemanticPriorValidationContext) -> Result<(), String> {
        if self.schema_version != SEMANTIC_PRIOR_VERSION {
            return Err("unsupported semantic prior schema".to_string());
        }
        if self.projection_version != SEMANTIC_PROJECTION_VERSION {
            return Err("unsupported L6-to-L3 projection version".to_string());
        }
        if self.prior_id.trim().is_empty()
            || self.proposition.subject.trim().is_empty()
            || self
                .proposition
                .object
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err("semantic prior id and typed proposition are required".to_string());
        }
        if self.evidence_refs.is_empty()
            || self
                .evidence_refs
                .iter()
                .any(|reference| !reference.matches_context(context))
        {
            return Err(
                "semantic prior evidence does not match the live physical observation".to_string(),
            );
        }
        if !(0.0..=1.0).contains(&self.confidence)
            || !self.confidence.is_finite()
            || self.confidence == 0.0
        {
            return Err("semantic prior confidence must be finite and within (0,1]".to_string());
        }
        if self.target_layer != 6 {
            return Err("semantic priors may only target Qualia layer 6".to_string());
        }
        if self.created_at_ms > context.now_ms.saturating_add(1_000)
            || self.expires_at_ms <= self.created_at_ms
            || self.expires_at_ms <= context.now_ms
            || self.expires_at_ms.saturating_sub(self.created_at_ms)
                > SEMANTIC_PRIOR_MAX_LIFETIME_MS
        {
            return Err("semantic prior is expired or has an invalid lifetime".to_string());
        }
        if self.source.model_id.trim().is_empty()
            || self.source.model_version.trim().is_empty()
            || self.source.request_id.trim().is_empty()
        {
            return Err("semantic prior model provenance is incomplete".to_string());
        }
        if let Some(preference) = &self.goal_preference {
            if self.proposition.class != SemanticClass::GoalPreference
                || preference.canonical_goal_id.trim().is_empty()
                || !preference.weight.is_finite()
                || !(0.0..=1.0).contains(&preference.weight)
                || !context
                    .known_canonical_goal_ids
                    .iter()
                    .any(|goal| goal == &preference.canonical_goal_id)
            {
                return Err("goal preference is not a validated canonical goal".to_string());
            }
        }
        Ok(())
    }

    pub fn fixed_features(&self) -> [f32; SEMANTIC_PRIOR_FEATURE_DIM] {
        let sign = match self.proposition.polarity {
            SemanticPolarity::Supports => 1.0,
            SemanticPolarity::Contradicts => -1.0,
        };
        let mut features = [0.0; SEMANTIC_PRIOR_FEATURE_DIM];
        features[self.proposition.class.feature_index()] = sign * self.confidence;
        features[self.proposition.relation.feature_index()] = sign * self.confidence;
        features
    }
}

impl SemanticEvidenceRefV2 {
    fn matches_context(&self, context: &SemanticPriorValidationContext) -> bool {
        self.producer_epoch == context.producer_epoch
            && self.runner_epoch == context.runner_epoch
            && self.inference_seq == context.inference_seq
            && self.timestamp_ms == context.timestamp_ms
            && self.camera_seq == context.camera_seq
            && self.lidar_seq == context.lidar_seq
            && self.pose_seq == context.pose_seq
            && self.action_seq == context.action_seq
            && self.physical_model_id == context.physical_model_id
            && self.checkpoint_id == context.checkpoint_id
            && context.now_ms.saturating_sub(self.timestamp_ms)
                <= SEMANTIC_PRIOR_EVIDENCE_MAX_AGE_MS
    }
}

/// Source compatibility for internal call sites. Serialized input is v2 only.
pub type SemanticPriorV1 = SemanticPriorV2;

/// Projects a frozen, versioned view of the priors into L3. The function only
/// reads: no JEPA or Cognition parameter is touched, and every typed feature
/// lands in its own 64-dimension span, so one feature can be traced or ablated
/// exactly.
pub fn project_typed_priors_to_l3(priors: &[SemanticPriorV2]) -> [f32; COGNITION_STATE_DIM] {
    let mut projected = [0.0; COGNITION_STATE_DIM];
    if priors.is_empty() {
        return projected;
    }
    let block = COGNITION_STATE_DIM / SEMANTIC_PRIOR_FEATURE_DIM;
    let divisor = priors.len() as f32;
    for prior in priors {
        for (feature, value) in prior.fixed_features().iter().copied().enumerate() {
            let start = feature * block;
            for (offset, target) in projected[start..start + 64].iter_mut().enumerate() {
                let basis = if (offset + feature) % 2 == 0 { 1.0 } else { -1.0 };
                *target += value * basis / divisor;
            }
        }
    }
    projected
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionCheckpointV1 {
    pub schema_version: String,
    pub checkpoint_id: String,
    pub created_at_ms: u128,
    pub runtime: String,
    pub backend: String,
    pub layer_sequences: Vec<u64>,
    pub state_digest: String,
    pub path: String,
}
