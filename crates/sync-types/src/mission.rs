use serde::{Deserialize, Serialize};

pub const MISSION_ENVELOPE_SCHEMA_VERSION: &str = "qualia.mission-envelope.v1";
pub const MISSION_EVENT_SCHEMA_VERSION: &str = "qualia.mission-event.v1";
/// Largest integer that survives a JSON round trip through JavaScript exactly.
pub const JSON_SAFE_INTEGER_MAX: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionCommandV1 {
    Start,
    Pause,
    Resume,
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionObjectiveKindV1 {
    ExploreFrontier,
    ObservePoint,
    NavigateTo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MissionObjectiveV1 {
    pub kind: MissionObjectiveKindV1,
    pub summary: String,
    pub target_x_m: Option<f32>,
    pub target_y_m: Option<f32>,
    pub tolerance_m: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MissionOperatingAreaV1 {
    pub frame_id: String,
    pub min_x_m: f32,
    pub min_y_m: f32,
    pub max_x_m: f32,
    pub max_y_m: f32,
}

impl MissionOperatingAreaV1 {
    pub fn contains(&self, x_m: f32, y_m: f32) -> bool {
        x_m >= self.min_x_m && x_m <= self.max_x_m && y_m >= self.min_y_m && y_m <= self.max_y_m
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MissionConstraintsV1 {
    pub operating_area: MissionOperatingAreaV1,
    pub speed_ceiling_mps: f32,
    pub max_distance_m: f32,
    pub max_runtime_ms: u64,
    pub max_replans: u8,
    pub evidence_max_age_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MissionEnvelopeV1 {
    pub schema_version: String,
    pub broker_id: String,
    pub producer_epoch: u64,
    pub sequence: u64,
    pub mission_id: String,
    pub idempotency_key: String,
    pub command: MissionCommandV1,
    pub issued_at_ms: u128,
    pub deadline_ms: u128,
    pub objective: MissionObjectiveV1,
    pub constraints: MissionConstraintsV1,
    pub evidence_refs: Vec<String>,
    /// Set once the connectome prior is loaded; older envelopes decode with the
    /// default of `false` because the field is serde-defaulted.
    #[serde(default)]
    pub fly_governed: bool,
}

impl MissionEnvelopeV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MISSION_ENVELOPE_SCHEMA_VERSION {
            return Err("mission envelope has an unsupported schema".to_string());
        }
        for (field, value) in [
            ("broker_id", self.broker_id.as_str()),
            ("mission_id", self.mission_id.as_str()),
            ("idempotency_key", self.idempotency_key.as_str()),
        ] {
            validate_identifier(field, value)?;
        }
        if self.producer_epoch == 0
            || self.producer_epoch > JSON_SAFE_INTEGER_MAX
            || self.sequence == 0
            || self.sequence > JSON_SAFE_INTEGER_MAX
        {
            return Err(
                "mission producer epoch and sequence must be non-zero JSON-safe integers"
                    .to_string(),
            );
        }
        if self.issued_at_ms == 0
            || self.issued_at_ms > JSON_SAFE_INTEGER_MAX as u128
            || self.deadline_ms > JSON_SAFE_INTEGER_MAX as u128
            || self.deadline_ms <= self.issued_at_ms
            || self.deadline_ms.saturating_sub(self.issued_at_ms) > 120_000
        {
            return Err("mission deadline must be within 120 seconds of issue time".to_string());
        }
        if self.objective.summary.trim().is_empty() || self.objective.summary.len() > 512 {
            return Err("mission objective summary must contain 1..=512 characters".to_string());
        }
        let has_target = self.objective.target_x_m.is_some() && self.objective.target_y_m.is_some();
        if matches!(
            self.objective.kind,
            MissionObjectiveKindV1::ObservePoint | MissionObjectiveKindV1::NavigateTo
        ) && !has_target
        {
            return Err("point objectives require target_x_m and target_y_m".to_string());
        }
        if self.objective.target_x_m.is_some() != self.objective.target_y_m.is_some() {
            return Err("mission target coordinates must be supplied together".to_string());
        }
        for value in [self.objective.target_x_m, self.objective.target_y_m]
            .into_iter()
            .flatten()
        {
            if !value.is_finite() {
                return Err("mission target coordinates must be finite".to_string());
            }
        }
        if self
            .objective
            .tolerance_m
            .is_some_and(|value| !value.is_finite() || !(0.05..=1.0).contains(&value))
        {
            return Err("mission tolerance_m must be in 0.05..=1.0".to_string());
        }
        let area = &self.constraints.operating_area;
        if area.frame_id != "odom"
            || [area.min_x_m, area.min_y_m, area.max_x_m, area.max_y_m]
                .iter()
                .any(|value| !value.is_finite())
            || area.min_x_m >= area.max_x_m
            || area.min_y_m >= area.max_y_m
            || area.max_x_m - area.min_x_m > 20.0
            || area.max_y_m - area.min_y_m > 20.0
        {
            return Err(
                "mission operating area must be a finite odom rectangle no larger than 20 m"
                    .to_string(),
            );
        }
        if let (Some(x), Some(y)) = (self.objective.target_x_m, self.objective.target_y_m) {
            if !area.contains(x, y) {
                return Err("mission target is outside the bounded operating area".to_string());
            }
        }
        if !self.constraints.speed_ceiling_mps.is_finite()
            || !(0.01..=0.25).contains(&self.constraints.speed_ceiling_mps)
            || !self.constraints.max_distance_m.is_finite()
            || !(0.05..=5.0).contains(&self.constraints.max_distance_m)
            || !(100..=120_000).contains(&self.constraints.max_runtime_ms)
            || self.constraints.max_replans > 8
            || !(100..=5_000).contains(&self.constraints.evidence_max_age_ms)
        {
            return Err("mission constraints exceed bounded low-speed limits".to_string());
        }
        if self.evidence_refs.len() > 64
            || self
                .evidence_refs
                .iter()
                .any(|value| value.trim().is_empty() || value.len() > 256)
        {
            return Err("mission evidence_refs are invalid".to_string());
        }
        if self.command == MissionCommandV1::Start && self.evidence_refs.is_empty() {
            return Err("mission start requires at least one evidence reference".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatusV1 {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl MissionStatusV1 {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionEventKindV1 {
    Accepted,
    AwaitingOperatorLease,
    Planning,
    PlanProposed,
    Started,
    Replanned,
    Paused,
    StopVerified,
    Completed,
    Failed,
    Cancelled,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MissionEventV1 {
    pub schema_version: String,
    pub producer_epoch: u64,
    pub sequence: u64,
    pub event_id: String,
    pub mission_id: String,
    pub mission_idempotency_key: String,
    pub event_kind: MissionEventKindV1,
    pub status: MissionStatusV1,
    pub occurred_at_ms: u128,
    pub code: String,
    pub detail: String,
    pub evidence_refs: Vec<String>,
    pub plan_id: Option<String>,
}

impl MissionEventV1 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MISSION_EVENT_SCHEMA_VERSION {
            return Err("mission event has an unsupported schema".to_string());
        }
        validate_identifier("event_id", &self.event_id)?;
        validate_identifier("mission_id", &self.mission_id)?;
        validate_identifier("mission_idempotency_key", &self.mission_idempotency_key)?;
        if self.producer_epoch == 0
            || self.producer_epoch > JSON_SAFE_INTEGER_MAX
            || self.sequence == 0
            || self.sequence > JSON_SAFE_INTEGER_MAX
            || self.occurred_at_ms == 0
            || self.occurred_at_ms > JSON_SAFE_INTEGER_MAX as u128
        {
            return Err(
                "mission event ordering and timestamp must be non-zero JSON-safe integers"
                    .to_string(),
            );
        }
        if self.code.trim().is_empty()
            || self.code.len() > 128
            || self.detail.len() > 2_048
            || self.evidence_refs.len() > 64
        {
            return Err("mission event detail is invalid".to_string());
        }
        Ok(())
    }
}

fn validate_identifier(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Err(format!("{field} must contain 1..=160 URL-safe characters"))
    } else {
        Ok(())
    }
}
