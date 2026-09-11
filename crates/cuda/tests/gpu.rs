//! Device tests for `qualia-cuda`.
//!
//! Every test in this file requires the `cuda` feature. Each one opens the
//! device itself and returns early with a message when no CUDA device is
//! present, so the suite skips cleanly instead of failing on a host without a
//! GPU. Where a kernel has a host reference, the device result is compared
//! against it within 1e-4 relative error.

#![cfg(feature = "cuda")]

use qualia_cuda::{
    cpu, default_params, ActionScoreContext, BeliefCoupleContext, BeliefSlot,
    CostmapStatsContext, CudaCognitionStack, CudaContext, JEPA_OCCUPANCY_CELLS,
    PerceptionVoxelContext, SmokeContext, STATE_DIM, VOXEL_TOTAL, WEIGHT_COUNT,
};

const DIM: usize = STATE_DIM;

/// Small deterministic generator so the tests do not depend on `rand`.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let unit = ((self.0 >> 33) as u32) as f32 / u32::MAX as f32;
        unit * 2.0 - 1.0
    }

    fn next_range(&mut self, low: f32, high: f32) -> f32 {
        low + (self.next_f32() * 0.5 + 0.5) * (high - low)
    }
}

/// Device and host perform the same f32 arithmetic but may contract a
/// multiply-add differently, so a value that is expected to be zero can show a
/// large *relative* error while being tiny in absolute terms. The tolerance is
/// therefore absolute plus relative.
fn assert_close(label: &str, device: f32, host: f32) {
    let scale = device.abs().max(host.abs());
    let difference = (device - host).abs();
    let tolerance = 1e-5 + 1e-4 * scale;
    assert!(
        difference <= tolerance,
        "{label}: device {device} vs host {host} (difference {difference}, tolerance {tolerance})"
    );
}

fn zero_slot() -> BeliefSlot {
    // SAFETY: every field of BeliefSlot is a plain integer or float, and an
    // all-zero bit pattern is a valid value for each of them.
    unsafe { std::mem::zeroed() }
}

#[test]
fn smoke_context_runs_a_kernel() {
    let context = match SmokeContext::new() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping CUDA smoke test: {error}");
            return;
        }
    };

    assert!(!context.device_name().is_empty());

    let mut rng = Rng::new(0x5eed);
    for _ in 0..8 {
        let input = [
            rng.next_range(-8.0, 8.0),
            rng.next_range(-8.0, 8.0),
            rng.next_range(-8.0, 8.0),
            rng.next_range(-8.0, 8.0),
        ];
        let mut expected = input;
        cpu::add_one(&mut expected);
        let actual = context.run_add_one(input).expect("smoke kernel runs");
        assert_eq!(actual, expected);
    }
}

#[test]
fn costmap_stats_context_matches_the_host_reference() {
    let context = match CostmapStatsContext::new() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping CUDA costmap test: {error}");
            return;
        }
    };

    assert!(!context.device_name().is_empty());

    let mut rng = Rng::new(0xc057);
    let occupied: Vec<u8> = (0..4096)
        .map(|_| u8::from(rng.next_f32() > 0.2))
        .collect();
    let cost: Vec<u8> = (0..4096).map(|_| rng.next_range(0.0, 255.0) as u8).collect();

    let expected = cpu::costmap_stats(&occupied, &cost).expect("equal lengths");
    let actual = context.run(&occupied, &cost).expect("costmap kernel runs");
    assert_eq!(actual.occupied_count, expected.occupied_count);
    assert_eq!(actual.high_cost_count, expected.high_cost_count);
    assert_eq!(actual.cost_sum, expected.cost_sum);
    assert_eq!(actual.blocked_count, expected.blocked_count);

    // An empty pair never reaches the device and must report zero.
    let empty = context.run(&[], &[]).expect("empty costmap pair");
    assert_eq!(empty, cpu::CostmapStats::default());

    // Shapes are validated before the launch.
    assert!(context.run(&[1u8, 2], &[3u8]).is_err());
}

#[test]
fn belief_update_matches_the_host_reference() {
    let params = default_params(2);
    let context = match CudaContext::new(&params) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping CUDA belief test: {error}");
            return;
        }
    };

    assert!(!context.device_name().is_empty());

    let mut rng = Rng::new(0xbe11ef);
    let mut host_belief = zero_slot();
    let mut host_below = zero_slot();
    let mut host_weights = vec![0.0f32; WEIGHT_COUNT];
    let mut host_bias = [0.0f32; DIM];
    for index in 0..DIM {
        host_belief.mean[index] = rng.next_range(-0.4, 0.4);
        host_belief.precision[index] = rng.next_range(0.5, 1.5);
        host_below.mean[index] = rng.next_range(-0.4, 0.4);
        host_bias[index] = rng.next_range(-0.01, 0.01);
    }
    for value in host_weights.iter_mut() {
        *value = rng.next_range(-0.01, 0.01);
    }

    let mut device_belief = host_belief;
    let mut device_weights = host_weights.clone();
    let mut device_bias = host_bias;

    // The kernel reads [threshold, learning rate, layer id, weight decay].
    let kernel_params = [
        params.threshold,
        params.learning_rate,
        params.layer_id as f32,
        params.weight_decay,
    ];
    let host_matrix: &mut [f32; WEIGHT_COUNT] =
        host_weights.as_mut_slice().try_into().expect("WEIGHT_COUNT");
    cpu::belief_update(
        &mut host_belief,
        &host_below,
        host_matrix,
        &mut host_bias,
        &kernel_params,
    );
    context.dispatch_belief_update(
        &mut device_belief,
        &host_below,
        device_weights
            .as_mut_slice()
            .try_into()
            .expect("WEIGHT_COUNT"),
        &mut device_bias,
    );

    for index in 0..DIM {
        assert_close(
            "belief.mean",
            device_belief.mean[index],
            host_belief.mean[index],
        );
        assert_close(
            "belief.prediction",
            device_belief.prediction[index],
            host_belief.prediction[index],
        );
        assert_close(
            "belief.residual",
            device_belief.residual[index],
            host_belief.residual[index],
        );
        assert_close(
            "belief.precision",
            device_belief.precision[index],
            host_belief.precision[index],
        );
        assert_close("bias", device_bias[index], host_bias[index]);
    }
    assert_close("belief.vfe", device_belief.vfe, host_belief.vfe);
    assert_close(
        "belief.challenge_vfe",
        device_belief.challenge_vfe,
        host_belief.challenge_vfe,
    );
    assert_eq!(device_belief.confirm_streak, host_belief.confirm_streak);
    assert_eq!(device_belief.compression, host_belief.compression);
    for (index, (device, host)) in device_weights.iter().zip(host_weights.iter()).enumerate() {
        assert_close(&format!("weights[{index}]"), *device, *host);
    }
}

#[test]
fn cognition_dispatch_matches_the_host_reference() {
    let mut stack = match CudaCognitionStack::new() {
        Ok(stack) => stack,
        Err(error) => {
            eprintln!("skipping CUDA cognition test: {error}");
            return;
        }
    };

    assert!(stack.backend_name().starts_with("cuda:"));

    let mut rng = Rng::new(0xc091);
    let lower: Vec<f32> = (0..DIM).map(|_| rng.next_range(-0.5, 0.5)).collect();
    let top_down: Vec<f32> = (0..DIM).map(|_| rng.next_range(-0.5, 0.5)).collect();
    let second_top_down: Vec<f32> = (0..DIM).map(|_| rng.next_range(-0.5, 0.5)).collect();
    let (source_precision, top_down_precision, learning_signal) = (0.4f32, 0.25f32, 0.6f32);
    let kernel_params = [
        source_precision,
        top_down_precision,
        0.00012f32,
        0.00008f32,
        learning_signal,
    ];

    // Layer 0 of a fresh stack starts with a 0.75 diagonal and a zero state.
    let mut host_state = [0.0f32; DIM];
    let mut host_weights = vec![0.0f32; WEIGHT_COUNT];
    let mut host_bias = [0.0f32; DIM];
    for index in 0..DIM {
        host_weights[index * DIM + index] = 0.75;
    }

    // The first tick primes the state so the second moves the weights.
    let first = stack
        .dispatch(
            0,
            &lower,
            &top_down,
            source_precision,
            top_down_precision,
            learning_signal,
        )
        .expect("first cognition tick");
    cpu::cognition_update(
        &mut host_state,
        &lower,
        &top_down,
        &mut host_weights,
        &mut host_bias,
        &kernel_params,
    )
    .expect("host reference");
    for index in 0..DIM {
        assert_close("cognition.state.first", first[index], host_state[index]);
    }

    let second = stack
        .dispatch(
            0,
            &lower,
            &second_top_down,
            source_precision,
            top_down_precision,
            learning_signal,
        )
        .expect("second cognition tick");
    cpu::cognition_update(
        &mut host_state,
        &lower,
        &second_top_down,
        &mut host_weights,
        &mut host_bias,
        &kernel_params,
    )
    .expect("host reference");

    let (device_weights, device_biases) = stack.checkpoint_parameters();
    for index in 0..DIM {
        assert_close("cognition.state.second", second[index], host_state[index]);
        assert_close("cognition.bias", device_biases[0][index], host_bias[index]);
    }
    for (index, (device, host)) in device_weights[0].iter().zip(host_weights.iter()).enumerate() {
        assert_close(&format!("cognition.weights[{index}]"), *device, *host);
    }

    // Out-of-range layers and vectors are rejected before any launch.
    assert!(stack.dispatch(9, &lower, &top_down, 0.4, 0.25, 0.6).is_err());
    assert!(stack
        .dispatch(0, &lower[..DIM - 1], &top_down, 0.4, 0.25, 0.6)
        .is_err());
    assert!(stack.dispatch(0, &lower, &[], 0.4, 0.25, 0.6).is_err());

    let states = vec![second.clone(), vec![0.0; DIM], vec![0.0; DIM], vec![0.0; DIM]];
    assert!(stack
        .restore_parameters(&device_weights, &device_biases, &states)
        .is_ok());

    // A malformed checkpoint is refused.
    assert!(stack
        .restore_parameters(&device_weights[..1], &device_biases, &states)
        .is_err());
}

#[test]
fn cognition_weight_edits_are_observable() {
    let mut stack = match CudaCognitionStack::new() {
        Ok(stack) => stack,
        Err(error) => {
            eprintln!("skipping CUDA cognition edit test: {error}");
            return;
        }
    };

    // Layer 1 starts with a 0.80 diagonal, so an off-diagonal delta is exact.
    let updated = stack
        .apply_weight_delta(1, 3, 4, 0.125)
        .expect("weight delta");
    assert!((updated - 0.125).abs() < 1e-6, "updated = {updated}");

    assert!(stack.apply_weight_delta(9, 0, 0, 0.1).is_err());
    assert!(stack.apply_weight_delta(0, DIM, 0, 0.1).is_err());
    assert!(stack.apply_weight_delta(0, 0, 0, f32::NAN).is_err());

    // A 16x16 block sketch of the 0.75 diagonal layer: 0.2652 on the diagonal
    // blocks, zero elsewhere.
    let sketch = stack.weight_sketch(0, 16).expect("weight sketch");
    assert_eq!(sketch.len(), 16 * 16);
    for block_row in 0..16 {
        for block_column in 0..16 {
            let value = sketch[block_row * 16 + block_column];
            if block_row == block_column {
                assert!((value - 0.2652).abs() < 1e-3, "diagonal block = {value}");
            } else {
                assert!(value.abs() < 1e-6, "off-diagonal block = {value}");
            }
        }
    }
    assert!(stack.weight_sketch(0, 0).is_err());
    assert!(stack.weight_sketch(0, 24).is_err());

    // A fork carries the parameters it was copied from.
    let fork = stack.fork().expect("fork");
    assert!(fork.backend_name().starts_with("cuda:"));
    let (weights, biases) = stack.checkpoint_parameters();
    let (forked_weights, forked_biases) = fork.checkpoint_parameters();
    assert_eq!(weights.len(), forked_weights.len());
    assert_eq!(biases.len(), forked_biases.len());
    for (layer, (left, right)) in weights.iter().zip(forked_weights.iter()).enumerate() {
        for (index, (a, b)) in left.iter().zip(right.iter()).enumerate() {
            assert_eq!(*a, *b, "fork weight {layer}[{index}]");
        }
    }
}

#[test]
fn belief_couple_context_matches_the_host_reference() {
    let context = match BeliefCoupleContext::new() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping CUDA belief couple test: {error}");
            return;
        }
    };

    assert!(!context.device_name().is_empty());

    let cols = [1u32, 1, 2, 0];
    let weights = [3u32, 5, 2, 4];
    let mut rng = Rng::new(0xb0115e);
    let belief: Vec<f32> = (0..4).map(|_| rng.next_range(-4.0, 4.0)).collect();

    // A mapped, an unmapped and a duplicated type all reach the device.
    let slots = [(0u32, 0usize), (1, 2), (2, 3), (1, 0)];
    let mut expected = belief.clone();
    let applied = cpu::belief_couple(&mut expected, &cols, &weights, &slots);
    let (actual, factors) = context.run(&belief, &cols, &weights, &slots).expect("couple runs");

    assert!(applied > 0.0);
    for (index, (device, host)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_close(&format!("coupled belief[{index}]"), *device, *host);
    }
    let expected_factors = [0.5f32, 1.0, 0.25, 1.0];
    for (index, factor) in factors.iter().enumerate() {
        assert_close(&format!("slot factor[{index}]"), *factor, expected_factors[index]);
    }

    // No edges and no mapped slots both apply nothing and never launch.
    let (unchanged, none) = context.run(&belief, &cols, &weights, &[]).expect("no slots");
    assert_eq!(unchanged, belief);
    assert!(none.is_empty());
    let (unchanged, none) = context.run(&belief, &[], &[], &slots).expect("no edges");
    assert_eq!(unchanged, belief);
    assert!(none.iter().all(|factor| *factor == 0.0));
}

#[test]
fn perception_voxel_context_matches_the_host_reference() {
    let context = match PerceptionVoxelContext::new() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping CUDA perception voxel test: {error}");
            return;
        }
    };

    assert!(!context.device_name().is_empty());

    let mut rng = Rng::new(0x70b1e1);
    let mut points = Vec::with_capacity(2 * 240);
    for _ in 0..240 {
        points.push(rng.next_range(-4.0, 4.0));
        points.push(rng.next_range(0.0, 8.0));
    }

    let expected = cpu::perception_voxel(&points, 0.25, -3.0, 1.25).expect("oracle");
    let actual = context
        .run(&points, 0.25, -3.0, 1.25)
        .expect("perception voxel runs");

    assert_eq!(actual.len(), VOXEL_TOTAL);
    for (index, (device, host)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_close(&format!("voxel logit[{index}]"), *device, *host);
    }

    // An empty sweep is answered without a device copy and matches the prior.
    let empty = context.run(&[], 0.25, -3.0, 1.25).expect("empty sweep");
    assert_eq!(empty, expected_empty(0.25, -3.0, 1.25));
}

/// The oracle the empty-sweep case must equal: every cell at the prior logit.
fn expected_empty(resolution_m: f32, prior_logit: f32, hit_log_odds: f32) -> Vec<f32> {
    cpu::perception_voxel(&[], resolution_m, prior_logit, hit_log_odds).expect("oracle")
}

#[test]
fn action_score_context_matches_the_host_reference() {
    let context = match ActionScoreContext::new() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping CUDA action score test: {error}");
            return;
        }
    };

    assert!(!context.device_name().is_empty());

    let config = cpu::ActionScoreConfig {
        max_wheel_speed_mps: 1.4,
        track_width_m: 0.5,
        resolution_m: 0.25,
        collision_radius_m: 0.5,
        goal_lateral_m: 0.2,
        goal_forward_m: 1.5,
        goal_tolerance_m: 0.1,
    };

    let mut rng = Rng::new(0xac7104);
    let candidate_steps = [3usize, 5];
    let steps: Vec<[f32; 4]> = (0..candidate_steps.iter().sum::<usize>())
        .map(|_| {
            [
                rng.next_range(-1.0, 1.0),
                rng.next_range(-1.0, 1.0),
                rng.next_range(0.3, 1.0),
                rng.next_range(0.05, 0.2),
            ]
        })
        .collect();
    let occupancy: Vec<f32> = (0..steps.len() * JEPA_OCCUPANCY_CELLS)
        .map(|_| rng.next_range(-6.0, 6.0))
        .collect();

    let actual = context
        .run(&steps, &candidate_steps, &occupancy, &config)
        .expect("action score runs");
    assert_eq!(actual.len(), candidate_steps.len());

    let mut start = 0usize;
    for (candidate, count) in candidate_steps.iter().enumerate() {
        let end = start + count;
        let flat: Vec<f32> = steps[start..end].iter().flatten().copied().collect();
        let maps =
            &occupancy[start * JEPA_OCCUPANCY_CELLS..end * JEPA_OCCUPANCY_CELLS];
        let expected = cpu::action_score(&flat, maps, &config).expect("oracle");
        assert_close(
            &format!("candidate {candidate} terminal distance"),
            actual[candidate].terminal_goal_distance_m,
            expected.terminal_goal_distance_m,
        );
        assert_close(
            &format!("candidate {candidate} max collision"),
            actual[candidate].max_collision_probability,
            expected.max_collision_probability,
        );
        start = end;
    }

    // Shape validation happens before any launch.
    assert!(context.run(&steps, &[steps.len() + 1], &occupancy, &config).is_err());
    assert!(context
        .run(&steps, &candidate_steps, &occupancy[..occupancy.len() - 1], &config)
        .is_err());
}
