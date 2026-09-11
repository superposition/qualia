//! Contract tests for the CPU reference mathematics.
//!
//! These exercise the public surface only, so any backend that claims to
//! implement the same contract can be checked against them.

use qualia_jepa::{
    camera_step_velocity, diagonal_mahalanobis_squared, gaussian_transition_nll,
    masked_occupancy_bce_with_logits, pack_observation, posterior_update, vicreg_statistics,
    AppliedAction, EvidencePrecision, MathError, ObservationQuality, PoseFeatures,
    PosteriorPrecision, TransitionLogVariance, ACTION_DIM, BELIEF_DIM, CAMERA_HEIGHT,
    CAMERA_PIXELS, CAMERA_WIDTH, CORE_DIM, GROUNDING_CELLS, GROUNDING_HEIGHT, GROUNDING_WIDTH,
    LIDAR_BINS, LIDAR_CHANNELS, OBS_DIM, POSE_DIM,
};

fn pose() -> PoseFeatures {
    PoseFeatures {
        x_m: 1.25,
        z_m: -0.5,
        yaw_rad: 0.25,
        linear_mps: 0.4,
        angular_rps: -0.1,
        normalized_age: 0.2,
        valid: true,
    }
}

fn camera() -> Vec<f32> {
    let mut luma = vec![0.0; CAMERA_PIXELS];
    luma[0] = 0.25;
    luma[CAMERA_PIXELS - 1] = 1.0;
    luma
}

fn lidar() -> Vec<f32> {
    let mut range = vec![0.0; LIDAR_BINS];
    range[0] = 0.5;
    range
}

fn mask() -> Vec<bool> {
    let mut valid = vec![false; LIDAR_BINS];
    valid[0] = true;
    valid
}

#[test]
fn dimensions_match_the_encoder_contract() {
    assert_eq!(CAMERA_WIDTH, 64);
    assert_eq!(CAMERA_HEIGHT, 48);
    assert_eq!(CAMERA_PIXELS, 3_072);
    assert_eq!(LIDAR_BINS, 720);
    assert_eq!(LIDAR_CHANNELS, 2);
    assert_eq!(POSE_DIM, 8);
    assert_eq!(ACTION_DIM, 4);
    assert_eq!(OBS_DIM, CAMERA_PIXELS + LIDAR_BINS * LIDAR_CHANNELS + POSE_DIM);
    assert_eq!(OBS_DIM, 4_520);
    assert_eq!(CORE_DIM, 256);
    assert_eq!(BELIEF_DIM, 1_024);
    assert_eq!(GROUNDING_WIDTH, 64);
    assert_eq!(GROUNDING_HEIGHT, 64);
    assert_eq!(GROUNDING_CELLS, 4_096);
}

#[test]
fn identity_prediction_has_zero_mahalanobis_distance() {
    let latent = [0.4, -1.2, 3.0, 0.0];
    let precision = [2.0, 0.5, 10.0, 1.0];
    let distance = diagonal_mahalanobis_squared(&latent, &latent, &precision).unwrap();
    assert_eq!(distance, 0.0);

    // A prediction that misses one dimension scales with that dimension's
    // precision only.
    let predicted = [0.4, -1.2, 3.0, 0.0];
    let observed = [1.4, -1.2, 3.0, 0.0];
    let distance = diagonal_mahalanobis_squared(&observed, &predicted, &precision).unwrap();
    assert!((distance - 2.0).abs() < 1.0e-6);
}

#[test]
fn ragged_slices_error_rather_than_panic() {
    assert!(matches!(
        diagonal_mahalanobis_squared(&[0.0, 0.0], &[0.0], &[1.0, 1.0]),
        Err(MathError::Shape { .. })
    ));
    assert!(matches!(
        diagonal_mahalanobis_squared(&[0.0], &[0.0], &[1.0, 1.0]),
        Err(MathError::Shape { .. })
    ));
    assert!(matches!(
        diagonal_mahalanobis_squared(&[], &[], &[]),
        Err(MathError::Empty(_))
    ));

    let log_variance = TransitionLogVariance::new(vec![0.0, 0.0]).unwrap();
    assert!(matches!(
        gaussian_transition_nll(&[0.0], &[0.0, 1.0], &log_variance),
        Err(MathError::Shape { .. })
    ));
    assert!(matches!(
        gaussian_transition_nll(&[0.0], &[0.0], &TransitionLogVariance::new(vec![0.0]).unwrap()),
        Ok(_)
    ));

    let prior_precision = PosteriorPrecision::new(vec![1.0, 1.0]).unwrap();
    assert!(matches!(
        posterior_update(
            &[0.0, 0.0],
            &prior_precision,
            &[1.0],
            &EvidencePrecision::new(vec![1.0]).unwrap(),
            ObservationQuality::new(1.0, 1.0, 1.0, 1.0).unwrap(),
        ),
        Err(MathError::Shape { .. })
    ));
    assert!(matches!(
        posterior_update(
            &[0.0, 0.0],
            &prior_precision,
            &[1.0, 1.0],
            &EvidencePrecision::new(vec![1.0]).unwrap(),
            ObservationQuality::new(1.0, 1.0, 1.0, 1.0).unwrap(),
        ),
        Err(MathError::Shape { .. })
    ));

    assert!(matches!(
        masked_occupancy_bce_with_logits(&[0.0, 1.0], &[1.0], &[true, true], 1.0),
        Err(MathError::Shape { .. })
    ));
    assert!(matches!(
        masked_occupancy_bce_with_logits(&[0.0, 1.0], &[1.0, 0.0], &[true], 1.0),
        Err(MathError::Shape { .. })
    ));
    assert!(matches!(
        masked_occupancy_bce_with_logits(&[], &[], &[], 1.0),
        Err(MathError::Empty(_))
    ));
}

#[test]
fn posterior_update_moves_the_mean_toward_the_more_precise_source() {
    let quality = ObservationQuality::new(1.0, 1.0, 1.0, 1.0).unwrap();

    // Evidence is three times as precise as the prior: the fused mean sits
    // three quarters of the way from the prior to the evidence.
    let evidence_heavy = posterior_update(
        &[0.0],
        &PosteriorPrecision::new(vec![1.0]).unwrap(),
        &[2.0],
        &EvidencePrecision::new(vec![3.0]).unwrap(),
        quality,
    )
    .unwrap();
    assert!((evidence_heavy.mean[0] - 1.5).abs() < 1.0e-6);
    assert!((evidence_heavy.precision.as_slice()[0] - 4.0).abs() < 1.0e-6);

    // Flip the imbalance and the mean must move the other way, staying between
    // the two sources.
    let prior_heavy = posterior_update(
        &[0.0],
        &PosteriorPrecision::new(vec![3.0]).unwrap(),
        &[2.0],
        &EvidencePrecision::new(vec![1.0]).unwrap(),
        quality,
    )
    .unwrap();
    assert!((prior_heavy.mean[0] - 0.5).abs() < 1.0e-6);
    assert!(prior_heavy.mean[0] < evidence_heavy.mean[0]);
    assert!(prior_heavy.mean[0] > 0.0 && evidence_heavy.mean[0] < 2.0);
}

#[test]
fn zero_quality_leaves_the_prior_untouched() {
    let prior = PosteriorPrecision::new(vec![2.0]).unwrap();
    let update = posterior_update(
        &[0.25],
        &prior,
        &[100.0],
        &EvidencePrecision::new(vec![100.0]).unwrap(),
        ObservationQuality::new(0.0, 1.0, 1.0, 1.0).unwrap(),
    )
    .unwrap();
    assert_eq!(update.mean, vec![0.25]);
    assert_eq!(update.precision, prior);
}

#[test]
fn quality_is_a_monotone_product_gate() {
    let full = ObservationQuality::new(1.0, 1.0, 1.0, 1.0)
        .unwrap()
        .precision_scale();
    let degraded = ObservationQuality::new(0.8, 0.5, 0.5, 0.5)
        .unwrap()
        .precision_scale();
    assert!((full - 1.0).abs() < f32::EPSILON);
    assert!((degraded - 0.1).abs() < 1.0e-6);
    assert!(degraded < full);
    assert_eq!(
        ObservationQuality::new(0.0, 1.0, 1.0, 1.0)
            .unwrap()
            .precision_scale(),
        0.0
    );
    assert!(ObservationQuality::new(1.1, 1.0, 1.0, 1.0).is_err());
    assert!(ObservationQuality::new(f32::NAN, 1.0, 1.0, 1.0).is_err());
}

#[test]
fn gaussian_nll_matches_the_unit_gaussian_fixture() {
    let loss = gaussian_transition_nll(
        &[1.0],
        &[0.0],
        &TransitionLogVariance::new(vec![0.0]).unwrap(),
    )
    .unwrap();
    assert!((loss.total - 0.5).abs() < f64::EPSILON);
    assert!((loss.per_dimension[0] - 0.5).abs() < f64::EPSILON);
    assert!((loss.gradient_mean[0] + 1.0).abs() < f64::EPSILON);
    assert!(loss.gradient_log_variance[0].abs() < f64::EPSILON);
}

#[test]
fn gaussian_nll_penalises_both_collapse_and_overconfidence() {
    let target = [2.0];
    let mean = [0.0];
    let optimal = gaussian_transition_nll(
        &target,
        &mean,
        &TransitionLogVariance::new(vec![(4.0_f32).ln()]).unwrap(),
    )
    .unwrap()
    .total;
    let collapsed = gaussian_transition_nll(
        &target,
        &mean,
        &TransitionLogVariance::new(vec![12.0]).unwrap(),
    )
    .unwrap()
    .total;
    let overconfident = gaussian_transition_nll(
        &target,
        &mean,
        &TransitionLogVariance::new(vec![-12.0]).unwrap(),
    )
    .unwrap()
    .total;
    assert!(optimal < collapsed);
    assert!(optimal < overconfident);
}

#[test]
fn occupancy_loss_ignores_unobserved_cells() {
    let observed = [true, true, false];
    let first = masked_occupancy_bce_with_logits(
        &[1.0, -1.0, 0.0],
        &[1.0, 0.0, 1.0],
        &observed,
        3.0,
    )
    .unwrap();
    // The third cell is unobserved, so neither its logit nor a target outside
    // {0, 1} may influence the loss.
    let second = masked_occupancy_bce_with_logits(
        &[1.0, -1.0, 500.0],
        &[1.0, 0.0, 0.75],
        &observed,
        3.0,
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.observed_cells, 2);
    assert_eq!(first.occupied_cells, 1);
}

#[test]
fn occupancy_loss_is_stable_for_saturated_logits() {
    let loss = masked_occupancy_bce_with_logits(
        &[100.0, -100.0],
        &[1.0, 0.0],
        &[true, true],
        1.0,
    )
    .unwrap();
    assert!(loss.mean.is_finite());
    assert!(loss.mean < 1.0e-20);
    assert!(masked_occupancy_bce_with_logits(&[0.5], &[0.5], &[true], 1.0).is_err());
    assert!(masked_occupancy_bce_with_logits(&[0.5], &[1.0], &[true], 0.0).is_err());
}

#[test]
fn pack_observation_is_invariant_to_action_and_elapsed_time() {
    let camera = camera();
    let lidar = lidar();
    let mask = mask();
    let pose = pose();

    let observation = || pack_observation(&camera, &lidar, &mask, pose).unwrap();
    let before = observation();

    // An action and a camera-correlated step are computed alongside the frame,
    // yet neither may reach the packed observation.
    let action = AppliedAction {
        left: -0.8,
        right: 0.8,
        speed_scale: 0.5,
        valid: true,
    }
    .packed()
    .unwrap();
    let velocity = camera_step_velocity([0.4, 0.0], 0.1, [0.0, 0.0], 0.0, 0.05).unwrap();
    assert!(action[0] < 0.0 && velocity.0 > 0.0);

    let after = observation();
    assert_eq!(before, after);
    assert_eq!(&before[OBS_DIM - POSE_DIM..], &pose.packed().unwrap()[..]);
}

#[test]
fn pack_observation_lays_out_camera_then_lidar_then_pose() {
    let packed = pack_observation(&camera(), &lidar(), &mask(), pose()).unwrap();
    assert_eq!(packed[0], 0.25);
    assert_eq!(packed[CAMERA_PIXELS], 0.5);
    assert_eq!(packed[CAMERA_PIXELS + LIDAR_BINS], 1.0);
    assert_eq!(packed[OBS_DIM - POSE_DIM], 1.25);
    assert_eq!(packed[OBS_DIM - POSE_DIM + 2], 0.25_f32.sin());
    assert_eq!(packed[OBS_DIM - 1], 1.0);

    let mut hot = camera();
    hot[0] = 1.5;
    assert!(pack_observation(&hot, &lidar(), &mask(), pose()).is_err());
    assert!(pack_observation(&[0.0; 1], &lidar(), &mask(), pose()).is_err());
    assert!(pack_observation(&camera(), &lidar(), &[false; 1], pose()).is_err());
}

#[test]
fn pose_features_reject_out_of_range_age() {
    assert!(PoseFeatures {
        normalized_age: 1.5,
        ..pose()
    }
    .packed()
    .is_err());
    assert!(PoseFeatures {
        normalized_age: f32::NAN,
        ..pose()
    }
    .packed()
    .is_err());
    assert!(PoseFeatures {
        valid: false,
        ..pose()
    }
    .packed()
    .is_ok());
}

#[test]
fn applied_action_packing_is_signed_unit() {
    let packed = AppliedAction {
        left: -0.2,
        right: 0.3,
        speed_scale: 0.4,
        valid: true,
    }
    .packed()
    .unwrap();
    assert_eq!(packed, [-0.2, 0.3, 0.4, 1.0]);
    assert!(AppliedAction {
        left: 1.5,
        ..AppliedAction {
            left: 0.0,
            right: 0.0,
            speed_scale: 0.0,
            valid: false,
        }
    }
    .packed()
    .is_err());
    assert!(AppliedAction {
        speed_scale: -0.1,
        ..AppliedAction {
            left: 0.0,
            right: 0.0,
            speed_scale: 0.0,
            valid: false,
        }
    }
    .packed()
    .is_err());
}

#[test]
fn camera_step_velocity_wraps_yaw_and_rejects_bad_steps() {
    let (linear, angular) = camera_step_velocity(
        [0.3, 0.4],
        -std::f32::consts::PI + 0.1,
        [0.0, 0.0],
        std::f32::consts::PI - 0.1,
        0.5,
    )
    .unwrap();
    assert!((linear - 1.0).abs() < 1.0e-6);
    assert!((angular - 0.4).abs() < 1.0e-5);

    assert!(camera_step_velocity([0.0; 2], 0.0, [0.0; 2], 0.0, 0.0).is_err());
    assert!(camera_step_velocity([0.0; 2], 0.0, [1.0; 2], 0.0, -0.5).is_err());
    assert!(camera_step_velocity([f32::NAN, 0.0], 0.0, [0.0; 2], 0.0, 0.1).is_err());
}

#[test]
fn vicreg_detects_collapsed_and_correlated_representations() {
    let collapsed = vicreg_statistics(&[vec![1.0, 1.0], vec![1.0, 1.0]], 1.0, 1.0e-4).unwrap();
    assert!(collapsed.variance_loss > 0.9);
    assert!(collapsed.minimum_stddev < 0.1);

    let correlated = vicreg_statistics(
        &[vec![-1.0, -1.0], vec![0.0, 0.0], vec![1.0, 1.0]],
        1.0,
        1.0e-4,
    )
    .unwrap();
    assert!(correlated.covariance_loss > 0.9);

    assert!(vicreg_statistics(&[vec![1.0, 1.0]], 1.0, 1.0e-4).is_err());
    assert!(vicreg_statistics(&[vec![1.0], vec![1.0, 2.0]], 1.0, 1.0e-4).is_err());
    assert!(vicreg_statistics(&[vec![], vec![]], 1.0, 1.0e-4).is_err());
    assert!(vicreg_statistics(&[vec![0.0], vec![1.0]], 0.0, 1.0e-4).is_err());
}

#[test]
fn precision_newtypes_fail_closed() {
    assert!(matches!(
        EvidencePrecision::new(vec![-0.5]),
        Err(MathError::OutOfRange(_))
    ));
    assert!(matches!(
        EvidencePrecision::new(vec![]),
        Err(MathError::Empty(_))
    ));
    assert!(EvidencePrecision::new(vec![0.0, 3.0]).is_ok());

    assert!(matches!(
        PosteriorPrecision::new(vec![0.0]),
        Err(MathError::OutOfRange(_))
    ));
    assert!(matches!(
        PosteriorPrecision::new(vec![]),
        Err(MathError::Empty(_))
    ));

    assert!(matches!(
        TransitionLogVariance::new(vec![]),
        Err(MathError::Empty(_))
    ));
    assert!(matches!(
        TransitionLogVariance::new(vec![f32::NAN]),
        Err(MathError::NonFinite(_))
    ));
    assert!(TransitionLogVariance::new(vec![-40.0]).is_ok());

    assert!(matches!(
        diagonal_mahalanobis_squared(&[0.0], &[0.0], &[-1.0]),
        Err(MathError::OutOfRange(_))
    ));
    assert!(matches!(
        diagonal_mahalanobis_squared(&[f32::INFINITY], &[0.0], &[1.0]),
        Err(MathError::NonFinite(_))
    ));
}
