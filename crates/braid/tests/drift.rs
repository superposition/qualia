//! What the drift measurement reports to the healing ladder.
//!
//! Drift is the join between the predictor and the latent it was asked to
//! predict: the diagonal Mahalanobis distance of the residual under the
//! predictor's own precision, `exp(-log_variance)`. Identity prediction leaves
//! a zero residual and so measures exactly zero. A sample whose three slices
//! are ragged, or which carries a non-finite value, is not a measurement at
//! all, and the report says so with `sample_count == 0` — the "no opinion" the
//! rule layer must not act on.

use qualia_braid::drift::{measure, DriftReport};

#[test]
fn drift_is_zero_for_identity_prediction() {
    let latent = [1.5_f32, -0.25, 3.0, 0.75];
    let predicted_log_variance = [0.0_f32, 0.0, 0.0, 0.0];

    let report = measure(&latent, &latent, &predicted_log_variance);

    assert_eq!(report.mahalanobis, 0.0, "the prediction sits on the observation");
    assert_eq!(report.sample_count, 1, "a well-formed sample is one opinion");
}

#[test]
fn residual_is_squared_and_weighted_by_predicted_precision() {
    // Unit variance is unit precision, so the measurement is the squared
    // residual the ladder's thresholds are written against.
    let report = measure(&[3.0], &[1.0], &[0.0]);

    assert_eq!(report.mahalanobis, 4.0);
    assert_eq!(report.sample_count, 1);
}

#[test]
fn mismatched_sample_is_no_opinion() {
    let report = measure(&[1.0, 2.0], &[1.0], &[0.0, 0.0]);

    assert_eq!(report, DriftReport { mahalanobis: 0.0, sample_count: 0 });
}

#[test]
fn non_zero_log_variance_weights_by_precision_not_variance() {
    // `exp(-1)` is the precision at this log-variance; the sign-flipped
    // variance weighting `exp(+1)` would give roughly 10.87 instead. Only a
    // non-zero log-variance can tell the two apart, and only `4·e⁻¹` is right.
    let report = measure(&[3.0], &[1.0], &[1.0]);

    assert_eq!(report.mahalanobis, 4.0 * (-1.0_f32).exp());
    assert_eq!(report.sample_count, 1);
}

#[test]
fn non_finite_predicted_log_variance_is_no_opinion() {
    // `+inf` is the channel a post-`exp` check would miss: its precision is a
    // finite `0.0`, which reads as a perfect prediction. `-inf` and `NaN` are
    // named too so the whole non-finite surface is pinned.
    for log_variance in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        assert_eq!(
            measure(&[1.0], &[0.0], &[log_variance]),
            DriftReport { mahalanobis: 0.0, sample_count: 0 },
            "log_variance {log_variance} must be no opinion"
        );
    }
}

#[test]
fn non_finite_latent_or_mean_is_no_opinion() {
    assert_eq!(
        measure(&[f32::NAN], &[0.0], &[0.0]),
        DriftReport { mahalanobis: 0.0, sample_count: 0 }
    );
    assert_eq!(
        measure(&[1.0], &[f32::INFINITY], &[0.0]),
        DriftReport { mahalanobis: 0.0, sample_count: 0 }
    );
}
