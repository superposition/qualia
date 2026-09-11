//! Bounded rollout proposals and the offline planner comparison lane.
//!
//! Nothing in this module commands hardware. A proposal carries
//! `direct_planner_write`/`direct_motor_write` flags that are hard-wired to
//! false, so a scored rollout can never be mistaken for an actuator write.

use qualia_jepa::{ACTION_DIM, CORE_DIM, GROUNDING_CELLS, GROUNDING_HEIGHT, GROUNDING_WIDTH};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::error::Error;

/// Result alias for the planner surface.
pub type PlannerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Schema literal of a scored rollout proposal.
pub const ROLLOUT_PROPOSAL_SCHEMA: &str = "qualia.jepa-rollout-proposal.v1";
/// Schema literal of an offline planner comparison report.
pub const PLANNER_COMPARISON_SCHEMA: &str = "qualia.jepa-planner-comparison.v1";

/// Hard bounds and cost weights for a rollout evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RolloutPlannerConfig {
    // Candidate search bounds.
    pub max_candidates: usize,
    pub max_steps: usize,
    // Step timing and horizon, in seconds.
    pub min_step_seconds: f32,
    pub max_step_seconds: f32,
    pub max_horizon_seconds: f32,
    // Robot geometry and reach, in metres.
    pub max_goal_distance_m: f32,
    pub max_wheel_speed_mps: f32,
    pub track_width_m: f32,
    pub grounding_resolution_m: f32,
    pub robot_radius_m: f32,
    pub collision_margin_m: f32,
    pub collision_probability_limit: f32,
    // Freshness bound on cited evidence, in milliseconds.
    pub max_evidence_age_ms: u64,
    // Cost weights.
    pub goal_cost_per_meter: f32,
    pub collision_cost: f32,
    pub uncertainty_cost_per_latent_stddev: f32,
    pub action_cost_per_second: f32,
    pub action_slew_cost: f32,
}

impl Default for RolloutPlannerConfig {
    fn default() -> Self {
        // Search bounds.
        Self {
            max_candidates: 64,
            max_steps: 16,
            // Step timing and horizon, in seconds.
            min_step_seconds: 0.05,
            max_step_seconds: 0.5,
            max_horizon_seconds: 4.0,
            // Robot geometry, in metres.
            max_goal_distance_m: 20.0,
            max_wheel_speed_mps: 0.6,
            track_width_m: 0.22,
            grounding_resolution_m: 0.05,
            robot_radius_m: 0.18,
            collision_margin_m: 0.08,
            collision_probability_limit: 0.35,
            // Evidence freshness, in milliseconds.
            max_evidence_age_ms: 500,
            // Cost weights.
            goal_cost_per_meter: 1.0,
            collision_cost: 20.0,
            uncertainty_cost_per_latent_stddev: 0.5,
            action_cost_per_second: 0.05,
            action_slew_cost: 0.1,
        }
    }
}

/// A physical goal expressed in the robot frame, in metres.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalPhysicalGoal {
    pub canonical_goal_id: String,
    pub source: String,
    pub lateral_m: f32,
    pub forward_m: f32,
    pub tolerance_m: f32,
}

/// The exact model and evidence identity a rollout was produced from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RolloutModelProvenance {
    // Model and checkpoint identity.
    pub architecture_id: String,
    pub checkpoint_id: String,
    // Digest evidence.
    pub checkpoint_sha256: String,
    pub training_report_sha256: String,
    pub dataset_digest: String,
    // Epochs and freshness.
    pub producer_epoch: u64,
    pub runner_epoch: u64,
    pub inference_seq: u64,
    pub evidence_age_ms: u64,
    // Independent attestations.
    pub sources_coherent: bool,
    pub checkpoint_verified: bool,
    pub output_finite: bool,
    pub grounding_calibrated: bool,
    pub action_covered: bool,
    pub llm_priors_ablated: bool,
}

/// One wheel-command step inside a candidate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CandidateActionStep {
    pub left: f32,
    pub right: f32,
    pub speed_scale: f32,
    pub dt_seconds: f32,
}

impl CandidateActionStep {
    /// The four-vector the transition predictor consumes.
    pub fn model_action(self) -> [f32; ACTION_DIM] {
        let CandidateActionStep {
            left,
            right,
            speed_scale,
            ..
        } = self;
        [left, right, speed_scale, 1.0]
    }
}

/// A named sequence of candidate steps.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RolloutCandidate {
    pub candidate_id: String,
    pub steps: Vec<CandidateActionStep>,
}

/// A complete, self-describing rollout evaluation request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RolloutEvaluationRequest {
    pub evaluated_at_ns: u64,
    pub source: RolloutModelProvenance,
    pub goal: CanonicalPhysicalGoal,
    pub start_latent: Vec<f32>,
    pub candidates: Vec<RolloutCandidate>,
    pub config: RolloutPlannerConfig,
}

/// One latent transition returned by a rollout predictor.
#[derive(Debug, Clone, PartialEq)]
pub struct PredictedRolloutStep {
    pub mean: Vec<f32>,
    pub log_variance: Vec<f32>,
    pub occupancy_logits: Vec<f32>,
}

/// The minimal predictor seam the rollout evaluator needs.
pub trait RolloutPredictor {
    fn predict_step(
        &self,
        latent: &[f32],
        action: [f32; ACTION_DIM],
        dt_seconds: f32,
    ) -> PlannerResult<PredictedRolloutStep>;
}

/// The cost decomposition of one scored candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RolloutCostTerms {
    pub terminal_goal_distance_m: f32,
    pub max_collision_probability: f32,
    pub mean_latent_uncertainty_stddev: f32,
    pub action_effort_seconds: f32,
    pub action_slew: f32,
    pub weighted_goal_cost: f32,
    pub weighted_collision_cost: f32,
    pub weighted_uncertainty_cost: f32,
    pub weighted_action_cost: f32,
    pub weighted_action_slew_cost: f32,
    pub rollout_cost: f32,
}

/// One candidate after integration and scoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScoredRolloutCandidate {
    pub candidate_id: String,
    pub horizon_steps: usize,
    pub horizon_seconds: f32,
    pub terminal_lateral_m: f32,
    pub terminal_forward_m: f32,
    pub terminal_yaw_rad: f32,
    pub admissible: bool,
    pub cost: RolloutCostTerms,
}

/// A proposal: scored candidates plus the explicitly false authority flags.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RolloutProposal {
    pub schema_version: String,
    pub evaluated_at_ns: u64,
    pub mode: String,
    pub authority: String,
    pub operator_enabled: bool,
    pub canonical_goal_id: String,
    pub model: RolloutModelProvenance,
    pub selected_candidate_id: Option<String>,
    pub candidates: Vec<ScoredRolloutCandidate>,
    pub direct_planner_write: bool,
    pub direct_motor_write: bool,
}

/// Score every candidate and select the cheapest admissible rollout.
///
/// The operator gate is tested first: a disabled proposal lane returns before
/// any validation or predictor call happens.
pub fn evaluate_rollout_proposals<P: RolloutPredictor>(
    predictor: &P,
    request: &RolloutEvaluationRequest,
    operator_enabled: bool,
) -> PlannerResult<RolloutProposal> {
    if !operator_enabled {
        return Err("JEPA rollout proposals are disabled; an operator flag is required".into());
    }
    validate_request(request)?;

    let mut scored = Vec::with_capacity(request.candidates.len());
    for candidate in request.candidates.iter() {
        let entry = score_candidate(
            predictor,
            request.start_latent.as_slice(),
            &request.goal,
            candidate,
            &request.config,
        )?;
        scored.push(entry);
    }
    scored.sort_by(rank_candidates);

    let selected_candidate_id = scored
        .iter()
        .find(|entry| entry.admissible)
        .map(|entry| entry.candidate_id.clone());
    Ok(RolloutProposal {
        schema_version: ROLLOUT_PROPOSAL_SCHEMA.to_owned(),
        evaluated_at_ns: request.evaluated_at_ns,
        mode: "proposal-only".to_owned(),
        authority: "none".to_owned(),
        operator_enabled: true,
        canonical_goal_id: request.goal.canonical_goal_id.clone(),
        model: request.source.clone(),
        selected_candidate_id,
        candidates: scored,
        direct_planner_write: false,
        direct_motor_write: false,
    })
}

/// Cheapest rollout cost first, then candidate ID so ties stay deterministic.
fn rank_candidates(left: &ScoredRolloutCandidate, right: &ScoredRolloutCandidate) -> Ordering {
    match left.cost.rollout_cost.total_cmp(&right.cost.rollout_cost) {
        Ordering::Equal => left.candidate_id.cmp(&right.candidate_id),
        ordering => ordering,
    }
}

fn validate_request(request: &RolloutEvaluationRequest) -> PlannerResult<()> {
    let config = &request.config;
    let latent_shaped = request.start_latent.len() == CORE_DIM
        && request.start_latent.iter().all(|entry| entry.is_finite());
    let candidates_bounded =
        !request.candidates.is_empty() && request.candidates.len() <= config.max_candidates;
    if request.evaluated_at_ns == 0 || !latent_shaped || !candidates_bounded {
        return Err("rollout request has invalid time, latent, or candidate count".into());
    }
    validate_config(config)?;
    validate_source(&request.source, config.max_evidence_age_ms)?;
    validate_goal(&request.goal, config.max_goal_distance_m)?;
    validate_candidates(config, &request.candidates)
}

fn validate_candidates(
    config: &RolloutPlannerConfig,
    candidates: &[RolloutCandidate],
) -> PlannerResult<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for candidate in candidates {
        let horizon_bounded =
            !candidate.steps.is_empty() && candidate.steps.len() <= config.max_steps;
        if candidate.candidate_id.is_empty()
            || !seen.insert(candidate.candidate_id.as_str())
            || !horizon_bounded
        {
            return Err(
                "rollout candidate IDs and horizons must be non-empty, unique, and bounded".into(),
            );
        }
        validate_candidate_steps(config, &candidate.steps)?;
    }
    Ok(())
}

fn validate_candidate_steps(
    config: &RolloutPlannerConfig,
    steps: &[CandidateActionStep],
) -> PlannerResult<()> {
    let mut horizon_seconds = 0.0_f32;
    for step in steps {
        let finite = step.left.is_finite()
            && step.right.is_finite()
            && step.speed_scale.is_finite()
            && step.dt_seconds.is_finite();
        let bounded = (-1.0..=1.0).contains(&step.left)
            && (-1.0..=1.0).contains(&step.right)
            && (0.0..=1.0).contains(&step.speed_scale)
            && (config.min_step_seconds..=config.max_step_seconds).contains(&step.dt_seconds);
        if !finite || !bounded {
            return Err("candidate action is outside the calibrated action/time bounds".into());
        }
        horizon_seconds += step.dt_seconds;
    }
    if horizon_seconds > config.max_horizon_seconds {
        return Err("candidate horizon exceeds the configured bound".into());
    }
    Ok(())
}

fn validate_config(config: &RolloutPlannerConfig) -> PlannerResult<()> {
    let strictly_positive = [
        config.min_step_seconds, config.max_step_seconds, config.max_horizon_seconds,
        config.max_goal_distance_m, config.max_wheel_speed_mps, config.track_width_m,
        config.grounding_resolution_m, config.robot_radius_m,
        config.goal_cost_per_meter, config.collision_cost,
    ];
    let non_negative = [
        config.collision_margin_m, config.uncertainty_cost_per_latent_stddev,
        config.action_cost_per_second, config.action_slew_cost,
    ];
    let counts_present =
        config.max_candidates != 0 && config.max_steps != 0 && config.max_evidence_age_ms != 0;
    let values_positive = strictly_positive
        .iter()
        .all(|entry| entry.is_finite() && *entry > 0.0);
    let values_non_negative = non_negative
        .iter()
        .all(|entry| entry.is_finite() && *entry >= 0.0);
    let steps_ordered = config.min_step_seconds <= config.max_step_seconds;
    let limit_normalized = (0.0..=1.0).contains(&config.collision_probability_limit);
    if !(counts_present
        && values_positive
        && values_non_negative
        && steps_ordered
        && limit_normalized)
    {
        return Err("rollout planner configuration is invalid".into());
    }
    Ok(())
}

fn validate_source(source: &RolloutModelProvenance, max_age_ms: u64) -> PlannerResult<()> {
    let named = !source.architecture_id.is_empty() && !source.checkpoint_id.is_empty();
    let digests = valid_sha256(&source.checkpoint_sha256)
        && valid_sha256(&source.training_report_sha256)
        && valid_sha256(&source.dataset_digest);
    let epochs =
        source.producer_epoch != 0 && source.runner_epoch != 0 && source.inference_seq != 0;
    let fresh = source.evidence_age_ms <= max_age_ms;
    let verified = source.sources_coherent
        && source.checkpoint_verified
        && source.output_finite
        && source.grounding_calibrated
        && source.action_covered
        && source.llm_priors_ablated;
    if !(named && digests && epochs && fresh && verified) {
        return Err(
            "rollout source is stale, unverified, uncalibrated, or language-influenced".into(),
        );
    }
    Ok(())
}

fn validate_goal(goal: &CanonicalPhysicalGoal, max_distance_m: f32) -> PlannerResult<()> {
    let distance_m = goal.lateral_m.hypot(goal.forward_m);
    let named = !goal.canonical_goal_id.is_empty();
    let canonical_source = goal.source == "world.model.v1";
    let finite_pose = goal.lateral_m.is_finite() && goal.forward_m.is_finite();
    let usable_tolerance = goal.tolerance_m.is_finite() && goal.tolerance_m > 0.0;
    let within_range = distance_m.is_finite() && distance_m <= max_distance_m;
    if !(named && canonical_source && finite_pose && usable_tolerance && within_range) {
        return Err("goal must be a bounded canonical physical target in meters".into());
    }
    Ok(())
}

fn score_candidate<P: RolloutPredictor>(
    predictor: &P,
    start_latent: &[f32],
    goal: &CanonicalPhysicalGoal,
    candidate: &RolloutCandidate,
    config: &RolloutPlannerConfig,
) -> PlannerResult<ScoredRolloutCandidate> {
    let footprint_radius_m = config.robot_radius_m + config.collision_margin_m;
    let mut belief = start_latent.to_vec();
    let mut lateral_m = 0.0_f32;
    let mut forward_m = 0.0_f32;
    let mut yaw_rad = 0.0_f32;
    let mut worst_collision = 0.0_f32;
    let mut variance_sum = 0.0_f64;
    let mut variance_terms = 0_u64;
    let mut effort_seconds = 0.0_f32;
    let mut slew_total = 0.0_f32;
    let mut previous_step: Option<CandidateActionStep> = None;
    let mut horizon_seconds = 0.0_f32;

    for step in candidate.steps.iter() {
        let prediction = predictor.predict_step(&belief, step.model_action(), step.dt_seconds)?;
        validate_prediction(&prediction)?;
        let collision = central_collision_probability(
            prediction.occupancy_logits.as_slice(),
            config.grounding_resolution_m,
            footprint_radius_m,
        )?;
        worst_collision = worst_collision.max(collision);
        for entry in prediction.log_variance.iter() {
            variance_sum += f64::from((0.5 * entry.clamp(-20.0, 20.0)).exp());
            variance_terms += 1;
        }
        integrate_kinematics(&mut lateral_m, &mut forward_m, &mut yaw_rad, step, config);
        effort_seconds +=
            ((step.left.abs() + step.right.abs()) * 0.5) * step.speed_scale * step.dt_seconds;
        if let Some(previous) = previous_step {
            slew_total += slew_between(step, &previous);
        }
        previous_step = Some(*step);
        horizon_seconds += step.dt_seconds;
        belief = prediction.mean;
    }

    if variance_terms == 0 {
        return Err("rollout produced no predictive uncertainty".into());
    }
    let mean_latent_uncertainty_stddev = (variance_sum / variance_terms as f64) as f32;
    let terminal_goal_distance_m =
        ((lateral_m - goal.lateral_m).hypot(forward_m - goal.forward_m) - goal.tolerance_m)
            .max(0.0);
    let weighted_goal_cost = terminal_goal_distance_m * config.goal_cost_per_meter;
    let weighted_collision_cost = worst_collision * config.collision_cost;
    let weighted_uncertainty_cost =
        mean_latent_uncertainty_stddev * config.uncertainty_cost_per_latent_stddev;
    let weighted_action_cost = effort_seconds * config.action_cost_per_second;
    let weighted_action_slew_cost = slew_total * config.action_slew_cost;
    let rollout_cost = weighted_goal_cost
        + weighted_collision_cost
        + weighted_uncertainty_cost
        + weighted_action_cost
        + weighted_action_slew_cost;
    if !rollout_cost.is_finite() {
        return Err("rollout cost is non-finite".into());
    }
    Ok(ScoredRolloutCandidate {
        candidate_id: candidate.candidate_id.clone(),
        horizon_steps: candidate.steps.len(),
        horizon_seconds,
        terminal_lateral_m: lateral_m,
        terminal_forward_m: forward_m,
        terminal_yaw_rad: yaw_rad,
        admissible: worst_collision <= config.collision_probability_limit,
        cost: RolloutCostTerms {
            terminal_goal_distance_m,
            max_collision_probability: worst_collision,
            mean_latent_uncertainty_stddev,
            action_effort_seconds: effort_seconds,
            action_slew: slew_total,
            weighted_goal_cost,
            weighted_collision_cost,
            weighted_uncertainty_cost,
            weighted_action_cost,
            weighted_action_slew_cost,
            rollout_cost,
        },
    })
}

/// Advance the robot pose by one differential-drive step.
///
/// The heading used for the translation is the step midpoint, so a fast turn
/// does not let the arc overshoot its endpoint.
fn integrate_kinematics(
    lateral_m: &mut f32,
    forward_m: &mut f32,
    yaw_rad: &mut f32,
    step: &CandidateActionStep,
    config: &RolloutPlannerConfig,
) {
    let left_rate = step.left * step.speed_scale * config.max_wheel_speed_mps;
    let right_rate = step.right * step.speed_scale * config.max_wheel_speed_mps;
    let forward_rate = (left_rate + right_rate) * 0.5;
    let turn_rate = (right_rate - left_rate) / config.track_width_m;
    let heading = *yaw_rad + turn_rate * step.dt_seconds * 0.5;
    *lateral_m += forward_rate * step.dt_seconds * heading.sin();
    *forward_m += forward_rate * step.dt_seconds * heading.cos();
    *yaw_rad = wrap_angle(*yaw_rad + turn_rate * step.dt_seconds);
}

/// Mean absolute command change between two consecutive steps.
fn slew_between(current: &CandidateActionStep, previous: &CandidateActionStep) -> f32 {
    let delta = (current.left - previous.left).abs()
        + (current.right - previous.right).abs()
        + (current.speed_scale - previous.speed_scale).abs();
    delta / 3.0
}

fn validate_prediction(prediction: &PredictedRolloutStep) -> PlannerResult<()> {
    let expected_shapes = [CORE_DIM, CORE_DIM, GROUNDING_CELLS];
    let actual_shapes = [
        prediction.mean.len(),
        prediction.log_variance.len(),
        prediction.occupancy_logits.len(),
    ];
    let shapes_match = actual_shapes == expected_shapes;
    let all_finite = prediction
        .mean
        .iter()
        .chain(prediction.log_variance.iter())
        .chain(prediction.occupancy_logits.iter())
        .all(|value| value.is_finite());
    if !shapes_match || !all_finite {
        return Err("rollout predictor returned an invalid shape or non-finite value".into());
    }
    Ok(())
}

/// Worst-case occupancy probability inside the robot's collision footprint.
fn central_collision_probability(
    logits: &[f32],
    resolution_m: f32,
    radius_m: f32,
) -> PlannerResult<f32> {
    let geometry_ok = logits.len() == GROUNDING_CELLS
        && resolution_m.is_finite()
        && resolution_m > 0.0
        && radius_m.is_finite()
        && radius_m > 0.0;
    if !geometry_ok {
        return Err("collision footprint geometry is invalid".into());
    }
    let half_width = GROUNDING_WIDTH as f32 * 0.5;
    let half_height = GROUNDING_HEIGHT as f32 * 0.5;
    let mut worst = 0.0_f32;
    let mut covered = 0_usize;
    for (row, row_logits) in logits.chunks(GROUNDING_WIDTH).enumerate() {
        let z_m = (row as f32 + 0.5 - half_height) * resolution_m;
        for (column, logit) in row_logits.iter().enumerate() {
            let x_m = (column as f32 + 0.5 - half_width) * resolution_m;
            if x_m.hypot(z_m) > radius_m {
                continue;
            }
            worst = worst.max(sigmoid(*logit));
            covered += 1;
        }
    }
    if covered == 0 {
        return Err("collision footprint selects no grounding cells".into());
    }
    Ok(worst)
}

/// Numerically stable logistic function.
fn sigmoid(value: f32) -> f32 {
    let exponential = (-value.abs()).exp();
    if value < 0.0 {
        exponential / (1.0 + exponential)
    } else {
        1.0 / (1.0 + exponential)
    }
}

/// Fold an angle into `[-pi, pi]`.
fn wrap_angle(mut angle: f32) -> f32 {
    const HALF_TURN: f32 = std::f32::consts::PI;
    const FULL_TURN: f32 = std::f32::consts::TAU;
    while angle > HALF_TURN {
        angle -= FULL_TURN;
    }
    while angle < -HALF_TURN {
        angle += FULL_TURN;
    }
    angle
}

/// One historical decision, with both planners scored against the outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OfflinePlannerDecision {
    pub decision_id: String,
    pub source_session_id: String,
    pub source_mcap_sha256: String,
    pub decision_timestamp_ns: u64,
    pub outcome_timestamp_ns: u64,
    pub observed_collision: bool,
    pub existing_collision_probability: f32,
    pub jepa_collision_probability: f32,
    pub existing_goal_progress_m: f32,
    pub jepa_goal_progress_m: f32,
    pub jepa_proposal_available: bool,
}

/// The evidence identity an offline comparison is allowed to cite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflinePlannerComparisonProvenance {
    pub dataset_digest: String,
    pub checkpoint_id: String,
    pub checkpoint_sha256: String,
    pub training_report_sha256: String,
    pub existing_planner_id: String,
    pub existing_planner_config_sha256: String,
    pub llm_priors_ablated: bool,
}

/// Tolerances applied when comparing the candidate against the existing planner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OfflinePlannerComparisonConfig {
    pub minimum_decisions: usize,
    pub brier_tolerance: f64,
    pub recall_tolerance: f64,
    pub goal_progress_tolerance_m: f64,
}

impl Default for OfflinePlannerComparisonConfig {
    fn default() -> Self {
        Self {
            minimum_decisions: 100,
            brier_tolerance: 0.01,
            recall_tolerance: 0.0,
            goal_progress_tolerance_m: 0.05,
        }
    }
}

/// The frozen result of one offline planner comparison.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OfflinePlannerComparisonReport {
    // Identity and inputs.
    pub schema_version: String,
    pub input_sha256: String,
    pub provenance: OfflinePlannerComparisonProvenance,
    // Evidence coverage.
    pub decision_count: usize,
    pub source_session_count: usize,
    pub source_mcap_count: usize,
    pub collision_positive_count: usize,
    pub proposal_coverage: f64,
    // Collision calibration.
    pub existing_collision_brier: f64,
    pub jepa_collision_brier: f64,
    pub existing_collision_recall: f64,
    pub jepa_collision_recall: f64,
    // Goal progress and verdict.
    pub existing_mean_goal_progress_m: f64,
    pub jepa_mean_goal_progress_m: f64,
    pub passes: bool,
}

/// Running totals accumulated while replaying the recorded decisions.
#[derive(Default)]
struct ComparisonTally {
    existing_squared_error: f64,
    jepa_squared_error: f64,
    observed_collisions: usize,
    existing_detections: usize,
    jepa_detections: usize,
    existing_progress_m: f64,
    jepa_progress_m: f64,
    proposals: usize,
}

impl ComparisonTally {
    fn record(&mut self, decision: &OfflinePlannerDecision) {
        let observed = if decision.observed_collision {
            1.0_f64
        } else {
            0.0_f64
        };
        self.existing_squared_error +=
            (f64::from(decision.existing_collision_probability) - observed).powi(2);
        self.jepa_squared_error +=
            (f64::from(decision.jepa_collision_probability) - observed).powi(2);
        if decision.observed_collision {
            self.observed_collisions += 1;
            self.existing_detections += usize::from(decision.existing_collision_probability >= 0.5);
            self.jepa_detections += usize::from(decision.jepa_collision_probability >= 0.5);
        }
        self.existing_progress_m += f64::from(decision.existing_goal_progress_m);
        self.jepa_progress_m += f64::from(decision.jepa_goal_progress_m);
        self.proposals += usize::from(decision.jepa_proposal_available);
    }
}

/// Compare the candidate and the incumbent planner over recorded decisions.
pub fn compare_offline_planners(
    decisions: &[OfflinePlannerDecision],
    config: &OfflinePlannerComparisonConfig,
    provenance: &OfflinePlannerComparisonProvenance,
    input_sha256: &str,
) -> PlannerResult<OfflinePlannerComparisonReport> {
    if !comparison_inputs_valid(decisions, config, provenance, input_sha256) {
        return Err("offline planner comparison configuration or decisions are invalid".into());
    }

    let mut tally = ComparisonTally::default();
    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut session_ids: HashSet<&str> = HashSet::new();
    let mut capture_digests: HashSet<&str> = HashSet::new();
    for decision in decisions.iter() {
        if !decision_is_well_formed(decision, &mut seen_ids) {
            return Err("offline planner decision is malformed or duplicated".into());
        }
        session_ids.insert(decision.source_session_id.as_str());
        capture_digests.insert(decision.source_mcap_sha256.as_str());
        tally.record(decision);
    }

    let decision_count = decisions.len();
    let count = decision_count as f64;
    let existing_collision_brier = tally.existing_squared_error / count;
    let jepa_collision_brier = tally.jepa_squared_error / count;
    let existing_collision_recall =
        detection_recall(tally.existing_detections, tally.observed_collisions);
    let jepa_collision_recall = detection_recall(tally.jepa_detections, tally.observed_collisions);
    let existing_mean_goal_progress_m = tally.existing_progress_m / count;
    let jepa_mean_goal_progress_m = tally.jepa_progress_m / count;
    let proposal_coverage = tally.proposals as f64 / count;
    let passes = decision_count >= config.minimum_decisions
        && tally.observed_collisions > 0
        && jepa_collision_brier <= existing_collision_brier + config.brier_tolerance
        && jepa_collision_recall + config.recall_tolerance >= existing_collision_recall
        && jepa_mean_goal_progress_m + config.goal_progress_tolerance_m
            >= existing_mean_goal_progress_m
        && proposal_coverage >= 0.95;
    Ok(OfflinePlannerComparisonReport {
        schema_version: PLANNER_COMPARISON_SCHEMA.to_owned(),
        input_sha256: input_sha256.to_owned(),
        provenance: provenance.clone(),
        decision_count,
        source_session_count: session_ids.len(),
        source_mcap_count: capture_digests.len(),
        collision_positive_count: tally.observed_collisions,
        proposal_coverage,
        existing_collision_brier,
        jepa_collision_brier,
        existing_collision_recall,
        jepa_collision_recall,
        existing_mean_goal_progress_m,
        jepa_mean_goal_progress_m,
        passes,
    })
}

/// Recall is defined as 1.0 when the replay contains no collision at all.
fn detection_recall(detections: usize, positives: usize) -> f64 {
    if positives == 0 {
        1.0
    } else {
        detections as f64 / positives as f64
    }
}

fn comparison_inputs_valid(
    decisions: &[OfflinePlannerDecision],
    config: &OfflinePlannerComparisonConfig,
    provenance: &OfflinePlannerComparisonProvenance,
    input_sha256: &str,
) -> bool {
    config.minimum_decisions != 0
        && non_negative(config.brier_tolerance)
        && non_negative(config.recall_tolerance)
        && non_negative(config.goal_progress_tolerance_m)
        && !decisions.is_empty()
        && valid_sha256(input_sha256)
        && valid_comparison_provenance(provenance)
}

fn non_negative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn decision_is_well_formed<'a>(
    decision: &'a OfflinePlannerDecision,
    seen_ids: &mut HashSet<&'a str>,
) -> bool {
    if decision.decision_id.is_empty() || !seen_ids.insert(decision.decision_id.as_str()) {
        return false;
    }
    !decision.source_session_id.is_empty()
        && valid_sha256(&decision.source_mcap_sha256)
        && decision.decision_timestamp_ns != 0
        && decision.outcome_timestamp_ns >= decision.decision_timestamp_ns
        && normalized_probability(decision.existing_collision_probability)
        && normalized_probability(decision.jepa_collision_probability)
        && decision.existing_goal_progress_m.is_finite()
        && decision.jepa_goal_progress_m.is_finite()
}

fn normalized_probability(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn valid_comparison_provenance(provenance: &OfflinePlannerComparisonProvenance) -> bool {
    let digests = valid_sha256(&provenance.dataset_digest)
        && valid_sha256(&provenance.checkpoint_sha256)
        && valid_sha256(&provenance.training_report_sha256)
        && valid_sha256(&provenance.existing_planner_config_sha256);
    let named =
        !provenance.checkpoint_id.is_empty() && !provenance.existing_planner_id.is_empty();
    digests && named && provenance.llm_priors_ablated
}

fn valid_sha256(value: &str) -> bool {
    const SHA256_HEX_LEN: usize = 64;
    value.len() == SHA256_HEX_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FixturePredictor {
        calls: Cell<usize>,
    }

    impl FixturePredictor {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
            }
        }
    }

    impl RolloutPredictor for FixturePredictor {
        fn predict_step(
            &self,
            latent: &[f32],
            action: [f32; ACTION_DIM],
            _dt_seconds: f32,
        ) -> PlannerResult<PredictedRolloutStep> {
            let count = self.calls.get();
            self.calls.set(count + 1);
            let mut mean = latent.to_owned();
            let (left, right) = (action[0], action[1]);
            mean[0] += left + right;
            let mut occupancy_logits = vec![-8.0_f32; GROUNDING_CELLS];
            if left < 0.0 {
                let centre = (GROUNDING_HEIGHT / 2) * GROUNDING_WIDTH + GROUNDING_WIDTH / 2;
                occupancy_logits[centre] = 8.0;
            }
            Ok(PredictedRolloutStep {
                mean,
                log_variance: vec![-4.0_f32; CORE_DIM],
                occupancy_logits,
            })
        }
    }

    fn step(left: f32, right: f32) -> CandidateActionStep {
        CandidateActionStep {
            left,
            right,
            speed_scale: 1.0,
            dt_seconds: 0.2,
        }
    }

    fn provenance() -> RolloutModelProvenance {
        let digest = |seed: &str| seed.repeat(64);
        RolloutModelProvenance {
            architecture_id: "qualia.jepa.grounded-tiny-cnn.v1".into(),
            checkpoint_id: "checkpoint-1".into(),
            checkpoint_sha256: digest("a"),
            training_report_sha256: digest("c"),
            dataset_digest: digest("b"),
            producer_epoch: 7,
            runner_epoch: 2,
            inference_seq: 99,
            evidence_age_ms: 20,
            sources_coherent: true,
            checkpoint_verified: true,
            output_finite: true,
            grounding_calibrated: true,
            action_covered: true,
            llm_priors_ablated: true,
        }
    }

    fn request() -> RolloutEvaluationRequest {
        RolloutEvaluationRequest {
            evaluated_at_ns: 1,
            source: provenance(),
            goal: CanonicalPhysicalGoal {
                canonical_goal_id: "world/canonical/goal-1".into(),
                source: "world.model.v1".into(),
                lateral_m: 0.0,
                forward_m: 0.12,
                tolerance_m: 0.05,
            },
            start_latent: vec![0.0; CORE_DIM],
            candidates: vec![
                RolloutCandidate {
                    candidate_id: "forward".into(),
                    steps: vec![step(1.0, 1.0)],
                },
                RolloutCandidate {
                    candidate_id: "collision".into(),
                    steps: vec![step(-1.0, 1.0)],
                },
            ],
            config: RolloutPlannerConfig::default(),
        }
    }

    #[test]
    fn a_disabled_operator_flag_never_touches_the_model() {
        let predictor = FixturePredictor::new();
        let error = evaluate_rollout_proposals(&predictor, &request(), false).unwrap_err();
        assert!(error.to_string().contains("operator flag"));
        assert_eq!(predictor.calls.get(), 0);
    }

    #[test]
    fn proposals_are_bounded_and_never_authoritative() {
        let predictor = FixturePredictor::new();
        let proposal = evaluate_rollout_proposals(&predictor, &request(), true).unwrap();
        assert_eq!(proposal.mode, "proposal-only");
        assert_eq!(proposal.authority, "none");
        assert!(!proposal.direct_planner_write);
        assert!(!proposal.direct_motor_write);
        assert_eq!(proposal.selected_candidate_id.as_deref(), Some("forward"));
        let collision = proposal
            .candidates
            .iter()
            .find(|entry| entry.candidate_id == "collision")
            .expect("the collision candidate is present");
        assert!(!collision.admissible);
        assert!(collision.cost.max_collision_probability > 0.99);
    }

    #[test]
    fn language_influenced_goals_and_out_of_bounds_actions_fail_closed() {
        let predictor = FixturePredictor::new();

        let mut language_goal = request();
        language_goal.goal.source = "llm".into();
        assert!(evaluate_rollout_proposals(&predictor, &language_goal, true).is_err());

        let mut language_source = request();
        language_source.source.llm_priors_ablated = false;
        assert!(evaluate_rollout_proposals(&predictor, &language_source, true).is_err());

        let mut saturated_action = request();
        saturated_action.candidates[0].steps[0].left = 1.01;
        assert!(evaluate_rollout_proposals(&predictor, &saturated_action, true).is_err());
    }
}
