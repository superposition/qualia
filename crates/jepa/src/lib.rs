//! CPU reference mathematics for the Qualia belief manifold.
//!
//! This crate is the numerical source of truth that the tensor backends are
//! checked against, so it deliberately carries no dependencies: a backend
//! cannot disagree with a contract it is not allowed to import. Every entry
//! point validates its inputs and fails closed with [`MathError`] instead of
//! letting a `NaN`, an infinity or a ragged slice reach the model.
//!
//! The golden braid's seam is joined here: the drift measurement in
//! `crates/braid` is expressed as a diagonal Mahalanobis distance against the
//! same precision conventions these functions define.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::{Display, Formatter};

/// Camera frame width in pixels.
pub const CAMERA_WIDTH: usize = 64;
/// Camera frame height in pixels.
pub const CAMERA_HEIGHT: usize = 48;
/// Number of luma values in one camera frame.
pub const CAMERA_PIXELS: usize = CAMERA_WIDTH * CAMERA_HEIGHT;
/// Number of angular bins in one LiDAR sweep.
pub const LIDAR_BINS: usize = 720;
/// Channels carried per LiDAR bin: normalized range and validity.
pub const LIDAR_CHANNELS: usize = 2;
/// Width of the packed pose feature vector.
pub const POSE_DIM: usize = 8;
/// Width of the encoder observation vector.
pub const OBS_DIM: usize = CAMERA_PIXELS + LIDAR_BINS * LIDAR_CHANNELS + POSE_DIM;
/// Width of the applied action vector.
pub const ACTION_DIM: usize = 4;
/// Width of the encoder core representation.
pub const CORE_DIM: usize = 256;
/// Width of the belief state.
pub const BELIEF_DIM: usize = 1_024;
/// Grounding map width in cells.
pub const GROUNDING_WIDTH: usize = 64;
/// Grounding map height in cells.
pub const GROUNDING_HEIGHT: usize = 64;
/// Number of cells in a grounding map.
pub const GROUNDING_CELLS: usize = GROUNDING_WIDTH * GROUNDING_HEIGHT;

/// A violated numerical contract.
///
/// The payload names the offending field so a caller can say which input was
/// rejected rather than only that something was.
#[derive(Debug, Clone, PartialEq)]
pub enum MathError {
    /// A required value was empty.
    Empty(&'static str),
    /// A value was `NaN` or infinite.
    NonFinite(&'static str),
    /// A value fell outside the range its meaning allows.
    OutOfRange(&'static str),
    /// A slice had an unexpected length.
    Shape {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
}

impl Display for MathError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty(field) => write!(formatter, "{field} must not be empty"),
            Self::NonFinite(field) => write!(formatter, "{field} contains a non-finite value"),
            Self::OutOfRange(field) => write!(formatter, "{field} is outside its valid range"),
            Self::Shape {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "{field} has length {actual}; expected {expected}"
            ),
        }
    }
}

impl Error for MathError {}

/// Result alias for the fallible entry points in this crate.
pub type MathResult<T> = Result<T, MathError>;

/// Translational and yaw velocity over one camera-correlated step.
///
/// Pose and camera producers run at different cadences. Both offline
/// materialization and live inference call this with the pose chosen for each
/// camera frame, so the model never depends on a backend's own velocity
/// semantics. The result is `(linear_mps, angular_rps)`.
pub fn camera_step_velocity(
    current_xz: [f32; 2],
    current_yaw_rad: f32,
    previous_xz: [f32; 2],
    previous_yaw_rad: f32,
    elapsed_seconds: f32,
) -> MathResult<(f32, f32)> {
    require_finite(
        "camera-step pose",
        &[
            current_xz[0],
            current_xz[1],
            current_yaw_rad,
            previous_xz[0],
            previous_xz[1],
            previous_yaw_rad,
            elapsed_seconds,
        ],
    )?;
    if elapsed_seconds <= 0.0 {
        return Err(MathError::OutOfRange("camera-step elapsed time"));
    }

    let dx = current_xz[0] - previous_xz[0];
    let dz = current_xz[1] - previous_xz[1];
    let linear_mps = (dx * dx + dz * dz).sqrt() / elapsed_seconds;

    // Wrap the yaw delta into [-pi, pi) so crossing the seam does not read as
    // a full turn.
    let yaw_delta = (current_yaw_rad - previous_yaw_rad + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    let angular_rps = yaw_delta / elapsed_seconds;

    if !linear_mps.is_finite() || !angular_rps.is_finite() {
        return Err(MathError::NonFinite("camera-step velocity"));
    }
    Ok((linear_mps, angular_rps))
}

/// Pose features the encoder consumes.
///
/// Packing order is `[x_m, z_m, sin(yaw), cos(yaw), linear_mps, angular_rps,
/// normalized_age, valid]`. The age is normalized by the producer against its
/// declared stale threshold, so it lies in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseFeatures {
    pub x_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub linear_mps: f32,
    pub angular_rps: f32,
    pub normalized_age: f32,
    pub valid: bool,
}

impl PoseFeatures {
    /// Packs the features into the encoder's fixed order.
    ///
    /// Yaw enters as its sine and cosine pair because the seam at `+/- pi`
    /// makes the raw angle non-continuous.
    pub fn packed(self) -> MathResult<[f32; POSE_DIM]> {
        let packed = [
            self.x_m,
            self.z_m,
            self.yaw_rad.sin(),
            self.yaw_rad.cos(),
            self.linear_mps,
            self.angular_rps,
            self.normalized_age,
            f32::from(self.valid),
        ];
        require_finite("pose", &packed)?;
        require_unit("pose normalized age", self.normalized_age)?;
        Ok(packed)
    }
}

/// The action that was actually applied during the interval leading to the
/// next sample, after the safety layer has had its say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedAction {
    pub left: f32,
    pub right: f32,
    pub speed_scale: f32,
    pub valid: bool,
}

impl AppliedAction {
    /// Packs the action into the fixed order `[left, right, speed_scale,
    /// valid]`.
    pub fn packed(self) -> MathResult<[f32; ACTION_DIM]> {
        require_signed_unit("applied left", self.left)?;
        require_signed_unit("applied right", self.right)?;
        require_unit("applied speed scale", self.speed_scale)?;
        Ok([
            self.left,
            self.right,
            self.speed_scale,
            f32::from(self.valid),
        ])
    }
}

/// Packs observation only: camera luma, LiDAR range, LiDAR validity, pose.
///
/// Action and elapsed time intentionally cannot enter this function, which
/// keeps temporal leakage out of the encoder contract. The layout is camera
/// luma, then the LiDAR range block, then the LiDAR validity block, then the
/// packed pose.
pub fn pack_observation(
    camera_luma: &[f32],
    lidar_normalized_range: &[f32],
    lidar_valid: &[bool],
    pose: PoseFeatures,
) -> MathResult<[f32; OBS_DIM]> {
    require_len("camera luma", camera_luma, CAMERA_PIXELS)?;
    require_len("LiDAR normalized range", lidar_normalized_range, LIDAR_BINS)?;
    if lidar_valid.len() != LIDAR_BINS {
        return Err(MathError::Shape {
            field: "LiDAR validity mask",
            expected: LIDAR_BINS,
            actual: lidar_valid.len(),
        });
    }
    require_finite("camera luma", camera_luma)?;
    require_finite("LiDAR normalized range", lidar_normalized_range)?;
    require_within_unit("camera luma", camera_luma)?;
    require_within_unit("LiDAR normalized range", lidar_normalized_range)?;

    let pose = pose.packed()?;

    let mut packed = [0.0_f32; OBS_DIM];
    packed[..CAMERA_PIXELS].copy_from_slice(camera_luma);
    let range_start = CAMERA_PIXELS;
    let validity_start = range_start + LIDAR_BINS;
    packed[range_start..validity_start].copy_from_slice(lidar_normalized_range);
    for (slot, valid) in packed[validity_start..validity_start + LIDAR_BINS]
        .iter_mut()
        .zip(lidar_valid)
    {
        *slot = f32::from(*valid);
    }
    packed[validity_start + LIDAR_BINS..].copy_from_slice(&pose);
    Ok(packed)
}

/// Independent quality gate applied to physical evidence before inference.
///
/// Each factor lies in `[0, 1]` and the product is monotone in every one of
/// them, so a single degraded modality can suppress the whole evidence vector
/// but can never amplify it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservationQuality {
    pub freshness: f32,
    pub exposure: f32,
    pub calibration: f32,
    pub modality_validity: f32,
}

impl ObservationQuality {
    pub fn new(
        freshness: f32,
        exposure: f32,
        calibration: f32,
        modality_validity: f32,
    ) -> MathResult<Self> {
        for (field, value) in [
            ("freshness", freshness),
            ("exposure", exposure),
            ("calibration", calibration),
            ("modality validity", modality_validity),
        ] {
            require_unit(field, value)?;
        }
        Ok(Self {
            freshness,
            exposure,
            calibration,
            modality_validity,
        })
    }

    /// The scalar that multiplies evidence precision.
    ///
    /// Because it is a product, any factor of zero produces exactly zero
    /// evidence: a stale frame cannot contribute no matter how confident the
    /// detector claims to be.
    pub fn precision_scale(self) -> f32 {
        self.freshness * self.exposure * self.calibration * self.modality_validity
    }
}

/// Per-dimension precision attached to an observation used as evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct EvidencePrecision(Vec<f32>);

impl EvidencePrecision {
    /// Rejects empty, non-finite or negative precision vectors.
    pub fn new(values: Vec<f32>) -> MathResult<Self> {
        require_nonnegative("evidence precision", &values)?;
        Ok(Self(values))
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

/// Per-dimension log variance of the transition predictor.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionLogVariance(Vec<f32>);

impl TransitionLogVariance {
    /// Rejects empty or non-finite vectors. Any finite value is allowed,
    /// because an over-confident prediction is a thing to be penalised rather
    /// than rejected.
    pub fn new(values: Vec<f32>) -> MathResult<Self> {
        if values.is_empty() {
            return Err(MathError::Empty("transition log variance"));
        }
        require_finite("transition log variance", &values)?;
        Ok(Self(values))
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

/// Per-dimension precision of a fused belief.
#[derive(Debug, Clone, PartialEq)]
pub struct PosteriorPrecision(Vec<f32>);

impl PosteriorPrecision {
    /// Rejects empty, non-finite or non-positive vectors. A precision of zero
    /// would make the belief undefined on that dimension.
    pub fn new(values: Vec<f32>) -> MathResult<Self> {
        if values.is_empty() {
            return Err(MathError::Empty("posterior precision"));
        }
        require_positive("posterior precision", &values)?;
        Ok(Self(values))
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

/// Diagonal Mahalanobis distance squared, `sum_i p_i (l_i - r_i)^2`.
///
/// This is the single number that joins the learned latent to the symbolic
/// rule layer; identity prediction therefore measures exactly zero.
pub fn diagonal_mahalanobis_squared(
    left: &[f32],
    right: &[f32],
    precision: &[f32],
) -> MathResult<f64> {
    require_triple_len("Mahalanobis vectors", left, right, precision)?;
    require_finite("Mahalanobis left", left)?;
    require_finite("Mahalanobis right", right)?;
    require_nonnegative("Mahalanobis precision", precision)?;

    let distance = left
        .iter()
        .zip(right)
        .zip(precision)
        .map(|((left, right), precision)| {
            let residual = f64::from(*left) - f64::from(*right);
            f64::from(*precision) * residual * residual
        })
        .sum::<f64>();
    if !distance.is_finite() {
        return Err(MathError::NonFinite("Mahalanobis distance"));
    }
    Ok(distance)
}

/// Gaussian negative log likelihood of a transition, with the gradients the
/// training step needs.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionNll {
    pub total: f64,
    pub per_dimension: Vec<f64>,
    pub gradient_mean: Vec<f64>,
    pub gradient_log_variance: Vec<f64>,
}

/// Computes `0.5 * sum_i (exp(-log_var_i) (target_i - mean_i)^2 + log_var_i)`.
///
/// Both gradients are returned so the trainer and the reference diverge only
/// numerically, never in sign or scale.
pub fn gaussian_transition_nll(
    target: &[f32],
    predicted_mean: &[f32],
    log_variance: &TransitionLogVariance,
) -> MathResult<TransitionNll> {
    require_triple_len(
        "transition NLL",
        target,
        predicted_mean,
        log_variance.as_slice(),
    )?;
    require_finite("transition target", target)?;
    require_finite("transition mean", predicted_mean)?;

    let mut total = 0.0_f64;
    let mut per_dimension = Vec::with_capacity(target.len());
    let mut gradient_mean = Vec::with_capacity(target.len());
    let mut gradient_log_variance = Vec::with_capacity(target.len());
    for ((target, mean), log_variance) in target
        .iter()
        .zip(predicted_mean)
        .zip(log_variance.as_slice())
    {
        let target = f64::from(*target);
        let mean = f64::from(*mean);
        let log_variance = f64::from(*log_variance);
        let residual = target - mean;
        let precision = (-log_variance).exp();
        let weighted_error = precision * residual * residual;
        let term = 0.5 * (weighted_error + log_variance);
        if !term.is_finite() {
            return Err(MathError::NonFinite("transition NLL"));
        }
        total += term;
        per_dimension.push(term);
        gradient_mean.push(-precision * residual);
        gradient_log_variance.push(0.5 * (1.0 - weighted_error));
    }
    Ok(TransitionNll {
        total,
        per_dimension,
        gradient_mean,
        gradient_log_variance,
    })
}

/// Fused belief: posterior mean and precision.
#[derive(Debug, Clone, PartialEq)]
pub struct PosteriorUpdate {
    pub mean: Vec<f32>,
    pub precision: PosteriorPrecision,
}

/// Diagonal Gaussian fusion of a prior with quality-scaled evidence.
///
/// Each dimension is fused independently as
/// `(p_prior * m_prior + p_evidence * m_evidence) / (p_prior + p_evidence)`
/// with `p_evidence` already multiplied by [`ObservationQuality::precision_scale`].
/// The posterior therefore moves toward whichever source is more precise.
pub fn posterior_update(
    prior_mean: &[f32],
    prior_precision: &PosteriorPrecision,
    evidence_mean: &[f32],
    evidence_precision: &EvidencePrecision,
    quality: ObservationQuality,
) -> MathResult<PosteriorUpdate> {
    require_triple_len(
        "posterior update",
        prior_mean,
        prior_precision.as_slice(),
        evidence_mean,
    )?;
    if evidence_precision.as_slice().len() != prior_mean.len() {
        return Err(MathError::Shape {
            field: "evidence precision",
            expected: prior_mean.len(),
            actual: evidence_precision.as_slice().len(),
        });
    }
    require_finite("prior mean", prior_mean)?;
    require_finite("evidence mean", evidence_mean)?;

    let scale = quality.precision_scale();
    let mut mean = Vec::with_capacity(prior_mean.len());
    let mut precision = Vec::with_capacity(prior_mean.len());
    for (((prior_mean, prior_precision), evidence_mean), evidence_precision) in prior_mean
        .iter()
        .zip(prior_precision.as_slice())
        .zip(evidence_mean)
        .zip(evidence_precision.as_slice())
    {
        let evidence_precision = scale * evidence_precision;
        let fused_precision = prior_precision + evidence_precision;
        let fused_mean =
            (prior_precision * prior_mean + evidence_precision * evidence_mean) / fused_precision;
        mean.push(fused_mean);
        precision.push(fused_precision);
    }
    Ok(PosteriorUpdate {
        mean,
        precision: PosteriorPrecision::new(precision)?,
    })
}

/// VICReg statistics over a batch of representations.
#[derive(Debug, Clone, PartialEq)]
pub struct VicregStats {
    pub variance_loss: f64,
    pub covariance_loss: f64,
    pub minimum_stddev: f64,
    pub mean_stddev: f64,
}

/// Computes the variance and covariance terms used to keep the learned
/// representation from collapsing or entangling its dimensions.
///
/// `variance_loss` is the mean hinge `max(target_stddev - stddev, 0)` over
/// dimensions and `covariance_loss` the mean squared off-diagonal covariance,
/// both with `epsilon` folded into the diagonal so a constant batch does not
/// produce a zero standard deviation.
pub fn vicreg_statistics(
    batch: &[Vec<f32>],
    target_stddev: f32,
    epsilon: f32,
) -> MathResult<VicregStats> {
    if batch.len() < 2 {
        return Err(MathError::Shape {
            field: "VICReg batch",
            expected: 2,
            actual: batch.len(),
        });
    }
    require_positive_scalar("VICReg target stddev", target_stddev)?;
    require_positive_scalar("VICReg epsilon", epsilon)?;

    let dimensions = batch[0].len();
    if dimensions == 0 {
        return Err(MathError::Empty("VICReg representation"));
    }
    for row in batch {
        require_len("VICReg representation", row, dimensions)?;
        require_finite("VICReg representation", row)?;
    }

    let sample_count = batch.len() as f64;
    let denominator = (batch.len() - 1) as f64;

    let mut means = vec![0.0_f64; dimensions];
    for row in batch {
        for (mean, value) in means.iter_mut().zip(row) {
            *mean += f64::from(*value) / sample_count;
        }
    }

    let mut covariance = vec![0.0_f64; dimensions * dimensions];
    for row in batch {
        for left in 0..dimensions {
            let centered_left = f64::from(row[left]) - means[left];
            for right in 0..dimensions {
                let centered_right = f64::from(row[right]) - means[right];
                covariance[left * dimensions + right] +=
                    centered_left * centered_right / denominator;
            }
        }
    }

    let stddev = (0..dimensions)
        .map(|index| (covariance[index * dimensions + index] + f64::from(epsilon)).sqrt())
        .collect::<Vec<_>>();
    let variance_loss = stddev
        .iter()
        .map(|stddev| (f64::from(target_stddev) - stddev).max(0.0))
        .sum::<f64>()
        / dimensions as f64;
    let covariance_loss = (0..dimensions)
        .flat_map(|left| (0..dimensions).map(move |right| (left, right)))
        .filter(|(left, right)| left != right)
        .map(|(left, right)| covariance[left * dimensions + right].powi(2))
        .sum::<f64>()
        / dimensions as f64;

    Ok(VicregStats {
        variance_loss,
        covariance_loss,
        minimum_stddev: stddev.iter().copied().fold(f64::INFINITY, f64::min),
        mean_stddev: stddev.iter().sum::<f64>() / dimensions as f64,
    })
}

/// Class-balanced occupancy loss over measured cells only.
#[derive(Debug, Clone, PartialEq)]
pub struct OccupancyLoss {
    pub mean: f64,
    pub observed_cells: usize,
    pub occupied_cells: usize,
}

/// Numerically stable, class-balanced BCE-with-logits.
///
/// Only cells the mask marks as observed contribute; everything else is
/// ignored, logits and targets alike. Occupied cells are scaled by
/// `occupied_weight` because free space dominates a lidar sweep.
pub fn masked_occupancy_bce_with_logits(
    logits: &[f32],
    targets: &[f32],
    observed: &[bool],
    occupied_weight: f32,
) -> MathResult<OccupancyLoss> {
    require_len("occupancy target", targets, logits.len())?;
    if observed.len() != logits.len() {
        return Err(MathError::Shape {
            field: "occupancy observed mask",
            expected: logits.len(),
            actual: observed.len(),
        });
    }
    require_finite("occupancy logits", logits)?;
    require_finite("occupancy targets", targets)?;
    require_positive_scalar("occupied class weight", occupied_weight)?;

    let mut total = 0.0_f64;
    let mut observed_cells = 0_usize;
    let mut occupied_cells = 0_usize;
    for ((logit, target), observed) in logits.iter().zip(targets).zip(observed) {
        if !observed {
            continue;
        }
        if !(*target == 0.0 || *target == 1.0) {
            return Err(MathError::OutOfRange("occupancy target"));
        }
        let logit = f64::from(*logit);
        let target = f64::from(*target);
        // max(z, 0) - z*t + ln(1 + exp(-|z|)) is the stable form of
        // -[t ln s(z) + (1 - t) ln(1 - s(z))].
        let cell = logit.max(0.0) - logit * target + (-logit.abs()).exp().ln_1p();
        let weight = if target == 1.0 {
            occupied_cells += 1;
            f64::from(occupied_weight)
        } else {
            1.0
        };
        total += weight * cell;
        observed_cells += 1;
    }
    if observed_cells == 0 {
        return Err(MathError::Empty("observed occupancy cells"));
    }
    Ok(OccupancyLoss {
        mean: total / observed_cells as f64,
        observed_cells,
        occupied_cells,
    })
}

fn require_triple_len(
    field: &'static str,
    left: &[f32],
    middle: &[f32],
    right: &[f32],
) -> MathResult<()> {
    if left.is_empty() {
        return Err(MathError::Empty(field));
    }
    if middle.len() != left.len() {
        return Err(MathError::Shape {
            field,
            expected: left.len(),
            actual: middle.len(),
        });
    }
    if right.len() != left.len() {
        return Err(MathError::Shape {
            field,
            expected: left.len(),
            actual: right.len(),
        });
    }
    Ok(())
}

fn require_len(field: &'static str, values: &[f32], expected: usize) -> MathResult<()> {
    if values.len() != expected {
        return Err(MathError::Shape {
            field,
            expected,
            actual: values.len(),
        });
    }
    Ok(())
}

fn require_finite(field: &'static str, values: &[f32]) -> MathResult<()> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(MathError::NonFinite(field));
    }
    Ok(())
}

fn require_within_unit(field: &'static str, values: &[f32]) -> MathResult<()> {
    if values.iter().any(|value| !(0.0..=1.0).contains(value)) {
        return Err(MathError::OutOfRange(field));
    }
    Ok(())
}

fn require_nonnegative(field: &'static str, values: &[f32]) -> MathResult<()> {
    if values.is_empty() {
        return Err(MathError::Empty(field));
    }
    require_finite(field, values)?;
    if values.iter().any(|value| *value < 0.0) {
        return Err(MathError::OutOfRange(field));
    }
    Ok(())
}

fn require_positive(field: &'static str, values: &[f32]) -> MathResult<()> {
    require_nonnegative(field, values)?;
    if values.iter().any(|value| *value <= 0.0) {
        return Err(MathError::OutOfRange(field));
    }
    Ok(())
}

fn require_unit(field: &'static str, value: f32) -> MathResult<()> {
    if !value.is_finite() {
        return Err(MathError::NonFinite(field));
    }
    if !(0.0..=1.0).contains(&value) {
        return Err(MathError::OutOfRange(field));
    }
    Ok(())
}

fn require_signed_unit(field: &'static str, value: f32) -> MathResult<()> {
    if !value.is_finite() {
        return Err(MathError::NonFinite(field));
    }
    if !(-1.0..=1.0).contains(&value) {
        return Err(MathError::OutOfRange(field));
    }
    Ok(())
}

fn require_positive_scalar(field: &'static str, value: f32) -> MathResult<()> {
    if !value.is_finite() {
        return Err(MathError::NonFinite(field));
    }
    if value <= 0.0 {
        return Err(MathError::OutOfRange(field));
    }
    Ok(())
}
