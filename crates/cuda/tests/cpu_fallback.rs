//! Behaviour of the host-side (CPU) reference implementation.
//!
//! These tests need no GPU, so the default `cargo test -p qualia-cuda`
//! exercises the fallback path on any machine. The same functions are the
//! oracle the device tests in `gpu.rs` compare against.

use qualia_cuda::cpu;
use qualia_cuda::{BeliefSlot, STATE_DIM, WEIGHT_COUNT};

const DIM: usize = STATE_DIM;

fn zero_slot() -> BeliefSlot {
    // SAFETY: every field of BeliefSlot is a plain integer or float, and an
    // all-zero bit pattern is a valid value for each of them.
    unsafe { std::mem::zeroed() }
}

/// A heap-backed 1024x1024 matrix: 4 MiB does not belong on a test stack.
fn matrix(fill: impl Fn(usize, usize) -> f32) -> Vec<f32> {
    let mut values = vec![0.0f32; WEIGHT_COUNT];
    for row in 0..DIM {
        for column in 0..DIM {
            values[row * DIM + column] = fill(row, column);
        }
    }
    values
}

fn identity(row: usize, column: usize) -> f32 {
    if row == column {
        1.0
    } else {
        0.0
    }
}

fn as_matrix(values: &mut [f32]) -> &mut [f32; WEIGHT_COUNT] {
    values.try_into().expect("WEIGHT_COUNT values")
}

#[test]
fn belief_update_learns_when_error_exceeds_threshold() {
    // threshold, learning rate, layer id, weight decay.
    let params = [0.05f32, 0.005, 2.0, 0.00005];

    let mut belief = zero_slot();
    let mut below = zero_slot();
    for index in 0..DIM {
        belief.precision[index] = 1.0;
        below.mean[index] = 0.5;
    }
    let mut weights = matrix(identity);
    let mut bias = [0.0f32; DIM];

    cpu::belief_update(
        &mut belief,
        &below,
        as_matrix(&mut weights),
        &mut bias,
        &params,
    );

    for index in 0..DIM {
        // An identity matrix over a zero belief predicts zero.
        assert!(belief.prediction[index].abs() < 1e-6);
        assert!((belief.residual[index] - 0.5).abs() < 1e-6);
        // clamp(lr * min(precision, 1) * residual, +-0.1)
        assert!(
            (belief.mean[index] - 0.0025).abs() < 1e-6,
            "mean[{index}] = {}",
            belief.mean[index]
        );
        // |residual| >= 0.01, so precision decays by 0.999.
        assert!((belief.precision[index] - 0.999).abs() < 1e-6);
        // clamp(lr / 10 * min(precision, 1) * residual, +-0.01), against an old mean of zero.
        assert!((bias[index] - 0.00025).abs() < 1e-7);
        for column in 0..DIM {
            let expected = if index == column { 0.99995 } else { 0.0 };
            assert!(
                (weights[index * DIM + column] - expected).abs() < 1e-6,
                "weights[{index}][{column}] = {}",
                weights[index * DIM + column]
            );
        }
    }

    // Every dimension contributes 0.25 of precision-weighted error.
    let expected_vfe = DIM as f32 * 0.25;
    assert!((belief.vfe - expected_vfe).abs() <= expected_vfe * 1e-6);
    assert_eq!(belief.challenge_vfe, belief.vfe);
    assert_eq!(belief.confirm_streak, 0);
    assert_eq!(belief.compression, 0);
}

#[test]
fn belief_update_holds_still_when_error_is_below_threshold() {
    let params = [0.05f32, 0.005, 2.0, 0.00005];

    let mut belief = zero_slot();
    for index in 0..DIM {
        belief.precision[index] = 1.0;
    }
    let below = zero_slot();
    let mut weights = matrix(identity);
    let mut bias = [0.0f32; DIM];

    cpu::belief_update(
        &mut belief,
        &below,
        as_matrix(&mut weights),
        &mut bias,
        &params,
    );

    assert_eq!(belief.vfe, 0.0);
    assert_eq!(belief.challenge_vfe, 0.0);
    // One more cycle without surprise.
    assert_eq!(belief.confirm_streak, 1);
    assert_eq!(belief.compression, 0);

    for index in 0..DIM {
        assert_eq!(belief.mean[index], 0.0);
        assert_eq!(belief.prediction[index], 0.0);
        assert_eq!(belief.residual[index], 0.0);
        assert_eq!(bias[index], 0.0);
        // |residual| < 0.01, so precision firms up.
        assert!((belief.precision[index] - 1.001).abs() < 1e-6);
        for column in 0..DIM {
            let expected = identity(index, column);
            assert_eq!(weights[index * DIM + column], expected);
        }
    }
}

#[test]
fn belief_update_stays_finite_for_an_all_zero_slot() {
    let params = [0.05f32, 0.005, 2.0, 0.00005];

    let mut belief = zero_slot();
    let below = zero_slot();
    let mut weights = vec![0.0f32; WEIGHT_COUNT];
    let mut bias = [0.0f32; DIM];

    cpu::belief_update(
        &mut belief,
        &below,
        as_matrix(&mut weights),
        &mut bias,
        &params,
    );

    assert!(belief.mean.iter().all(|value| value.is_finite()));
    assert!(belief.prediction.iter().all(|value| value.is_finite()));
    assert!(belief.residual.iter().all(|value| value.is_finite()));
    assert!(belief.precision.iter().all(|value| value.is_finite()));
    assert!(weights.iter().all(|value| value.is_finite()));
    assert!(bias.iter().all(|value| value.is_finite()));
    assert_eq!(belief.vfe, 0.0);
    assert_eq!(belief.confirm_streak, 1);
}

#[test]
fn costmap_stats_counts_a_fixed_pair() {
    let occupied = [0u8, 1, 0, 1, 1, 0, 0, 0];
    let cost = [10u8, 250, 199, 200, 0, 255, 1, 2];

    let stats = cpu::costmap_stats(&occupied, &cost).expect("equal lengths");

    assert_eq!(stats.occupied_count, 3);
    assert_eq!(stats.blocked_count, 3);
    assert_eq!(stats.high_cost_count, 3);
    assert_eq!(stats.cost_sum, 917);
}

#[test]
fn costmap_stats_of_an_empty_pair_is_zero() {
    let stats = cpu::costmap_stats(&[], &[]).expect("equal lengths");
    assert_eq!(stats.occupied_count, 0);
    assert_eq!(stats.blocked_count, 0);
    assert_eq!(stats.high_cost_count, 0);
    assert_eq!(stats.cost_sum, 0);
    assert_eq!(stats, cpu::CostmapStats::default());
}

#[test]
fn costmap_stats_rejects_a_length_mismatch() {
    let error = cpu::costmap_stats(&[1, 2, 3], &[1, 2]).expect_err("lengths differ");
    assert!(!error.is_empty());
    assert!(cpu::validate_costmap_pair(&[0u8; 4], &[0u8; 4]).is_ok());
}

/// A fresh 1024x1024 generative model for the cognition fallback.
fn cognition_model() -> Vec<f32> {
    matrix(|row, column| if row == column { 0.75 } else { 0.0 })
}

#[test]
fn cognition_update_applies_a_fixed_step() {
    let mut state = [1.0f32; DIM];
    let lower = [1.0f32; DIM];
    let top_down = [0.0f32; DIM];
    let mut weights = vec![0.0f32; WEIGHT_COUNT];
    let mut bias = [0.0f32; DIM];
    // source precision, top-down precision, state rate, weight rate, learning signal.
    let params = [1.0f32, 1.0, 0.00012, 0.00008, 1.0];

    cpu::cognition_update(
        &mut state,
        &lower,
        &top_down,
        &mut weights,
        &mut bias,
        &params,
    )
    .expect("well-shaped inputs");

    for index in 0..DIM {
        // state + rate * source_precision * 0 - rate * top_down_precision * state
        assert!(
            (state[index] - 0.99988).abs() < 1e-5,
            "state[{index}] = {}",
            state[index]
        );
        assert!((bias[index] - 0.00008).abs() < 1e-8);
        for column in 0..DIM {
            assert!((weights[index * DIM + column] - 0.00008).abs() < 1e-8);
        }
    }
}

#[test]
fn cognition_update_stays_zero_for_empty_inputs() {
    let mut state = [0.0f32; DIM];
    let lower = [0.0f32; DIM];
    let top_down = [0.0f32; DIM];
    let mut weights = vec![0.0f32; WEIGHT_COUNT];
    let mut bias = [0.0f32; DIM];
    let params = [1.0f32, 1.0, 0.00012, 0.00008, 1.0];

    cpu::cognition_update(
        &mut state,
        &lower,
        &top_down,
        &mut weights,
        &mut bias,
        &params,
    )
    .expect("well-shaped inputs");

    assert!(state.iter().all(|value| *value == 0.0));
    assert!(weights.iter().all(|value| *value == 0.0));
    assert!(bias.iter().all(|value| *value == 0.0));
}

#[test]
fn cognition_update_rejects_bad_shapes() {
    let params = [1.0f32; 5];
    let lower = [0.0f32; DIM];
    let top_down = [0.0f32; DIM];

    let too_short = vec![0.0f32; DIM - 1];
    let too_long = vec![0.0f32; DIM + 1];

    for state in [too_short, too_long] {
        let mut state = state;
        assert!(cpu::cognition_update(
            &mut state,
            &lower,
            &top_down,
            &mut cognition_model(),
            &mut [0.0f32; DIM],
            &params,
        )
        .is_err());
    }

    assert!(cpu::cognition_update(
        &mut [0.0f32; DIM],
        &[],
        &top_down,
        &mut cognition_model(),
        &mut [0.0f32; DIM],
        &params,
    )
    .is_err());

    assert!(cpu::cognition_update(
        &mut [0.0f32; DIM],
        &lower,
        &[],
        &mut cognition_model(),
        &mut [0.0f32; DIM],
        &params,
    )
    .is_err());

    assert!(cpu::cognition_update(
        &mut [0.0f32; DIM],
        &lower,
        &top_down,
        &mut vec![0.0f32; WEIGHT_COUNT - 1],
        &mut [0.0f32; DIM],
        &params,
    )
    .is_err());

    assert!(cpu::cognition_update(
        &mut [0.0f32; DIM],
        &lower,
        &top_down,
        &mut cognition_model(),
        &mut [0.0f32; DIM - 1],
        &params,
    )
    .is_err());
}

#[test]
fn smoke_kernel_fallback_adds_one() {
    let mut data = [1.0f32, 2.0, 3.0, 4.0];
    cpu::add_one(&mut data);
    assert_eq!(data, [2.0, 3.0, 4.0, 5.0]);

    let mut zeroed = [0.0f32; 4];
    cpu::add_one(&mut zeroed);
    assert_eq!(zeroed, [1.0, 1.0, 1.0, 1.0]);
}
