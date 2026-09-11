//! Contract tests for the rebuilt `qualia-jepa-model` crate.
//!
//! These pin the observable behaviour a consumer (the registry, the runtime
//! runner, the evidence gates) relies on: deterministic initialization, digest
//! checked loading, generation identity on swap, a training step that actually
//! descends, and a parity harness that is exact when the two backends are the
//! same device.

use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};
use qualia_jepa::{ACTION_DIM, BELIEF_DIM, CORE_DIM, GROUNDING_CELLS, OBS_DIM};
use qualia_jepa_model::parity::{compare_checkpoint_backends, ParityThresholds};
use qualia_jepa_model::runtime::{CoherentJepaRuntime, RuntimeInput};
use qualia_jepa_model::train::{OfflineTrainer, TrainerConfig, TransitionExample};
use qualia_jepa_model::{
    empty_candidate_manifest, initialize_deterministic, write_candidate_checkpoint, ActionSupport,
    BaselineGate, GroundingGeometry, HeldOutMetrics, JepaCandidateModel,
};
use std::path::{Path, PathBuf};

fn cpu_observation(fill: f32) -> Vec<f32> {
    (0..OBS_DIM)
        .map(|index| fill + (index % 13) as f32 * 1.0e-3)
        .collect()
}

fn fixture_input() -> RuntimeInput {
    RuntimeInput {
        observation: cpu_observation(0.25),
        applied_action: [0.2, -0.1, 0.3, 1.0],
        delta_seconds: 0.125,
    }
}

fn passing_gate() -> BaselineGate {
    let point = |transition_nll: f64, rollout_error: f64| HeldOutMetrics {
        transition_nll,
        rollout_error,
    };
    BaselineGate {
        dataset_digest: "d".repeat(64),
        valid_transitions: 50_000,
        sessions: 12,
        conditions: 3,
        environments: 3,
        constant: point(3.0, 3.0),
        flat_mlp: point(2.0, 2.0),
        tiny_cnn: point(1.0, 1.0),
    }
}

fn passing_support() -> ActionSupport {
    ActionSupport {
        sample_count: 50_000,
        min_left: -1.0,
        max_left: 1.0,
        min_right: -1.0,
        max_right: 1.0,
        min_effective_forward: -1.0,
        max_effective_forward: 1.0,
        min_effective_turn: -1.0,
        max_effective_turn: 1.0,
        min_speed_scale: 0.0,
        max_speed_scale: 1.0,
        min_delta_seconds: 0.05,
        max_delta_seconds: 0.5,
    }
}

fn passing_geometry() -> GroundingGeometry {
    GroundingGeometry {
        width: qualia_jepa::GROUNDING_WIDTH,
        height: qualia_jepa::GROUNDING_HEIGHT,
        resolution_m: 0.05,
    }
}

fn initialized_vars(seed: u64) -> VarMap {
    let vars = VarMap::new();
    JepaCandidateModel::load(VarBuilder::from_varmap(&vars, DType::F32, &Device::Cpu)).unwrap();
    initialize_deterministic(&vars, seed).unwrap();
    vars
}

/// Persist one full candidate checkpoint and return its directory.
fn write_checkpoint(root: &Path, checkpoint_id: &str, seed: u64) -> PathBuf {
    let vars = initialized_vars(seed);
    let manifest = empty_candidate_manifest(
        checkpoint_id,
        &"d".repeat(64),
        seed,
        "cpu",
        passing_gate(),
        passing_support(),
        passing_geometry(),
    );
    write_candidate_checkpoint(root, checkpoint_id, &vars, &vars, manifest).unwrap();
    root.join(checkpoint_id)
}

#[test]
fn encoding_is_deterministic_for_a_fixed_seed_and_input() {
    let first = initialized_vars(0x5eed);
    let second = initialized_vars(0x5eed);
    let differently_seeded = initialized_vars(0x5ef0);
    let device = Device::Cpu;
    let observation = qualia_jepa::pack_observation(
        &vec![0.4f32; qualia_jepa::CAMERA_PIXELS],
        &vec![0.5f32; qualia_jepa::LIDAR_BINS],
        &vec![true; qualia_jepa::LIDAR_BINS],
        qualia_jepa::PoseFeatures {
            x_m: 0.1,
            z_m: -0.2,
            yaw_rad: 0.3,
            linear_mps: 0.0,
            angular_rps: 0.0,
            normalized_age: 0.0,
            valid: true,
        },
    )
    .unwrap()
    .to_vec();

    let encode = |vars: &VarMap| {
        let model =
            JepaCandidateModel::load(VarBuilder::from_varmap(vars, DType::F32, &device)).unwrap();
        let tensor =
            candle_core::Tensor::from_slice(&observation, (1, OBS_DIM), &device).unwrap();
        model
            .encode_observation(&tensor)
            .unwrap()
            .flatten_all()
            .unwrap()
            .to_vec1::<f32>()
            .unwrap()
    };

    let reference = encode(&first);
    assert_eq!(reference, encode(&second));
    assert_ne!(reference, encode(&differently_seeded));
    assert_eq!(reference.len(), CORE_DIM);

    // The frozen fixture path must be equally reproducible, head to head.
    let left = CoherentJepaRuntime::frozen_fixture(Device::Cpu, 0xc0ffee)
        .unwrap()
        .infer(&fixture_input())
        .unwrap();
    let right = CoherentJepaRuntime::frozen_fixture(Device::Cpu, 0xc0ffee)
        .unwrap()
        .infer(&fixture_input())
        .unwrap();
    assert_eq!(left.latent, right.latent);
    assert_eq!(left.predicted_mean, right.predicted_mean);
    assert_eq!(left.predicted_log_variance, right.predicted_log_variance);
    assert_eq!(left.evidence, right.evidence);
    assert_eq!(left.occupancy_logits, right.occupancy_logits);
    assert!(left.is_finite());
}

#[test]
fn runtime_rejects_weights_whose_digest_does_not_match_the_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let directory = write_checkpoint(temp.path(), "candidate-a", 3);

    // A clean load succeeds and reports the manifest identity.
    let loaded = CoherentJepaRuntime::from_checkpoint(&directory, Device::Cpu).unwrap();
    assert_eq!(loaded.checkpoint_id(), "candidate-a");
    assert_eq!(loaded.weights_sha256().len(), 64);

    // Rewriting the declared digest must make the same bytes unloadable.
    let manifest_path = directory.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["weights_sha256"] = serde_json::Value::String("a".repeat(64));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = match CoherentJepaRuntime::from_checkpoint(&directory, Device::Cpu) {
        Ok(runtime) => panic!(
            "a rewritten digest must not load: {} stayed resident",
            runtime.checkpoint_id()
        ),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("digest mismatch"),
        "unexpected error: {error}"
    );
}

#[test]
fn publication_refuses_a_manifest_with_a_non_finite_metric() {
    let temp = tempfile::tempdir().unwrap();
    let vars = initialized_vars(0x5eed);
    let mut gate = passing_gate();
    gate.flat_mlp.rollout_error = f64::NAN;
    let manifest = empty_candidate_manifest(
        "non-finite-candidate",
        &"d".repeat(64),
        0x5eed,
        "cpu",
        gate,
        passing_support(),
        passing_geometry(),
    );

    let error = match write_candidate_checkpoint(
        temp.path(),
        "non-finite-candidate",
        &vars,
        &vars,
        manifest,
    ) {
        Ok(_) => panic!("a manifest with a non-finite metric must not be published"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("non-finite manifest metric baseline_gate.flat_mlp.rollout_error"),
        "refusal must name the offending metric: {message}"
    );
    assert!(
        message.contains("non-finite-candidate"),
        "refusal must name the candidate: {message}"
    );
    // A refused publication leaves neither a destination nor a staging dir.
    assert!(!temp.path().join("non-finite-candidate").exists());
    assert!(!temp.path().join(".non-finite-candidate.partial").exists());

    // The same path with a finite metric still publishes and reads back.
    let directory = write_checkpoint(temp.path(), "finite-candidate", 0x5eed);
    let loaded = CoherentJepaRuntime::from_checkpoint(&directory, Device::Cpu).unwrap();
    assert_eq!(loaded.checkpoint_id(), "finite-candidate");
}

#[test]
fn generation_swap_keeps_the_previous_generation_resident_and_reports_the_new_one() {
    let temp = tempfile::tempdir().unwrap();
    let first_dir = write_checkpoint(temp.path(), "generation-1", 11);
    let second_dir = write_checkpoint(temp.path(), "generation-2", 12);

    let first = CoherentJepaRuntime::from_checkpoint(&first_dir, Device::Cpu).unwrap();
    let before_swap = first.infer(&fixture_input()).unwrap();

    // The swap publishes a new generation without retiring the old one: the
    // runner keeps the superseded runtime resident for rollback.
    let second = CoherentJepaRuntime::from_checkpoint(&second_dir, Device::Cpu).unwrap();
    assert_eq!(second.checkpoint_id(), "generation-2");
    assert_ne!(first.weights_sha256(), second.weights_sha256());

    assert_eq!(first.checkpoint_id(), "generation-1");
    let after_swap = first.infer(&fixture_input()).unwrap();
    assert_eq!(before_swap.latent, after_swap.latent);
    assert_eq!(before_swap.evidence, after_swap.evidence);
    assert!(second.infer(&fixture_input()).unwrap().is_finite());
}

fn synthetic_batch() -> Vec<TransitionExample> {
    (0..4)
        .map(|index| {
            let base = 0.1 + index as f32 * 0.05;
            TransitionExample {
                sample_id: format!("synthetic-{index}"),
                observation_sequence: index as u64 + 1,
                observation_timestamp_ns: (index as u64 + 1) * 100,
                target_sequence: index as u64 + 2,
                target_timestamp_ns: (index as u64 + 2) * 100,
                observation: cpu_observation(base),
                target_observation: cpu_observation(base + 0.02),
                applied_action: [0.1, 0.1, 0.5, 1.0],
                delta_seconds: 0.1,
                future_occupied: (0..GROUNDING_CELLS)
                    .map(|cell| f32::from(cell % 17 == 0))
                    .collect(),
                future_observed: vec![1.0; GROUNDING_CELLS],
            }
        })
        .collect()
}

#[test]
fn training_step_reduces_its_loss_on_a_fixed_synthetic_batch() {
    let batch = synthetic_batch();
    let config = TrainerConfig {
        weight_decay: 0.0,
        ..TrainerConfig::default()
    };
    let mut trainer = OfflineTrainer::new(Device::Cpu, 7, config).unwrap();
    let before = trainer.evaluate_batch(&batch).unwrap().total;
    assert!(before.is_finite());
    // Forty steps already move the fixed batch well past its initial loss
    // (28.63 -> 24.04 on the probe trajectory), so the descent is observed
    // without paying for the plateau.
    for _ in 0..40 {
        let metrics = trainer.train_batch(&batch).unwrap();
        assert!(metrics.total.is_finite());
    }
    let after = trainer.evaluate_batch(&batch).unwrap().total;
    assert!(
        after < before,
        "training must descend on a fixed batch: {before} -> {after}"
    );
    assert_eq!(trainer.steps(), 40);
}

#[test]
fn parity_harness_reports_zero_relative_error_for_identical_tensors() {
    let temp = tempfile::tempdir().unwrap();
    let directory = write_checkpoint(temp.path(), "parity-candidate", 0xc0ffee);
    let report =
        compare_checkpoint_backends(&directory, "cpu", ParityThresholds::default()).unwrap();
    assert!(report.passes);
    assert!(report.outputs_finite);
    assert_eq!(report.checkpoint_id, "parity-candidate");
    assert_eq!(report.rollout_steps, 8);
    assert_eq!(report.single_step.len(), 5);
    assert_eq!(report.eight_step.len(), 3);
    for (name, metric) in report.single_step.iter().chain(report.eight_step.iter()) {
        assert_eq!(metric.rmse, 0.0, "{name} rmse");
        assert_eq!(metric.max_abs, 0.0, "{name} max_abs");
        // Cosine is accumulated in f64, so agreement is exact only to rounding.
        assert!(
            (metric.cosine - 1.0).abs() <= 1.0e-12,
            "{name} cosine: {}",
            metric.cosine
        );
        assert!(metric.passes, "{name} gate");
    }
}

#[test]
fn every_public_head_has_the_documented_width() {
    let runtime = CoherentJepaRuntime::frozen_fixture(Device::Cpu, 5).unwrap();
    let output = runtime.infer(&fixture_input()).unwrap();
    assert_eq!(output.latent.len(), CORE_DIM);
    assert_eq!(output.predicted_mean.len(), CORE_DIM);
    assert_eq!(output.predicted_log_variance.len(), CORE_DIM);
    assert_eq!(output.evidence.len(), BELIEF_DIM);
    assert_eq!(output.occupancy_logits.len(), GROUNDING_CELLS);
    assert_eq!(output.transition_nll, 0.0);

    let previous = fixture_input();
    let mut target = previous.observation.clone();
    target[0] += 0.05;
    let observed = runtime.observe_transition(&previous, &target).unwrap();
    assert!(observed.transition_nll.is_finite());
    assert_ne!(observed.transition_nll, 0.0);
    assert_eq!(observed.predicted_mean, output.predicted_mean);

    let action = previous.applied_action;
    let step = runtime
        .predict_latent_step(&output.latent, action, previous.delta_seconds)
        .unwrap();
    assert_eq!(step.mean.len(), CORE_DIM);
    assert_eq!(step.log_variance.len(), CORE_DIM);
    assert_eq!(step.occupancy_logits.len(), GROUNDING_CELLS);
    assert_eq!(ACTION_DIM, 4);
}
