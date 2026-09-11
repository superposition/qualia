//! Candle encoder, predictive world model, grounding head, calibration and
//! checkpoint discipline for the evidence-gated Qualia JEPA candidate.
//!
//! The crate is deliberately narrow. It produces latent transitions, a
//! heteroscedastic prediction, a belief-space evidence projection and an
//! occupancy logit map, plus the immutable artifacts the evidence gates read
//! back (`weights.safetensors` beside a `manifest.json` whose SHA-256 covers
//! the weights). It never writes a plan or a motor command.

use candle_core::{DType, Device, Result as CandleResult, Tensor};
use candle_nn::{
    conv1d, conv2d, layer_norm, linear, Conv1d, Conv1dConfig, Conv2d, Conv2dConfig, LayerNorm,
    Linear, Module, VarBuilder, VarMap,
};
use qualia_jepa::{ACTION_DIM, BELIEF_DIM, CORE_DIM, GROUNDING_HEIGHT, GROUNDING_WIDTH, POSE_DIM};
use qualia_jepa_dataset::{DatasetManifest, DatasetSplit};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

pub mod evaluation;
pub mod parity;
pub mod planner;
pub mod runtime;
pub mod train;

/// Result alias shared by every fallible entry point in the crate.
pub type ModelResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Architecture literal stamped into every checkpoint manifest.
pub const ARCHITECTURE_ID: &str = "qualia.jepa.grounded-tiny-cnn.v1";
/// Versioned fixed random projection used as the evidence adapter.
pub const EVIDENCE_ADAPTER_ID: &str = "qualia.jepa.fixed-rp-256x1024.v1";
/// Baseline literal for the flat MLP reference model.
pub const FLAT_BASELINE_ID: &str = "qualia.jepa.flat-mlp.v1";
/// Schema literal of the on-disk training report.
pub const TRAINING_REPORT_SCHEMA: &str = "qualia.jepa-training-report.v1";
/// Default age after which a training report stops being promotable.
pub const DEFAULT_MAX_TRAINING_REPORT_AGE_MS: u128 = 7 * 24 * 60 * 60 * 1_000;
/// Width of the camera branch of the fused representation.
pub const CAMERA_FEATURES: usize = 128;
/// Width of the LiDAR branch of the fused representation.
pub const LIDAR_FEATURES: usize = 96;
/// Width of the pose branch of the fused representation.
pub const POSE_FEATURES: usize = 32;
/// Input width of the transition predictor.
pub const PREDICTOR_INPUT_DIM: usize = CORE_DIM + ACTION_DIM + 1;
/// Hidden width of the transition predictor.
pub const PREDICTOR_HIDDEN_DIM: usize = 512;
/// Hidden width of the future-occupancy decoder.
pub const GROUNDING_HIDDEN_DIM: usize = 512;
/// VICReg variance target standard deviation.
pub const VICREG_TARGET_STDDEV: f64 = 1.0;
/// VICReg standard-deviation epsilon.
pub const VICREG_EPSILON: f64 = 1.0e-4;
/// Sample floor below which an action-support envelope is not promotion-worthy.
pub const ACTION_SUPPORT_MINIMUM_SAMPLES: u64 = 4_096;

/// Camera/LiDAR/pose encoder producing the fused core representation.
#[derive(Clone)]
pub struct TinyCnnEncoder {
    camera_conv1: Conv2d,
    camera_conv2: Conv2d,
    camera_conv3: Conv2d,
    camera_projection: Linear,
    lidar_conv1: Conv1d,
    lidar_conv2: Conv1d,
    lidar_conv3: Conv1d,
    lidar_projection: Linear,
    pose_projection: Linear,
    fusion_norm: LayerNorm,
}

impl TinyCnnEncoder {
    /// Build the encoder from a variable builder namespace.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let camera_config = Conv2dConfig {
            padding: 1,
            stride: 2,
            ..Default::default()
        };
        let lidar_config = Conv1dConfig {
            padding: 1,
            stride: 2,
            ..Default::default()
        };
        let camera_conv1 = conv2d(1, 8, 3, camera_config, vb.pp("camera.conv1"))?;
        let camera_conv2 = conv2d(8, 16, 3, camera_config, vb.pp("camera.conv2"))?;
        let camera_conv3 = conv2d(16, 32, 3, camera_config, vb.pp("camera.conv3"))?;
        let camera_projection =
            linear(32 * 6 * 8, CAMERA_FEATURES, vb.pp("camera.projection"))?;
        let lidar_conv1 = conv1d(2, 8, 3, lidar_config, vb.pp("lidar.conv1"))?;
        let lidar_conv2 = conv1d(8, 16, 3, lidar_config, vb.pp("lidar.conv2"))?;
        let lidar_conv3 = conv1d(16, 32, 3, lidar_config, vb.pp("lidar.conv3"))?;
        let lidar_projection = linear(32 * 90, LIDAR_FEATURES, vb.pp("lidar.projection"))?;
        let pose_projection = linear(POSE_DIM, POSE_FEATURES, vb.pp("pose.projection"))?;
        let fusion_norm = layer_norm(CORE_DIM, 1e-5, vb.pp("fusion.norm"))?;
        Ok(Self {
            camera_conv1,
            camera_conv2,
            camera_conv3,
            camera_projection,
            lidar_conv1,
            lidar_conv2,
            lidar_conv3,
            lidar_projection,
            pose_projection,
            fusion_norm,
        })
    }

    /// Encode one batch. Action and elapsed time are intentionally absent from
    /// this signature: the representation must not see the future.
    pub fn forward(&self, camera: &Tensor, lidar: &Tensor, pose: &Tensor) -> CandleResult<Tensor> {
        let mut camera_features = self.camera_conv1.forward(camera)?.relu()?;
        for layer in [&self.camera_conv2, &self.camera_conv3] {
            camera_features = layer.forward(&camera_features)?.relu()?;
        }
        let camera_features = self
            .camera_projection
            .forward(&camera_features.flatten_from(1)?)?
            .relu()?;

        let mut lidar_features = self.lidar_conv1.forward(lidar)?.relu()?;
        for layer in [&self.lidar_conv2, &self.lidar_conv3] {
            lidar_features = layer.forward(&lidar_features)?.relu()?;
        }
        let lidar_features = self
            .lidar_projection
            .forward(&lidar_features.flatten_from(1)?)?
            .relu()?;

        let pose_features = self.pose_projection.forward(pose)?.relu()?;
        let fused = Tensor::cat(&[&camera_features, &lidar_features, &pose_features], 1)?;
        self.fusion_norm.forward(&fused)
    }
}

/// Gaussian transition head over the core representation.
#[derive(Clone)]
pub struct TransitionPredictor {
    hidden: Linear,
    output: Linear,
}

/// Mean and log-variance of the predicted next latent.
#[derive(Clone)]
pub struct TransitionPrediction {
    pub mean: Tensor,
    pub log_variance: Tensor,
}

/// The decomposed JEPA training objective.
#[derive(Clone)]
pub struct JepaLoss {
    pub total: Tensor,
    pub transition_nll: Tensor,
    pub variance: Tensor,
    pub covariance: Tensor,
}

impl TransitionPredictor {
    /// Build the predictor from a variable builder namespace.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let hidden = linear(PREDICTOR_INPUT_DIM, PREDICTOR_HIDDEN_DIM, vb.pp("hidden"))?;
        let output = linear(PREDICTOR_HIDDEN_DIM, CORE_DIM * 2, vb.pp("output"))?;
        Ok(Self { hidden, output })
    }

    /// Predict the next latent from the current latent, action and elapsed time.
    ///
    /// The log-variance is clamped to `[-10, 5]`; the calibration gate treats a
    /// saturated dimension as evidence against promotion rather than signal.
    pub fn forward(
        &self,
        latent: &Tensor,
        action: &Tensor,
        delta_seconds: &Tensor,
    ) -> CandleResult<TransitionPrediction> {
        let joined = Tensor::cat(&[latent, action, delta_seconds], 1)?;
        let hidden = self.hidden.forward(&joined)?.relu()?;
        let projected = self.output.forward(&hidden)?;
        let mean = projected.narrow(1, 0, CORE_DIM)?.contiguous()?;
        let log_variance = projected
            .narrow(1, CORE_DIM, CORE_DIM)?
            .contiguous()?
            .clamp(-10f32, 5f32)?;
        Ok(TransitionPrediction {
            mean,
            log_variance,
        })
    }
}

/// Fixed random projection from the core latent into belief space.
#[derive(Clone)]
pub struct EvidenceAdapter {
    projection: Linear,
}

/// Decoder from a predicted latent to future occupancy logits.
#[derive(Clone)]
pub struct FutureOccupancyDecoder {
    hidden: Linear,
    output: Linear,
}

impl FutureOccupancyDecoder {
    /// Build the decoder from a variable builder namespace.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let hidden = linear(CORE_DIM, GROUNDING_HIDDEN_DIM, vb.pp("hidden"))?;
        let output = linear(
            GROUNDING_HIDDEN_DIM,
            qualia_jepa::GROUNDING_CELLS,
            vb.pp("output"),
        )?;
        Ok(Self { hidden, output })
    }

    /// Decode occupancy logits from a latent.
    pub fn forward(&self, predicted_latent: &Tensor) -> CandleResult<Tensor> {
        let hidden = self.hidden.forward(predicted_latent)?.relu()?;
        self.output.forward(&hidden)
    }
}

/// Masked, class-balanced grounding loss and its cell accounting.
#[derive(Clone)]
pub struct GroundingLoss {
    pub mean: Tensor,
    pub observed_cells: usize,
    pub occupied_cells: usize,
}

/// Binary cross entropy with logits over measured grounding cells only.
///
/// Free space dominates a LiDAR window, so occupied cells are weighted up by
/// `occupied_weight`. The log-sum-exp tail is evaluated in its stable form, so
/// a large-magnitude logit cannot overflow the loss.
pub fn masked_grounding_bce_with_logits(
    logits: &Tensor,
    targets: &Tensor,
    observed: &Tensor,
    occupied_weight: f64,
) -> CandleResult<GroundingLoss> {
    if logits.dims() != targets.dims() || observed.dims() != logits.dims() {
        candle_core::bail!("grounding logits, targets, and mask must have matching shapes")
    }
    if !occupied_weight.is_finite() || occupied_weight <= 0.0 {
        candle_core::bail!("occupied grounding weight must be finite and positive")
    }

    let host_values = |tensor: &Tensor| -> CandleResult<Vec<f32>> {
        Ok(tensor
            .flatten_all()?
            .to_device(&Device::Cpu)?
            .to_vec1::<f32>()?)
    };
    let target_values = host_values(targets)?;
    let observed_values = host_values(observed)?;
    let is_binary = |values: &[f32]| values.iter().all(|value| *value == 0.0 || *value == 1.0);
    if !is_binary(&target_values) || !is_binary(&observed_values) {
        candle_core::bail!("grounding targets and masks must be binary")
    }
    let observed_cells = observed_values.iter().filter(|value| **value == 1.0).count();
    if observed_cells == 0 {
        candle_core::bail!("grounding mask contains no measured cells")
    }
    let occupied_cells = target_values
        .iter()
        .zip(observed_values.iter())
        .filter(|(target, mask)| **target == 1.0 && **mask == 1.0)
        .count();

    // log(1 + exp(-|x|)) is the overflow-safe remainder of softplus(x).
    let tail = logits.abs()?.neg()?.exp()?.affine(1.0, 1.0)?.log()?;
    let logit_term = logits.relu()?.sub(&logits.mul(targets)?)?;
    let per_cell = logit_term.add(&tail)?;
    let class_weight = targets.affine(occupied_weight - 1.0, 1.0)?;
    let masked_sum = per_cell.mul(&class_weight)?.mul(observed)?.sum_all()?;
    Ok(GroundingLoss {
        mean: masked_sum.affine(1.0 / observed_cells as f64, 0.0)?,
        observed_cells,
        occupied_cells,
    })
}

impl EvidenceAdapter {
    /// Build the adapter from a variable builder namespace.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let projection = linear(CORE_DIM, BELIEF_DIM, vb.pp("projection"))?;
        Ok(Self { projection })
    }

    /// Project a latent into belief space.
    pub fn forward(&self, latent: &Tensor) -> CandleResult<Tensor> {
        self.projection.forward(latent)
    }
}

/// The full candidate: encoder, predictor, adapter and occupancy decoder.
#[derive(Clone)]
pub struct JepaCandidateModel {
    pub encoder: TinyCnnEncoder,
    pub predictor: TransitionPredictor,
    pub adapter: EvidenceAdapter,
    pub occupancy_decoder: FutureOccupancyDecoder,
}

impl JepaCandidateModel {
    /// Build the whole candidate from one variable builder.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let encoder = TinyCnnEncoder::load(vb.pp("encoder"))?;
        let predictor = TransitionPredictor::load(vb.pp("predictor"))?;
        let adapter = EvidenceAdapter::load(vb.pp("adapter"))?;
        let occupancy_decoder = FutureOccupancyDecoder::load(vb.pp("occupancy_decoder"))?;
        Ok(Self {
            encoder,
            predictor,
            adapter,
            occupancy_decoder,
        })
    }

    /// Encode a packed observation batch.
    pub fn encode_observation(&self, observation: &Tensor) -> CandleResult<Tensor> {
        encode_observation(&self.encoder, observation)
    }
}

/// Split a packed observation and encode it.
pub fn encode_observation(encoder: &TinyCnnEncoder, observation: &Tensor) -> CandleResult<Tensor> {
    let batch = observation.dim(0)?;
    if observation.dims() != [batch, qualia_jepa::OBS_DIM] {
        candle_core::bail!("observation tensor must have shape [batch, 4520]")
    }
    let lidar_width = qualia_jepa::LIDAR_BINS * qualia_jepa::LIDAR_CHANNELS;
    let camera_end = qualia_jepa::CAMERA_PIXELS;
    let lidar_end = camera_end + lidar_width;
    let camera = observation
        .narrow(1, 0, camera_end)?
        .contiguous()?
        .reshape((
            batch,
            1,
            qualia_jepa::CAMERA_HEIGHT,
            qualia_jepa::CAMERA_WIDTH,
        ))?;
    let lidar = observation
        .narrow(1, camera_end, lidar_width)?
        .contiguous()?
        .reshape((batch, qualia_jepa::LIDAR_CHANNELS, qualia_jepa::LIDAR_BINS))?;
    let pose = observation
        .narrow(1, lidar_end, POSE_DIM)?
        .contiguous()?;
    encoder.forward(&camera, &lidar, &pose)
}

/// The composite JEPA objective: transition NLL plus VICReg on both branches.
pub fn jepa_loss(
    prediction: &TransitionPrediction,
    target: &Tensor,
    online_latent: &Tensor,
    variance_weight: f64,
    covariance_weight: f64,
) -> CandleResult<JepaLoss> {
    let shapes_match = prediction.mean.dims() == target.dims()
        && prediction.log_variance.dims() == target.dims()
        && online_latent.dims() == target.dims();
    if !shapes_match {
        candle_core::bail!("JEPA loss tensors must have matching shapes")
    }
    let transition_nll = transition_nll_tensor(prediction, target)?;
    let (online_variance, online_covariance) = vicreg_loss(online_latent)?;
    let (target_variance, target_covariance) = vicreg_loss(target)?;
    let variance = online_variance.add(&target_variance)?.affine(0.5, 0.0)?;
    let covariance = online_covariance.add(&target_covariance)?.affine(0.5, 0.0)?;
    let variance_term = variance.affine(variance_weight, 0.0)?;
    let covariance_term = covariance.affine(covariance_weight, 0.0)?;
    let total = transition_nll.add(&variance_term)?.add(&covariance_term)?;
    Ok(JepaLoss {
        total,
        transition_nll,
        variance,
        covariance,
    })
}

/// Mean heteroscedastic Gaussian NLL of a prediction against a target.
pub fn transition_nll_tensor(
    prediction: &TransitionPrediction,
    target: &Tensor,
) -> CandleResult<Tensor> {
    if prediction.mean.dims() != target.dims() || prediction.log_variance.dims() != target.dims() {
        candle_core::bail!("transition NLL tensors must have matching shapes")
    }
    let squared_residual = target.sub(&prediction.mean)?.sqr()?;
    let precision = prediction.log_variance.neg()?.exp()?;
    let per_element = squared_residual
        .mul(&precision)?
        .add(&prediction.log_variance)?;
    per_element.mean_all()?.affine(0.5, 0.0)
}

/// VICReg variance and covariance terms of one representation batch.
pub fn vicreg_loss(representations: &Tensor) -> CandleResult<(Tensor, Tensor)> {
    let (batch, dimensions) = representations.dims2()?;
    if batch < 2 || dimensions == 0 {
        candle_core::bail!("VICReg requires at least two non-empty representations")
    }
    let centered = representations.broadcast_sub(&representations.mean_keepdim(0)?)?;
    let standard_deviation = centered
        .sqr()?
        .mean_keepdim(0)?
        .affine(1.0, VICREG_EPSILON)?
        .sqrt()?;
    let variance = standard_deviation
        .affine(-1.0, VICREG_TARGET_STDDEV)?
        .relu()?
        .mean_all()?;

    let covariance = centered
        .t()?
        .matmul(&centered)?
        .affine(1.0 / (batch - 1) as f64, 0.0)?;
    let off_diagonal = (0..dimensions * dimensions)
        .map(|cell| {
            if cell / dimensions == cell % dimensions {
                0.0f32
            } else {
                1.0f32
            }
        })
        .collect::<Vec<_>>();
    let mask = Tensor::from_vec(off_diagonal, (dimensions, dimensions), representations.device())?;
    let covariance = covariance
        .sqr()?
        .mul(&mask)?
        .sum_all()?
        .affine(1.0 / dimensions as f64, 0.0)?;
    Ok((variance, covariance))
}

/// Flat MLP encoder used as the frozen comparison baseline.
#[derive(Clone)]
pub struct FlatMlpBaseline {
    hidden: Linear,
    output: Linear,
}

impl FlatMlpBaseline {
    /// Build the baseline from a variable builder namespace.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let hidden = linear(qualia_jepa::OBS_DIM, 512, vb.pp("hidden"))?;
        let output = linear(512, CORE_DIM, vb.pp("output"))?;
        Ok(Self { hidden, output })
    }

    /// Encode a packed observation batch.
    pub fn forward(&self, observation: &Tensor) -> CandleResult<Tensor> {
        let hidden = self.hidden.forward(observation)?.relu()?;
        self.output.forward(&hidden)
    }
}

/// The flat baseline paired with the same transition head.
#[derive(Clone)]
pub struct FlatJepaBaseline {
    pub encoder: FlatMlpBaseline,
    pub predictor: TransitionPredictor,
}

impl FlatJepaBaseline {
    /// Build the baseline pair from one variable builder.
    pub fn load(vb: VarBuilder<'_>) -> CandleResult<Self> {
        let encoder = FlatMlpBaseline::load(vb.pp("encoder"))?;
        let predictor = TransitionPredictor::load(vb.pp("predictor"))?;
        Ok(Self {
            encoder,
            predictor,
        })
    }
}

/// Total number of scalar parameters held by a variable map.
pub fn parameter_count(var_map: &VarMap) -> usize {
    let variables = var_map.data().lock().expect("Candle VarMap lock");
    variables
        .values()
        .map(|variable| variable.elem_count())
        .sum()
}

/// Seed every parameter from a named host-side stream instead of backend RNG.
///
/// Values are produced on the host and copied to each parameter's device, so a
/// seed and architecture produce bit-identical weights on CPU and on an
/// accelerator. The evidence adapter ignores `seed` and instead derives from
/// `EVIDENCE_ADAPTER_ID`, which freezes it across candidates.
pub fn initialize_deterministic(var_map: &VarMap, seed: u64) -> CandleResult<()> {
    let variables = var_map.data().lock().expect("Candle VarMap lock");
    for (name, variable) in variables.iter() {
        let values = initial_parameter_values(name, variable.dims(), seed);
        let host = Tensor::from_vec(values, variable.shape(), &Device::Cpu)?;
        variable.set(&host.to_device(variable.device())?)?;
    }
    Ok(())
}

/// Draw the initial values of one parameter tensor.
///
/// Biases start at zero and normalization scales at one. Learned heads use the
/// Kaiming-uniform limit `sqrt(6 / fan_in)` from the run's seed; the evidence
/// adapter uses `sqrt(3 / BELIEF_DIM)`, which keeps a fixed projection's column
/// norms near unit length, from a stream keyed only by its versioned id.
fn initial_parameter_values(name: &str, dims: &[usize], seed: u64) -> Vec<f32> {
    let count = dims.iter().product::<usize>();
    if name.ends_with(".bias") {
        return vec![0.0; count];
    }
    if name.contains(".norm.weight") {
        return vec![1.0; count];
    }
    let (stream, limit) = if name.starts_with("adapter.") {
        (
            stable_name_hash(EVIDENCE_ADAPTER_ID),
            (3.0f32 / BELIEF_DIM as f32).sqrt(),
        )
    } else {
        let fan_in = dims.iter().skip(1).product::<usize>().max(1);
        (seed, (6.0f32 / fan_in as f32).sqrt())
    };
    let mut state = stream ^ stable_name_hash(name);
    (0..count)
        .map(|_| {
            state = splitmix64(state);
            let unit = (state >> 40) as f32 / (1u32 << 24) as f32;
            (unit * 2.0 - 1.0) * limit
        })
        .collect()
}

/// FNV-1a 64-bit hash of a parameter name.
fn stable_name_hash(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in name.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// SplitMix64 step used as the parameter stream.
fn splitmix64(state: u64) -> u64 {
    let mut value = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Move a target parameter set toward the online one with the given EMA decay.
pub fn ema_update(target: &VarMap, online: &VarMap, decay: f64) -> CandleResult<()> {
    ema_update_prefix(target, online, decay, "")
}

/// Prefix-scoped EMA, used for the encoder-only target network.
///
/// The named parameter sets must match exactly; a partial target would
/// otherwise become the JEPA training target without anyone noticing.
pub fn ema_update_prefix(
    target: &VarMap,
    online: &VarMap,
    decay: f64,
    prefix: &str,
) -> CandleResult<()> {
    if !(0.0..=1.0).contains(&decay) || !decay.is_finite() {
        candle_core::bail!("EMA decay must be finite and in [0, 1]")
    }

    let online_values = {
        let variables = online.data().lock().expect("Candle online VarMap lock");
        let mut values = HashMap::new();
        for (name, variable) in variables.iter() {
            if name.starts_with(prefix) {
                values.insert(name.clone(), variable.as_detached_tensor());
            }
        }
        values
    };

    let target_variables = target.data().lock().expect("Candle target VarMap lock");
    if target_variables.len() != online_values.len()
        || target_variables
            .keys()
            .any(|name| !online_values.contains_key(name))
    {
        candle_core::bail!("EMA parameter sets do not match")
    }
    for (name, target_variable) in target_variables.iter() {
        let online_tensor = &online_values[name];
        let metadata_matches = target_variable.shape() == online_tensor.shape()
            && target_variable.dtype() == online_tensor.dtype()
            && target_variable.device().location() == online_tensor.device().location();
        if !metadata_matches {
            candle_core::bail!("EMA parameter {name} has incompatible tensor metadata")
        }
        let retained = target_variable.as_tensor().affine(decay, 0.0)?;
        let learned = online_tensor.affine(1.0 - decay, 0.0)?;
        target_variable.set(&retained.add(&learned)?.detach())?;
    }
    Ok(())
}

/// Resolve a backend name to a device, rejecting uncompiled backends.
pub fn device_for_backend(backend: &str) -> CandleResult<Device> {
    #[cfg(feature = "metal")]
    if backend == "metal" {
        return Device::new_metal(0);
    }
    #[cfg(feature = "cuda")]
    if backend == "cuda" {
        return Device::new_cuda(0);
    }
    if backend == "cpu" {
        return Ok(Device::Cpu);
    }
    candle_core::bail!("backend {backend} is not compiled into this binary")
}

/// One point in the held-out comparison.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeldOutMetrics {
    pub transition_nll: f64,
    pub rollout_error: f64,
}

/// Aggregate support a promoted dataset must demonstrate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BaselineGate {
    pub dataset_digest: String,
    pub valid_transitions: u64,
    pub sessions: u64,
    pub conditions: u64,
    pub environments: u64,
    pub constant: HeldOutMetrics,
    pub flat_mlp: HeldOutMetrics,
    pub tiny_cnn: HeldOutMetrics,
}

/// The measured joint action/time envelope of the training split.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionSupport {
    pub sample_count: u64,
    pub min_left: f32,
    pub max_left: f32,
    pub min_right: f32,
    pub max_right: f32,
    pub min_effective_forward: f32,
    pub max_effective_forward: f32,
    pub min_effective_turn: f32,
    pub max_effective_turn: f32,
    pub min_speed_scale: f32,
    pub max_speed_scale: f32,
    pub min_delta_seconds: f32,
    pub max_delta_seconds: f32,
}

/// Geometry of the robot-centric grounding window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GroundingGeometry {
    pub width: usize,
    pub height: usize,
    pub resolution_m: f32,
}

impl GroundingGeometry {
    /// The window must be the frozen size and carry a usable metric scale.
    pub fn passes(&self) -> bool {
        self.width == GROUNDING_WIDTH
            && self.height == GROUNDING_HEIGHT
            && self.resolution_m > 0.0
            && self.resolution_m.is_finite()
    }
}

/// One held-out split of a training report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SplitEvaluation {
    pub samples: u64,
    pub sessions: u64,
    pub constant: HeldOutMetrics,
    pub flat_mlp: HeldOutMetrics,
    pub tiny_cnn: HeldOutMetrics,
    pub calibration: evaluation::CalibrationReport,
    pub occupancy: evaluation::OccupancyReport,
}

/// The complete on-disk training report the evidence gates read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TrainingReport {
    pub schema_version: String,
    pub created_at_ms: u128,
    pub checkpoint_id: String,
    pub dataset_digest: String,
    pub backend: String,
    pub seed: u64,
    pub epochs: usize,
    pub batch_size: usize,
    pub cnn_steps: u64,
    pub flat_steps: u64,
    pub skipped_singletons: u64,
    pub action_support: ActionSupport,
    pub grounding_geometry: GroundingGeometry,
    pub validation: SplitEvaluation,
    pub test: SplitEvaluation,
    pub effective_rank: evaluation::EffectiveRankReport,
    pub llm_priors_ablated: bool,
    pub baseline_gate_passed: bool,
    pub grounding_calibration_gate_passed: bool,
    pub all_gates_passed: bool,
}

impl TrainingReport {
    /// Fail-closed admission: identity, freshness, then every held-out gate.
    pub fn validate_for_promotion(
        &self,
        manifest: &CheckpointManifest,
        now_ms: u128,
        max_report_age_ms: u128,
    ) -> ModelResult<()> {
        let identity_holds = self.schema_version == TRAINING_REPORT_SCHEMA
            && self.checkpoint_id == manifest.checkpoint_id
            && self.dataset_digest == manifest.dataset_digest
            && self.backend == manifest.backend
            && self.seed == manifest.training_seed;
        let schedule_holds = self.epochs > 0
            && self.batch_size >= 2
            && self.cnn_steps > 0
            && self.flat_steps > 0
            && self.cnn_steps == self.flat_steps;
        let envelope_holds = self.action_support == manifest.action_support
            && self.action_support.passes()
            && self.grounding_geometry == manifest.grounding_geometry
            && self.grounding_geometry.passes();
        let baselines_hold = manifest.baseline_gate.dataset_digest == self.dataset_digest
            && manifest.baseline_gate.constant == self.test.constant
            && manifest.baseline_gate.flat_mlp == self.test.flat_mlp
            && manifest.baseline_gate.tiny_cnn == self.test.tiny_cnn;
        let fresh = self.created_at_ms <= now_ms.saturating_add(60_000)
            && now_ms.saturating_sub(self.created_at_ms) <= max_report_age_ms;
        if !(identity_holds && schedule_holds && envelope_holds && baselines_hold && fresh) {
            return Err("training report identity or freshness gate failed".into());
        }

        let summary_flags_hold = self.llm_priors_ablated
            && self.baseline_gate_passed
            && self.grounding_calibration_gate_passed
            && self.all_gates_passed
            && self.effective_rank.passes();
        if !summary_flags_hold {
            return Err("training report sensor-only predictive gate failed".into());
        }
        validate_split_evaluation(&self.validation)?;
        validate_split_evaluation(&self.test)?;
        if !split_predictive_gate_passes(&self.validation)
            || !split_predictive_gate_passes(&self.test)
        {
            return Err("training report held-out predictive gate failed".into());
        }
        if !split_grounding_calibration_gate_passes(&self.validation)
            || !split_grounding_calibration_gate_passes(&self.test)
        {
            return Err("training report grounding, non-finite, or clamp gate failed".into());
        }
        Ok(())
    }
}

/// Whether one held-out split beats the constant and flat baselines outright.
///
/// The trainer computes its summary booleans from this same split-local
/// predicate, so the registry cannot be talked into a different decision.
pub fn split_predictive_gate_passes(split: &SplitEvaluation) -> bool {
    if validate_split_evaluation(split).is_err() {
        return false;
    }
    let candidate = &split.tiny_cnn;
    candidate.transition_nll < split.constant.transition_nll
        && candidate.transition_nll < split.flat_mlp.transition_nll
        && candidate.rollout_error < split.constant.rollout_error
        && candidate.rollout_error < split.flat_mlp.rollout_error
}

/// Whether a split's uncertainty is calibrated and its occupancy informative.
pub fn split_grounding_calibration_gate_passes(split: &SplitEvaluation) -> bool {
    validate_split_evaluation(split).is_ok()
        && split.calibration.passes_contract()
        && split.occupancy.passes_contract()
}

fn validate_split_evaluation(split: &SplitEvaluation) -> ModelResult<()> {
    let scale_ok = split.samples > 0 && split.sessions > 0;
    let metrics_ok = metrics_finite(&split.constant)
        && metrics_finite(&split.flat_mlp)
        && metrics_finite(&split.tiny_cnn);
    if !scale_ok
        || !metrics_ok
        || !split.calibration.is_well_formed()
        || !split.occupancy.is_well_formed()
    {
        return Err("held-out evaluation contains empty or non-finite metrics".into());
    }
    Ok(())
}

impl ActionSupport {
    /// Finite, adequately sampled, and inside the unit action box.
    pub fn passes(&self) -> bool {
        let ordered = |low: f32, high: f32, floor: f32| {
            low.is_finite() && high.is_finite() && floor <= low && low <= high && high <= 1.0
        };
        self.sample_count >= ACTION_SUPPORT_MINIMUM_SAMPLES
            && ordered(self.min_left, self.max_left, -1.0)
            && ordered(self.min_right, self.max_right, -1.0)
            && ordered(self.min_effective_forward, self.max_effective_forward, -1.0)
            && ordered(self.min_effective_turn, self.max_effective_turn, -1.0)
            && ordered(self.min_speed_scale, self.max_speed_scale, 0.0)
            && self.min_delta_seconds.is_finite()
            && self.max_delta_seconds.is_finite()
            && 0.0 < self.min_delta_seconds
            && self.min_delta_seconds <= self.max_delta_seconds
    }

    /// Whether one four-axis command lies inside the measured envelope.
    pub fn contains(&self, left: f32, right: f32, speed_scale: f32, dt_seconds: f32) -> bool {
        let inputs_finite = [left, right, speed_scale, dt_seconds]
            .into_iter()
            .all(f32::is_finite);
        if !self.passes() || !inputs_finite {
            return false;
        }
        let (effective_forward, effective_turn) = effective_action_axes(left, right, speed_scale);
        within(self.min_left, self.max_left, left)
            && within(self.min_right, self.max_right, right)
            && within(
                self.min_effective_forward,
                self.max_effective_forward,
                effective_forward,
            )
            && within(
                self.min_effective_turn,
                self.max_effective_turn,
                effective_turn,
            )
            && within(self.min_speed_scale, self.max_speed_scale, speed_scale)
            && within(self.min_delta_seconds, self.max_delta_seconds, dt_seconds)
    }
}

/// Differential-drive forward and turn axes from the two wheel commands.
pub fn effective_action_axes(left: f32, right: f32, speed_scale: f32) -> (f32, f32) {
    let forward = 0.5 * (left + right) * speed_scale;
    let turn = 0.5 * (right - left) * speed_scale;
    (forward, turn)
}

/// Measure the joint action/time envelope from the train split's samples alone.
///
/// Trainer and registry both call this, so a serialized checkpoint cannot
/// widen, narrow, or relabel the measured joint action/time support.
pub fn measured_action_support(manifest: &DatasetManifest) -> ModelResult<ActionSupport> {
    let mut support: Option<ActionSupport> = None;
    for sample in manifest
        .samples
        .iter()
        .filter(|sample| sample.split == DatasetSplit::Train)
    {
        let action = &sample.action;
        let interval_ns = action
            .interval_end_ns
            .checked_sub(action.interval_start_ns)
            .ok_or("dataset action interval is inverted")?;
        let delta_seconds = interval_ns as f32 / 1_000_000_000.0;
        let (effective_forward, effective_turn) =
            effective_action_axes(action.left, action.right, action.speed_scale);
        support = Some(match support {
            None => ActionSupport {
                sample_count: 1,
                min_left: action.left,
                max_left: action.left,
                min_right: action.right,
                max_right: action.right,
                min_effective_forward: effective_forward,
                max_effective_forward: effective_forward,
                min_effective_turn: effective_turn,
                max_effective_turn: effective_turn,
                min_speed_scale: action.speed_scale,
                max_speed_scale: action.speed_scale,
                min_delta_seconds: delta_seconds,
                max_delta_seconds: delta_seconds,
            },
            Some(previous) => ActionSupport {
                sample_count: previous.sample_count + 1,
                min_left: previous.min_left.min(action.left),
                max_left: previous.max_left.max(action.left),
                min_right: previous.min_right.min(action.right),
                max_right: previous.max_right.max(action.right),
                min_effective_forward: previous.min_effective_forward.min(effective_forward),
                max_effective_forward: previous.max_effective_forward.max(effective_forward),
                min_effective_turn: previous.min_effective_turn.min(effective_turn),
                max_effective_turn: previous.max_effective_turn.max(effective_turn),
                min_speed_scale: previous.min_speed_scale.min(action.speed_scale),
                max_speed_scale: previous.max_speed_scale.max(action.speed_scale),
                min_delta_seconds: previous.min_delta_seconds.min(delta_seconds),
                max_delta_seconds: previous.max_delta_seconds.max(delta_seconds),
            },
        });
    }
    let support = support.ok_or("training split has no action support samples")?;
    if !support.passes() {
        return Err("dataset action support is non-finite, undersized, or out of bounds".into());
    }
    Ok(support)
}

impl BaselineGate {
    /// Dataset scale plus a strict win over both frozen baselines.
    pub fn passes(&self) -> bool {
        let scale_ok = self.valid_transitions >= 50_000
            && self.sessions >= 12
            && self.conditions >= 3
            && self.environments >= 3;
        let metrics_ok = metrics_finite(&self.constant)
            && metrics_finite(&self.flat_mlp)
            && metrics_finite(&self.tiny_cnn);
        let beats = |baseline: &HeldOutMetrics| {
            self.tiny_cnn.transition_nll < baseline.transition_nll
                && self.tiny_cnn.rollout_error < baseline.rollout_error
        };
        scale_ok && metrics_ok && beats(&self.constant) && beats(&self.flat_mlp)
    }
}

/// The immutable metadata published beside `weights.safetensors`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointManifest {
    pub schema_version: String,
    pub architecture_id: String,
    pub checkpoint_id: String,
    pub dataset_digest: String,
    pub training_seed: u64,
    pub backend: String,
    pub dtype: String,
    pub target_encoder: String,
    pub parameter_count: u64,
    pub weights_sha256: String,
    pub training_report_path: String,
    pub training_report_sha256: String,
    pub baseline_gate: BaselineGate,
    pub action_support: ActionSupport,
    pub grounding_geometry: GroundingGeometry,
    pub status: String,
}

/// Flush a freshly written file to stable storage.
///
/// Windows refuses to flush a handle without write access, so the file is
/// reopened for writing even though nothing more is written through it.
fn sync_file(path: &Path) -> ModelResult<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()?;
    Ok(())
}

/// Whether every byte of a checkpoint id is a filename-safe character.
fn valid_checkpoint_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

/// Write an atomic candidate checkpoint and return `(weights, manifest)`.
///
/// The online model and the EMA encoder are stored in a single safetensors
/// file under `target_encoder.` names, so a reader that verifies the digest has
/// verified the exact bytes the runtime will load.
pub fn write_candidate_checkpoint(
    root: impl AsRef<Path>,
    checkpoint_id: &str,
    online_vars: &VarMap,
    target_encoder_vars: &VarMap,
    mut manifest: CheckpointManifest,
) -> ModelResult<(PathBuf, PathBuf)> {
    if checkpoint_id.is_empty() || !checkpoint_id.bytes().all(valid_checkpoint_id_byte) {
        return Err("unsafe checkpoint id".into());
    }
    let identity_holds = manifest.schema_version == "qualia.jepa-checkpoint.v1"
        && manifest.architecture_id == ARCHITECTURE_ID
        && manifest.checkpoint_id == checkpoint_id
        && manifest.target_encoder == "ema"
        && manifest.action_support.passes()
        && manifest.grounding_geometry.passes();
    if !identity_holds {
        return Err("checkpoint manifest identity mismatch".into());
    }
    if manifest.status != "candidate" {
        return Err("new checkpoints must be candidates".into());
    }

    let root = root.as_ref();
    fs::create_dir_all(root)?;
    let destination = root.join(checkpoint_id);
    let staging = root.join(format!(".{checkpoint_id}.partial"));
    if destination.exists() || staging.exists() {
        return Err("checkpoint id already exists".into());
    }
    let merged = checkpoint_variables(online_vars, target_encoder_vars)?;
    fs::create_dir(&staging)?;
    if let Err(error) = publish_checkpoint(&staging, &merged, &mut manifest) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    fs::rename(&staging, &destination)?;
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
    Ok((
        destination.join("weights.safetensors"),
        destination.join("manifest.json"),
    ))
}

/// Refuse to serialize a manifest carrying a non-finite floating-point metric.
///
/// JSON has no NaN or infinity literal, so `serde_json` writes `null` where a
/// non-finite `f64`/`f32` stood; the manifest then fails its own reader with
/// `invalid type: null, expected f64`. A non-finite held-out metric is a real
/// outcome of a diverged run rather than a formatting accident, so the
/// publication boundary refuses it by name and publishes nothing.
fn ensure_manifest_metrics_finite(manifest: &CheckpointManifest) -> ModelResult<()> {
    let gate = &manifest.baseline_gate;
    let support = &manifest.action_support;
    let fields: [(&str, f64); 19] = [
        (
            "baseline_gate.constant.transition_nll",
            gate.constant.transition_nll,
        ),
        (
            "baseline_gate.constant.rollout_error",
            gate.constant.rollout_error,
        ),
        (
            "baseline_gate.flat_mlp.transition_nll",
            gate.flat_mlp.transition_nll,
        ),
        (
            "baseline_gate.flat_mlp.rollout_error",
            gate.flat_mlp.rollout_error,
        ),
        (
            "baseline_gate.tiny_cnn.transition_nll",
            gate.tiny_cnn.transition_nll,
        ),
        (
            "baseline_gate.tiny_cnn.rollout_error",
            gate.tiny_cnn.rollout_error,
        ),
        ("action_support.min_left", f64::from(support.min_left)),
        ("action_support.max_left", f64::from(support.max_left)),
        ("action_support.min_right", f64::from(support.min_right)),
        ("action_support.max_right", f64::from(support.max_right)),
        (
            "action_support.min_effective_forward",
            f64::from(support.min_effective_forward),
        ),
        (
            "action_support.max_effective_forward",
            f64::from(support.max_effective_forward),
        ),
        (
            "action_support.min_effective_turn",
            f64::from(support.min_effective_turn),
        ),
        (
            "action_support.max_effective_turn",
            f64::from(support.max_effective_turn),
        ),
        (
            "action_support.min_speed_scale",
            f64::from(support.min_speed_scale),
        ),
        (
            "action_support.max_speed_scale",
            f64::from(support.max_speed_scale),
        ),
        (
            "action_support.min_delta_seconds",
            f64::from(support.min_delta_seconds),
        ),
        (
            "action_support.max_delta_seconds",
            f64::from(support.max_delta_seconds),
        ),
        (
            "grounding_geometry.resolution_m",
            f64::from(manifest.grounding_geometry.resolution_m),
        ),
    ];
    for (field, value) in fields {
        if !value.is_finite() {
            return Err(format!(
                "refusing to publish checkpoint {}: non-finite manifest metric {field} = {value}; \
                 a non-finite held-out metric means the evaluation diverged, and JSON writes it as \
                 null, which the manifest reader rejects",
                manifest.checkpoint_id
            )
            .into());
        }
    }
    Ok(())
}

/// Serialize one checkpoint's weights and manifest into a staging directory.
fn publish_checkpoint(
    staging: &Path,
    checkpoint_vars: &VarMap,
    manifest: &mut CheckpointManifest,
) -> ModelResult<()> {
    ensure_manifest_metrics_finite(manifest)?;
    let weights_partial = staging.join("weights.safetensors");
    checkpoint_vars.save(&weights_partial)?;
    sync_file(&weights_partial)?;
    let weights = fs::read(&weights_partial)?;
    manifest.weights_sha256 = format!("{:x}", Sha256::digest(&weights));
    manifest.parameter_count = parameter_count(checkpoint_vars) as u64;
    validate_checkpoint_weights(&weights, manifest.parameter_count)?;

    let manifest_partial = staging.join("manifest.json");
    fs::write(&manifest_partial, serde_json::to_vec_pretty(manifest)?)?;
    sync_file(&manifest_partial)?;
    #[cfg(unix)]
    std::fs::File::open(staging)?.sync_all()?;
    Ok(())
}

/// Build the manifest skeleton a fresh candidate starts from.
pub fn empty_candidate_manifest(
    checkpoint_id: &str,
    dataset_digest: &str,
    training_seed: u64,
    backend: &str,
    baseline_gate: BaselineGate,
    action_support: ActionSupport,
    grounding_geometry: GroundingGeometry,
) -> CheckpointManifest {
    CheckpointManifest {
        checkpoint_id: checkpoint_id.to_string(),
        schema_version: "qualia.jepa-checkpoint.v1".to_string(),
        dataset_digest: dataset_digest.to_string(),
        architecture_id: ARCHITECTURE_ID.to_string(),
        training_seed,
        backend: backend.to_string(),
        dtype: format!("{:?}", DType::F32),
        target_encoder: "ema".to_string(),
        status: "candidate".to_string(),
        parameter_count: 0,
        weights_sha256: String::new(),
        training_report_path: String::new(),
        training_report_sha256: String::new(),
        baseline_gate,
        action_support,
        grounding_geometry,
    }
}

/// Merge the online model and the encoder-only EMA target into one namespace.
fn checkpoint_variables(online_vars: &VarMap, target_encoder_vars: &VarMap) -> ModelResult<VarMap> {
    let online: HashMap<_, _> = {
        let variables = online_vars
            .data()
            .lock()
            .expect("Candle online checkpoint VarMap lock");
        variables
            .iter()
            .map(|(name, variable)| (name.clone(), variable.clone()))
            .collect()
    };
    let target: HashMap<_, _> = {
        let variables = target_encoder_vars
            .data()
            .lock()
            .expect("Candle target checkpoint VarMap lock");
        variables
            .iter()
            .map(|(name, variable)| (name.clone(), variable.clone()))
            .collect()
    };
    let online_encoder: HashMap<_, _> = online
        .iter()
        .filter_map(|(name, variable)| {
            name.strip_prefix("encoder.")
                .map(|suffix| (suffix.to_string(), variable.clone()))
        })
        .collect();
    let target_encoder: HashMap<_, _> = target
        .iter()
        .filter_map(|(name, variable)| {
            name.strip_prefix("encoder.")
                .map(|suffix| (suffix.to_string(), variable.clone()))
        })
        .collect();
    let encoders_match = !online_encoder.is_empty()
        && online_encoder.len() == target_encoder.len()
        && online_encoder.iter().all(|(suffix, online)| {
            target_encoder.get(suffix).is_some_and(|target| {
                online.shape() == target.shape()
                    && online.dtype() == target.dtype()
                    && online.device().location() == target.device().location()
            })
        });
    if !encoders_match {
        return Err("online and EMA target encoder checkpoint parameters do not match".into());
    }

    let checkpoint = VarMap::new();
    {
        let mut destination = checkpoint
            .data()
            .lock()
            .expect("Candle combined checkpoint VarMap lock");
        for (name, variable) in online {
            if name.starts_with("target_encoder.") || destination.contains_key(&name) {
                return Err("online checkpoint contains a reserved or duplicate parameter".into());
            }
            destination.insert(name, variable);
        }
        for (suffix, variable) in target_encoder {
            let key = format!("target_encoder.{suffix}");
            if destination.contains_key(&key) {
                return Err("EMA target checkpoint parameter collides with online weights".into());
            }
            destination.insert(key, variable);
        }
    }
    Ok(checkpoint)
}

/// Check every stored tensor against the frozen architecture inventory before
/// registration or runtime loading.
///
/// A matching digest proves byte identity but not that the bytes hold every
/// required online and EMA tensor with the frozen shape, so the inventory is
/// rebuilt from the architecture and compared tensor by tensor.
pub fn validate_checkpoint_weights(weights: &[u8], declared_parameters: u64) -> ModelResult<()> {
    let device = Device::Cpu;
    let online = VarMap::new();
    JepaCandidateModel::load(VarBuilder::from_varmap(&online, DType::F32, &device))?;
    initialize_deterministic(&online, 0)?;
    let target = VarMap::new();
    TinyCnnEncoder::load(VarBuilder::from_varmap(&target, DType::F32, &device).pp("encoder"))?;
    let expected = checkpoint_variables(&online, &target)?;
    let expected: HashMap<_, _> = {
        let variables = expected
            .data()
            .lock()
            .expect("Candle expected checkpoint VarMap lock");
        variables
            .iter()
            .map(|(name, variable)| (name.clone(), variable.clone()))
            .collect()
    };

    let archive = candle_core::safetensors::SliceSafetensors::new(weights)?;
    let tensors = archive.tensors();
    if tensors.len() != expected.len() {
        return Err("checkpoint tensor inventory does not match the frozen architecture".into());
    }
    let mut parameters = 0_u64;
    for (name, tensor) in tensors {
        let Some(variable) = expected.get(&name) else {
            return Err(format!("checkpoint contains unexpected tensor {name}").into());
        };
        let tensor_dtype = DType::try_from(tensor.dtype())?;
        let metadata_ok = tensor.shape() == variable.dims()
            && tensor_dtype == variable.dtype()
            && tensor_dtype == DType::F32;
        if !metadata_ok {
            return Err(format!("checkpoint tensor {name} has incompatible metadata").into());
        }
        if name.starts_with("adapter.") {
            let stored = archive
                .load(&name, &device)?
                .flatten_all()?
                .to_vec1::<f32>()?;
            let regenerated = variable
                .as_detached_tensor()
                .flatten_all()?
                .to_vec1::<f32>()?;
            if stored != regenerated {
                return Err(format!("checkpoint tensor {name} changed the fixed adapter").into());
            }
        }
        let elements = tensor
            .shape()
            .iter()
            .try_fold(1_u64, |total, dimension| {
                total.checked_mul(*dimension as u64)
            })
            .ok_or("checkpoint tensor element count overflows")?;
        parameters = parameters
            .checked_add(elements)
            .ok_or("checkpoint parameter count overflows")?;
    }
    if parameters != declared_parameters {
        return Err("checkpoint parameter count does not match safetensors contents".into());
    }
    Ok(())
}

fn metrics_finite(metrics: &HeldOutMetrics) -> bool {
    [metrics.transition_nll, metrics.rollout_error]
        .into_iter()
        .all(f64::is_finite)
}

/// Inclusive membership test used by the measured action envelope.
fn within(low: f32, high: f32, value: f32) -> bool {
    low <= value && value <= high
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_jepa::{CAMERA_HEIGHT, CAMERA_WIDTH, LIDAR_BINS};

    fn prefixed(var_map: &VarMap, prefix: &str) -> Vec<(String, Vec<f32>)> {
        let variables = var_map.data().lock().unwrap();
        let mut entries = variables
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, variable)| {
                let values = variable
                    .as_detached_tensor()
                    .flatten_all()
                    .unwrap()
                    .to_vec1::<f32>()
                    .unwrap();
                (name.clone(), values)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    #[test]
    fn adapter_is_frozen_across_seeds_while_the_encoder_is_not() {
        let device = Device::Cpu;
        let initialized = |seed: u64| {
            let var_map = VarMap::new();
            JepaCandidateModel::load(VarBuilder::from_varmap(&var_map, DType::F32, &device))
                .unwrap();
            initialize_deterministic(&var_map, seed).unwrap();
            var_map
        };
        let first = initialized(11);
        let second = initialized(12);

        assert_eq!(prefixed(&first, "adapter."), prefixed(&second, "adapter."));
        assert_ne!(prefixed(&first, "encoder."), prefixed(&second, "encoder."));

        let projection = first.data().lock().unwrap()["adapter.projection.weight"]
            .as_detached_tensor()
            .to_vec2::<f32>()
            .unwrap();
        let first_column_norm = projection
            .iter()
            .map(|row| row[0] * row[0])
            .sum::<f32>()
            .sqrt();
        assert!((0.9..=1.1).contains(&first_column_norm));
    }

    #[test]
    fn architecture_shapes_and_parameter_budget_hold() {
        let device = Device::Cpu;
        let var_map = VarMap::new();
        let model =
            JepaCandidateModel::load(VarBuilder::from_varmap(&var_map, DType::F32, &device))
                .unwrap();
        let camera =
            Tensor::zeros((2, 1, CAMERA_HEIGHT, CAMERA_WIDTH), DType::F32, &device).unwrap();
        let lidar = Tensor::zeros((2, 2, LIDAR_BINS), DType::F32, &device).unwrap();
        let pose = Tensor::zeros((2, POSE_DIM), DType::F32, &device).unwrap();
        let action = Tensor::zeros((2, ACTION_DIM), DType::F32, &device).unwrap();
        let delta = Tensor::ones((2, 1), DType::F32, &device).unwrap();

        let latent = model.encoder.forward(&camera, &lidar, &pose).unwrap();
        assert_eq!(latent.dims(), &[2, CORE_DIM]);
        let observation = Tensor::zeros((2, qualia_jepa::OBS_DIM), DType::F32, &device).unwrap();
        let encoded_dims = model.encode_observation(&observation).unwrap().dims().to_vec();
        assert_eq!(encoded_dims, vec![2, CORE_DIM]);

        let prediction = model.predictor.forward(&latent, &action, &delta).unwrap();
        assert_eq!(prediction.mean.dims().to_vec(), vec![2, CORE_DIM]);
        assert_eq!(prediction.log_variance.dims().to_vec(), vec![2, CORE_DIM]);
        let belief_dims = model.adapter.forward(&latent).unwrap().dims().to_vec();
        assert_eq!(belief_dims, vec![2, BELIEF_DIM]);
        let occupancy_dims = model
            .occupancy_decoder
            .forward(&prediction.mean)
            .unwrap()
            .dims()
            .to_vec();
        assert_eq!(occupancy_dims, vec![2, qualia_jepa::GROUNDING_CELLS]);
        assert!(parameter_count(&var_map) < 8_000_000);
    }

    #[test]
    fn masked_grounding_loss_matches_the_class_balanced_fixture() {
        let device = Device::Cpu;
        let logits = Tensor::zeros((1, 4), DType::F32, &device).unwrap();
        let targets = Tensor::new(&[[1.0f32, 0.0, 1.0, 0.0]], &device).unwrap();
        let observed = Tensor::new(&[[1.0f32, 1.0, 0.0, 0.0]], &device).unwrap();
        let loss = masked_grounding_bce_with_logits(&logits, &targets, &observed, 2.0).unwrap();
        // Both measured cells cost ln 2, and the occupied one carries weight 2,
        // so the balanced mean is (2 + 1) * ln 2 / 2.
        let expected = 1.5 * std::f32::consts::LN_2;
        assert!((loss.mean.to_scalar::<f32>().unwrap() - expected).abs() < 1.0e-6);
        assert_eq!((loss.observed_cells, loss.occupied_cells), (2, 1));

        let spike = Tensor::new(&[[80.0f32, -80.0, 0.0, 0.0]], &device)
            .unwrap()
            .contiguous()
            .unwrap();
        let stable = masked_grounding_bce_with_logits(&spike, &targets, &observed, 2.0).unwrap();
        assert!(stable.mean.to_scalar::<f32>().unwrap().is_finite());
    }

    #[test]
    fn gates_fail_closed_on_fixture_scale_or_regression() {
        let point = |nll: f64, rollout: f64| HeldOutMetrics {
            transition_nll: nll,
            rollout_error: rollout,
        };
        let mut gate = BaselineGate {
            dataset_digest: "d".repeat(64),
            valid_transitions: 50_000,
            sessions: 12,
            conditions: 3,
            environments: 3,
            constant: point(3.0, 2.0),
            flat_mlp: point(2.0, 1.5),
            tiny_cnn: point(1.0, 0.8),
        };
        assert!(gate.passes());
        gate.valid_transitions -= 1;
        assert!(!gate.passes());
        gate.valid_transitions = 50_000;
        gate.environments = 2;
        assert!(!gate.passes());
    }
}
