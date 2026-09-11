//! Coherent single-device inference runtime.
//!
//! All four heads (encoder, predictor, evidence adapter, occupancy decoder)
//! are owned by one object on one device. Schedule them as separate runners
//! and a generation swap can publish a half-updated graph; keeping the graph
//! whole means the swap above this layer either sees the old runtime or the
//! new one, never a blend.

use std::fs;
use std::path::Path;
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::{VarBuilder, VarMap};
use qualia_jepa::{ACTION_DIM, BELIEF_DIM, CORE_DIM, GROUNDING_CELLS, OBS_DIM};
use sha2::{Digest, Sha256};

use crate::planner::{PlannerResult, PredictedRolloutStep, RolloutPredictor};
use crate::{
    encode_observation, initialize_deterministic, transition_nll_tensor,
    validate_checkpoint_weights, CheckpointManifest, JepaCandidateModel, ModelResult,
    TinyCnnEncoder, TransitionPrediction, ARCHITECTURE_ID,
};

/// Identifier of the synchronized, tiled inference runtime.
///
/// Accelerators queue convolution and matmul work without waiting, so the
/// latency reported here brackets a device synchronize: it tracks when the
/// work actually retired, not when the host finished enqueuing it.
pub const RUNTIME_ID: &str = "qualia.jepa.coherent-tiled-runtime.v1";

/// Everything the runtime needs for one transition.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeInput {
    pub observation: Vec<f32>,
    pub applied_action: [f32; ACTION_DIM],
    pub delta_seconds: f32,
}

/// Every head of the runtime, flattened and host-side.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeOutput {
    pub latent: Vec<f32>,
    pub predicted_mean: Vec<f32>,
    pub predicted_log_variance: Vec<f32>,
    pub evidence: Vec<f32>,
    pub occupancy_logits: Vec<f32>,
    /// Scored heteroscedastic NLL against a target observation. A plain
    /// forward probe has no target, so it reports zero here.
    pub transition_nll: f32,
    pub synchronized_latency_us: u64,
}

impl RuntimeOutput {
    /// True when every published head, scored NLL included, is finite.
    pub fn is_finite(&self) -> bool {
        let heads = [
            self.latent.as_slice(),
            self.predicted_mean.as_slice(),
            self.predicted_log_variance.as_slice(),
            self.evidence.as_slice(),
            self.occupancy_logits.as_slice(),
        ];
        heads
            .iter()
            .all(|head| head.iter().all(|value| value.is_finite()))
            && self.transition_nll.is_finite()
    }
}

/// The loaded candidate plus its immutable identity.
pub struct CoherentJepaRuntime {
    device: Device,
    model: JepaCandidateModel,
    target_encoder: TinyCnnEncoder,
    checkpoint_id: String,
    weights_sha256: String,
}

impl CoherentJepaRuntime {
    /// Deterministic frozen model used only by ABI, parity and profiler fixtures.
    pub fn frozen_fixture(device: Device, seed: u64) -> candle_core::Result<Self> {
        let variables = VarMap::new();
        let builder = VarBuilder::from_varmap(&variables, DType::F32, &device);
        let model = JepaCandidateModel::load(builder)?;
        initialize_deterministic(&variables, seed)?;
        Ok(Self {
            target_encoder: model.encoder.clone(),
            checkpoint_id: format!("fixture-seed-{seed}"),
            weights_sha256: String::new(),
            device,
            model,
        })
    }

    /// Load an immutable candidate for offline replay and profiling.
    ///
    /// The constructor checks the declared digest against the bytes on disk
    /// and then checks the tensor inventory against the frozen architecture.
    /// Promotion policy is deliberately out of scope: whoever read the
    /// generation pointer decides which directory gets handed over.
    pub fn from_checkpoint(checkpoint_dir: impl AsRef<Path>, device: Device) -> ModelResult<Self> {
        let root = checkpoint_dir.as_ref();
        let manifest_bytes = fs::read(root.join("manifest.json"))?;
        let manifest: CheckpointManifest = serde_json::from_slice(&manifest_bytes)?;
        let declared_identity = manifest.schema_version == "qualia.jepa-checkpoint.v1"
            && manifest.architecture_id == ARCHITECTURE_ID
            && manifest.status == "candidate"
            && manifest.target_encoder == "ema";
        let gates_cleared = manifest.baseline_gate.passes()
            && manifest.action_support.passes()
            && manifest.grounding_geometry.passes();
        if !declared_identity || !gates_cleared {
            return Err("checkpoint is not a verified replay candidate".into());
        }
        let weights = fs::read(root.join("weights.safetensors"))?;
        let digest = format!("{:x}", Sha256::digest(&weights));
        if digest != manifest.weights_sha256 {
            return Err("checkpoint weights digest mismatch".into());
        }
        validate_checkpoint_weights(&weights, manifest.parameter_count)?;
        let builder = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)?;
        let target_encoder = TinyCnnEncoder::load(builder.pp("target_encoder"))?;
        let model = JepaCandidateModel::load(builder.clone())?;
        Ok(Self {
            device,
            model,
            target_encoder,
            checkpoint_id: manifest.checkpoint_id,
            weights_sha256: digest,
        })
    }

    /// Identity of the loaded generation.
    pub fn checkpoint_id(&self) -> &str {
        &self.checkpoint_id
    }

    /// Digest of the exact weights bytes this runtime loaded.
    pub fn weights_sha256(&self) -> &str {
        &self.weights_sha256
    }

    /// The device the whole graph lives on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Encode, predict, project and decode one transition.
    pub fn infer(&self, input: &RuntimeInput) -> candle_core::Result<RuntimeOutput> {
        validate_input(input)?;
        let observation = row_tensor(&input.observation, OBS_DIM, &self.device)?;
        let action = row_tensor(&input.applied_action, ACTION_DIM, &self.device)?;
        let delta = dt_tensor(input.delta_seconds, &self.device)?;

        let started = begin_measurement(&self.device)?;
        let latent = self.model.encode_observation(&observation)?;
        let prediction = self.model.predictor.forward(&latent, &action, &delta)?;
        let evidence = self.model.adapter.forward(&latent)?;
        let occupancy_logits = self.model.occupancy_decoder.forward(&prediction.mean)?;
        let synchronized_latency_us = end_measurement(&self.device, started)?;

        let values = head_values(&latent, &prediction, &evidence, &occupancy_logits, None)?;
        let [latent, predicted_mean, predicted_log_variance, evidence, occupancy_logits] =
            split_values(&values, HEAD_WIDTHS);
        let output = RuntimeOutput {
            latent, predicted_mean, predicted_log_variance,
            evidence, occupancy_logits,
            transition_nll: 0.0,
            synchronized_latency_us,
        };
        if !output.is_finite() {
            candle_core::bail!("JEPA runtime produced non-finite output")
        }
        Ok(output)
    }

    /// Score an observed transition without temporal leakage.
    ///
    /// `previous` carries `(o_t, a_t, dt_t)`; the target is encoded only as
    /// `z_{t+1}`. Prediction and grounding follow the previous latent and the
    /// applied action, whereas the evidence head and the scored NLL are fed by
    /// the target.
    pub fn observe_transition(
        &self,
        previous: &RuntimeInput,
        target_observation: &[f32],
    ) -> candle_core::Result<RuntimeOutput> {
        validate_input(previous)?;
        if target_observation.len() != OBS_DIM
            || target_observation.iter().any(|value| !value.is_finite())
        {
            candle_core::bail!("target observation must be a finite {OBS_DIM}-vector")
        }
        let previous_observation = row_tensor(&previous.observation, OBS_DIM, &self.device)?;
        let target = row_tensor(target_observation, OBS_DIM, &self.device)?;
        let action = row_tensor(&previous.applied_action, ACTION_DIM, &self.device)?;
        let delta = dt_tensor(previous.delta_seconds, &self.device)?;

        let started = begin_measurement(&self.device)?;
        let previous_latent = self.model.encode_observation(&previous_observation)?;
        let target_latent = self.model.encode_observation(&target)?;
        let metric_latent = encode_observation(&self.target_encoder, &target)?;
        let prediction = self.model.predictor.forward(&previous_latent, &action, &delta)?;
        let evidence = self.model.adapter.forward(&target_latent)?;
        let occupancy_logits = self.model.occupancy_decoder.forward(&prediction.mean)?;
        let nll_tensor = transition_nll_tensor(&prediction, &metric_latent)?;
        let synchronized_latency_us = end_measurement(&self.device, started)?;
        let values = head_values(
            &target_latent,
            &prediction,
            &evidence,
            &occupancy_logits,
            Some(&nll_tensor),
        )?;
        let (heads, scored) = values.split_at(HEAD_VALUES);
        let transition_nll = scored[0];

        let [latent, predicted_mean, predicted_log_variance, evidence, occupancy_logits] =
            split_values(heads, HEAD_WIDTHS);
        let output = RuntimeOutput {
            latent, predicted_mean, predicted_log_variance,
            evidence, occupancy_logits,
            transition_nll,
            synchronized_latency_us,
        };
        if !output.is_finite() {
            candle_core::bail!("JEPA transition runtime produced non-finite output")
        }
        Ok(output)
    }

    /// Advance an already-encoded latent through the transition and grounding
    /// heads. This exists for bounded offline proposal evaluation: it has no
    /// planner input and no motor writer.
    pub fn predict_latent_step(
        &self,
        latent: &[f32],
        action: [f32; ACTION_DIM],
        delta_seconds: f32,
    ) -> candle_core::Result<PredictedRolloutStep> {
        let latent_bad = latent.len() != CORE_DIM
            || latent.iter().any(|value| !value.is_finite());
        let step_bad = action.iter().any(|value| !value.is_finite())
            || !delta_seconds.is_finite()
            || delta_seconds <= 0.0;
        if latent_bad || step_bad {
            candle_core::bail!(
                "rollout input must contain a finite {CORE_DIM}-latent, finite action, and positive dt"
            )
        }
        let latent_tensor = row_tensor(latent, CORE_DIM, &self.device)?;
        let action_tensor = row_tensor(&action, ACTION_DIM, &self.device)?;
        let delta_tensor = dt_tensor(delta_seconds, &self.device)?;

        let prediction = self
            .model
            .predictor
            .forward(&latent_tensor, &action_tensor, &delta_tensor)?;
        let occupancy = self.model.occupancy_decoder.forward(&prediction.mean)?;
        let output = PredictedRolloutStep {
            mean: tensor_values(&prediction.mean, CORE_DIM)?,
            log_variance: tensor_values(&prediction.log_variance, CORE_DIM)?,
            occupancy_logits: tensor_values(&occupancy, GROUNDING_CELLS)?,
        };
        let all_finite = output
            .mean
            .iter()
            .chain(&output.log_variance)
            .chain(&output.occupancy_logits)
            .all(|value| value.is_finite());
        if !all_finite {
            candle_core::bail!("JEPA rollout runtime produced non-finite output")
        }
        Ok(output)
    }
}

impl RolloutPredictor for CoherentJepaRuntime {
    fn predict_step(
        &self,
        latent: &[f32],
        action: [f32; ACTION_DIM],
        dt_seconds: f32,
    ) -> PlannerResult<PredictedRolloutStep> {
        match self.predict_latent_step(latent, action, dt_seconds) {
            Ok(step) => Ok(step),
            Err(error) => Err(error.to_string().into()),
        }
    }
}

/// One-row tensor for a single transition, laid out along the batch axis.
fn row_tensor(values: &[f32], width: usize, device: &Device) -> candle_core::Result<Tensor> {
    Tensor::from_slice(values, (1, width), device)
}

/// One-row, one-column tensor holding the elapsed time.
fn dt_tensor(seconds: f32, device: &Device) -> candle_core::Result<Tensor> {
    Tensor::from_slice(&[seconds], (1, 1), device)
}

/// Drain queued device work, then open a latency window.
fn begin_measurement(device: &Device) -> candle_core::Result<Instant> {
    device.synchronize()?;
    Ok(Instant::now())
}

/// Drain queued device work again and close the latency window, saturated.
///
/// The window closes on this synchronize rather than on the head readback, so
/// the readback that follows never waits: the copy's own drain would otherwise
/// be charged to the readback instead of to the queue it waits on.
fn end_measurement(device: &Device, started: Instant) -> candle_core::Result<u64> {
    device.synchronize()?;
    Ok(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64)
}

/// Contract width of each published head, in readback order.
const HEAD_WIDTHS: [usize; 5] = [CORE_DIM, CORE_DIM, CORE_DIM, BELIEF_DIM, GROUNDING_CELLS];

/// Number of `f32` values one concatenated head readback carries.
const HEAD_VALUES: usize = CORE_DIM + CORE_DIM + CORE_DIM + BELIEF_DIM + GROUNDING_CELLS;

/// Copy every published head down to the host at its contract width.
///
/// One device-side concatenation and one host copy, not five. A host copy pays
/// a fixed setup on an accelerator, so five separate copies of the same bytes
/// cost five times one joined copy. `scored_nll`, when present, rides in the
/// same copy as a trailing one-element column and lands at `HEAD_VALUES`.
fn head_values(
    latent: &Tensor,
    prediction: &TransitionPrediction,
    evidence: &Tensor,
    occupancy_logits: &Tensor,
    scored_nll: Option<&Tensor>,
) -> candle_core::Result<Vec<f32>> {
    let heads = [
        latent,
        &prediction.mean,
        &prediction.log_variance,
        evidence,
        occupancy_logits,
    ];
    let joined = match scored_nll {
        None => Tensor::cat(&heads, 1)?,
        Some(nll) => {
            let nll = nll.reshape((1, 1))?;
            Tensor::cat(&[heads[0], heads[1], heads[2], heads[3], heads[4], &nll], 1)?
        }
    };
    tensor_values(&joined, HEAD_VALUES + usize::from(scored_nll.is_some()))
}

/// Split one concatenated readback at the given contract widths.
fn split_values<const N: usize>(values: &[f32], widths: [usize; N]) -> [Vec<f32>; N] {
    let mut parts: [Vec<f32>; N] = std::array::from_fn(|_| Vec::new());
    let mut rest = values;
    for (part, width) in parts.iter_mut().zip(widths) {
        let (head, tail) = rest.split_at(width);
        part.extend_from_slice(head);
        rest = tail;
    }
    debug_assert!(rest.is_empty());
    parts
}

fn validate_input(input: &RuntimeInput) -> candle_core::Result<()> {
    if input.observation.len() != OBS_DIM {
        candle_core::bail!("runtime observation must contain {OBS_DIM} values")
    }
    let any_non_finite = input.observation.iter().any(|value| !value.is_finite())
        || input.applied_action.iter().any(|value| !value.is_finite())
        || !input.delta_seconds.is_finite();
    if any_non_finite || input.delta_seconds <= 0.0 {
        candle_core::bail!("runtime input must be finite and dt must be positive")
    }
    Ok(())
}

fn tensor_values(tensor: &Tensor, expected: usize) -> candle_core::Result<Vec<f32>> {
    let host = tensor.flatten_all()?.to_device(&Device::Cpu)?;
    let values = host.to_vec1::<f32>()?;
    if values.len() != expected {
        candle_core::bail!("runtime output has an incompatible dimension")
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        empty_candidate_manifest, write_candidate_checkpoint, ActionSupport, BaselineGate,
        GroundingGeometry, HeldOutMetrics,
    };

    fn candidate_manifest(checkpoint_id: &str) -> CheckpointManifest {
        let point = |transition_nll: f64, rollout_error: f64| HeldOutMetrics {
            transition_nll,
            rollout_error,
        };
        let digest = "d".repeat(64);
        let baseline = BaselineGate {
            dataset_digest: digest.clone(),
            valid_transitions: 50_000, sessions: 12,
            conditions: 3, environments: 3,
            constant: point(3.0, 3.0),
            flat_mlp: point(2.0, 2.0),
            tiny_cnn: point(1.0, 1.0),
        };
        let support = ActionSupport {
            sample_count: 50_000,
            min_left: -1.0, max_left: 1.0,
            min_right: -1.0, max_right: 1.0,
            min_effective_forward: -1.0, max_effective_forward: 1.0,
            min_effective_turn: -1.0, max_effective_turn: 1.0,
            min_speed_scale: 0.0, max_speed_scale: 1.0,
            min_delta_seconds: 0.05, max_delta_seconds: 0.5,
        };
        let geometry = GroundingGeometry {
            width: qualia_jepa::GROUNDING_WIDTH, height: qualia_jepa::GROUNDING_HEIGHT,
            resolution_m: 0.05,
        };
        empty_candidate_manifest(checkpoint_id, &digest, 42, "cpu", baseline, support, geometry)
    }

    fn fixture_input() -> RuntimeInput {
        let observation = (0..OBS_DIM)
            .map(|index| {
                let raw = (index * 17 % 251) as f32;
                (raw - 125.0) / 125.0
            })
            .collect();
        RuntimeInput {
            observation,
            applied_action: [0.2, -0.1, 0.3, 0.05],
            delta_seconds: 0.125,
        }
    }

    #[test]
    fn every_head_is_published_with_its_contract_width() {
        let runtime = CoherentJepaRuntime::frozen_fixture(Device::Cpu, 0x5eed).unwrap();
        let output = runtime.infer(&fixture_input()).unwrap();
        let widths = [
            output.latent.len(),
            output.predicted_mean.len(),
            output.predicted_log_variance.len(),
            output.evidence.len(),
            output.occupancy_logits.len(),
        ];
        assert_eq!(widths, [CORE_DIM, CORE_DIM, CORE_DIM, BELIEF_DIM, GROUNDING_CELLS]);
        assert_eq!(output.transition_nll, 0.0);
        assert!(output.is_finite());
    }

    #[test]
    fn observed_transition_keeps_the_target_out_of_the_predictor_input() {
        let runtime = CoherentJepaRuntime::frozen_fixture(Device::Cpu, 17).unwrap();
        let previous = fixture_input();
        let mut target = fixture_input().observation;
        target[0] = 0.75;

        let observed = runtime.observe_transition(&previous, &target).unwrap();
        let forward = runtime.infer(&previous).unwrap();

        assert!(observed.transition_nll.is_finite());
        assert_ne!(observed.transition_nll, 0.0);
        assert_eq!(observed.predicted_mean, forward.predicted_mean);
        assert_eq!(observed.predicted_log_variance, forward.predicted_log_variance);
        assert_eq!(observed.occupancy_logits, forward.occupancy_logits);
        assert_ne!(observed.latent, forward.latent);
        assert_ne!(observed.evidence, forward.evidence);
    }

    #[test]
    fn a_checkpoint_runtime_uses_the_persisted_ema_target_only_for_the_nll() {
        let device = Device::Cpu;
        let online = {
            let variables = VarMap::new();
            JepaCandidateModel::load(VarBuilder::from_varmap(&variables, DType::F32, &device))
                .unwrap();
            initialize_deterministic(&variables, 11).unwrap();
            variables
        };
        let encoder_variables = |seed: u64| {
            let variables = VarMap::new();
            TinyCnnEncoder::load(
                VarBuilder::from_varmap(&variables, DType::F32, &device).pp("encoder"),
            )
            .unwrap();
            initialize_deterministic(&variables, seed).unwrap();
            variables
        };
        let matching_target = encoder_variables(11);
        let distinct_target = encoder_variables(12);

        let temp = tempfile::tempdir().unwrap();
        let write = |checkpoint_id: &str, target: &VarMap| {
            write_candidate_checkpoint(
                temp.path(),
                checkpoint_id,
                &online,
                target,
                candidate_manifest(checkpoint_id),
            )
            .unwrap();
            temp.path().join(checkpoint_id)
        };
        let matching_dir = write("matching-target", &matching_target);
        let distinct_dir = write("distinct-target", &distinct_target);
        let matching =
            CoherentJepaRuntime::from_checkpoint(matching_dir, Device::Cpu).unwrap();
        let distinct =
            CoherentJepaRuntime::from_checkpoint(distinct_dir, Device::Cpu).unwrap();

        let previous = fixture_input();
        let mut target = fixture_input().observation;
        target[0] = 0.75;
        let with_matching = matching.observe_transition(&previous, &target).unwrap();
        let with_distinct = distinct.observe_transition(&previous, &target).unwrap();

        assert_eq!(with_matching.latent, with_distinct.latent);
        assert_eq!(with_matching.evidence, with_distinct.evidence);
        assert_eq!(with_matching.predicted_mean, with_distinct.predicted_mean);
        assert_eq!(
            with_matching.predicted_log_variance,
            with_distinct.predicted_log_variance
        );
        assert_eq!(with_matching.occupancy_logits, with_distinct.occupancy_logits);
        assert_ne!(with_matching.transition_nll, with_distinct.transition_nll);
    }

    #[test]
    fn malformed_inputs_are_rejected_before_inference() {
        let runtime = CoherentJepaRuntime::frozen_fixture(Device::Cpu, 7).unwrap();

        let mut short = fixture_input();
        short.observation.pop();
        assert!(runtime.infer(&short).is_err());

        let mut nan_action = fixture_input();
        nan_action.applied_action[0] = f32::NAN;
        assert!(runtime.infer(&nan_action).is_err());

        let input = fixture_input();
        let short_latent = &input.observation[..CORE_DIM - 1];
        assert!(runtime
            .predict_latent_step(short_latent, [0.0; ACTION_DIM], 0.1)
            .is_err());
    }
}
