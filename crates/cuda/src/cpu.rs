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
//!
//! The emitted bits are part of that contract and not only the order of
//! operations: the kernels accumulate each row in ascending column order and
//! pin the scalar multiply-add's contraction with `fmaf`, because nvcc's own
//! choice of FMA contraction moves a few percent of the weights by one ULP
//! while the parity tests' tolerance cannot see it. D-008/D-009 make emitted
//! values interface, so a walk that re-associates the sum, or drops the pin,
//! is a value change — refused in `kernels/belief_update.cu` and
//! `kernels/cognition_update.cu`, which carry that sentence above each vector
//! loop.

use qualia_types::{
    BeliefSlot, JEPA_OCCUPANCY_CELLS, JEPA_OCCUPANCY_H, JEPA_OCCUPANCY_W, STATE_DIM, VOXEL_D,
    VOXEL_H, VOXEL_TOTAL, VOXEL_W, WEIGHT_COUNT,
};

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

/// Scales each mapped belief slot by its type's normalised in-strength.
///
/// The device twin of `qualia_jepa::prior::CouplingPrior::couple`, and the
/// oracle `belief_couple.cu` is checked against. A type's in-strength is the
/// summed weight of the edges that end on it, normalised by the largest
/// in-strength of any type, so the most strongly innervated type couples at
/// unit weight and every other type scales down in proportion: coupling can
/// attenuate a belief but never amplify it. `slots` pairs a type index with
/// the belief slot it feeds; a pair naming a type outside the graph the edges
/// describe or a slot outside `belief` is ignored. Returns the weight actually
/// applied, which is zero when no slot is mapped or every edge weight is zero.
pub fn belief_couple(
    belief: &mut [f32],
    cols: &[u32],
    weights: &[u32],
    slots: &[(u32, usize)],
) -> f32 {
    if cols.len() != weights.len() {
        return 0.0;
    }
    // The graph's type count is the highest type any edge names plus one; a
    // well-formed CSR names every type it declares, and a type no edge reaches
    // has zero strength and would couple at zero either way.
    let type_count = cols.iter().copied().max().map_or(0, |type_index| {
        type_index as usize + 1
    });
    let mut in_strength = vec![0u64; type_count];
    let mut peak = 0u64;
    for (column, weight) in cols.iter().zip(weights.iter()) {
        let strength = &mut in_strength[*column as usize];
        *strength += u64::from(*weight);
        peak = peak.max(*strength);
    }
    if peak == 0 {
        return 0.0;
    }

    let mut applied = 0.0f32;
    for &(type_index, slot) in slots {
        let Some(strength) = in_strength.get(type_index as usize) else {
            continue;
        };
        let Some(value) = belief.get_mut(slot) else {
            continue;
        };
        let factor = *strength as f32 / peak as f32;
        *value *= factor;
        applied += factor;
    }
    applied
}

/// Projects flat ground-plane LiDAR returns into occupancy logits.
///
/// The device twin of `perception_voxel.cu`. `points` is the interleaved
/// `points_xy_m` array of `LeashSpatialEvidenceV1`
/// (`crates/sync-types/src/spatial.rs`): `points[2 * i]` and `points[2 * i + 1]`
/// are one return's lateral and forward metres, so a sweep of n returns is a
/// `2 * n` float array. That wire shape is not a kernel input — Leash's
/// `project_occupancy` takes an int8 cell grid — but its extrusion is the same:
/// a return marks its whole voxel column because the scanner is planar. Each
/// cell's logit is `prior_logit` plus one hit's log-odds per return in the
/// cell, clamped to `[-20, 20]`. That bound is this kernel's own choice, not an
/// inherited one: ticket #31 names no band, and the reference — which clamps
/// belief residuals, means and weights at ±10 and log-variance at
/// `[-20, 20]` / `[-10, 5]` — has no occupancy-logit band. It suits this input
/// domain (the oracle's -2 prior and +1.5 log-odds need ~15 hits in one cell to
/// reach +20, more than a 0.25 m ground-plane cell collects) and keeps a stored
/// logit finite if a dense or malformed sweep inflates the count. The lattice
/// is the `qualia_types` world volume, indexed
/// `vx * VOXEL_D * VOXEL_H + vz * VOXEL_H + vy`, with x centred on the room and
/// z measured from the near wall.
pub fn perception_voxel(
    points: &[f32],
    resolution_m: f32,
    prior_logit: f32,
    hit_log_odds: f32,
) -> Result<Vec<f32>, String> {
    if points.len() % 2 != 0 {
        return Err("lidar points must be interleaved x, y coordinates".to_string());
    }
    if !resolution_m.is_finite() || resolution_m <= 0.0 {
        return Err("voxel resolution must be positive and finite".to_string());
    }
    if points.iter().any(|value| !value.is_finite()) {
        return Err("lidar points must be finite".to_string());
    }
    if !prior_logit.is_finite() || !hit_log_odds.is_finite() {
        return Err("occupancy logit parameters must be finite".to_string());
    }

    let point_count = points.len() / 2;
    let half = 0.5 * resolution_m;
    let mut logits = vec![0.0f32; VOXEL_TOTAL];
    for (index, logit) in logits.iter_mut().enumerate() {
        let vx = index / (VOXEL_D * VOXEL_H);
        let vz = (index % (VOXEL_D * VOXEL_H)) / VOXEL_H;
        let centre_x = (vx as f32 + 0.5 - 0.5 * VOXEL_W as f32) * resolution_m;
        let centre_y = (vz as f32 + 0.5) * resolution_m;

        let mut hits = 0u32;
        for point in 0..point_count {
            let x = points[2 * point];
            let y = points[2 * point + 1];
            if (x - centre_x).abs() <= half && (y - centre_y).abs() <= half {
                hits += 1;
            }
        }

        *logit = (prior_logit + hit_log_odds * hits as f32).clamp(-20.0, 20.0);
    }
    Ok(logits)
}

/// The geometry one rollout candidate is scored against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionScoreConfig {
    /// Wheel speed at unit scale, m/s.
    pub max_wheel_speed_mps: f32,
    /// Distance between the wheels, m.
    pub track_width_m: f32,
    /// Grounding-map metres per cell.
    pub resolution_m: f32,
    /// Collision footprint radius around the grounding origin, m.
    pub collision_radius_m: f32,
    /// Goal lateral offset in the robot frame, m.
    pub goal_lateral_m: f32,
    /// Goal forward offset in the robot frame, m.
    pub goal_forward_m: f32,
    /// Goal tolerance already satisfied by the terminal pose, m.
    pub goal_tolerance_m: f32,
}

/// The two rollout cost terms `action_score` produces for one candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateScore {
    /// L2 goal error at the terminal pose, less the goal tolerance, floored at zero.
    pub terminal_goal_distance_m: f32,
    /// Worst probability any step's central footprint selects.
    pub max_collision_probability: f32,
}

/// Counts the grounding cells a central footprint selects.
///
/// Shared by the oracle and the device context so both refuse a footprint that
/// selects nothing before any work happens.
pub fn central_footprint_cells(resolution_m: f32, radius_m: f32) -> Result<usize, String> {
    if !resolution_m.is_finite()
        || resolution_m <= 0.0
        || !radius_m.is_finite()
        || radius_m <= 0.0
    {
        return Err("collision footprint geometry is invalid".to_string());
    }
    let mut selected = 0usize;
    for row in 0..JEPA_OCCUPANCY_H {
        for column in 0..JEPA_OCCUPANCY_W {
            let x_m = (column as f32 + 0.5 - JEPA_OCCUPANCY_W as f32 * 0.5) * resolution_m;
            let z_m = (row as f32 + 0.5 - JEPA_OCCUPANCY_H as f32 * 0.5) * resolution_m;
            if x_m.hypot(z_m) <= radius_m {
                selected += 1;
            }
        }
    }
    if selected == 0 {
        return Err("collision footprint selects no grounding cells".to_string());
    }
    Ok(selected)
}

/// The reference sigmoid: the sign branch keeps `exp` from overflowing.
fn sigmoid(value: f32) -> f32 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exponent = value.exp();
        exponent / (1.0 + exponent)
    }
}

/// Wraps an angle into `[-PI, PI]`, the reference's exact loop.
fn wrap_angle(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
}

/// Scores one rollout candidate's terminal goal distance and worst collision.
///
/// The device twin of `action_score.cu`, mirroring the reference planner's
/// per-step midpoint integration and central-collision footprint. `steps` is
/// the candidate's flat `[left, right, speed_scale, dt, ...]` sequence and
/// `occupancy_logits` carries one `JEPA_OCCUPANCY_CELLS` grounding map per
/// step.
pub fn action_score(
    steps: &[f32],
    occupancy_logits: &[f32],
    config: &ActionScoreConfig,
) -> Result<CandidateScore, String> {
    if steps.len() % 4 != 0 {
        return Err("rollout steps must be left, right, speed_scale, dt tuples".to_string());
    }
    let step_count = steps.len() / 4;
    if occupancy_logits.len() != step_count * JEPA_OCCUPANCY_CELLS {
        return Err(format!(
            "one grounding map per step needs {} logits",
            step_count * JEPA_OCCUPANCY_CELLS
        ));
    }
    if steps.iter().any(|value| !value.is_finite())
        || occupancy_logits.iter().any(|value| !value.is_finite())
    {
        return Err("rollout inputs must be finite".to_string());
    }
    if !config.max_wheel_speed_mps.is_finite() || !config.track_width_m.is_finite() {
        return Err("drive geometry must be finite".to_string());
    }
    if config.track_width_m <= 0.0 {
        return Err("track width must be positive".to_string());
    }
    central_footprint_cells(config.resolution_m, config.collision_radius_m)?;

    let mut lateral_m = 0.0f32;
    let mut forward_m = 0.0f32;
    let mut yaw_rad = 0.0f32;
    let mut worst = 0.0f32;

    for step in 0..step_count {
        let map = &occupancy_logits[step * JEPA_OCCUPANCY_CELLS..(step + 1) * JEPA_OCCUPANCY_CELLS];
        let mut local = 0.0f32;
        for (cell, logit) in map.iter().enumerate() {
            let row = cell / JEPA_OCCUPANCY_W;
            let column = cell % JEPA_OCCUPANCY_W;
            let x_m =
                (column as f32 + 0.5 - JEPA_OCCUPANCY_W as f32 * 0.5) * config.resolution_m;
            let z_m = (row as f32 + 0.5 - JEPA_OCCUPANCY_H as f32 * 0.5) * config.resolution_m;
            if x_m.hypot(z_m) <= config.collision_radius_m {
                local = local.max(sigmoid(*logit));
            }
        }
        worst = worst.max(local);

        let left = steps[4 * step];
        let right = steps[4 * step + 1];
        let speed_scale = steps[4 * step + 2];
        let dt_seconds = steps[4 * step + 3];
        let left_mps = left * speed_scale * config.max_wheel_speed_mps;
        let right_mps = right * speed_scale * config.max_wheel_speed_mps;
        let linear_mps = (left_mps + right_mps) * 0.5;
        let angular_rps = (right_mps - left_mps) / config.track_width_m;
        let midpoint_yaw = yaw_rad + angular_rps * dt_seconds * 0.5;
        lateral_m += linear_mps * dt_seconds * midpoint_yaw.sin();
        forward_m += linear_mps * dt_seconds * midpoint_yaw.cos();
        yaw_rad = wrap_angle(yaw_rad + angular_rps * dt_seconds);
    }

    let distance =
        (lateral_m - config.goal_lateral_m).hypot(forward_m - config.goal_forward_m)
            - config.goal_tolerance_m;
    Ok(CandidateScore {
        terminal_goal_distance_m: distance.max(0.0),
        max_collision_probability: worst,
    })
}
