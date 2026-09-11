//! Contract of the non-macOS path: a CPU fallback with the same public surface
//! as the Metal stack, plus an honest refusal from `run_layer`.
//!
//! The Metal branches in `src/macos.rs` and `src/cognition_macos.rs` are
//! `cfg(target_os = "macos")` and cannot be built here, so this file is the
//! whole executable specification on a non-macOS host.

#![cfg(not(target_os = "macos"))]

use qualia_metal::{MetalCognitionStack, STATE_DIM, WEIGHT_COUNT};

const LAYERS: usize = 4;

#[test]
fn re_exports_the_shared_types() {
    // Consumers import these straight from `qualia_metal`; the crate must keep
    // re-exporting `qualia_types`.
    assert_eq!(STATE_DIM, 1024);
    assert_eq!(WEIGHT_COUNT, STATE_DIM * STATE_DIM);
}

#[test]
fn fallback_names_itself_a_cpu_fallback() {
    let stack = MetalCognitionStack::new().expect("the CPU fallback always constructs");
    assert_eq!(stack.backend_name(), "cpu-fallback");
}

#[test]
fn new_seeds_identity_diagonal_weights_per_layer() {
    let stack = MetalCognitionStack::new().unwrap();
    let (weights, biases) = stack.checkpoint_parameters();
    assert_eq!(weights.len(), LAYERS);
    assert_eq!(biases.len(), LAYERS);

    for (layer, (layer_weights, layer_biases)) in weights.iter().zip(&biases).enumerate() {
        assert_eq!(layer_weights.len(), WEIGHT_COUNT);
        assert_eq!(layer_biases.len(), STATE_DIM);
        let diagonal = 0.75 + layer as f32 * 0.05;
        for index in 0..STATE_DIM {
            assert_eq!(layer_weights[index * STATE_DIM + index], diagonal);
            assert_eq!(layer_weights[index * STATE_DIM + (index + 1) % STATE_DIM], 0.0);
        }
        assert!(layer_biases.iter().all(|value| *value == 0.0));
    }
}

#[test]
fn fork_copies_state_and_isolates_patches() {
    let canonical = MetalCognitionStack::new().unwrap();
    let before = canonical.weight_sketch(0, 16).unwrap();

    let mut shadow = canonical.fork().unwrap();
    shadow.apply_weight_delta(0, 0, 0, 0.25).unwrap();

    assert_eq!(canonical.weight_sketch(0, 16).unwrap(), before);
    assert_ne!(shadow.weight_sketch(0, 16).unwrap(), before);
}

#[test]
fn apply_weight_delta_patches_any_coordinate_and_clamps() {
    let mut stack = MetalCognitionStack::new().unwrap();

    // Off-diagonal coordinates are a legal patch: the fallback keeps the whole
    // dense matrix, exactly like the GPU path.
    assert_eq!(stack.apply_weight_delta(1, 5, 700, 0.25).unwrap(), 0.25);

    // The stored magnitude is clamped to the same bound as the Metal kernel.
    assert_eq!(stack.apply_weight_delta(1, 5, 700, 10.0).unwrap(), 2.0);
    assert_eq!(stack.apply_weight_delta(1, 5, 700, -10.0).unwrap(), -2.0);
}

#[test]
fn apply_weight_delta_refuses_bad_coordinates() {
    let mut stack = MetalCognitionStack::new().unwrap();
    assert!(stack.apply_weight_delta(LAYERS, 0, 0, 0.1).is_err());
    assert!(stack.apply_weight_delta(0, STATE_DIM, 0, 0.1).is_err());
    assert!(stack.apply_weight_delta(0, 0, STATE_DIM, 0.1).is_err());
    assert!(stack.apply_weight_delta(0, 0, 0, f32::NAN).is_err());
    assert!(stack.apply_weight_delta(0, 0, 0, f32::INFINITY).is_err());
}

#[test]
fn weight_sketch_is_side_squared_and_rejects_bad_shapes() {
    let stack = MetalCognitionStack::new().unwrap();

    assert_eq!(stack.weight_sketch(0, 4).unwrap().len(), 16);
    assert_eq!(stack.weight_sketch(0, 1).unwrap().len(), 1);
    assert_eq!(stack.weight_sketch(0, STATE_DIM).unwrap().len(), WEIGHT_COUNT);

    assert!(stack.weight_sketch(LAYERS, 4).is_err());
    assert!(stack.weight_sketch(0, 0).is_err());
    assert!(stack.weight_sketch(0, STATE_DIM + 1).is_err());
    assert!(stack.weight_sketch(0, 3).is_err(), "3 does not divide 1024");
}

#[test]
fn weight_sketch_of_identity_diagonal_is_diagonal() {
    let stack = MetalCognitionStack::new().unwrap();
    let side = 16;
    let sketch = stack.weight_sketch(0, side).unwrap();
    for row in 0..side {
        for column in 0..side {
            let cell = sketch[row * side + column];
            if row == column {
                assert!(cell > 0.0, "diagonal block {row} carries the seeded weight");
            } else {
                assert_eq!(cell, 0.0, "off-diagonal block {row},{column} is empty");
            }
        }
    }
}

#[test]
fn dispatch_refuses_wrong_shapes_and_layers() {
    let mut stack = MetalCognitionStack::new().unwrap();
    let full = vec![0.0_f32; STATE_DIM];
    let short = vec![0.0_f32; STATE_DIM - 1];

    assert!(stack.dispatch(LAYERS, &full, &full, 1.0, 0.0, 1.0).is_err());
    assert!(stack.dispatch(0, &short, &full, 1.0, 0.0, 1.0).is_err());
    assert!(stack.dispatch(0, &full, &short, 1.0, 0.0, 1.0).is_err());
}

#[test]
fn dispatch_updates_state_and_learns_weights() {
    let mut stack = MetalCognitionStack::new().unwrap();
    let mut lower = vec![0.0_f32; STATE_DIM];
    lower[42] = 10.0;
    let top_down = vec![0.0_f32; STATE_DIM];

    let (checkpoint_before, _) = stack.checkpoint_parameters();
    assert_eq!(checkpoint_before[0][0], 0.75);
    assert_eq!(checkpoint_before[0][1], 0.0);

    let first = stack.dispatch(0, &lower, &top_down, 1.0, 0.0, 1.0).unwrap();
    assert!(first[42] > 0.0, "state follows the layer below");
    assert!(first.iter().all(|value| value.is_finite()));

    let second = stack.dispatch(0, &lower, &top_down, 1.0, 0.0, 1.0).unwrap();
    let (checkpoint_after, _) = stack.checkpoint_parameters();
    assert_ne!(
        checkpoint_after[0], checkpoint_before[0],
        "a nonzero learning signal must move the persistent weights"
    );
    assert_eq!(second.len(), STATE_DIM);
}

#[test]
fn dispatch_is_deterministic() {
    let lower: Vec<f32> = (0..STATE_DIM).map(|index| (index % 7) as f32 * 0.1).collect();
    let top_down: Vec<f32> = (0..STATE_DIM).map(|index| (index % 5) as f32 * -0.05).collect();

    let mut left = MetalCognitionStack::new().unwrap();
    let mut right = MetalCognitionStack::new().unwrap();
    let a = left.dispatch(2, &lower, &top_down, 0.7, 0.3, 0.9).unwrap();
    let b = right.dispatch(2, &lower, &top_down, 0.7, 0.3, 0.9).unwrap();
    assert_eq!(a, b);
}

#[test]
fn restore_parameters_round_trips_and_validates_shape() {
    let mut stack = MetalCognitionStack::new().unwrap();
    let mut lower = vec![0.0_f32; STATE_DIM];
    lower[7] = 3.0;
    let top_down = vec![0.0_f32; STATE_DIM];
    stack.dispatch(0, &lower, &top_down, 1.0, 0.2, 1.0).unwrap();

    let (weights, biases) = stack.checkpoint_parameters();
    let states: Vec<Vec<f32>> = (0..LAYERS).map(|_| vec![0.0_f32; STATE_DIM]).collect();

    // A fresh stack adopts the checkpoint exactly.
    let mut restored = MetalCognitionStack::new().unwrap();
    restored.restore_parameters(&weights, &biases, &states).unwrap();
    assert_eq!(restored.checkpoint_parameters(), (weights.clone(), biases.clone()));

    // Shape and finiteness are enforced.
    assert!(restored.restore_parameters(&weights[..LAYERS - 1], &biases, &states).is_err());
    assert!(restored.restore_parameters(&weights, &biases[..LAYERS - 1], &states).is_err());
    assert!(restored.restore_parameters(&weights, &biases, &states[..LAYERS - 1]).is_err());

    let mut short = weights.clone();
    short[0].pop();
    assert!(restored.restore_parameters(&short, &biases, &states).is_err());

    let mut nan = weights.clone();
    nan[0][0] = f32::NAN;
    assert!(restored.restore_parameters(&nan, &biases, &states).is_err());
}

#[test]
#[should_panic(expected = "only supported on macOS")]
fn run_layer_refuses_on_non_macos() {
    qualia_metal::run_layer(1, "l1-belief");
}
