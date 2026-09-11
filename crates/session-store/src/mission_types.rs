//! Mission, planning and world-model record types shared by the session store.
//!
//! Every enum here is persisted as its `snake_case` JSON text so that rows stay
//! readable in a SQLite browser and stable across releases.

use serde::{Deserialize, Serialize};

/// What an `analysis_jobs` row was queued to do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisJobKind {
    GraphInference,
    EnvironmentMerge,
    Planning,
    CoachScenario,
}

/// Lifecycle of an `analysis_jobs` row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisJobStatus {
    Queued,
    Running,
    Complete,
    Failed,
}

/// A stored analysis job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisJobRow {
    pub id: i64,
    pub session_id: i64,
    pub environment_id: Option<i64>,
    pub job_kind: AnalysisJobKind,
    pub status: AnalysisJobStatus,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub window_start_sec: Option<f64>,
    pub window_end_sec: Option<f64>,
    pub spec_json: String,
    pub summary_json: String,
    pub failure_json: Option<String>,
}

/// Which abstraction graph a fragment belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphKind {
    Sensorimotor,
    LocalSpace,
    Navigation,
    Object,
    Task,
    Communication,
}

/// The factor-graph topology of a fragment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphForm {
    Tree,
    JunctionTree,
    Loopy,
}

/// How faithfully the fragment's inference tracked the exact posterior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExactnessKind {
    Exact,
    Approximate,
    Failed,
}

/// A factor-graph fragment cut from one stream window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphFragmentRow {
    pub id: i64,
    pub analysis_job_id: i64,
    pub session_id: i64,
    pub epoch_id: Option<i64>,
    pub stream_key: String,
    pub fragment_key: String,
    pub graph_kind: GraphKind,
    pub graph_form: GraphForm,
    pub exactness: ExactnessKind,
    pub variable_count: i64,
    pub factor_count: i64,
    pub tree_width: Option<i64>,
    pub root_variable_key: Option<String>,
    pub window_start_sec: f64,
    pub window_end_sec: f64,
    pub summary_json: String,
}

/// The domain a belief variable ranges over.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DomainKind {
    Binary,
    Discrete,
    Gaussian,
    Hybrid,
}

/// A marginal belief over one fragment variable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeliefMatrixRow {
    pub id: i64,
    pub fragment_id: i64,
    pub variable_key: String,
    pub domain_kind: DomainKind,
    pub normalization_error: f64,
    pub entropy: f64,
    pub max_state_key: String,
    pub values_json: String,
    pub matrix_hash: String,
}

/// Which way a belief-propagation message travelled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    VariableToFactor,
    FactorToVariable,
}

/// A single belief-propagation message captured at an iteration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageSnapshotRow {
    pub id: i64,
    pub fragment_id: i64,
    pub edge_key: String,
    pub direction: MessageDirection,
    pub iteration_index: i64,
    pub source_node_key: String,
    pub target_node_key: String,
    pub values_json: String,
    pub residual_norm: f64,
    pub message_hash: String,
}

/// The geometric role of a world region.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    LocalSpace,
    Corridor,
    ObstacleCluster,
    LandmarkField,
    Unknown,
}

/// A region lifted from a session into the environment model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldRegionRow {
    pub id: i64,
    pub environment_id: Option<i64>,
    pub session_id: i64,
    pub source_fragment_id: i64,
    pub region_key: String,
    pub region_kind: RegionKind,
    pub support_point_count: i64,
    pub confidence: f64,
    pub centroid_json: String,
    pub bounds_json: String,
    pub signature_hash: String,
    pub metadata_json: String,
}

/// Why two regions are thought to overlap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Overlap,
    TransformAlignment,
    SharedObject,
    SharedTask,
}

/// Review state of a proposed link or merge candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Proposed,
    Accepted,
    Rejected,
    Conflicted,
}

/// A scored link between two world regions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegionLinkRow {
    pub id: i64,
    pub left_region_id: i64,
    pub right_region_id: i64,
    pub link_kind: LinkKind,
    pub score: f64,
    pub relative_transform_json: String,
    pub contradiction_score: f64,
    pub state: CandidateState,
    pub evidence_json: String,
}

/// A candidate for merging two sessions, scored from their region pairs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMergeCandidateRow {
    pub id: i64,
    pub left_session_id: i64,
    pub right_session_id: i64,
    pub left_region_id: i64,
    pub right_region_id: i64,
    pub candidate_kind: LinkKind,
    pub score: f64,
    pub transform_consistency: f64,
    pub contradiction_score: f64,
    pub state: CandidateState,
    pub reason_json: String,
}

/// Terminal state of a planner run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Queued,
    Complete,
    Failed,
    NoFeasiblePath,
}

/// One planning run over the environment graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanRunRow {
    pub id: i64,
    pub environment_id: i64,
    pub session_id: Option<i64>,
    pub source_region_id: Option<i64>,
    pub target_region_id: Option<i64>,
    pub planner_kind: String,
    pub status: PlanStatus,
    pub path_cost: f64,
    pub risk_score: f64,
    pub clearance_min_ft: f64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub summary_json: String,
}

/// A coach's review of one scenario.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoachScenarioReviewRow {
    pub id: i64,
    pub environment_id: i64,
    pub plan_run_id: Option<i64>,
    pub generated_at: String,
    pub risk_score: f64,
    pub confidence_score: f64,
    pub summary_json: String,
}

/// A gate the vehicle passed, recorded for later review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateReviewEntryRow {
    pub id: i64,
    pub session_id: i64,
    pub gate_number: i64,
    pub gate_label: String,
    pub track_section: String,
    pub frame_index: Option<i64>,
    pub timestamp_sec: Option<f64>,
    pub confidence_score: f64,
    pub pass_order: Option<i64>,
    pub status: String,
    pub source: String,
    pub note: String,
    pub summary_json: String,
}

/// A reusable mission definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionPortfolioRow {
    pub id: i64,
    pub portfolio_key: String,
    pub mission_type: String,
    pub mission_subtype: String,
    pub display_name: String,
    pub description: String,
    pub rubric_json: String,
    pub metadata_json: String,
}

/// One attempted execution of a portfolio mission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionInstanceRow {
    pub id: i64,
    pub portfolio_id: i64,
    pub session_id: Option<i64>,
    pub instance_key: String,
    pub title: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub objective_summary: String,
    pub summary_json: String,
}

/// A phase inside a mission instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionPhaseRow {
    pub id: i64,
    pub mission_instance_id: i64,
    pub phase_key: String,
    pub phase_kind: String,
    pub window_start_sec: f64,
    pub window_end_sec: Option<f64>,
    pub status: String,
    pub summary_json: String,
}

/// The graded outcome of a mission instance or one of its phases.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeAssessmentRow {
    pub id: i64,
    pub mission_instance_id: i64,
    pub phase_id: Option<i64>,
    pub assessment_status: String,
    pub overall_result: String,
    pub scorecard_json: String,
    pub evidence_json: String,
    pub recommended_plan_revision: String,
    pub assessor: String,
    pub assessed_at: String,
    pub summary_json: String,
}

impl BeliefMatrixRow {
    pub fn is_normalized(&self, tolerance: f64) -> bool {
        self.normalization_error.abs() <= tolerance
    }
}

impl GraphFragmentRow {
    pub fn is_exact_tree_family(&self) -> bool {
        matches!(self.graph_form, GraphForm::Tree | GraphForm::JunctionTree)
            && matches!(self.exactness, ExactnessKind::Exact)
    }
}

impl RegionLinkRow {
    pub fn score_is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.score) && (0.0..=1.0).contains(&self.contradiction_score)
    }
}

impl SessionMergeCandidateRow {
    pub fn score_is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.score)
            && (0.0..=1.0).contains(&self.transform_consistency)
            && (0.0..=1.0).contains(&self.contradiction_score)
    }
}

impl WorldRegionRow {
    pub fn confidence_is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.confidence) && self.support_point_count >= 0
    }
}

impl PlanRunRow {
    pub fn risk_is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.risk_score) && self.clearance_min_ft >= 0.0
    }
}

impl CoachScenarioReviewRow {
    pub fn scores_are_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.risk_score) && (0.0..=1.0).contains(&self.confidence_score)
    }
}

impl GateReviewEntryRow {
    pub fn score_is_valid(&self) -> bool {
        self.gate_number > 0
            && (0.0..=1.0).contains(&self.confidence_score)
            && self.pass_order.is_none_or(|value| value > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_tree_family_needs_a_tree_and_exact_inference() {
        let mut fragment = GraphFragmentRow {
            id: 1,
            analysis_job_id: 1,
            session_id: 1,
            epoch_id: None,
            stream_key: "ego".into(),
            fragment_key: "tree_3var_binary".into(),
            graph_kind: GraphKind::Sensorimotor,
            graph_form: GraphForm::Tree,
            exactness: ExactnessKind::Exact,
            variable_count: 3,
            factor_count: 2,
            tree_width: Some(1),
            root_variable_key: Some("x1".into()),
            window_start_sec: 0.0,
            window_end_sec: 1.0,
            summary_json: "{}".into(),
        };
        assert!(fragment.is_exact_tree_family());

        fragment.graph_form = GraphForm::Loopy;
        fragment.exactness = ExactnessKind::Approximate;
        assert!(!fragment.is_exact_tree_family());
    }

    #[test]
    fn normalization_gate_tracks_the_reported_error() {
        let belief = BeliefMatrixRow {
            id: 1,
            fragment_id: 1,
            variable_key: "x1".into(),
            domain_kind: DomainKind::Discrete,
            normalization_error: 1e-9,
            entropy: 0.42,
            max_state_key: "true".into(),
            values_json: r#"{"true":0.73,"false":0.27}"#.into(),
            matrix_hash: "abc".into(),
        };
        assert!(belief.is_normalized(1e-6));
        assert!(!belief.is_normalized(1e-12));
    }

    #[test]
    fn merge_candidate_rejects_out_of_range_scores() {
        let candidate = SessionMergeCandidateRow {
            id: 1,
            left_session_id: 1,
            right_session_id: 2,
            left_region_id: 10,
            right_region_id: 20,
            candidate_kind: LinkKind::Overlap,
            score: 0.88,
            transform_consistency: 0.93,
            contradiction_score: 0.04,
            state: CandidateState::Proposed,
            reason_json: "{}".into(),
        };
        assert!(candidate.score_is_valid());
        assert!(!SessionMergeCandidateRow {
            score: 1.2,
            ..candidate
        }
        .score_is_valid());
    }

    #[test]
    fn row_validators_bound_their_ranges() {
        let region = WorldRegionRow {
            id: 1,
            environment_id: Some(1),
            session_id: 1,
            source_fragment_id: 1,
            region_key: "yard_a".into(),
            region_kind: RegionKind::LocalSpace,
            support_point_count: 221,
            confidence: 0.81,
            centroid_json: "[0.0,0.0,0.0]".into(),
            bounds_json: "{}".into(),
            signature_hash: "sig".into(),
            metadata_json: "{}".into(),
        };
        assert!(region.confidence_is_valid());

        let plan = PlanRunRow {
            id: 1,
            environment_id: 1,
            session_id: Some(1),
            source_region_id: Some(10),
            target_region_id: Some(20),
            planner_kind: "deliberate_route".into(),
            status: PlanStatus::Complete,
            path_cost: 12.3,
            risk_score: 0.18,
            clearance_min_ft: 1.25,
            started_at: "2026-04-06T13:00:00Z".into(),
            completed_at: Some("2026-04-06T13:00:01Z".into()),
            summary_json: "{}".into(),
        };
        assert!(plan.risk_is_valid());

        let review = CoachScenarioReviewRow {
            id: 1,
            environment_id: 1,
            plan_run_id: Some(1),
            generated_at: "2026-04-06T13:00:02Z".into(),
            risk_score: 0.41,
            confidence_score: 0.76,
            summary_json: "{}".into(),
        };
        assert!(review.scores_are_valid());
    }

    #[test]
    fn enums_persist_as_snake_case_text() {
        assert_eq!(
            serde_json::to_string(&AnalysisJobKind::CoachScenario).unwrap(),
            "\"coach_scenario\""
        );
        assert_eq!(
            serde_json::to_string(&CandidateState::Conflicted).unwrap(),
            "\"conflicted\""
        );
        assert_eq!(
            serde_json::from_str::<PlanStatus>("\"no_feasible_path\"").unwrap(),
            PlanStatus::NoFeasiblePath
        );
    }
}
