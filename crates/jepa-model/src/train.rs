//! Offline candidate training.
//!
//! This module owns one online candidate together with an encoder-only EMA
//! target. Promotion reports quote held-out numbers that originate here, so the
//! split bookkeeping is intentionally rigid: optimizer steps only ever see
//! training transitions, and the no-change baseline derives its single fixed
//! scale from training residuals and nothing else.

use candle_core::Result as CandleResult;
use candle_core::{DType, Device, Tensor};
use candle_nn::{AdamW, Optimizer, ParamsAdamW, VarBuilder, VarMap};
use qualia_jepa::{ACTION_DIM, CORE_DIM, GROUNDING_CELLS, OBS_DIM};
use qualia_jepa_dataset::MaterializedTransition;

use crate::ema_update_prefix;
use crate::encode_observation;
use crate::initialize_deterministic;
use crate::jepa_loss;
use crate::masked_grounding_bce_with_logits;
use crate::transition_nll_tensor;
use crate::FlatJepaBaseline;
use crate::FlatMlpBaseline;
use crate::HeldOutMetrics;
use crate::JepaCandidateModel;
use crate::TinyCnnEncoder;
use crate::TransitionPrediction;

/// Default EMA decay of the target encoder.
pub const DEFAULT_EMA_DECAY: f64 = 0.996;
/// Default VICReg variance weight.
pub const DEFAULT_VARIANCE_WEIGHT: f64 = 25.0;
/// Default VICReg covariance weight.
pub const DEFAULT_COVARIANCE_WEIGHT: f64 = 1.0;

/// One materialized transition in the form the trainer consumes.
#[derive(Debug, Clone)]
pub struct TransitionExample {
    pub sample_id: String,
    pub observation_sequence: u64,
    pub observation_timestamp_ns: u64,
    pub target_sequence: u64,
    pub target_timestamp_ns: u64,
    pub observation: Vec<f32>,
    pub target_observation: Vec<f32>,
    pub applied_action: [f32; ACTION_DIM],
    pub delta_seconds: f32,
    pub future_occupied: Vec<f32>,
    pub future_observed: Vec<f32>,
}

impl TryFrom<&MaterializedTransition> for TransitionExample {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(transition: &MaterializedTransition) -> Result<Self, Self::Error> {
        let grounding = transition.future_occupancy.robot_centric_grounding()?;
        let observation = transition.observation.clone();
        let target_observation = transition.target_observation.clone();
        let sample_id = transition.sample_id.clone();
        Ok(Self {
            future_occupied: grounding.occupied,
            future_observed: grounding.observed,
            observation,
            target_observation,
            sample_id,
            applied_action: transition.applied_action,
            delta_seconds: transition.delta_seconds,
            observation_sequence: transition.observation_sequence,
            observation_timestamp_ns: transition.observation_timestamp_ns,
            target_sequence: transition.target_sequence,
            target_timestamp_ns: transition.target_timestamp_ns,
        })
    }
}

/// Optimizer and objective weights for one training run.
#[derive(Debug, Clone, Copy)]
pub struct TrainerConfig {
    pub learning_rate: f64,
    pub weight_decay: f64,
    pub ema_decay: f64,
    pub variance_weight: f64,
    pub covariance_weight: f64,
    pub grounding_weight: f64,
}

impl Default for TrainerConfig {
    fn default() -> Self {
        Self {
            learning_rate: 3.0e-4,
            weight_decay: 1.0e-4,
            grounding_weight: 1.0,
            variance_weight: DEFAULT_VARIANCE_WEIGHT,
            covariance_weight: DEFAULT_COVARIANCE_WEIGHT,
            ema_decay: DEFAULT_EMA_DECAY,
        }
    }
}

/// The loss decomposition reported for one training step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepMetrics {
    pub total: f32,
    pub transition_nll: f32,
    pub variance: f32,
    pub covariance: f32,
    pub grounding: f32,
}

/// Host-side predictions for a held-out chunk, used by the report builder.
#[derive(Debug, Clone)]
pub struct PredictionBatch {
    pub predicted_mean: Vec<Vec<f32>>,
    pub predicted_log_variance: Vec<Vec<f32>>,
    pub target_latent: Vec<Vec<f32>>,
    pub occupancy_logits: Vec<Vec<f32>>,
}

/// Online CNN candidate plus its EMA target encoder.
pub struct OfflineTrainer {
    device: Device,
    online_vars: VarMap,
    target_vars: VarMap,
    model: JepaCandidateModel,
    target_encoder: TinyCnnEncoder,
    optimizer: AdamW,
    config: TrainerConfig,
    steps: u64,
    constant_log_variance: Option<Vec<f32>>,
}

/// Adapter shared by every caller that needs a frozen target-encoder embedding.
fn ema_target_latent(trainer: &OfflineTrainer, observation: &Tensor) -> CandleResult<Tensor> {
    let encoded = encode_observation(&trainer.target_encoder, observation)?;
    Ok(encoded.detach())
}

impl OfflineTrainer {
    /// Build an online candidate and copy the encoder into the target.
    pub fn new(device: Device, seed: u64, config: TrainerConfig) -> CandleResult<Self> {
        validate_config(config)?;
        let online_vars = VarMap::new();
        let online_builder = VarBuilder::from_varmap(&online_vars, DType::F32, &device);
        let model = JepaCandidateModel::load(online_builder)?;
        initialize_deterministic(&online_vars, seed)?;
        let target_vars = VarMap::new();
        let target_builder = VarBuilder::from_varmap(&target_vars, DType::F32, &device).pp("encoder");
        let target_encoder = TinyCnnEncoder::load(target_builder)?;
        ema_update_prefix(&target_vars, &online_vars, 0.0, "encoder.")?;
        let schedule = ParamsAdamW {
            lr: config.learning_rate,
            weight_decay: config.weight_decay,
            ..ParamsAdamW::default()
        };
        let optimizer = AdamW::new(online_vars.all_vars(), schedule)?;
        Ok(Self {
            model,
            target_encoder,
            optimizer,
            online_vars,
            target_vars,
            device,
            config,
            steps: 0,
            constant_log_variance: None,
        })
    }

    /// One gradient step on a batch of at least two transitions.
    pub fn train_batch(&mut self, examples: &[TransitionExample]) -> CandleResult<StepMetrics> {
        let tensors = BatchTensors::new(examples, &self.device)?;
        let visible = self.model.encode_observation(&tensors.observation)?;
        let future = ema_target_latent(self, &tensors.target_observation)?;
        let forecast = self.model.predictor.forward(
            &visible,
            &tensors.applied_action,
            &tensors.delta_seconds,
        )?;
        let decomposition = jepa_loss(
            &forecast,
            &future,
            &visible,
            self.config.variance_weight,
            self.config.covariance_weight,
        )?;
        let occupancy_logits = self.model.occupancy_decoder.forward(&forecast.mean)?;
        let grounding = masked_grounding_bce_with_logits(
            &occupancy_logits,
            &tensors.future_occupied,
            &tensors.future_observed,
            tensors.occupied_weight,
        )?;
        let objective = decomposition
            .total
            .add(&grounding.mean.affine(self.config.grounding_weight, 0.0)?)?;
        let mut metrics = metrics_from_loss(&decomposition)?;
        metrics.total = objective.to_scalar()?;
        metrics.grounding = grounding.mean.to_scalar()?;
        validate_metrics(metrics)?;
        self.optimizer.backward_step(&objective)?;
        let decay = self.config.ema_decay;
        ema_update_prefix(&self.target_vars, &self.online_vars, decay, "encoder.")?;
        self.steps = self.steps.saturating_add(1);
        Ok(metrics)
    }

    /// Loss without a gradient step and without the grounding term.
    pub fn evaluate_batch(&self, examples: &[TransitionExample]) -> CandleResult<StepMetrics> {
        let tensors = BatchTensors::new(examples, &self.device)?;
        let visible = self.model.encode_observation(&tensors.observation)?;
        let future = ema_target_latent(self, &tensors.target_observation)?;
        let forecast = self.model.predictor.forward(
            &visible,
            &tensors.applied_action,
            &tensors.delta_seconds,
        )?;
        let decomposition = jepa_loss(
            &forecast,
            &future,
            &visible,
            self.config.variance_weight,
            self.config.covariance_weight,
        )?;
        let mut metrics = metrics_from_loss(&decomposition)?;
        metrics.grounding = 0.0;
        Ok(metrics)
    }

    /// Encode observations through the EMA target encoder.
    pub fn encode_targets(&self, observations: &[Vec<f32>]) -> CandleResult<Vec<Vec<f32>>> {
        if observations.is_empty() {
            return Ok(Vec::new());
        }
        let batched = observation_tensor(observations, &self.device)?;
        let encoded = encode_observation(&self.target_encoder, &batched)?;
        encoded.to_device(&Device::Cpu)?.to_vec2()
    }

    /// Host-side predictions for a held-out chunk.
    pub fn predict_batch(&self, examples: &[TransitionExample]) -> CandleResult<PredictionBatch> {
        let tensors = BatchTensors::new_allow_single(examples, &self.device)?;
        let visible = self.model.encode_observation(&tensors.observation)?;
        let future = ema_target_latent(self, &tensors.target_observation)?;
        let forecast = self.model.predictor.forward(
            &visible,
            &tensors.applied_action,
            &tensors.delta_seconds,
        )?;
        let occupancy_logits = self.model.occupancy_decoder.forward(&forecast.mean)?;
        let host = |tensor: &Tensor| -> CandleResult<Vec<Vec<f32>>> {
            tensor.to_device(&Device::Cpu)?.to_vec2::<f32>()
        };
        Ok(PredictionBatch {
            predicted_mean: host(&forecast.mean)?,
            predicted_log_variance: host(&forecast.log_variance)?,
            target_latent: host(&future)?,
            occupancy_logits: host(&occupancy_logits)?,
        })
    }

    /// The online candidate.
    pub fn model(&self) -> &JepaCandidateModel {
        &self.model
    }

    /// The EMA target encoder.
    pub fn target_encoder(&self) -> &TinyCnnEncoder {
        &self.target_encoder
    }

    /// The online parameter map, as written into a checkpoint.
    pub fn online_vars(&self) -> &VarMap {
        &self.online_vars
    }

    /// The EMA target parameter map, as written into a checkpoint.
    pub fn target_vars(&self) -> &VarMap {
        &self.target_vars
    }

    /// Completed gradient steps.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Sequence-aware held-out NLL and rollout error of the candidate.
    pub fn evaluate_sequence(
        &self,
        examples: &[TransitionExample],
    ) -> CandleResult<HeldOutMetrics> {
        evaluate_cnn_sequence(self, examples)
    }

    /// Sequence-aware metrics of the no-change baseline.
    pub fn constant_sequence(
        &self,
        examples: &[TransitionExample],
    ) -> CandleResult<HeldOutMetrics> {
        let fitted = match self.constant_log_variance.as_deref() {
            Some(fitted) => fitted,
            None => candle_core::bail!(
                "constant baseline variance has not been fit on training data"
            ),
        };
        evaluate_constant_sequence(self, examples, fitted)
    }

    /// Calibrate the no-change baseline from training residuals alone.
    ///
    /// The no-change mean stays the current target-encoder representation;
    /// this only supplies that baseline with its best finite Gaussian scale.
    /// Validation and test targets never enter the estimate, so the comparison
    /// remains honest.
    pub fn calibrate_constant_baseline(
        &mut self,
        examples: &[TransitionExample],
    ) -> CandleResult<()> {
        if examples.len() < 2 {
            candle_core::bail!("constant baseline calibration requires at least two samples")
        }
        let mut residual_squares = vec![0.0_f64; CORE_DIM];
        let mut rows_seen = 0_u64;
        for window in examples.chunks(64) {
            let tensors = BatchTensors::new_allow_single(window, &self.device)?;
            let anchor = encode_observation(&self.target_encoder, &tensors.observation)?.detach();
            let moved =
                encode_observation(&self.target_encoder, &tensors.target_observation)?.detach();
            let delta = moved.sub(&anchor)?.sqr()?;
            let host_rows = delta.to_device(&Device::Cpu)?.to_vec2::<f32>()?;
            for row in host_rows.iter() {
                if row.len() != CORE_DIM || !row.iter().all(|value| value.is_finite()) {
                    candle_core::bail!("constant baseline calibration residual is invalid")
                }
                for (slot, residual) in residual_squares.iter_mut().zip(row.iter()) {
                    *slot += f64::from(*residual);
                }
            }
            rows_seen += host_rows.len() as u64;
        }
        if rows_seen != examples.len() as u64 {
            candle_core::bail!("constant baseline calibration sample count changed")
        }
        let divisor = rows_seen as f64;
        let floor = (-10.0_f64).exp();
        let ceiling = 5.0_f64.exp();
        let mut fitted = Vec::with_capacity(CORE_DIM);
        for sum in residual_squares {
            let mean_square = (sum / divisor).clamp(floor, ceiling);
            fitted.push(mean_square.ln() as f32);
        }
        self.constant_log_variance = Some(fitted);
        Ok(())
    }
}

/// The flat MLP baseline trained with the same objective and schedule.
pub struct FlatOfflineTrainer {
    device: Device,
    online_vars: VarMap,
    target_vars: VarMap,
    model: FlatJepaBaseline,
    target_encoder: FlatMlpBaseline,
    optimizer: AdamW,
    config: TrainerConfig,
    steps: u64,
}

impl FlatOfflineTrainer {
    /// Build a flat online baseline and copy its encoder into the target.
    pub fn new(device: Device, seed: u64, config: TrainerConfig) -> CandleResult<Self> {
        validate_config(config)?;
        let online_vars = VarMap::new();
        let online_builder = VarBuilder::from_varmap(&online_vars, DType::F32, &device);
        let model = FlatJepaBaseline::load(online_builder)?;
        initialize_deterministic(&online_vars, seed)?;
        let target_vars = VarMap::new();
        let target_builder = VarBuilder::from_varmap(&target_vars, DType::F32, &device).pp("encoder");
        let target_encoder = FlatMlpBaseline::load(target_builder)?;
        ema_update_prefix(&target_vars, &online_vars, 0.0, "encoder.")?;
        let schedule = ParamsAdamW {
            lr: config.learning_rate,
            weight_decay: config.weight_decay,
            ..ParamsAdamW::default()
        };
        let optimizer = AdamW::new(online_vars.all_vars(), schedule)?;
        Ok(Self {
            model,
            target_encoder,
            optimizer,
            online_vars,
            target_vars,
            device,
            config,
            steps: 0,
        })
    }

    /// One gradient step on a batch of at least two transitions.
    pub fn train_batch(&mut self, examples: &[TransitionExample]) -> CandleResult<StepMetrics> {
        let tensors = BatchTensors::new(examples, &self.device)?;
        let visible = self.model.encoder.forward(&tensors.observation)?;
        let future = self
            .target_encoder
            .forward(&tensors.target_observation)?
            .detach();
        let forecast = self.model.predictor.forward(
            &visible,
            &tensors.applied_action,
            &tensors.delta_seconds,
        )?;
        let decomposition = jepa_loss(
            &forecast,
            &future,
            &visible,
            self.config.variance_weight,
            self.config.covariance_weight,
        )?;
        let metrics = metrics_from_loss(&decomposition)?;
        validate_metrics(metrics)?;
        self.optimizer.backward_step(&decomposition.total)?;
        let decay = self.config.ema_decay;
        ema_update_prefix(&self.target_vars, &self.online_vars, decay, "encoder.")?;
        self.steps = self.steps.saturating_add(1);
        Ok(metrics)
    }

    /// Sequence-aware held-out NLL and rollout error of the baseline.
    pub fn evaluate_sequence(
        &self,
        examples: &[TransitionExample],
    ) -> CandleResult<HeldOutMetrics> {
        if examples.is_empty() {
            candle_core::bail!("held-out sequence must not be empty")
        }
        let mut nll_weighted = 0.0_f64;
        let mut scored_rows = 0_usize;
        for window in examples.chunks(64) {
            let tensors = BatchTensors::new_allow_single(window, &self.device)?;
            let visible = self.model.encoder.forward(&tensors.observation)?;
            let future = self
                .target_encoder
                .forward(&tensors.target_observation)?
                .detach();
            let forecast = self.model.predictor.forward(
                &visible,
                &tensors.applied_action,
                &tensors.delta_seconds,
            )?;
            let row_loss = transition_nll_tensor(&forecast, &future)?.to_scalar::<f32>()?;
            nll_weighted += f64::from(row_loss) * window.len() as f64;
            scored_rows += window.len();
        }
        let mut carried = None;
        let mut anchor = None;
        let mut rollout_sum = 0.0_f64;
        for example in examples {
            let single = std::slice::from_ref(example);
            let tensors = BatchTensors::new_allow_single(single, &self.device)?;
            if !continues_sequence(anchor, example) {
                carried = Some(self.model.encoder.forward(&tensors.observation)?);
            }
            let state = carried.as_ref().ok_or_else(|| {
                candle_core::Error::Msg("rollout latent is absent".into())
            })?;
            let forecast = self.model.predictor.forward(
                state,
                &tensors.applied_action,
                &tensors.delta_seconds,
            )?;
            let future = self
                .target_encoder
                .forward(&tensors.target_observation)?
                .detach();
            let gap = future
                .sub(&forecast.mean)?
                .sqr()?
                .mean_all()?
                .to_scalar::<f32>()?;
            rollout_sum += f64::from(gap);
            carried = Some(forecast.mean.detach());
            anchor = Some((example.target_sequence, example.target_timestamp_ns));
        }
        Ok(HeldOutMetrics {
            transition_nll: nll_weighted / scored_rows as f64,
            rollout_error: rollout_sum / scored_rows as f64,
        })
    }

    /// Completed gradient steps.
    pub fn steps(&self) -> u64 {
        self.steps
    }
}

fn evaluate_cnn_sequence(
    trainer: &OfflineTrainer,
    examples: &[TransitionExample],
) -> CandleResult<HeldOutMetrics> {
    if examples.is_empty() {
        candle_core::bail!("held-out sequence must not be empty")
    }
    let mut nll_weighted = 0.0_f64;
    let mut scored_rows = 0_usize;
    for window in examples.chunks(64) {
        let tensors = BatchTensors::new_allow_single(window, &trainer.device)?;
        let visible = trainer.model.encode_observation(&tensors.observation)?;
        let future = ema_target_latent(trainer, &tensors.target_observation)?;
        let forecast = trainer.model.predictor.forward(
            &visible,
            &tensors.applied_action,
            &tensors.delta_seconds,
        )?;
        let row_loss = transition_nll_tensor(&forecast, &future)?.to_scalar::<f32>()?;
        nll_weighted += f64::from(row_loss) * window.len() as f64;
        scored_rows += window.len();
    }
    let mut carried = None;
    let mut anchor = None;
    let mut rollout_sum = 0.0_f64;
    for example in examples {
        let single = std::slice::from_ref(example);
        let tensors = BatchTensors::new_allow_single(single, &trainer.device)?;
        if !continues_sequence(anchor, example) {
            carried = Some(trainer.model.encode_observation(&tensors.observation)?);
        }
        let state = carried.as_ref().ok_or_else(|| {
            candle_core::Error::Msg("rollout latent is absent".into())
        })?;
        let forecast = trainer.model.predictor.forward(
            state,
            &tensors.applied_action,
            &tensors.delta_seconds,
        )?;
        let future = ema_target_latent(trainer, &tensors.target_observation)?;
        let gap = future
            .sub(&forecast.mean)?
            .sqr()?
            .mean_all()?
            .to_scalar::<f32>()?;
        rollout_sum += f64::from(gap);
        carried = Some(forecast.mean.detach());
        anchor = Some((example.target_sequence, example.target_timestamp_ns));
    }
    Ok(HeldOutMetrics {
        transition_nll: nll_weighted / scored_rows as f64,
        rollout_error: rollout_sum / scored_rows as f64,
    })
}

fn evaluate_constant_sequence(
    trainer: &OfflineTrainer,
    examples: &[TransitionExample],
    constant_log_variance: &[f32],
) -> CandleResult<HeldOutMetrics> {
    if examples.is_empty() {
        candle_core::bail!("held-out sequence must not be empty")
    }
    if constant_log_variance.len() != CORE_DIM
        || constant_log_variance.iter().any(|value| !value.is_finite())
    {
        candle_core::bail!("constant baseline variance is invalid")
    }
    let fixed_scale = Tensor::from_slice(constant_log_variance, (1, CORE_DIM), &trainer.device)?;
    let mut segment_start = None;
    let mut anchor = None;
    let mut nll_sum = 0.0_f64;
    let mut rollout_sum = 0.0_f64;
    for example in examples {
        let single = std::slice::from_ref(example);
        let tensors = BatchTensors::new_allow_single(single, &trainer.device)?;
        let current = encode_observation(&trainer.target_encoder, &tensors.observation)?.detach();
        if !continues_sequence(anchor, example) {
            segment_start = Some(current.clone());
        }
        let future = ema_target_latent(trainer, &tensors.target_observation)?;
        let baseline = TransitionPrediction {
            mean: current,
            log_variance: fixed_scale.clone(),
        };
        nll_sum += f64::from(transition_nll_tensor(&baseline, &future)?.to_scalar::<f32>()?);
        let start = segment_start.as_ref().ok_or_else(|| {
            candle_core::Error::Msg("constant rollout latent is absent".into())
        })?;
        let gap = future.sub(start)?.sqr()?.mean_all()?.to_scalar::<f32>()?;
        rollout_sum += f64::from(gap);
        anchor = Some((example.target_sequence, example.target_timestamp_ns));
    }
    let rows = examples.len() as f64;
    Ok(HeldOutMetrics {
        transition_nll: nll_sum / rows,
        rollout_error: rollout_sum / rows,
    })
}

/// Whether `next` continues the camera sequence that produced `previous_target`.
fn continues_sequence(previous_target: Option<(u64, u64)>, next: &TransitionExample) -> bool {
    match previous_target {
        Some((sequence, timestamp)) => {
            (sequence, timestamp) == (next.observation_sequence, next.observation_timestamp_ns)
        }
        None => false,
    }
}

fn metrics_from_loss(loss: &crate::JepaLoss) -> CandleResult<StepMetrics> {
    let grounding = 0.0_f32;
    Ok(StepMetrics {
        total: loss.total.to_scalar()?,
        transition_nll: loss.transition_nll.to_scalar()?,
        variance: loss.variance.to_scalar()?,
        covariance: loss.covariance.to_scalar()?,
        grounding,
    })
}

fn validate_metrics(metrics: StepMetrics) -> CandleResult<()> {
    let reported = [
        metrics.total,
        metrics.transition_nll,
        metrics.variance,
        metrics.covariance,
        metrics.grounding,
    ];
    if reported.iter().all(|value| value.is_finite()) {
        return Ok(());
    }
    candle_core::bail!("non-finite JEPA training loss")
}

/// Batch tensors for one training step.
struct BatchTensors {
    observation: Tensor,
    target_observation: Tensor,
    applied_action: Tensor,
    delta_seconds: Tensor,
    future_occupied: Tensor,
    future_observed: Tensor,
    occupied_weight: f64,
}

impl BatchTensors {
    fn new(examples: &[TransitionExample], device: &Device) -> CandleResult<Self> {
        Self::with_minimum(examples, device, 2)
    }

    fn new_allow_single(examples: &[TransitionExample], device: &Device) -> CandleResult<Self> {
        Self::with_minimum(examples, device, 1)
    }

    fn with_minimum(
        examples: &[TransitionExample],
        device: &Device,
        minimum: usize,
    ) -> CandleResult<Self> {
        if examples.len() < minimum {
            candle_core::bail!("JEPA batch does not meet its minimum sample count")
        }
        let mut observations = Vec::with_capacity(examples.len());
        let mut target_observations = Vec::with_capacity(examples.len());
        let mut actions = Vec::with_capacity(examples.len() * ACTION_DIM);
        let mut intervals = Vec::with_capacity(examples.len());
        let mut occupied = Vec::with_capacity(examples.len() * GROUNDING_CELLS);
        let mut observed = Vec::with_capacity(examples.len() * GROUNDING_CELLS);
        let mut measured = 0_usize;
        let mut hits = 0_usize;
        for example in examples {
            if example.future_occupied.len() != GROUNDING_CELLS
                || example.future_observed.len() != GROUNDING_CELLS
            {
                candle_core::bail!("JEPA grounding target has the wrong dimension")
            }
            observations.push(example.observation.clone());
            target_observations.push(example.target_observation.clone());
            actions.extend_from_slice(&example.applied_action);
            intervals.push(example.delta_seconds);
            occupied.extend_from_slice(&example.future_occupied);
            observed.extend_from_slice(&example.future_observed);
            for (target, mask) in example
                .future_occupied
                .iter()
                .zip(example.future_observed.iter())
            {
                if *mask == 1.0 {
                    measured += 1;
                    if *target == 1.0 {
                        hits += 1;
                    }
                }
            }
        }
        if measured == 0 {
            candle_core::bail!("JEPA grounding batch has no measured cells")
        }
        let occupied_weight = if hits == 0 {
            1.0
        } else {
            ((measured - hits) as f64 / hits as f64).clamp(1.0, 100.0)
        };
        if intervals.iter().any(|value| !value.is_finite() || *value <= 0.0) {
            candle_core::bail!("JEPA batch contains an invalid elapsed time")
        }
        let rows = examples.len();
        Ok(Self {
            observation: observation_tensor(&observations, device)?,
            target_observation: observation_tensor(&target_observations, device)?,
            applied_action: Tensor::from_vec(actions, (rows, ACTION_DIM), device)?,
            delta_seconds: Tensor::from_vec(intervals, (rows, 1), device)?,
            future_occupied: Tensor::from_vec(occupied, (rows, GROUNDING_CELLS), device)?,
            future_observed: Tensor::from_vec(observed, (rows, GROUNDING_CELLS), device)?,
            occupied_weight,
        })
    }
}

fn observation_tensor(observations: &[Vec<f32>], device: &Device) -> CandleResult<Tensor> {
    if !observations
        .iter()
        .all(|observation| observation.len() == OBS_DIM)
    {
        candle_core::bail!("JEPA observation has the wrong dimension")
    }
    let mut values = Vec::with_capacity(observations.len() * OBS_DIM);
    for observation in observations {
        values.extend_from_slice(observation);
    }
    Tensor::from_vec(values, (observations.len(), OBS_DIM), device)
}

fn validate_config(config: TrainerConfig) -> CandleResult<()> {
    let non_negative = [
        ("learning rate", config.learning_rate),
        ("weight decay", config.weight_decay),
        ("variance weight", config.variance_weight),
        ("covariance weight", config.covariance_weight),
        ("grounding weight", config.grounding_weight),
    ];
    for (name, value) in non_negative {
        if !value.is_finite() || value < 0.0 {
            candle_core::bail!("{name} must be finite and non-negative")
        }
    }
    let decay_in_range = (0.0..=1.0).contains(&config.ema_decay) && config.ema_decay.is_finite();
    if !decay_in_range {
        candle_core::bail!("EMA decay must be finite and in [0, 1]")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn digest_of(var_map: &VarMap, prefix: &str) -> String {
        let variables = var_map.data().lock().expect("Candle VarMap lock");
        let mut names = variables
            .keys()
            .filter(|name| name.starts_with(prefix))
            .collect::<Vec<_>>();
        names.sort_unstable();
        let mut hasher = Sha256::new();
        for name in names {
            hasher.update(name.as_bytes());
            let flat_values = variables[name]
                .as_detached_tensor()
                .flatten_all()
                .unwrap()
                .to_device(&Device::Cpu)
                .unwrap()
                .to_vec1::<f32>()
                .unwrap();
            for value in flat_values {
                hasher.update(value.to_le_bytes());
            }
        }
        format!("{:x}", hasher.finalize())
    }

    fn example(value: f32) -> TransitionExample {
        let mut observation = vec![0.0_f32; OBS_DIM];
        observation.fill(value);
        let mut target_observation = vec![0.0_f32; OBS_DIM];
        target_observation.fill(value + 0.01);
        TransitionExample {
            sample_id: format!("fixture-{value}"),
            observation,
            target_observation,
            applied_action: [0.0, 0.0, 1.0, 1.0],
            delta_seconds: 0.1,
            observation_sequence: 1,
            observation_timestamp_ns: 100,
            target_sequence: 2,
            target_timestamp_ns: 200,
            future_occupied: vec![0.0; GROUNDING_CELLS],
            future_observed: vec![1.0; GROUNDING_CELLS],
        }
    }

    #[test]
    fn a_step_updates_online_and_ema_parameters_but_not_the_frozen_adapter() {
        let config = TrainerConfig {
            weight_decay: 0.0,
            ..TrainerConfig::default()
        };
        let mut trainer = OfflineTrainer::new(Device::Cpu, 7, config).unwrap();
        let online_before = digest_of(&trainer.online_vars, "");
        let target_before = digest_of(&trainer.target_vars, "");
        let adapter_before = digest_of(&trainer.online_vars, "adapter.");
        let metrics = trainer.train_batch(&[example(0.1), example(0.2)]).unwrap();
        assert_eq!(trainer.steps(), 1);
        assert!(metrics.total.is_finite());
        let online_after = digest_of(&trainer.online_vars, "");
        let target_after = digest_of(&trainer.target_vars, "");
        assert_ne!(online_after, online_before);
        assert_ne!(target_after, target_before);
        assert_eq!(digest_of(&trainer.online_vars, "adapter."), adapter_before);

        let sequence = [example(0.1), example(0.2)];
        assert!(trainer.constant_sequence(&sequence).is_err());
        trainer.calibrate_constant_baseline(&sequence).unwrap();
        let calibrated = trainer.constant_log_variance.as_ref().unwrap();
        assert_eq!(calibrated.len(), CORE_DIM);
        let encoded = trainer
            .encode_targets(&[vec![0.1; OBS_DIM], vec![0.2; OBS_DIM]])
            .unwrap();
        assert_eq!(encoded[0].len(), CORE_DIM);
    }

    #[test]
    fn singleton_batches_and_non_positive_elapsed_time_are_rejected() {
        let mut trainer = OfflineTrainer::new(Device::Cpu, 7, TrainerConfig::default()).unwrap();
        assert!(trainer.train_batch(&[example(0.1)]).is_err());
        let mut invalid = example(0.2);
        invalid.delta_seconds = 0.0;
        assert!(trainer.train_batch(&[example(0.1), invalid]).is_err());
    }

    #[test]
    fn rollout_state_resets_when_the_camera_provenance_is_not_contiguous() {
        let trainer = OfflineTrainer::new(Device::Cpu, 19, TrainerConfig::default()).unwrap();
        let mut earlier = example(0.1);
        earlier.observation_sequence = 1;
        earlier.observation_timestamp_ns = 100;
        earlier.target_sequence = 2;
        earlier.target_timestamp_ns = 200;
        let mut later = example(0.7);
        later.observation_sequence = 10;
        later.observation_timestamp_ns = 1_000;
        later.target_sequence = 11;
        later.target_timestamp_ns = 1_100;

        let combined = trainer
            .evaluate_sequence(&[earlier.clone(), later.clone()])
            .unwrap();
        let earlier_only = trainer.evaluate_sequence(&[earlier]).unwrap();
        let later_only = trainer.evaluate_sequence(&[later]).unwrap();
        let separate = (earlier_only.rollout_error + later_only.rollout_error) / 2.0;
        assert!((combined.rollout_error - separate).abs() < 1.0e-6);
    }
}
