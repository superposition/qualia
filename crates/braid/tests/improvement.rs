//! The self-improvement loop's braid-side contract.
//!
//! Step 31 closes the loop: a sealed segment asks for one bounded training job,
//! and a trained candidate becomes a generation only when the registry's gates
//! pass. These tests drive the loop the way the agent does and read what a
//! later consumer reads — the pointer the registry left on disk and the braid's
//! own view — never the loop's internals.
//!
//! `coupling_scale_is_bounded` (step 30) shares this file; step 32 adds the
//! loop's failure paths.

use qualia_braid::improvement::{
    verify_and_promote, ImprovementConfig, ImprovementLoop, PromotionOutcome, QueueError,
    TrainingQueue, TRAINING_JOB_DEADLINE_MS,
};
use qualia_braid::rules::{
    next_coupling_scale, COUPLING_SCALE_CEILING, COUPLING_SCALE_DEFAULT, COUPLING_SCALE_FLOOR,
};
use qualia_braid::{observe, BraidEvent, BraidState};
use qualia_jepa_registry::CandidateRegistry;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn config(root: &Path) -> ImprovementConfig {
    ImprovementConfig {
        catalog: root.join("catalog.json"),
        dataset_dir: root.join("datasets"),
        checkpoint_dir: root.join("checkpoints"),
        backend: "cpu".to_string(),
    }
}

#[test]
fn sealed_evidence_asks_for_one_bounded_training_job() {
    let temp = tempfile::tempdir().unwrap();
    let improvement = ImprovementLoop::new(config(temp.path()));
    let digest = "9f".repeat(32);
    let sealed = BraidEvent::EvidenceSealed {
        sha256: digest.clone(),
    };

    // A seal while the session's mission is still open waits: nothing trains
    // under a mission that has not closed.
    let mut state = BraidState {
        open_missions: 1,
        ..Default::default()
    };
    assert!(
        improvement.on_event(&state, &sealed).is_none(),
        "an open mission defers the sealed segment"
    );

    // The same seal with the mission closed is the job, named for the evidence
    // it came from and bounded by the deadline the step names.
    state.open_missions = 0;
    let job = improvement
        .on_event(&state, &sealed)
        .expect("a sealed, closed session trains");
    assert_eq!(job.checkpoint_id, format!("cnn-{}", &digest[..12]));
    assert_eq!(
        job.deadline(),
        Duration::from_millis(TRAINING_JOB_DEADLINE_MS)
    );

    // The command the agent runs is emitted interface, so it is pinned here.
    let manifest = temp.path().join("manifest.json");
    assert_eq!(
        job.dataset_argv(),
        vec![
            "--catalog".to_string(),
            temp.path().join("catalog.json").display().to_string(),
            "--output-dir".to_string(),
            temp.path().join("datasets").display().to_string(),
        ]
    );
    assert_eq!(
        job.training_argv(&manifest),
        vec![
            "--manifest".to_string(),
            manifest.display().to_string(),
            "--checkpoint-id".to_string(),
            job.checkpoint_id.clone(),
            "--output-dir".to_string(),
            temp.path().join("checkpoints").display().to_string(),
            "--backend".to_string(),
            "cpu".to_string(),
        ]
    );

    // A mission that closes without a seal asks for nothing: there is no
    // evidence in it to train on.
    assert!(improvement
        .on_event(
            &BraidState::default(),
            &BraidEvent::MissionClosed {
                mission_id: "mission-1".to_string(),
                outcome: "completed".to_string(),
            },
        )
        .is_none());

    // And a malformed digest is not evidence either; no candidate is invented.
    assert!(improvement
        .on_event(
            &BraidState::default(),
            &BraidEvent::EvidenceSealed {
                sha256: "sealed".to_string(),
            },
        )
        .is_none());

    // The queue brokers one at a time: a second submission while the first is
    // in flight is refused, and finishing frees the single slot.
    let mut queue = TrainingQueue::new();
    queue.submit(job.clone()).unwrap();
    assert_eq!(queue.in_flight(), Some(&job));
    assert_eq!(
        queue.submit(job.clone()),
        Err(QueueError::Busy),
        "a second job waits: training runs one at a time"
    );
    assert_eq!(queue.finish(), Some(job));
    assert!(queue.in_flight().is_none());
}

/// A checkpoint manifest whose held-out numbers tie the flat baseline instead
/// of beating it. That is the training report's baseline gate failing, and it
/// fails before the registry has to read a single weight.
fn failing_candidate_manifest() -> serde_json::Value {
    let digest = "3c".repeat(32);
    serde_json::json!({
        "schema_version": "qualia.jepa-checkpoint.v1",
        "architecture_id": "qualia.jepa.grounded-tiny-cnn.v1",
        "checkpoint_id": "cnn-regressed",
        "dataset_digest": digest,
        "training_seed": 7,
        "backend": "cpu",
        "dtype": "F32",
        "target_encoder": "ema",
        "parameter_count": 4096,
        "weights_sha256": "5e".repeat(32),
        "training_report_path": "evidence/training-report.json",
        "training_report_sha256": "b7".repeat(32),
        "baseline_gate": {
            "dataset_digest": digest,
            "valid_transitions": 51_200,
            "sessions": 16,
            "conditions": 4,
            "environments": 5,
            "constant": { "transition_nll": 2.75, "rollout_error": 1.9 },
            "flat_mlp": { "transition_nll": 1.8, "rollout_error": 1.4 },
            "tiny_cnn": { "transition_nll": 1.8, "rollout_error": 1.4 }
        },
        "action_support": {
            "sample_count": 50_000,
            "min_left": -1.0,
            "max_left": 1.0,
            "min_right": -0.5,
            "max_right": 0.5,
            "min_effective_forward": -0.75,
            "max_effective_forward": 0.75,
            "min_effective_turn": -0.5,
            "max_effective_turn": 0.5,
            "min_speed_scale": 0.0,
            "max_speed_scale": 1.0,
            "min_delta_seconds": 0.05,
            "max_delta_seconds": 0.4
        },
        "grounding_geometry": { "width": 64, "height": 64, "resolution_m": 0.05 },
        "status": "candidate"
    })
}

/// Publish a generation pointer the way a previous promotion left one.
fn write_generation(root: &Path, generation: u64) -> PathBuf {
    let pointer = root.join("active-generation.json");
    let body = serde_json::json!({
        "schema_version": "qualia.jepa-generation.v1",
        "generation": generation,
        "checkpoint_dir": root.join("checkpoints").join("cnn-accepted").display().to_string(),
        "checkpoint_id": "cnn-accepted",
        "weights_sha256": "ab".repeat(32),
        "mode": "observe-only",
        "approved_observe_only": true,
    });
    fs::write(&pointer, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    pointer
}

/// Step 31's test: a candidate whose report fails a gate never becomes a
/// generation, and the pointer every reader sees does not move.
#[tokio::test]
async fn promotion_requires_gate_pass() {
    let temp = tempfile::tempdir().unwrap();
    let registry = CandidateRegistry::open(temp.path().join("registry.turso"))
        .await
        .unwrap();

    // The generation a previous promotion published is on disk, and the braid
    // already shows it.
    let generation_file = write_generation(temp.path(), 3);
    let before = fs::read(&generation_file).unwrap();
    let mut braid = BraidState {
        generation: 3,
        last_promotion_ns: 111,
        ..Default::default()
    };

    // The candidate's held-out numbers did not clear the baseline gate. The
    // dataset path is never reached: the checkpoint's own evidence gate refuses
    // the candidate first.
    let candidate = temp.path().join("cnn-regressed");
    fs::create_dir_all(&candidate).unwrap();
    fs::write(
        candidate.join("manifest.json"),
        serde_json::to_vec(&failing_candidate_manifest()).unwrap(),
    )
    .unwrap();

    let outcome = verify_and_promote(
        &registry,
        &candidate,
        temp.path().join("dataset.json"),
        &generation_file,
        qualia_jepa_registry::now_ms(),
    )
    .await;

    match &outcome {
        PromotionOutcome::RolledBack { generation, reason } => {
            assert_eq!(*generation, 3, "the pointer still names generation 3");
            assert!(!reason.is_empty(), "a refusal carries the gate's reason");
        }
        PromotionOutcome::Accepted { .. } => {
            panic!("a candidate below the baseline was promoted")
        }
    }
    assert_eq!(
        outcome.generation(),
        3,
        "the outcome names the generation that stayed current"
    );

    // The refusal is observable two ways: the registry did not move the
    // pointer, and the event the braid folds does not move its view either.
    assert_eq!(
        fs::read(&generation_file).unwrap(),
        before,
        "the generation pointer is unchanged"
    );
    observe(&mut braid, &outcome.event()).unwrap();
    assert_eq!(
        braid.generation, 3,
        "the braid still shows the generation the pointer names"
    );
    assert_eq!(braid.last_promotion_ns, 111, "a refusal is not a promotion");

    // And nothing was admitted: a candidate the gate refused is not in the
    // registry for a later promotion to find.
    assert_eq!(registry.status(None).await.unwrap().candidate_count, 0);
}

/// The agent's dial on the prior's coupling (T30, #46): the scale a belief
/// layer is handed, clamped to the floor and the ceiling.
#[test]
fn coupling_scale_is_bounded() {
    // A run of failures carries the dial down one step at a time; the floor is
    // what stops a failure from ever coupling at zero.
    let mut scale = COUPLING_SCALE_DEFAULT;
    for _ in 0..40 {
        scale = next_coupling_scale(scale, false);
        assert!(
            scale >= COUPLING_SCALE_FLOOR,
            "a failure stepped the coupling below its floor: {scale}"
        );
    }
    assert_eq!(
        scale, COUPLING_SCALE_FLOOR,
        "forty failures settle on the floor, not on zero"
    );

    // And a run of successes climbs toward the ceiling; the clamp is what stops
    // it at infinity.
    let mut scale = COUPLING_SCALE_DEFAULT;
    for _ in 0..40 {
        scale = next_coupling_scale(scale, true);
        assert!(
            scale <= COUPLING_SCALE_CEILING,
            "a success stepped the coupling above its ceiling: {scale}"
        );
    }
    assert_eq!(
        scale, COUPLING_SCALE_CEILING,
        "forty successes settle on the ceiling, not on infinity"
    );

    // A reading that is already outside the range — a hand-edited manifest, or
    // a value that is not a number at all — cannot start a step outside it
    // either, from either direction.
    assert_eq!(next_coupling_scale(0.0, false), COUPLING_SCALE_FLOOR);
    assert_eq!(
        next_coupling_scale(f32::INFINITY, true),
        COUPLING_SCALE_CEILING
    );
    assert_eq!(
        next_coupling_scale(f32::NAN, false),
        COUPLING_SCALE_DEFAULT,
        "an unreadable dial is the default dial, not a NaN coupling"
    );
}
