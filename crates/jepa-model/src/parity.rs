//! CPU/accelerator parity harness.
//!
//! A checkpoint generation is loaded once on the CPU reference and once on the
//! requested target device; both runtimes then replay one immutable fixture.
//! Every published head is scored after a single transition and after an
//! eight-step latent rollout, and the observed agreement is returned as a
//! report. Persisting that report is the caller's decision.

use crate::runtime::{CoherentJepaRuntime, RuntimeInput, RuntimeOutput, RUNTIME_ID};
use crate::{device_for_backend, ModelResult, ARCHITECTURE_ID};
use candle_core::Device;
use qualia_jepa::{pack_observation, PoseFeatures, ACTION_DIM, LIDAR_BINS};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// Schema literal of a persisted parity report.
pub const PARITY_REPORT_SCHEMA: &str = "qualia.jepa-parity-report.v1";
/// Schema literal of the built-in parity fixture.
pub const PARITY_FIXTURE_SCHEMA: &str = "qualia.jepa-parity-fixture.v1";
/// Number of rollout steps the harness always evaluates.
pub const PARITY_ROLLOUT_STEPS: usize = 8;

/// The three frozen gates a parity metric must satisfy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParityThresholds {
    /// Largest tolerated root-mean-square deviation.
    pub rmse_max: f64,
    /// Largest tolerated single-element absolute deviation.
    pub max_abs_max: f64,
    /// Smallest tolerated cosine agreement.
    pub cosine_min: f64,
}

impl Default for ParityThresholds {
    fn default() -> Self {
        ParityThresholds { rmse_max: 2.0e-4, max_abs_max: 2.0e-3, cosine_min: 0.99999 }
    }
}

impl ParityThresholds {
    /// Reject thresholds that could never fail, or that are not finite.
    pub fn validate(self) -> ModelResult<Self> {
        let usable_bound = |bound: f64| bound.is_finite() && bound >= 0.0;
        let usable_cosine = self.cosine_min.is_finite() && (0.0..=1.0).contains(&self.cosine_min);
        if usable_bound(self.rmse_max) && usable_bound(self.max_abs_max) && usable_cosine {
            return Ok(self);
        }
        Err("invalid parity thresholds".into())
    }
}

/// Aggregate agreement of one compared tensor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParityMetric {
    /// Number of elements compared.
    pub values: usize,
    /// Root-mean-square deviation over the pair.
    pub rmse: f64,
    /// Largest absolute deviation over the pair.
    pub max_abs: f64,
    /// Cosine agreement of the two vectors.
    pub cosine: f64,
    /// Whether every frozen gate accepted this pair.
    pub passes: bool,
}

/// Synchronized latency samples and their percentiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParityLatency {
    /// Every synchronized sample, ascending.
    pub synchronized_samples_us: Vec<u64>,
    /// Median sample.
    pub p50_us: u64,
    /// 95th-percentile sample.
    pub p95_us: u64,
    /// Largest sample.
    pub max_us: u64,
}

/// The complete parity report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParityReport {
    /// Schema literal this report was written against.
    pub schema_version: String,
    /// Architecture identity shared by both runtimes.
    pub architecture_id: String,
    /// Runtime identity shared by both runtimes.
    pub runtime_id: String,
    /// Generation name the checkpoint directory declared.
    pub checkpoint_id: String,
    /// Digest of the exact weight bytes both runtimes loaded.
    pub weights_sha256: String,
    /// Schema literal of the replayed fixture.
    pub fixture_schema: String,
    /// Digest of the replayed fixture bytes.
    pub fixture_sha256: String,
    /// Rollout length the harness evaluated.
    pub rollout_steps: usize,
    /// Backend used as the reference side.
    pub reference_backend: String,
    /// Backend under test.
    pub target_backend: String,
    /// Gates applied to every metric.
    pub thresholds: ParityThresholds,
    /// Per-head agreement after the first transition.
    pub single_step: BTreeMap<String, ParityMetric>,
    /// Per-head agreement accumulated over the full rollout.
    pub eight_step: BTreeMap<String, ParityMetric>,
    /// Latency observed on the reference backend.
    pub reference_latency: ParityLatency,
    /// Latency observed on the target backend.
    pub target_latency: ParityLatency,
    /// Whether every published value stayed finite.
    pub outputs_finite: bool,
    /// Whether every gate passed on every head.
    pub passes: bool,
}

struct ParityFixture {
    input: RuntimeInput,
    actions: [[f32; ACTION_DIM]; PARITY_ROLLOUT_STEPS],
    steps_dt: [f32; PARITY_ROLLOUT_STEPS],
    sha256: String,
}

struct RuntimeTrace {
    single: RuntimeOutput,
    mean: Vec<f32>,
    log_variance: Vec<f32>,
    occupancy: Vec<f32>,
    latencies: Vec<u64>,
}

/// Compare one immutable checkpoint between CPU and `target_backend`.
pub fn compare_checkpoint_backends(
    checkpoint_dir: impl AsRef<Path>,
    target_backend: &str,
    thresholds: ParityThresholds,
) -> ModelResult<ParityReport> {
    let thresholds = thresholds.validate()?;
    let directory = checkpoint_dir.as_ref();
    let reference = CoherentJepaRuntime::from_checkpoint(directory, Device::Cpu)?;
    let target =
        CoherentJepaRuntime::from_checkpoint(directory, device_for_backend(target_backend)?)?;
    compare_runtimes(reference, target, target_backend, thresholds)
}

/// Score one head per name; every pair must satisfy the thresholds.
fn compare_heads(
    heads: &[(&str, &[f32], &[f32])],
    thresholds: ParityThresholds,
) -> ModelResult<BTreeMap<String, ParityMetric>> {
    let mut compared = BTreeMap::new();
    for (head, reference, target) in heads {
        compared.insert((*head).to_string(), compare_tensors(reference, target, thresholds)?);
    }
    Ok(compared)
}

fn compare_runtimes(
    reference: CoherentJepaRuntime,
    target: CoherentJepaRuntime,
    target_backend: &str,
    thresholds: ParityThresholds,
) -> ModelResult<ParityReport> {
    let same_generation = reference.checkpoint_id() == target.checkpoint_id()
        && reference.weights_sha256() == target.weights_sha256();
    if !same_generation {
        return Err("parity runtimes did not load the same immutable checkpoint".into());
    }
    let generation = reference.checkpoint_id().to_string();
    let weights_digest = reference.weights_sha256().to_string();
    let fixture = build_fixture()?;
    let reference_trace = record_trace(&reference, &fixture)?;
    let target_trace = record_trace(&target, &fixture)?;

    let single_step = compare_heads(
        &[
            (
                "latent",
                reference_trace.single.latent.as_slice(),
                target_trace.single.latent.as_slice(),
            ),
            (
                "predicted_mean",
                reference_trace.single.predicted_mean.as_slice(),
                target_trace.single.predicted_mean.as_slice(),
            ),
            (
                "predicted_log_variance",
                reference_trace.single.predicted_log_variance.as_slice(),
                target_trace.single.predicted_log_variance.as_slice(),
            ),
            (
                "evidence",
                reference_trace.single.evidence.as_slice(),
                target_trace.single.evidence.as_slice(),
            ),
            (
                "occupancy_logits",
                reference_trace.single.occupancy_logits.as_slice(),
                target_trace.single.occupancy_logits.as_slice(),
            ),
        ],
        thresholds,
    )?;
    let eight_step = compare_heads(
        &[
            (
                "predicted_mean",
                reference_trace.mean.as_slice(),
                target_trace.mean.as_slice(),
            ),
            (
                "predicted_log_variance",
                reference_trace.log_variance.as_slice(),
                target_trace.log_variance.as_slice(),
            ),
            (
                "occupancy_logits",
                reference_trace.occupancy.as_slice(),
                target_trace.occupancy.as_slice(),
            ),
        ],
        thresholds,
    )?;

    let finite = |values: &[f32]| values.iter().all(|value| value.is_finite());
    let rollout_finite = [
        reference_trace.mean.as_slice(),
        reference_trace.log_variance.as_slice(),
        reference_trace.occupancy.as_slice(),
        target_trace.mean.as_slice(),
        target_trace.log_variance.as_slice(),
        target_trace.occupancy.as_slice(),
    ]
    .iter()
    .all(|values| finite(values));
    let outputs_finite =
        reference_trace.single.is_finite() && target_trace.single.is_finite() && rollout_finite;

    let gates_hold = |metrics: &BTreeMap<String, ParityMetric>| {
        metrics.values().all(|entry| entry.passes)
    };
    let passes = outputs_finite && gates_hold(&single_step) && gates_hold(&eight_step);

    Ok(ParityReport {
        schema_version: PARITY_REPORT_SCHEMA.to_string(),
        architecture_id: ARCHITECTURE_ID.to_string(),
        runtime_id: RUNTIME_ID.to_string(),
        checkpoint_id: generation,
        weights_sha256: weights_digest,
        fixture_schema: PARITY_FIXTURE_SCHEMA.to_string(),
        fixture_sha256: fixture.sha256,
        rollout_steps: PARITY_ROLLOUT_STEPS,
        reference_backend: "cpu".to_string(),
        target_backend: target_backend.to_string(),
        thresholds,
        single_step,
        eight_step,
        reference_latency: summarize_latency(reference_trace.latencies),
        target_latency: summarize_latency(target_trace.latencies),
        outputs_finite,
        passes,
    })
}

/// Hash the fixture in its frozen byte order: schema, observation, action,
/// delta, every rollout action row, then every rollout timestep.
fn fixture_sha256(
    input: &RuntimeInput,
    actions: &[[f32; ACTION_DIM]; PARITY_ROLLOUT_STEPS],
    steps_dt: &[f32; PARITY_ROLLOUT_STEPS],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PARITY_FIXTURE_SCHEMA.as_bytes());
    let scalars = input
        .observation
        .iter()
        .chain(input.applied_action.iter())
        .chain(std::iter::once(&input.delta_seconds));
    for scalar in scalars {
        hasher.update(scalar.to_le_bytes());
    }
    for row in actions {
        for component in row {
            hasher.update(component.to_le_bytes());
        }
    }
    for step in steps_dt {
        hasher.update(step.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Build the fixed fixture and its digest.
fn build_fixture() -> ModelResult<ParityFixture> {
    let camera_value = |index: usize| ((index * 29 + index / 7 + 31) % 256) as f32 / 255.0;
    let camera = (0..qualia_jepa::CAMERA_PIXELS).map(camera_value).collect::<Vec<_>>();
    let range_value = |index: usize| ((index * 17 + 13) % 997) as f32 / 996.0;
    let ranges = (0..LIDAR_BINS).map(range_value).collect::<Vec<_>>();
    let valid = (0..LIDAR_BINS)
        .map(|index| index % 11 != 0 && index % 29 != 0)
        .collect::<Vec<_>>();
    let pose = PoseFeatures {
        x_m: 1.25,
        z_m: -0.75,
        yaw_rad: 0.37,
        linear_mps: 0.12,
        angular_rps: -0.08,
        normalized_age: 0.04,
        valid: true,
    };
    let observation = pack_observation(&camera, &ranges, &valid, pose)?.to_vec();
    let input = RuntimeInput {
        observation,
        applied_action: [0.0, 0.0, 0.0, 1.0],
        delta_seconds: 0.1,
    };
    let actions = [
        [0.0, 0.0, 0.0, 1.0],
        [0.15, 0.15, 0.25, 1.0],
        [0.2, -0.2, 0.25, 1.0],
        [-0.1, -0.1, 0.2, 1.0],
        [-0.2, 0.2, 0.25, 1.0],
        [0.3, 0.3, 0.4, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.1, 0.05, 0.15, 1.0],
    ];
    let steps_dt = [0.1, 0.08, 0.12, 0.1, 0.09, 0.11, 0.1, 0.1];
    let sha256 = fixture_sha256(&input, &actions, &steps_dt);
    Ok(ParityFixture { input, actions, steps_dt, sha256 })
}

fn record_trace(runtime: &CoherentJepaRuntime, fixture: &ParityFixture) -> ModelResult<RuntimeTrace> {
    let single = runtime.infer(&fixture.input)?;
    let mut latencies = vec![single.synchronized_latency_us];
    let mut mean = Vec::new();
    let mut log_variance = Vec::new();
    let mut occupancy = Vec::new();
    let mut latent = single.latent.clone();
    let steps = fixture.actions.iter().zip(fixture.steps_dt.iter().copied());
    for (action, dt) in steps {
        let clock = std::time::Instant::now();
        let step = runtime.predict_latent_step(&latent, *action, dt)?;
        let micros = clock.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        latencies.push(micros);
        mean.extend_from_slice(&step.mean);
        log_variance.extend_from_slice(&step.log_variance);
        occupancy.extend_from_slice(&step.occupancy_logits);
        latent = step.mean;
    }
    Ok(RuntimeTrace { single, mean, log_variance, occupancy, latencies })
}

/// Score two equal-length finite vectors, rejecting anything else.
fn compare_tensors(left: &[f32], right: &[f32], thresholds: ParityThresholds) -> ModelResult<ParityMetric> {
    let incompatible = left.is_empty() || left.len() != right.len();
    let non_finite = left.iter().chain(right.iter()).any(|value| !value.is_finite());
    if incompatible || non_finite {
        return Err("parity metric requires equal non-empty finite vectors".into());
    }
    let paired = left.iter().zip(right.iter());
    let squared: f64 = paired
        .clone()
        .map(|(left, right)| {
            let delta = f64::from(*left) - f64::from(*right);
            delta * delta
        })
        .sum();
    let max_abs = paired
        .map(|(left, right)| (f64::from(*left) - f64::from(*right)).abs())
        .fold(0.0_f64, f64::max);
    let dot: f64 = left
        .iter()
        .zip(right.iter())
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum();
    let left_norm: f64 = left.iter().map(|value| f64::from(*value) * f64::from(*value)).sum();
    let right_norm: f64 = right.iter().map(|value| f64::from(*value) * f64::from(*value)).sum();
    let rmse = (squared / left.len() as f64).sqrt();
    let cosine = match (left_norm == 0.0, right_norm == 0.0) {
        (true, true) => 1.0,
        (true, false) | (false, true) => 0.0,
        (false, false) => (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0),
    };
    let passes = rmse <= thresholds.rmse_max
        && max_abs <= thresholds.max_abs_max
        && cosine >= thresholds.cosine_min;
    Ok(ParityMetric { values: left.len(), rmse, max_abs, cosine, passes })
}

fn summarize_latency(mut samples: Vec<u64>) -> ParityLatency {
    samples.sort_unstable();
    let last = samples.len().saturating_sub(1);
    let pick = |percent: usize| -> u64 {
        if samples.is_empty() {
            return 0;
        }
        let rank = (last * percent).div_ceil(100);
        samples[rank.min(last)]
    };
    ParityLatency {
        p50_us: pick(50),
        p95_us: pick(95),
        max_us: samples.last().copied().unwrap_or(0),
        synchronized_samples_us: samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::DType;
    use candle_nn::{VarBuilder, VarMap};
    use crate::{
        empty_candidate_manifest,
        initialize_deterministic,
        write_candidate_checkpoint,
        ActionSupport,
        BaselineGate,
        GroundingGeometry,
        HeldOutMetrics,
        JepaCandidateModel,
    };

    fn baseline_gate() -> BaselineGate {
        BaselineGate {
            dataset_digest: "d".repeat(64),
            valid_transitions: 50_000,
            sessions: 12,
            conditions: 3,
            environments: 3,
            constant: HeldOutMetrics { transition_nll: 2.0, rollout_error: 2.0 },
            flat_mlp: HeldOutMetrics { transition_nll: 1.5, rollout_error: 1.5 },
            tiny_cnn: HeldOutMetrics { transition_nll: 1.0, rollout_error: 1.0 },
        }
    }

    fn action_support() -> ActionSupport {
        ActionSupport {
            sample_count: 50_000,
            min_left: -1.0,
            max_left: 1.0,
            min_right: -1.0,
            max_right: 1.0,
            min_effective_forward: -1.0,
            max_effective_forward: 1.0,
            min_effective_turn: -1.0,
            max_effective_turn: 1.0,
            min_speed_scale: 0.0,
            max_speed_scale: 1.0,
            min_delta_seconds: 0.05,
            max_delta_seconds: 0.5,
        }
    }

    fn create_checkpoint() -> tempfile::TempDir {
        let workspace = tempfile::tempdir().unwrap();
        let weights = VarMap::new();
        JepaCandidateModel::load(VarBuilder::from_varmap(&weights, DType::F32, &Device::Cpu))
            .unwrap();
        initialize_deterministic(&weights, 0xc0ffee).unwrap();
        let manifest = empty_candidate_manifest(
            "parity-candidate",
            &"d".repeat(64),
            0xc0ffee,
            "cpu",
            baseline_gate(),
            action_support(),
            GroundingGeometry {
                width: qualia_jepa::GROUNDING_WIDTH,
                height: qualia_jepa::GROUNDING_HEIGHT,
                resolution_m: 0.05,
            },
        );
        write_candidate_checkpoint(workspace.path(), "parity-candidate", &weights, &weights, manifest)
            .unwrap();
        workspace
    }

    #[test]
    fn the_three_frozen_gates_are_enforced() {
        let gates = ParityThresholds::default();
        let exact = compare_tensors(&[1.0, 2.0], &[1.0, 2.0], gates).unwrap();
        assert!(exact.passes);
        let drifted = compare_tensors(&[1.0, 2.0], &[1.01, 2.0], gates).unwrap();
        assert!(!drifted.passes);
        assert!(compare_tensors(&[], &[], gates).is_err());
    }

    #[test]
    fn the_fixture_is_stable_and_covers_every_rollout_step() {
        let first = build_fixture().unwrap();
        let second = build_fixture().unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.sha256.len(), 64);
        assert_eq!(first.actions.len(), PARITY_ROLLOUT_STEPS);
        assert_eq!(first.steps_dt.len(), PARITY_ROLLOUT_STEPS);
    }

    #[test]
    fn a_cpu_to_cpu_run_passes_and_is_exact() {
        let workspace = create_checkpoint();
        let report = compare_checkpoint_backends(
            workspace.path().join("parity-candidate"),
            "cpu",
            ParityThresholds::default(),
        )
        .unwrap();
        assert!(report.passes);
        assert_eq!(report.checkpoint_id, "parity-candidate");
        assert_eq!(report.rollout_steps, 8);
        assert_eq!(report.single_step.len(), 5);
        assert_eq!(report.eight_step.len(), 3);
        let all_metrics = || report.single_step.values().chain(report.eight_step.values());
        assert!(all_metrics().all(|entry| entry.values > 0));
        assert!(all_metrics().all(|entry| entry.rmse == 0.0 && entry.max_abs == 0.0));
    }
}
