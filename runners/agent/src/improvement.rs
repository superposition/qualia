//! Step 31's caller: the agent in the loop.
//!
//! `qualia-braid`'s [`improvement`](qualia_braid::improvement) module is the
//! policy; this module is the edge that drives it. A strand reports a
//! [`BraidEvent::EvidenceSealed`] over `/braid`, the braid folds it, and this
//! edge asks [`ImprovementLoop::on_event`] whether the seal asks for training.
//! The loop answers with one [`TrainingJob`] when the session's mission has
//! closed, and [`TrainingQueue`] brokers it: one job at a time, under the
//! 120 s deadline the step names, which is the discipline `runners/cuda-service`
//! runs planner work under.
//!
//! The two binaries a job names run through [`TrainingRunner`]. The agent's
//! [`ProcessRunner`] runs the real ones, bounded by the job's deadline; a host
//! with no device — or a test that must observe the loop without a GPU —
//! substitutes its own runner, which is the seam that keeps the submission
//! observable.
//!
//! When the trainer's evidence has landed, the candidate goes to
//! [`qualia_jepa_registry`] through `qualia-braid`'s `verify_and_promote`: both
//! of the registry's existing gates must pass before the generation pointer
//! moves, and the outcome's event is folded into the braid like any other
//! strand's report. The braid decides nothing. The fly can never promote
//! itself, because the gates run on held-out splits, not on the mission's own
//! outcome — nothing here reads a mission result either.
//!
//! The bound the queue enforces is also the caller's: a seal that arrives while
//! a job is in flight is refused rather than queued, and a runner that fails
//! publishes no promotion event — there is no report to verify.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qualia_braid::improvement::{
    verify_and_promote, ImprovementLoop, PromotionOutcome, QueueError, TrainingJob, TrainingQueue,
    DATASET_BINARY, TRAINING_BINARY,
};
use qualia_braid::{BraidEvent, BraidState};
use qualia_jepa_registry::CandidateRegistry;

use crate::braid::BraidRuntime;
use crate::config::PromotionConfig;
use crate::AppState;

/// How often the supervisor looks for a job to run.
const SUPERVISOR_INTERVAL_MS: u64 = 200;

/// What the loop did with one reported event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submission {
    /// One training job was submitted to the single-flight queue.
    Submitted,
    /// A job was asked for, but one is already in flight: Step 31 trains one at
    /// a time, so the second seal waits rather than starting a second job.
    Busy,
    /// The event asked for no job.
    None,
}

/// Runs a job's two binaries.
///
/// The agent's [`ProcessRunner`] is the real one. Substituting another is how a
/// caller observes the loop without a GPU: the runner is handed exactly the
/// [`TrainingJob`] the loop built, and answers the manifest path the dataset
/// step wrote.
pub trait TrainingRunner: Send + Sync {
    /// Build the dataset manifest and train the candidate, answering the
    /// manifest path `qualia-jepa-train` was pointed at.
    fn run(&self, job: &TrainingJob) -> Result<PathBuf, String>;
}

/// Runs `qualia-jepa-dataset` then `qualia-jepa-train`, each under the job's
/// deadline.
pub struct ProcessRunner;

impl TrainingRunner for ProcessRunner {
    fn run(&self, job: &TrainingJob) -> Result<PathBuf, String> {
        let output = run_bounded(DATASET_BINARY, &job.dataset_argv(), job.deadline())?;
        // The manifest's path is the first line the dataset binary prints; it
        // is not knowable before the binary runs.
        let manifest = PathBuf::from(output.lines().next().unwrap_or_default().trim());
        if manifest.as_os_str().is_empty() {
            return Err(format!("{DATASET_BINARY} printed no manifest path"));
        }
        run_bounded(
            TRAINING_BINARY,
            &job.training_argv(&manifest),
            job.deadline(),
        )?;
        Ok(manifest)
    }
}

/// Run one binary, bounded by `deadline`.
///
/// The child's output is small (one manifest path), so the poll reads it after
/// the exit; a child that outlives its bound is killed rather than waited on
/// forever.
fn run_bounded(program: &str, argv: &[String], deadline: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn {program}: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("collect {program} output: {error}"))?;
                if !status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("{program} exited {}: {}", status, stderr.trim()));
                }
                return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
            }
            Ok(None) if started.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} exceeded its {} ms deadline",
                    deadline.as_millis()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(format!("wait for {program}: {error}")),
        }
    }
}

/// The loop's caller, as this process holds it.
///
/// It owns the one [`TrainingQueue`] this process brokers jobs through and the
/// braid it reports the promotion outcome to. Cheap to clone: every field is
/// behind an `Arc` or is itself a handle.
#[derive(Clone)]
pub struct ImprovementRuntime {
    braid: BraidRuntime,
    policy: ImprovementLoop,
    queue: Arc<Mutex<TrainingQueue>>,
    runner: Arc<dyn TrainingRunner>,
    registry: PathBuf,
    generation_file: PathBuf,
}

impl ImprovementRuntime {
    /// The runtime that runs the real binaries where `config` points.
    pub fn new(braid: BraidRuntime, config: &PromotionConfig) -> Self {
        Self::with_runner(braid, config, Arc::new(ProcessRunner))
    }

    /// The same runtime over a substituted runner.
    pub fn with_runner(
        braid: BraidRuntime,
        config: &PromotionConfig,
        runner: Arc<dyn TrainingRunner>,
    ) -> Self {
        Self {
            braid,
            policy: ImprovementLoop::new(config.improvement()),
            queue: Arc::new(Mutex::new(TrainingQueue::new())),
            runner,
            registry: config.registry.clone(),
            generation_file: config.generation_file.clone(),
        }
    }

    /// Fold one strand event through the loop and submit the job it asks for.
    ///
    /// Only a seal under a closed mission asks for one, and only while none is
    /// in flight. The queue is the broker, so a second ask is refused rather
    /// than run alongside the first.
    pub fn on_event(&self, state: &BraidState, event: &BraidEvent) -> Submission {
        let Some(job) = self.policy.on_event(state, event) else {
            return Submission::None;
        };
        match self.queue.lock().expect("training queue lock").submit(job) {
            Ok(()) => Submission::Submitted,
            Err(QueueError::Busy) => Submission::Busy,
        }
    }

    /// The job in flight, if any.
    pub fn in_flight(&self) -> Option<TrainingJob> {
        self.queue
            .lock()
            .expect("training queue lock")
            .in_flight()
            .cloned()
    }

    /// Run the in-flight job's binaries, hand the candidate to the registry's
    /// gates, and fold the promotion's event into the braid.
    ///
    /// A job whose binaries fail publishes nothing: there is no training report
    /// to verify. A job whose candidate a gate refuses is a
    /// [`PromotionOutcome::RolledBack`] — a real answer, not an error — and the
    /// braid records it exactly as [`verify_and_promote`] reports it.
    pub async fn run_in_flight(&self) -> Option<PromotionOutcome> {
        let job = self.queue.lock().expect("training queue lock").finish()?;
        let runner = Arc::clone(&self.runner);
        let submitted = job.clone();
        let manifest = match tokio::task::spawn_blocking(move || runner.run(&submitted)).await {
            Ok(Ok(manifest)) => manifest,
            Ok(Err(error)) => {
                eprintln!(
                    "qualia-agent: training job {} failed: {error}",
                    job.checkpoint_id
                );
                return None;
            }
            Err(error) => {
                eprintln!(
                    "qualia-agent: training job {} did not finish: {error}",
                    job.checkpoint_id
                );
                return None;
            }
        };
        let registry = match CandidateRegistry::open(&self.registry).await {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!(
                    "qualia-agent: cannot open the promotion registry {}: {error}",
                    self.registry.display()
                );
                return None;
            }
        };
        let outcome = verify_and_promote(
            &registry,
            &job.checkpoint_dir,
            &manifest,
            &self.generation_file,
            crate::now_ms(),
        )
        .await;
        match &outcome {
            PromotionOutcome::Accepted {
                generation,
                checkpoint_id,
            } => eprintln!("qualia-agent: promoted {checkpoint_id} as generation {generation}"),
            PromotionOutcome::RolledBack { generation, reason } => {
                eprintln!(
                    "qualia-agent: candidate {} rolled back, generation {generation} stays: {reason}",
                    job.checkpoint_id
                )
            }
        }
        // The braid folds the outcome's event and decides nothing. The JEPA
        // runtime reports the pointer's own move when it observes one; the
        // braid folds re-deliveries of the same generation without a second
        // generation, and that reconciliation belongs to the reporting strand.
        self.braid.observe(outcome.event());
        Some(outcome)
    }
}

/// Start the supervisor loop that runs submitted jobs, one at a time.
///
/// The mission broker's supervisor is started the same way. A pass with nothing
/// in flight sleeps rather than spins, and the loop holds the job's own
/// deadline, not a second one.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        loop {
            state.improvement.run_in_flight().await;
            tokio::time::sleep(Duration::from_millis(SUPERVISOR_INTERVAL_MS)).await;
        }
    });
}
