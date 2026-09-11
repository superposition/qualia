//! Drift: the join between the predictor and the latent it was asked to predict.
//!
//! The measurement is the diagonal Mahalanobis distance of the residual under
//! the predictor's own precision — the number the healing ladder escalates on
//! and the symbolic rules threshold. It is the *squared* distance, exactly what
//! [`qualia_jepa::diagonal_mahalanobis_squared`] returns, so there is one
//! Mahalanobis in the workspace rather than a second one that could disagree
//! with the contract the model is trained under.
//!
//! The precision is the predictor's `log_variance` inverted the way the JEPA
//! math defines it, `exp(-log_variance)`, so training loss, posterior fusion and
//! this measurement all read the same uncertainty the same way.
//!
//! Only a well-formed sample is an opinion. Anything the JEPA math rejects —
//! the three slices differing in length above all, but also an empty sample, a
//! non-finite value or a precision that is not a real number — yields
//! `sample_count == 0`, and a consumer must read that count as "nothing was
//! measured": no rule fires on it and no ladder step advances.

/// How far the observed latent sits from the prediction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftReport {
    /// Diagonal Mahalanobis distance squared, `sum_i precision_i * residual_i^2`.
    pub mahalanobis: f32,
    /// The samples this report is an opinion about: one well-formed sample, or
    /// zero when the input was rejected and the report is no opinion at all.
    pub sample_count: u64,
}

/// Measure the drift of `latent` from `predicted_mean` under the predictor's
/// `predicted_log_variance`.
///
/// Returns `DriftReport { mahalanobis: 0.0, sample_count: 0 }` when the three
/// slices differ in length, and for any other input the JEPA math rejects: a
/// malformed sample is no opinion, not a zero-distance one.
pub fn measure(
    latent: &[f32],
    predicted_mean: &[f32],
    predicted_log_variance: &[f32],
) -> DriftReport {
    if latent.len() != predicted_mean.len() || latent.len() != predicted_log_variance.len() {
        return DriftReport { mahalanobis: 0.0, sample_count: 0 };
    }

    // Reject non-finite log-variance before the `exp`: `exp(-(+inf))` is a
    // finite `0.0`, a precision the JEPA math accepts, so a saturated predictor
    // would otherwise read as a perfect prediction instead of no opinion.
    if !predicted_log_variance.iter().all(|log_variance| log_variance.is_finite()) {
        return DriftReport { mahalanobis: 0.0, sample_count: 0 };
    }

    let precision = predicted_log_variance
        .iter()
        .map(|log_variance| (-log_variance).exp())
        .collect::<Vec<f32>>();

    match qualia_jepa::diagonal_mahalanobis_squared(latent, predicted_mean, &precision) {
        Ok(squared) => DriftReport { mahalanobis: squared as f32, sample_count: 1 },
        Err(_) => DriftReport { mahalanobis: 0.0, sample_count: 0 },
    }
}
