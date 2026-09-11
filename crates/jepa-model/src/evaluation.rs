//! Held-out evaluation instruments: the effective-rank gate, heteroscedastic
//! calibration, and robot-centric occupancy grounding.
//!
//! Each report recomputes its own contract predicate rather than trusting the
//! serialized `passes` bit, since a report is only useful as evidence when the
//! verdict can be re-derived from the numbers shipped beside it.

use qualia_jepa::CORE_DIM;
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Result alias for the evaluation instruments.
pub type EvaluationResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Held-out samples the effective-rank gate needs before it has an opinion.
pub const EFFECTIVE_RANK_MINIMUM_SAMPLES: usize = 4_096;
/// Minimum effective rank a promoted representation is allowed to collapse to.
pub const EFFECTIVE_RANK_MINIMUM: f64 = 64.0;
/// Fraction of latent dimensions allowed to sit on the log-variance clamp.
const CALIBRATION_CLAMP_FRACTION_MAX: f64 = 0.01;
/// Reservoir resolution of the precision/recall curve.
const PR_BINS: usize = 1_024;
/// Half-width of the central 50% of a standard normal.
const NORMAL_HALF_WIDTH_50: f64 = 0.674_489_750_196_081_7;
/// Half-width of the central 90% of a standard normal.
const NORMAL_HALF_WIDTH_90: f64 = 1.644_853_626_951_472_2;
/// Half-width of the central 95% of a standard normal.
const NORMAL_HALF_WIDTH_95: f64 = 1.959_963_984_540_054;
/// Log-variance at or below which a prediction sits on the numerical clamp.
const LOG_VARIANCE_FLOOR: f64 = -10.0;
/// Log-variance at or above which a prediction sits on the numerical clamp.
const LOG_VARIANCE_CEILING: f64 = 5.0;

/// Effective rank of the held-out target representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EffectiveRankReport {
    pub sample_count: usize,
    pub dimensions: usize,
    pub effective_rank: f64,
    pub trace: f64,
    pub converged: bool,
    pub sweeps: usize,
}

impl EffectiveRankReport {
    /// A promoted candidate must show a representation that has not collapsed.
    pub fn passes(&self) -> bool {
        let enough_samples = self.sample_count >= EFFECTIVE_RANK_MINIMUM_SAMPLES;
        let correct_width = self.dimensions == CORE_DIM;
        let rank_alive =
            self.effective_rank.is_finite() && self.effective_rank >= EFFECTIVE_RANK_MINIMUM;
        let mass_alive = self.trace.is_finite() && self.trace > 0.0;
        enough_samples && correct_width && rank_alive && mass_alive && self.converged
    }
}

/// Calibration of the heteroscedastic transition head on held-out transitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalibrationReport {
    pub dimensions: u64,
    pub transition_nll: f64,
    pub mean_standardized_squared_residual: f64,
    pub coverage_50: f64,
    pub coverage_90: f64,
    pub coverage_95: f64,
    pub calibration_slope: f64,
    pub clamp_fraction: f64,
    pub nonfinite_values: u64,
    pub passes: bool,
}

impl CalibrationReport {
    /// Recompute the frozen calibration gate instead of trusting `passes`.
    pub fn passes_contract(&self) -> bool {
        self.passes && self.metrics_pass()
    }

    /// Shape and finiteness of the recorded metrics.
    pub fn is_well_formed(&self) -> bool {
        let recorded = [
            self.transition_nll,
            self.mean_standardized_squared_residual,
            self.coverage_50,
            self.coverage_90,
            self.coverage_95,
            self.calibration_slope,
            self.clamp_fraction,
        ];
        if self.dimensions == 0 || !recorded.iter().all(|value| value.is_finite()) {
            return false;
        }
        let squared_residual_sane = self.mean_standardized_squared_residual >= 0.0;
        let slope_sane = self.calibration_slope >= 0.0;
        squared_residual_sane
            && slope_sane
            && within_unit_interval(self.coverage_50)
            && within_unit_interval(self.coverage_90)
            && within_unit_interval(self.coverage_95)
            && within_unit_interval(self.clamp_fraction)
    }

    fn metrics_pass(&self) -> bool {
        if !self.is_well_formed() || self.nonfinite_values != 0 {
            return false;
        }
        let residual_scale_ok =
            (0.9..=1.1).contains(&self.mean_standardized_squared_residual);
        let slope_scale_ok = (0.9..=1.1).contains(&self.calibration_slope);
        let coverage_ok = centered_on(self.coverage_50, 0.5)
            && centered_on(self.coverage_90, 0.9)
            && centered_on(self.coverage_95, 0.95);
        residual_scale_ok
            && slope_scale_ok
            && coverage_ok
            && self.clamp_fraction <= CALIBRATION_CLAMP_FRACTION_MAX
    }
}

/// Streaming calibration statistics over projected latent residuals.
///
/// The slope regresses the squared residual on the predicted variance, which
/// is what detects a head whose uncertainty is mis-scaled rather than merely
/// offset.
#[derive(Debug, Default, Clone)]
pub struct CalibrationAccumulator {
    dimensions: u64,
    nll: f64,
    standardized: f64,
    coverage_50: u64,
    coverage_90: u64,
    coverage_95: u64,
    slope_numerator: f64,
    slope_denominator: f64,
    clamps: u64,
    nonfinite: u64,
}

impl CalibrationAccumulator {
    /// Add one projected transition. Non-finite entries are counted, not
    /// silently skipped, so a poisoned batch cannot improve the statistics.
    pub fn push(
        &mut self,
        predicted_mean: &[f32],
        predicted_log_variance: &[f32],
        target: &[f32],
    ) -> EvaluationResult<()> {
        let width = predicted_mean.len();
        if width == 0 || predicted_log_variance.len() != width || target.len() != width {
            return Err("calibration vectors have incompatible shapes".into());
        }
        for offset in 0..width {
            self.observe(predicted_mean[offset], predicted_log_variance[offset], target[offset]);
        }
        Ok(())
    }

    /// Fold a single finite triple into the running statistics.
    fn observe(&mut self, mean: f32, log_variance: f32, target: f32) {
        if !(mean.is_finite() && log_variance.is_finite() && target.is_finite()) {
            self.nonfinite += 1;
            return;
        }
        let residual = f64::from(target - mean);
        let log_variance = f64::from(log_variance);
        let variance = log_variance.exp();
        let standardized = residual * residual / variance;
        let deviation = residual.abs() / variance.sqrt();
        self.dimensions += 1;
        self.nll += 0.5 * (standardized + log_variance);
        self.standardized += standardized;
        self.coverage_50 += u64::from(deviation <= NORMAL_HALF_WIDTH_50);
        self.coverage_90 += u64::from(deviation <= NORMAL_HALF_WIDTH_90);
        self.coverage_95 += u64::from(deviation <= NORMAL_HALF_WIDTH_95);
        self.slope_numerator += variance * residual * residual;
        self.slope_denominator += variance * variance;
        self.clamps += u64::from(log_variance <= LOG_VARIANCE_FLOOR || log_variance >= LOG_VARIANCE_CEILING);
    }

    /// Freeze the accumulated statistics into a report.
    pub fn finish(self) -> EvaluationResult<CalibrationReport> {
        let count = self.dimensions as f64;
        if self.dimensions == 0 || self.slope_denominator <= f64::EPSILON {
            return Err("calibration report has no finite predictive dimensions".into());
        }
        let mut report = CalibrationReport {
            dimensions: self.dimensions,
            transition_nll: self.nll / count,
            mean_standardized_squared_residual: self.standardized / count,
            coverage_50: self.coverage_50 as f64 / count,
            coverage_90: self.coverage_90 as f64 / count,
            coverage_95: self.coverage_95 as f64 / count,
            calibration_slope: self.slope_numerator / self.slope_denominator,
            clamp_fraction: self.clamps as f64 / count,
            nonfinite_values: self.nonfinite,
            passes: false,
        };
        report.passes = report.metrics_pass();
        Ok(report)
    }
}

/// Grounding quality over the robot-centric occupancy window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OccupancyReport {
    pub observed_cells: u64,
    pub occupied_cells: u64,
    pub occupied_prevalence: f64,
    pub intersection_over_union: f64,
    pub pr_auc: f64,
    pub trivial_iou: f64,
    pub trivial_pr_auc: f64,
    pub passes: bool,
}

impl OccupancyReport {
    /// Recompute count consistency and the non-triviality gate.
    pub fn passes_contract(&self) -> bool {
        self.passes
            && self.is_well_formed()
            && self.intersection_over_union > self.trivial_iou
            && self.pr_auc > self.trivial_pr_auc
    }

    /// Counts, ranges, and the trivial baselines must agree with prevalence.
    pub fn is_well_formed(&self) -> bool {
        let counts_agree = self.observed_cells > 0
            && self.occupied_cells > 0
            && self.occupied_cells <= self.observed_cells;
        let scores = [
            self.occupied_prevalence,
            self.intersection_over_union,
            self.pr_auc,
            self.trivial_iou,
            self.trivial_pr_auc,
        ];
        if !counts_agree || !scores.iter().all(|value| value.is_finite() && within_unit_interval(*value))
        {
            return false;
        }
        let prevalence = self.occupied_cells as f64 / self.observed_cells as f64;
        [self.occupied_prevalence, self.trivial_iou, self.trivial_pr_auc]
            .iter()
            .all(|baseline| approximately_equal(*baseline, prevalence))
    }
}

fn approximately_equal(left: f64, right: f64) -> bool {
    let magnitude = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= 1.0e-9 * magnitude
}

/// `true` when `value` lies in the closed unit interval.
fn within_unit_interval(value: f64) -> bool {
    (0.0..=1.0).contains(&value)
}

/// `true` when `value` sits within the two-sided calibration band around
/// `nominal`.
fn centered_on(value: f64, nominal: f64) -> bool {
    (value - nominal).abs() <= 0.05
}

/// `true` when a target or mask entry is exactly one of the two hard labels.
fn is_binary(value: f32) -> bool {
    value == 0.0 || value == 1.0
}

/// Numerically stable logistic function.
fn logistic(value: f64) -> f64 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exponential = value.exp();
        exponential / (1.0 + exponential)
    }
}

/// Streaming occupancy statistics with a binned precision/recall curve.
#[derive(Debug, Clone)]
pub struct OccupancyAccumulator {
    observed: u64,
    occupied: u64,
    intersection: u64,
    union: u64,
    positive_bins: [u64; PR_BINS],
    negative_bins: [u64; PR_BINS],
}

impl Default for OccupancyAccumulator {
    fn default() -> Self {
        Self {
            observed: 0,
            occupied: 0,
            intersection: 0,
            union: 0,
            positive_bins: [0; PR_BINS],
            negative_bins: [0; PR_BINS],
        }
    }
}

impl OccupancyAccumulator {
    /// Add one grounding map. Only measured cells count; binary targets are
    /// required so a soft label cannot smuggle a fractional cell in.
    pub fn push(
        &mut self,
        logits: &[f32],
        targets: &[f32],
        observed: &[f32],
    ) -> EvaluationResult<()> {
        let cells = logits.len();
        if cells == 0 || targets.len() != cells || observed.len() != cells {
            return Err("occupancy evaluation vectors have incompatible shapes".into());
        }
        for cell in 0..cells {
            let logit = logits[cell];
            let target = targets[cell];
            let mask = observed[cell];
            if !logit.is_finite() || !is_binary(target) || !is_binary(mask) {
                return Err("occupancy evaluation contains non-finite or non-binary values".into());
            }
            if mask == 0.0 {
                continue;
            }
            let actual = target == 1.0;
            let predicted = logit >= 0.0;
            self.observed += 1;
            self.occupied += u64::from(actual);
            self.intersection += u64::from(actual && predicted);
            self.union += u64::from(actual || predicted);
            let probability = logistic(f64::from(logit));
            let bin = (probability * (PR_BINS - 1) as f64).round() as usize;
            let reservoir = if actual {
                &mut self.positive_bins
            } else {
                &mut self.negative_bins
            };
            reservoir[bin] += 1;
        }
        Ok(())
    }

    /// Freeze the accumulated statistics into a report.
    pub fn finish(self) -> EvaluationResult<OccupancyReport> {
        if self.observed == 0 || self.occupied == 0 || self.union == 0 {
            return Err("occupancy report lacks observed occupied cells".into());
        }
        let prevalence = self.occupied as f64 / self.observed as f64;
        let iou = self.intersection as f64 / self.union as f64;
        let mut true_positive = 0u64;
        let mut false_positive = 0u64;
        let mut previous_recall = 0.0f64;
        let mut pr_auc = 0.0f64;
        let bins = self.positive_bins.iter().zip(self.negative_bins.iter());
        for (positive, negative) in bins.rev() {
            true_positive += *positive;
            false_positive += *negative;
            if *positive == 0 {
                continue;
            }
            let recall = true_positive as f64 / self.occupied as f64;
            let precision = true_positive as f64 / (true_positive + false_positive) as f64;
            pr_auc += (recall - previous_recall) * precision;
            previous_recall = recall;
        }
        let mut report = OccupancyReport {
            observed_cells: self.observed,
            occupied_cells: self.occupied,
            occupied_prevalence: prevalence,
            intersection_over_union: iou,
            pr_auc,
            trivial_iou: prevalence,
            trivial_pr_auc: prevalence,
            passes: false,
        };
        report.passes = report.intersection_over_union > report.trivial_iou
            && report.pr_auc > report.trivial_pr_auc;
        debug_assert!(report.is_well_formed());
        Ok(report)
    }
}

/// Effective rank of a batch of `CORE_DIM` representations, using the
/// production sample minimum.
pub fn effective_rank_report(
    representations: &[Vec<f32>],
) -> EvaluationResult<EffectiveRankReport> {
    let wrong_width = representations
        .first()
        .is_some_and(|row| row.len() != CORE_DIM);
    if wrong_width {
        return Err("effective-rank representations must be 256-dimensional".into());
    }
    effective_rank_with_minimum(representations, EFFECTIVE_RANK_MINIMUM_SAMPLES)
}

/// Message for a batch that is too small to support an effective-rank claim.
fn too_few_samples(minimum_samples: usize, observed: usize) -> String {
    format!("effective rank requires at least {minimum_samples} samples; got {observed}")
}

/// Column means of a rectangular batch of observations.
fn column_means(representations: &[Vec<f32>], dimensions: usize) -> Vec<f64> {
    let count = representations.len() as f64;
    let mut means = vec![0.0f64; dimensions];
    for row in representations {
        for (mean, value) in means.iter_mut().zip(row) {
            *mean += f64::from(*value) / count;
        }
    }
    means
}

/// Sample covariance of a rectangular batch, stored row-major in full.
fn sample_covariance(representations: &[Vec<f32>], means: &[f64], dimensions: usize) -> Vec<f64> {
    let mut centered = vec![0.0f64; dimensions];
    let mut covariance = vec![0.0f64; dimensions * dimensions];
    for row in representations {
        for (axis, value) in row.iter().enumerate() {
            centered[axis] = f64::from(*value) - means[axis];
        }
        for left in 0..dimensions {
            for right in 0..=left {
                covariance[left * dimensions + right] += centered[left] * centered[right];
            }
        }
    }
    let denominator = (representations.len() - 1) as f64;
    for left in 0..dimensions {
        let row = left * dimensions;
        for right in 0..=left {
            let normalized = covariance[row + right] / denominator;
            covariance[row + right] = normalized;
            covariance[right * dimensions + left] = normalized;
        }
    }
    covariance
}

fn effective_rank_with_minimum(
    representations: &[Vec<f32>],
    minimum_samples: usize,
) -> EvaluationResult<EffectiveRankReport> {
    let observed_samples = representations.len();
    if observed_samples < minimum_samples {
        return Err(too_few_samples(minimum_samples, observed_samples).into());
    }
    let dimensions = match representations.first() {
        Some(row) if !row.is_empty() => row.len(),
        _ => return Err("effective-rank representation is empty".into()),
    };
    let malformed = representations
        .iter()
        .any(|row| row.len() != dimensions || row.iter().any(|value| !value.is_finite()));
    if malformed {
        return Err("effective-rank representations have invalid values or shapes".into());
    }

    let means = column_means(representations, dimensions);
    let covariance = sample_covariance(representations, &means, dimensions);
    let (eigenvalues, converged, sweeps) =
        symmetric_jacobi_eigenvalues(covariance, dimensions);

    let trace = eigenvalues.iter().map(|value| value.max(0.0)).sum::<f64>();
    if !trace.is_finite() || trace <= f64::EPSILON {
        return Err("effective-rank covariance has no finite variance".into());
    }
    let entropy = eigenvalues
        .iter()
        .map(|value| value.max(0.0) / trace)
        .filter(|probability| *probability > 0.0)
        .fold(0.0f64, |total, probability| {
            total - probability * probability.ln()
        });
    let effective_rank = entropy.exp();
    if !effective_rank.is_finite() {
        return Err("effective rank is non-finite".into());
    }
    Ok(EffectiveRankReport {
        sample_count: observed_samples,
        dimensions,
        effective_rank,
        trace,
        converged,
        sweeps,
    })
}

/// Largest absolute diagonal entry, floored at one so the relative tolerance
/// stays meaningful on a flat spectrum.
fn diagonal_scale(matrix: &[f64], dimensions: usize) -> f64 {
    let peak = (0..dimensions)
        .map(|index| matrix[index * dimensions + index].abs())
        .fold(0.0f64, f64::max);
    peak.max(1.0)
}

/// Diagonal of a row-major square matrix.
fn diagonal(matrix: &[f64], dimensions: usize) -> Vec<f64> {
    (0..dimensions)
        .map(|index| matrix[index * dimensions + index])
        .collect()
}

/// One Jacobi plane rotation that annihilates entry `(left, right)`.
fn annihilate(matrix: &mut [f64], dimensions: usize, left: usize, right: usize) {
    let off_diagonal = matrix[left * dimensions + right];
    let left_value = matrix[left * dimensions + left];
    let right_value = matrix[right * dimensions + right];
    let tau = (right_value - left_value) / (2.0 * off_diagonal);
    let orientation = if tau >= 0.0 { 1.0 } else { -1.0 };
    let tangent = orientation / (tau.abs() + (1.0 + tau * tau).sqrt());
    let cosine = 1.0 / (1.0 + tangent * tangent).sqrt();
    let sine = tangent * cosine;

    matrix[left * dimensions + left] = left_value - tangent * off_diagonal;
    matrix[right * dimensions + right] = right_value + tangent * off_diagonal;
    matrix[left * dimensions + right] = 0.0;
    matrix[right * dimensions + left] = 0.0;

    let untouched = (0..dimensions).filter(|index| *index != left && *index != right);
    for index in untouched {
        let row_left = matrix[index * dimensions + left];
        let row_right = matrix[index * dimensions + right];
        let rotated_left = cosine * row_left - sine * row_right;
        let rotated_right = sine * row_left + cosine * row_right;
        matrix[index * dimensions + left] = rotated_left;
        matrix[left * dimensions + index] = rotated_left;
        matrix[index * dimensions + right] = rotated_right;
        matrix[right * dimensions + index] = rotated_right;
    }
}

/// Cyclic Jacobi eigendecomposition of a symmetric matrix; only the spectrum
/// is needed, so the accumulated rotation is discarded.
fn symmetric_jacobi_eigenvalues(
    mut matrix: Vec<f64>,
    dimensions: usize,
) -> (Vec<f64>, bool, usize) {
    const MAX_SWEEPS: usize = 32;
    const RELATIVE_TOLERANCE: f64 = 1.0e-12;
    let mut sweeps = 0usize;
    let mut converged = false;
    while sweeps < MAX_SWEEPS {
        let threshold = RELATIVE_TOLERANCE * diagonal_scale(&matrix, dimensions);
        let mut rotated = false;
        for left in 0..dimensions {
            for right in (left + 1)..dimensions {
                if matrix[left * dimensions + right].abs() <= threshold {
                    continue;
                }
                annihilate(&mut matrix, dimensions, left, right);
                rotated = true;
            }
        }
        sweeps += 1;
        if !rotated {
            converged = true;
            break;
        }
    }
    (diagonal(&matrix, dimensions), converged, sweeps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_spread_on_two_axes_reports_rank_two() {
        let rows = vec![
            vec![1.0, 0.0],
            vec![-1.0, 0.0],
            vec![0.0, 1.0],
            vec![0.0, -1.0],
        ];
        let report = effective_rank_with_minimum(&rows, 4).unwrap();
        assert!((report.effective_rank - 2.0).abs() < 1.0e-9);
        assert!(report.converged);
    }

    #[test]
    fn calibration_and_occupancy_reports_recompute_their_contracts() {
        let mut residuals = vec![0.5f32; 50];
        residuals.extend(vec![1.0f32; 40]);
        residuals.extend(vec![1.8f32; 5]);
        residuals.extend(vec![6.26f32.sqrt(); 5]);

        let mut calibration = CalibrationAccumulator::default();
        calibration
            .push(
                &vec![0.0; residuals.len()],
                &vec![0.0; residuals.len()],
                &residuals,
            )
            .unwrap();
        let intact = calibration.finish().unwrap();
        assert!(intact.passes_contract());

        let mut tampered = intact;
        tampered.coverage_95 = 0.1;
        assert!(!tampered.passes_contract());

        let mut occupancy = OccupancyAccumulator::default();
        occupancy
            .push(
                &[5.0, -5.0, 5.0, -5.0],
                &[1.0, 0.0, 1.0, 0.0],
                &[1.0, 1.0, 1.0, 1.0],
            )
            .unwrap();
        let intact = occupancy.finish().unwrap();
        assert!(intact.passes_contract());

        let mut tampered = intact;
        tampered.occupied_prevalence = 0.9;
        assert!(!tampered.passes_contract());
    }
}
