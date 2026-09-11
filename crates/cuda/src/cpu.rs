//! Host-side reference implementations of the CUDA kernels.
//!
//! This module is always compiled — it has no dependency on `cudarc` — so a
//! build without the `cuda` feature still has a working, testable
//! implementation of the same arithmetic. When the device path is available
//! these functions are the oracle it is checked against; when it is not, they
//! are the fallback.
//!
//! The arithmetic here is deliberately written the same way the kernels are:
//! the same clamps, the same reduction order for the VFE tree, and the same
//! ordering of the belief, weight and precision updates. Any change to one
//! side must be mirrored in the other.

use qualia_types::{BeliefSlot, STATE_DIM, WEIGHT_COUNT};

/// A `u8` cost at or above this value counts as high cost.
pub const HIGH_COST_THRESHOLD: u8 = 200;

/// The four counters a costmap reduction produces.
///
/// Field names and semantics match the response the compute service exposes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CostmapStats {
    pub occupied_count: u32,
    pub high_cost_count: u32,
    pub cost_sum: u32,
    pub blocked_count: u32,
}

/// The one shape rule the costmap reduction has: the two grids must be the
/// same length. Shared by the host reference and the device context.
pub fn validate_costmap_pair(occupied: &[u8], cost: &[u8]) -> Result<(), String> {
    if occupied.len() != cost.len() {
        return Err(format!(
            "occupied and cost must have equal length ({} vs {})",
            occupied.len(),
            cost.len()
        ));
    }
    Ok(())
}

/// Counts occupancy, high cost, total cost and blocked cells.
///
/// An empty pair is valid and yields the zero counters.
pub fn costmap_stats(occupied: &[u8], cost: &[u8]) -> Result<CostmapStats, String> {
    validate_costmap_pair(occupied, cost)?;

    let mut stats = CostmapStats::default();
    for (occupied_cell, cost_cell) in occupied.iter().zip(cost.iter()) {
        if *occupied_cell != 0 {
            stats.occupied_count += 1;
            stats.blocked_count += 1;
        }
        if *cost_cell >= HIGH_COST_THRESHOLD {
            stats.high_cost_count += 1;
        }
        stats.cost_sum += u32::from(*cost_cell);
    }
    Ok(stats)
}

/// Runs one predictive-coding update over a whole layer.
///
/// `params` is the kernel's `[threshold, learning_rate, layer_id, weight_decay]`
/// vector. The belief, weight and bias buffers are mutated in place.
pub fn belief_update(
    belief: &mut BeliefSlot,
    below: &BeliefSlot,
    weights: &mut [f32; WEIGHT_COUNT],
    bias: &mut [f32; STATE_DIM],
    params: &[f32; 4],
) {
    let threshold = params[0];
    let learning_rate = params[1];
    let weight_decay = params[3];
    let weight_rate = learning_rate * 0.1;

    // The weight step is Hebbian against the pre-update state, so both the
    // mean and the precision are read from these snapshots throughout.
    let prior_mean = belief.mean;
    let prior_precision = belief.precision;

    let mut terms = [0.0f32; STATE_DIM];
    for index in 0..STATE_DIM {
        let row = index * STATE_DIM;
        let mut prediction = bias[index];
        for column in 0..STATE_DIM {
            prediction += weights[row + column] * prior_mean[column];
        }
        belief.prediction[index] = prediction;

        let residual = below.mean[index] - prediction;
        belief.residual[index] = residual;

        let clipped = residual.clamp(-10.0, 10.0);
        terms[index] = clipped * prior_precision[index].min(10.0) * clipped;
    }

    // The same fixed binary tree the kernel reduces with.
    let mut span = STATE_DIM / 2;
    while span > 0 {
        for index in 0..span {
            terms[index] += terms[index + span];
        }
        span >>= 1;
    }
    let vfe = terms[0];

    belief.vfe = vfe;
    belief.challenge_vfe = vfe;
    if vfe <= threshold {
        belief.confirm_streak += 1;
        if belief.compression < 255 && belief.confirm_streak > 100 {
            belief.compression += 1;
        }
    } else {
        belief.confirm_streak = 0;
        if belief.compression > 0 {
            belief.compression -= 1;
        }
    }

    if vfe > threshold {
        for index in 0..STATE_DIM {
            let gain = prior_precision[index].min(1.0);
            let step = (learning_rate * gain * belief.residual[index]).clamp(-0.1, 0.1);
            belief.mean[index] = (belief.mean[index] + step).clamp(-10.0, 10.0);
        }

        for index in 0..STATE_DIM {
            let gain = prior_precision[index].min(1.0);
            let step = (weight_rate * gain * belief.residual[index]).clamp(-0.01, 0.01);
            let row = index * STATE_DIM;
            for column in 0..STATE_DIM {
                let decayed = weights[row + column] * (1.0 - weight_decay);
                weights[row + column] = (decayed + step * prior_mean[column]).clamp(-10.0, 10.0);
            }
            bias[index] = (bias[index] * (1.0 - weight_decay) + step).clamp(-10.0, 10.0);
        }
    }

    for index in 0..STATE_DIM {
        if !belief.mean[index].is_finite() {
            belief.mean[index] = below.mean[index];
            belief.precision[index] = 0.01;
        }
        // The kernel resolves precision from the value read before the guard,
        // so the guard's assignment above is deliberately not what survives.
        let precision = prior_precision[index];
        if belief.residual[index].abs() < 0.01 {
            belief.precision[index] = (precision * 1.001).min(10.0);
        } else {
            belief.precision[index] = (precision * 0.999).max(0.01);
        }
    }
}

/// Runs one cognition update over a 1024-dimension layer.
///
/// `params` is the kernel's
/// `[source_precision, top_down_precision, state_rate, weight_rate, learning_signal]`
/// vector. Shapes are validated before anything is mutated.
pub fn cognition_update(
    state: &mut [f32],
    lower: &[f32],
    top_down: &[f32],
    weights: &mut [f32],
    bias: &mut [f32],
    params: &[f32; 5],
) -> Result<(), String> {
    if state.len() != STATE_DIM {
        return Err(format!("state must hold {STATE_DIM} values"));
    }
    if lower.len() != STATE_DIM {
        return Err(format!("lower must hold {STATE_DIM} values"));
    }
    if top_down.len() != STATE_DIM {
        return Err(format!("top_down must hold {STATE_DIM} values"));
    }
    if weights.len() != WEIGHT_COUNT {
        return Err(format!("weights must hold {WEIGHT_COUNT} values"));
    }
    if bias.len() != STATE_DIM {
        return Err(format!("bias must hold {STATE_DIM} values"));
    }

    let source_precision = params[0];
    let top_down_precision = params[1];
    let state_rate = params[2];
    let weight_rate = params[3];
    let learning_signal = params[4];

    let mut previous = [0.0f32; STATE_DIM];
    previous.copy_from_slice(state);
    let mut bottom_up = [0.0f32; STATE_DIM];
    for index in 0..STATE_DIM {
        let row = index * STATE_DIM;
        let mut prediction = bias[index];
        for column in 0..STATE_DIM {
            prediction += weights[row + column] * previous[column];
        }
        bottom_up[index] = (lower[index] - prediction).clamp(-10.0, 10.0);
    }

    for index in 0..STATE_DIM {
        let mut gradient = 0.0f32;
        for row in 0..STATE_DIM {
            gradient += weights[row * STATE_DIM + index] * bottom_up[row];
        }
        let next = previous[index] + state_rate * source_precision * gradient
            - state_rate * top_down_precision * (previous[index] - top_down[index]);
        state[index] = next.clamp(-4.0, 4.0);

        let step =
            (weight_rate * learning_signal * source_precision * bottom_up[index]).clamp(-0.001, 0.001);
        let row = index * STATE_DIM;
        for column in 0..STATE_DIM {
            weights[row + column] = (weights[row + column] + step * previous[column]).clamp(-2.0, 2.0);
        }
        bias[index] = (bias[index] + step).clamp(-1.0, 1.0);
    }

    Ok(())
}

/// The smoke kernel, on the host: an in-place increment of four values.
pub fn add_one(data: &mut [f32; 4]) {
    for value in data.iter_mut() {
        *value += 1.0;
    }
}
