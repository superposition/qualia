//! Step 31: the agent in the loop — sealed evidence in, a gated generation out.
//!
//! The loop is the policy the agent drives between the braid's events and the
//! registry's gates, and it decides nothing on its own:
//!
//! - [`ImprovementLoop::on_event`] turns a sealed segment into the
//!   [`TrainingJob`] that follows it. The `qualia-jepa-dataset` binary builds
//!   the manifest from the sealed catalog and `qualia-jepa-train` trains the
//!   candidate; [`TrainingJob`] carries both invocations' argv, because the
//!   command a binary is run with is emitted interface (D-008/D-009).
//! - [`TrainingQueue`] is the bound the submission runs under: one job at a
//!   time, under [`TRAINING_JOB_DEADLINE_MS`] — the discipline
//!   `runners/cuda-service` runs planner work under.
//! - [`verify_and_promote`] hands the trained candidate to
//!   `qualia-jepa-registry` and publishes it only when both of the registry's
//!   existing gates pass. A refusal is a [`PromotionOutcome::RolledBack`], and
//!   the generation pointer the registry left on disk is the pointer every
//!   reader still sees.
//!
//! The braid records whichever happened and decides nothing: the outcome's
//! [`event`](PromotionOutcome::event) is folded by [`observe`](crate::observe)
//! like any other strand's report, and the promotion's durable record is the
//! registry's own pointer write. The generation the braid shows is the
//! generation that write published.
//!
//! The fly can never promote itself. Promotion needs the gates' evidence, and
//! both gates run on held-out splits — the dataset's quotas and the training
//! report's held-out numbers — never on the mission's own outcome, which is why
//! nothing here reads a mission result.

use crate::{BraidEvent, BraidState};
use qualia_jepa_registry::{CandidateRegistry, GenerationPointer, DEFAULT_MAX_REPORT_AGE_MS};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The deadline a training job is submitted under (Step 31): 120 s.
pub const TRAINING_JOB_DEADLINE_MS: u64 = 120_000;

/// The existing binary that builds the immutable dataset manifest.
pub const DATASET_BINARY: &str = "qualia-jepa-dataset";

/// The existing binary that trains one candidate and writes its report.
pub const TRAINING_BINARY: &str = "qualia-jepa-train";

/// The prefix the loop gives a candidate; the sealed digest follows it.
pub const CHECKPOINT_ID_PREFIX: &str = "cnn-";

/// Hex characters of the sealed digest a candidate id carries.
pub const CHECKPOINT_ID_DIGEST_CHARS: usize = 12;

/// Where the loop's jobs read and write, as the agent configures them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImprovementConfig {
    /// The catalog of sealed session evidence the dataset binary reads.
    pub catalog: PathBuf,
    /// The directory the dataset binary writes the immutable manifest under.
    pub dataset_dir: PathBuf,
    /// The directory `qualia-jepa-train` writes the checkpoint and report under.
    pub checkpoint_dir: PathBuf,
    /// The trainer backend: `cpu`, `metal` or `cuda`.
    pub backend: String,
}

/// One training submission: the two binaries' invocations and their bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainingJob {
    /// The immutable checkpoint id the candidate is registered under.
    pub checkpoint_id: String,
    /// The catalog of sealed evidence the dataset binary reads.
    pub catalog: PathBuf,
    /// Where the dataset binary writes the manifest.
    pub dataset_dir: PathBuf,
    /// Where the trainer writes the checkpoint and its report.
    pub checkpoint_dir: PathBuf,
    /// The trainer backend.
    pub backend: String,
}

impl TrainingJob {
    /// The bound every job runs under: 120 s, one at a time.
    pub fn deadline(&self) -> Duration {
        Duration::from_millis(TRAINING_JOB_DEADLINE_MS)
    }

    /// The `qualia-jepa-dataset` invocation that builds the manifest. The
    /// manifest's own path is not known until the binary prints it, so the
    /// training step takes it as an argument.
    pub fn dataset_argv(&self) -> Vec<String> {
        vec![
            "--catalog".to_string(),
            self.catalog.display().to_string(),
            "--output-dir".to_string(),
            self.dataset_dir.display().to_string(),
        ]
    }

    /// The `qualia-jepa-train` invocation that trains the candidate, against
    /// the manifest the dataset step wrote.
    pub fn training_argv(&self, manifest: impl AsRef<Path>) -> Vec<String> {
        vec![
            "--manifest".to_string(),
            manifest.as_ref().display().to_string(),
            "--checkpoint-id".to_string(),
            self.checkpoint_id.clone(),
            "--output-dir".to_string(),
            self.checkpoint_dir.display().to_string(),
            "--backend".to_string(),
            self.backend.clone(),
        ]
    }
}

/// Why a job could not be brokered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// A job is already in flight; Step 31 trains one at a time.
    Busy,
}

impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueueError::Busy => write!(f, "a training job is already in flight"),
        }
    }
}

/// The single-flight queue a training job is submitted on.
///
/// `runners/cuda-service` runs one planner job at a time, and a training job
/// takes the whole device for its two binaries, so the loop submits the same
/// way: [`submit`](TrainingQueue::submit) refuses while a job is in flight
/// rather than starting a second one, and [`finish`](TrainingQueue::finish)
/// retires the one that ran.
#[derive(Debug, Default)]
pub struct TrainingQueue {
    in_flight: Option<TrainingJob>,
}

impl TrainingQueue {
    /// An empty queue: nothing has been submitted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Submit `job`, or refuse while one is already in flight.
    pub fn submit(&mut self, job: TrainingJob) -> Result<(), QueueError> {
        if self.in_flight.is_some() {
            return Err(QueueError::Busy);
        }
        self.in_flight = Some(job);
        Ok(())
    }

    /// The job in flight, if any.
    pub fn in_flight(&self) -> Option<&TrainingJob> {
        self.in_flight.as_ref()
    }

    /// Retire the in-flight job; its report is on disk by now.
    pub fn finish(&mut self) -> Option<TrainingJob> {
        self.in_flight.take()
    }
}

/// What the registry's gates decided, as the braid records it.
///
/// The two variants are the two events the braid knows (Step 31): a generation
/// published, or a candidate that did not become one. The braid decides nothing
/// — [`event`](PromotionOutcome::event) is the report the caller folds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionOutcome {
    /// Both gates passed and the registry published `generation`.
    Accepted {
        generation: u64,
        checkpoint_id: String,
    },
    /// A gate refused. The generation pointer did not move, so `generation` is
    /// the one every reader still sees, and `reason` is what the gate said.
    RolledBack { generation: u64, reason: String },
}

impl PromotionOutcome {
    /// The braid event this outcome is.
    pub fn event(&self) -> BraidEvent {
        match self {
            PromotionOutcome::Accepted { generation, .. } => {
                BraidEvent::PromotionAccepted {
                    generation: *generation,
                }
            }
            PromotionOutcome::RolledBack { generation, reason } => {
                BraidEvent::PromotionRolledBack {
                    generation: *generation,
                    reason: reason.clone(),
                }
            }
        }
    }

    /// The generation the outcome leaves current.
    pub fn generation(&self) -> u64 {
        match self {
            PromotionOutcome::Accepted { generation, .. }
            | PromotionOutcome::RolledBack { generation, .. } => *generation,
        }
    }
}

/// The loop's policy: which events ask for a training job.
#[derive(Debug, Clone)]
pub struct ImprovementLoop {
    config: ImprovementConfig,
}

impl ImprovementLoop {
    /// A loop configured to read and write where the agent says.
    pub fn new(config: ImprovementConfig) -> Self {
        Self { config }
    }

    /// The job `event` asks for, if it asks for one.
    ///
    /// Only a seal does. A [`BraidEvent::EvidenceSealed`] whose digest is a
    /// well-formed sha256 asks for training when the braid has no mission open
    /// — which is what "the session's mission has closed" means to the fold —
    /// and waits, asking for nothing, while one is. A mission that closes
    /// without a seal asks for nothing either: there is no evidence in it to
    /// train on. The loop keeps no queue of its own beyond the single job
    /// [`TrainingQueue`] brokers, and it does not deduplicate: the candidate id
    /// is the sealed digest's, so a re-delivered seal names the candidate the
    /// registry already knows.
    pub fn on_event(&self, state: &BraidState, event: &BraidEvent) -> Option<TrainingJob> {
        let BraidEvent::EvidenceSealed { sha256 } = event else {
            return None;
        };
        let digest = sealed_digest(sha256)?;
        (state.open_missions == 0).then(|| TrainingJob {
            checkpoint_id: format!(
                "{CHECKPOINT_ID_PREFIX}{}",
                &digest[..CHECKPOINT_ID_DIGEST_CHARS]
            ),
            catalog: self.config.catalog.clone(),
            dataset_dir: self.config.dataset_dir.clone(),
            checkpoint_dir: self.config.checkpoint_dir.clone(),
            backend: self.config.backend.clone(),
        })
    }
}

/// The sealed digest a candidate id can be built from: exactly 64 hex
/// characters, the sha256 the evidence writer seals with. A shorter or non-hex
/// digest is not evidence the loop can name a candidate for, so it asks for no
/// job rather than inventing an id.
fn sealed_digest(sha256: &str) -> Option<&str> {
    (sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(sha256)
}

/// Ask the registry to verify a trained candidate and promote it only when both
/// of its existing gates pass.
///
/// The first gate is `register`: the checkpoint manifest, the safetensors
/// weights, the training report and the dataset manifest are re-read from disk,
/// re-digested and re-derived, and a candidate that does not reproduce from its
/// own evidence — or whose report fails its held-out baseline gate — is refused
/// before anything is stored. The second is `promote`: the verified candidate
/// is refused if it predicts worse than the generation it would replace, or if
/// its report has gone stale. Only after both does the registry rename the
/// generation pointer into place, atomically, and return the pointer it
/// published.
///
/// A refusal is not an error to the loop: it is the
/// [`PromotionOutcome::RolledBack`] the gate exists to produce, and the pointer
/// file is exactly where it was. Nothing is published on that path, so the
/// generation the caller folds into the braid is the generation already current.
pub async fn verify_and_promote(
    registry: &CandidateRegistry,
    checkpoint_dir: impl AsRef<Path>,
    dataset_manifest: impl AsRef<Path>,
    generation_file: impl AsRef<Path>,
    now_ms: u128,
) -> PromotionOutcome {
    let generation_file = generation_file.as_ref();
    let current = generation_on_file(generation_file);
    let record = match registry
        .register(
            checkpoint_dir,
            dataset_manifest,
            now_ms,
            DEFAULT_MAX_REPORT_AGE_MS,
        )
        .await
    {
        Ok(record) => record,
        Err(error) => {
            return PromotionOutcome::RolledBack {
                generation: current,
                reason: error.to_string(),
            }
        }
    };
    match registry
        .promote(&record.checkpoint_id, generation_file, now_ms)
        .await
    {
        Ok(pointer) => PromotionOutcome::Accepted {
            generation: pointer.generation,
            checkpoint_id: pointer.checkpoint_id,
        },
        Err(error) => PromotionOutcome::RolledBack {
            generation: current,
            reason: error.to_string(),
        },
    }
}

/// The generation the pointer file currently names, or `0` when it names none.
///
/// The loop needs the number only on the refusal path, and the registry is the
/// pointer's only writer, so a file that does not parse as a pointer is treated
/// as no generation rather than as a reason to guess one.
fn generation_on_file(pointer_file: &Path) -> u64 {
    std::fs::read(pointer_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<GenerationPointer>(&bytes).ok())
        .map(|pointer| pointer.generation)
        .unwrap_or(0)
}
