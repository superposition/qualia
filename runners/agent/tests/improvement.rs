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

/// A runner that publishes a candidate manifest the way the real trainer does —
/// or, for the counterfactual, at the output root — so the test can read which
/// directory the registry's gate was actually handed.
struct PublishingRunner {
    jobs: Mutex<Vec<TrainingJob>>,
    /// `true`: write `<checkpoint_dir>/<checkpoint_id>/manifest.json` (what
    /// `write_candidate_checkpoint` does). `false`: write the root only.
    nested: bool,
}

impl PublishingRunner {
    fn new(nested: bool) -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            nested,
        }
    }
}

impl TrainingRunner for PublishingRunner {
    fn run(&self, job: &TrainingJob) -> Result<PathBuf, String> {
        self.jobs
            .lock()
            .expect("runner jobs lock")
            .push(job.clone());
        let candidate = if self.nested {
            job.checkpoint_dir.join(&job.checkpoint_id)
        } else {
            job.checkpoint_dir.clone()
        };
        std::fs::create_dir_all(&candidate).expect("candidate dir");
        // A manifest that exists but does not parse as a checkpoint. The gate's
        // answer then says which file it read: the manifest's own parse error if
        // it was handed the candidate directory, a read error if it was not.
        std::fs::write(candidate.join("manifest.json"), b"{}").expect("manifest");
        Ok(job.dataset_dir.join(format!("{}.json", job.checkpoint_id)))
    }
}

/// A runner that publishes nothing, tells the test it has started, and blocks
/// until released — so a seal can be offered while the job's binaries run.
struct BlockingRunner {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl TrainingRunner for BlockingRunner {
    fn run(&self, job: &TrainingJob) -> Result<PathBuf, String> {
        if let Some(started) = self.started.lock().expect("started lock").take() {
            let _ = started.send(());
        }
        let _ = self.release.lock().expect("release lock").recv();
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

/// The gate is handed the per-candidate directory the trainer writes, not the
/// output root: with the candidate published the way `write_candidate_checkpoint`
/// nests it, the refusal is the candidate's own manifest, not a missing file.
#[tokio::test]
async fn the_gate_is_handed_the_nested_candidate_directory() {
    let temp = tempfile::tempdir().unwrap();
    let config = promotion_config(temp.path());
    let (_pointer, _before) = write_generation(temp.path(), 3);
    let braid = BraidRuntime::new();
    let runtime =
        ImprovementRuntime::with_runner(braid, &config, Arc::new(PublishingRunner::new(true)));
    let digest = "5c".repeat(32);
    let candidate = config.checkpoint_dir.join(format!("cnn-{}", &digest[..12]));

    assert_eq!(
        runtime.on_event(&BraidState::default(), &sealed(&digest)),
        Submission::Submitted
    );
    let outcome = runtime.run_in_flight().await.expect("the job ran");
    assert!(
        candidate.join("manifest.json").is_file(),
        "the runner nested the candidate under the output root"
    );
    match outcome {
        PromotionOutcome::RolledBack { generation, reason } => {
            assert_eq!(generation, 3, "the gate did not publish");
            assert!(
                reason.contains("missing field"),
                "the gate read the candidate's own manifest and failed its content: {reason}"
            );
        }
        PromotionOutcome::Accepted { .. } => panic!("a manifest of {{}} was promoted"),
    }
}

/// The counterfactual the defect would produce: a manifest at the output root
/// only is not accepted, because the gate reads the per-candidate directory.
#[tokio::test]
async fn a_root_only_candidate_is_not_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let config = promotion_config(temp.path());
    let (_pointer, _before) = write_generation(temp.path(), 3);
    let braid = BraidRuntime::new();
    let runtime =
        ImprovementRuntime::with_runner(braid, &config, Arc::new(PublishingRunner::new(false)));
    let digest = "7e".repeat(32);

    assert_eq!(
        runtime.on_event(&BraidState::default(), &sealed(&digest)),
        Submission::Submitted
    );
    let outcome = runtime.run_in_flight().await.expect("the job ran");
    assert!(
        config.checkpoint_dir.join("manifest.json").is_file(),
        "the runner published at the root, the way a mis-joined caller would read it"
    );
    match outcome {
        PromotionOutcome::RolledBack { generation, reason } => {
            assert_eq!(generation, 3);
            assert!(
                !reason.contains("missing field"),
                "a root-only candidate was read as if it were the candidate directory: {reason}"
            );
        }
        PromotionOutcome::Accepted { .. } => panic!("a root-only candidate was promoted"),
    }
}

/// The queue holds the job for the whole run, so a seal that arrives while the
/// binaries are running is refused, not buffered behind the running job.
#[tokio::test]
async fn a_seal_while_the_job_runs_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let config = promotion_config(temp.path());
    let (_pointer, _before) = write_generation(temp.path(), 3);
    let braid = BraidRuntime::new();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let runner = Arc::new(BlockingRunner {
        started: Mutex::new(Some(started_tx)),
        release: Mutex::new(release_rx),
    });
    let runtime = Arc::new(ImprovementRuntime::with_runner(braid, &config, runner));
    let closed = BraidState::default();
    let digest = "3a".repeat(32);
    let checkpoint_id = format!("cnn-{}", &digest[..12]);

    assert_eq!(
        runtime.on_event(&closed, &sealed(&digest)),
        Submission::Submitted
    );
    let running = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move { runtime.run_in_flight().await })
    };
    started_rx.await.expect("the runner started");

    // The binaries are running and the job is still the queue's: a seal now is
    // refused, and the queued job is unchanged.
    assert_eq!(
        runtime.on_event(&closed, &sealed(&digest)),
        Submission::Busy
    );
    assert_eq!(
        runtime.in_flight().map(|job| job.checkpoint_id),
        Some(checkpoint_id.clone())
    );

    release_tx.send(()).expect("release the runner");
    assert!(
        running.await.expect("the run task").is_some(),
        "the job's run produced an outcome"
    );
    assert!(
        runtime.in_flight().is_none(),
        "the slot is freed by the run's end"
    );
    assert_eq!(
        runtime.on_event(&closed, &sealed(&digest)),
        Submission::Submitted
    );
}
