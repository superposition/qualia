//! Step 31's caller: a seal in, a bounded job out, the promotion's event back.
//!
//! These drive the agent's improvement loop the way `/braid` does — a
//! `BraidEvent::EvidenceSealed` folded through the loop — with a runner that
//! writes no GPU work, and read what a later consumer reads: the job the agent
//! submitted, the registry's pointer file, and the braid's own state. Nothing
//! here inspects the loop's internals.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qualia_agent::braid::BraidRuntime;
use qualia_agent::config::PromotionConfig;
use qualia_agent::improvement::{ImprovementRuntime, Submission, TrainingRunner};
use qualia_braid::improvement::{PromotionOutcome, TrainingJob, TRAINING_JOB_DEADLINE_MS};
use qualia_braid::{observe, BraidEvent, BraidState};

/// The loop's paths, all inside one scratch root.
fn promotion_config(root: &Path) -> PromotionConfig {
    PromotionConfig {
        catalog: root.join("catalog.json"),
        dataset_dir: root.join("datasets"),
        checkpoint_dir: root.join("checkpoints"),
        backend: "cpu".to_string(),
        registry: root.join("registry.turso"),
        generation_file: root.join("active-generation.json"),
    }
}

/// A strand seals a segment.
fn sealed(digest: &str) -> BraidEvent {
    BraidEvent::EvidenceSealed {
        sha256: digest.to_string(),
    }
}

/// The runner a host with no device substitutes: it records the jobs the agent
/// submits and publishes no candidate evidence, so the registry's gate has
/// something to refuse.
#[derive(Default)]
struct RecordingRunner {
    jobs: Mutex<Vec<TrainingJob>>,
}

impl RecordingRunner {
    fn runs(&self) -> usize {
        self.jobs.lock().expect("runner jobs lock").len()
    }

    fn last(&self) -> TrainingJob {
        self.jobs
            .lock()
            .expect("runner jobs lock")
            .last()
            .cloned()
            .expect("the runner ran a job")
    }
}

impl TrainingRunner for RecordingRunner {
    fn run(&self, job: &TrainingJob) -> Result<PathBuf, String> {
        self.jobs
            .lock()
            .expect("runner jobs lock")
            .push(job.clone());
        // The manifest the dataset step would have written. No checkpoint
        // evidence follows it, so `register` refuses the candidate and the
        // promotion is a rollback — the outcome this test reads.
        Ok(job.dataset_dir.join(format!("{}.json", job.checkpoint_id)))
    }
}

/// Publish a generation pointer the way a previous promotion left one, and
/// answer its path and exact bytes.
fn write_generation(root: &Path, generation: u64) -> (PathBuf, Vec<u8>) {
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
    let bytes = serde_json::to_vec_pretty(&body).unwrap();
    std::fs::write(&pointer, &bytes).unwrap();
    (pointer, bytes)
}

/// One seal under a closed mission runs exactly one bounded job; a seal under an
/// open mission runs none; and the event the braid records is the outcome the
/// registry's gate produced.
#[tokio::test]
async fn a_seal_under_a_closed_mission_submits_one_bounded_job() {
    let temp = tempfile::tempdir().unwrap();
    let config = promotion_config(temp.path());
    let (_pointer_path, pointer_before) = write_generation(temp.path(), 3);
    let braid = BraidRuntime::new();
    let runner = Arc::new(RecordingRunner::default());
    let runtime = ImprovementRuntime::with_runner(braid.clone(), &config, runner.clone());
    let digest = "9f".repeat(32);

    // A seal while the session's mission is open waits: nothing trains under a
    // mission that has not closed.
    let open = BraidState {
        open_missions: 1,
        ..Default::default()
    };
    assert_eq!(runtime.on_event(&open, &sealed(&digest)), Submission::None);
    assert!(runtime.in_flight().is_none());

    // The same seal with the mission closed is one job, and submitting it runs
    // nothing by itself.
    let closed = BraidState::default();
    assert_eq!(
        runtime.on_event(&closed, &sealed(&digest)),
        Submission::Submitted
    );
    assert_eq!(runner.runs(), 0, "a submission starts no binary by itself");

    // A second seal while that job is in flight asks for a second job and is
    // refused: Step 31 trains one at a time.
    assert_eq!(
        runtime.on_event(&closed, &sealed(&digest)),
        Submission::Busy
    );
    assert_eq!(runner.runs(), 0, "a refused submission starts no binary");

    // The job the agent submits is the one the loop named for the sealed
    // digest, under the deadline the step names and the paths the config gave.
    let submitted = runtime.in_flight().expect("one job is in flight");
    assert_eq!(submitted.checkpoint_id, format!("cnn-{}", &digest[..12]));
    assert_eq!(
        submitted.deadline(),
        Duration::from_millis(TRAINING_JOB_DEADLINE_MS)
    );
    assert_eq!(submitted.catalog, config.catalog);
    assert_eq!(submitted.dataset_dir, config.dataset_dir);
    assert_eq!(submitted.checkpoint_dir, config.checkpoint_dir);
    assert_eq!(submitted.backend, "cpu");

    // Run it. The candidate carries no evidence, so the registry's gate refuses
    // it — a rollback, not a promotion.
    let outcome = runtime.run_in_flight().await.expect("the job ran");
    assert_eq!(runner.runs(), 1, "one submission ran exactly one job");
    assert_eq!(
        runner.last(),
        submitted,
        "the runner ran the job the agent submitted"
    );
    assert_eq!(
        runner.last().dataset_argv(),
        vec![
            "--catalog".to_string(),
            config.catalog.display().to_string(),
            "--output-dir".to_string(),
            config.dataset_dir.display().to_string(),
        ]
    );
    match &outcome {
        PromotionOutcome::RolledBack { generation, reason } => {
            assert_eq!(*generation, 3, "the pointer still names generation 3");
            assert!(!reason.is_empty(), "a refusal carries the gate's reason");
        }
        PromotionOutcome::Accepted { .. } => panic!("a candidate with no evidence was promoted"),
    }

    // The braid records exactly the event the outcome is, and the pointer the
    // registry left is byte-identical: a refusal publishes nothing.
    let mut expected = BraidState::default();
    observe(&mut expected, &outcome.event()).unwrap();
    assert_eq!(
        braid.snapshot(),
        expected,
        "the braid records the promotion outcome's own event"
    );
    assert_eq!(braid.view().generation, 3, "the pointer did not move");
    assert_eq!(
        braid.view().last_promotion_ns,
        0,
        "a refusal is not a promotion"
    );
    assert_eq!(
        std::fs::read(&config.generation_file).unwrap(),
        pointer_before,
        "the generation pointer is unchanged"
    );

    // The queue's one slot is free again: the next seal submits.
    assert_eq!(
        runtime.on_event(&closed, &sealed(&digest)),
        Submission::Submitted
    );
    assert_eq!(
        runner.runs(),
        1,
        "the freed slot did not run a second job yet"
    );
}

/// A digest that is not a sha256 is not evidence: no mission is open, but no
/// candidate is invented either.
#[tokio::test]
async fn a_malformed_digest_submits_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let config = promotion_config(temp.path());
    let braid = BraidRuntime::new();
    let runner = Arc::new(RecordingRunner::default());
    let runtime = ImprovementRuntime::with_runner(braid, &config, runner.clone());

    assert_eq!(
        runtime.on_event(
            &BraidState::default(),
            &BraidEvent::EvidenceSealed {
                sha256: "sealed".to_string(),
            },
        ),
        Submission::None
    );
    assert!(runtime.in_flight().is_none());
    assert_eq!(runner.runs(), 0);
}
