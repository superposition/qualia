//! Candidate admission and atomic generation management for the Qualia JEPA
//! runtime.
//!
//! The registry is the only component allowed to approve an immutable JEPA
//! checkpoint. It re-reads the checkpoint manifest, the safetensors weights,
//! the training report and the dataset manifest from disk, re-derives their
//! digests, and refuses anything that does not reproduce from that evidence.
//! Turso holds the admitted candidates, the 12-dimensional metric signature
//! used for similarity search, and the generation journal.
//!
//! Activation is a three-step journal: a `prepared` row, an atomically renamed
//! pointer file, then the `active` row. A crash between the last two steps is
//! repaired by [`CandidateRegistry::reconcile`]. Health failures never rewind
//! the counter: they publish a new generation that points at the previous
//! accepted checkpoint.

use anyhow::{anyhow, bail, Context, Result};
use qualia_jepa_dataset::{
    build_manifest, manifest_digest, validate_dataset_promotion_gate, DatasetManifest, DatasetSplit,
};
use qualia_jepa_model::{
    measured_action_support, validate_checkpoint_weights, CheckpointManifest, TrainingReport,
    ARCHITECTURE_ID, DEFAULT_MAX_TRAINING_REPORT_AGE_MS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use turso::{params, Builder, Connection};

/// Schema tag carried by [`RegistryStatus`].
pub const REGISTRY_SCHEMA: &str = "qualia.jepa-registry.v1";
/// Schema tag every generation pointer file carries.
pub const GENERATION_SCHEMA: &str = "qualia.jepa-generation.v1";
/// Default ceiling on how old a training report may be at admission.
pub const DEFAULT_MAX_REPORT_AGE_MS: u128 = DEFAULT_MAX_TRAINING_REPORT_AGE_MS;

/// Metric signature dimension stored in the native vector column.
const SIGNATURE_DIM: usize = 12;

const DATASET_SCHEMA: &str = "qualia.jepa-dataset.v3";
const CHECKPOINT_SCHEMA: &str = "qualia.jepa-checkpoint.v1";
const OBSERVE_ONLY_MODE: &str = "observe-only";

const CANDIDATE_ELIGIBLE: &str = "eligible";
const CANDIDATE_ACCEPTED: &str = "accepted";
const CANDIDATE_ACTIVE: &str = "active";

const GENERATION_PREPARED: &str = "prepared";
const GENERATION_ACTIVE: &str = "active";
const GENERATION_SUPERSEDED: &str = "superseded";
const HEALTH_PENDING: &str = "pending";
const HEALTH_PASSED: &str = "passed";
const HEALTH_FAILED: &str = "failed";

const REASON_CATALOGUE: &str = "promotion";

/// The pointer file the observe-only runner reads to find its checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPointer {
    pub schema_version: String,
    pub generation: u64,
    pub checkpoint_dir: String,
    pub checkpoint_id: String,
    pub weights_sha256: String,
    pub mode: String,
    pub approved_observe_only: bool,
}

impl GenerationPointer {
    /// Reject a pointer that is incomplete, unapproved, or not observe-only.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != GENERATION_SCHEMA
            || self.generation == 0
            || self.checkpoint_dir.trim().is_empty()
            || !safe_id(&self.checkpoint_id)
            || !sha256_text(&self.weights_sha256)
            || self.mode != OBSERVE_ONLY_MODE
            || !self.approved_observe_only
        {
            bail!("invalid JEPA generation pointer");
        }
        Ok(())
    }
}

/// One admitted candidate, as stored in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub checkpoint_id: String,
    pub checkpoint_dir: String,
    pub dataset_digest: String,
    pub weights_sha256: String,
    pub manifest_sha256: String,
    pub training_report_sha256: String,
    pub backend: String,
    pub status: String,
    pub report_created_at_ms: u128,
    pub registered_at_ms: u128,
    pub transition_nll: f64,
    pub rollout_error: f64,
    pub occupancy_iou: f64,
    pub occupancy_pr_auc: f64,
    pub clamp_fraction: f64,
    pub nonfinite_values: u64,
}

/// A verified candidate plus the evidence bytes that justify it.
#[derive(Debug, Clone)]
struct EvidenceCandidate {
    record: CandidateRecord,
    manifest_json: String,
    report_json: String,
    dataset_manifest_path: String,
    signature: [f32; SIGNATURE_DIM],
}

/// Registry-wide totals for operators and dashboards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryStatus {
    pub schema_version: String,
    pub active: Option<GenerationPointer>,
    pub candidate_count: u64,
    pub generation_count: u64,
}

/// Handle to one Turso registry database.
pub struct CandidateRegistry {
    connection: Connection,
    path: PathBuf,
}

impl CandidateRegistry {
    /// Open (or create) the registry, install the schema, and probe the native
    /// vector path the similarity search depends on.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        todo!()
    }

    /// The database file this handle owns.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Verify a checkpoint against all of its immutable evidence and admit it.
    pub async fn register(
        &self,
        checkpoint_dir: impl AsRef<Path>,
        dataset_manifest_path: impl AsRef<Path>,
        now_ms: u128,
        max_report_age_ms: u128,
    ) -> Result<CandidateRecord> {
        todo!()
    }

    /// Activate an already-admitted candidate as a new generation.
    pub async fn promote(
        &self,
        checkpoint_id: &str,
        generation_file: impl AsRef<Path>,
        now_ms: u128,
    ) -> Result<GenerationPointer> {
        todo!()
    }

    /// Finish an activation that a crash interrupted after the pointer rename.
    pub async fn reconcile(
        &self,
        generation_file: impl AsRef<Path>,
        now_ms: u128,
    ) -> Result<GenerationPointer> {
        todo!()
    }

    /// Record a soak result; a failure publishes a rollback generation.
    pub async fn record_health(
        &self,
        generation_file: impl AsRef<Path>,
        generation: u64,
        healthy: bool,
        now_ms: u128,
        reason: &str,
    ) -> Result<GenerationPointer> {
        todo!()
    }

    /// Publish a new generation pointing at the previous accepted checkpoint.
    pub async fn rollback(
        &self,
        generation_file: impl AsRef<Path>,
        now_ms: u128,
        reason: &str,
    ) -> Result<GenerationPointer> {
        todo!()
    }

    /// Look up one candidate by checkpoint id.
    pub async fn candidate(&self, checkpoint_id: &str) -> Result<Option<CandidateRecord>> {
        todo!()
    }

    /// Checkpoint ids nearest to `checkpoint_id` by cosine distance.
    pub async fn nearest_candidates(&self, checkpoint_id: &str, limit: u64) -> Result<Vec<String>> {
        todo!()
    }

    /// Totals, plus the active pointer when a generation file is given.
    pub async fn status(&self, generation_file: Option<&Path>) -> Result<RegistryStatus> {
        todo!()
    }

    /// Insert a fully verified candidate, refusing a predictive regression.
    async fn insert_verified(&self, candidate: &EvidenceCandidate) -> Result<()> {
        todo!()
    }

    /// Refuse a candidate that predictively regresses the active model.
    async fn reject_regression(&self, candidate: &CandidateRecord) -> Result<()> {
        todo!()
    }

    /// Commit the three-step activation journal for one candidate.
    async fn activate(
        &self,
        candidate: CandidateRecord,
        generation_file: &Path,
        now_ms: u128,
        rollback_of: Option<u64>,
        reason: &str,
    ) -> Result<GenerationPointer> {
        todo!()
    }

    /// Commit step three: mark the prepared generation active.
    async fn finish_activation(&self, pointer: &GenerationPointer, now_ms: u128) -> Result<()> {
        todo!()
    }

    /// The generation and checkpoint the registry currently considers active.
    async fn active_identity(&self) -> Result<(Option<u64>, Option<String>)> {
        todo!()
    }

    /// One past the highest generation ever journalled.
    async fn next_generation(&self) -> Result<u64> {
        todo!()
    }
}

/// Re-read and digest-verify a checkpoint, its report, and its dataset.
fn verify_candidate(
    checkpoint_dir: &Path,
    dataset_manifest_path: &Path,
    now_ms: u128,
    max_report_age_ms: u128,
) -> Result<EvidenceCandidate> {
    todo!()
}

/// Re-derive a dataset manifest and re-run its promotion quotas.
fn verify_dataset(manifest: &DatasetManifest, expected_digest: &str) -> Result<()> {
    todo!()
}

/// Recompute split counts, action support and step accounting from evidence.
fn validate_report_dataset_alignment(
    report: &TrainingReport,
    dataset: &DatasetManifest,
) -> Result<()> {
    todo!()
}

/// Rebuild a record from one `jepa_candidates` row.
fn candidate_from_row(row: turso::Row) -> Result<CandidateRecord> {
    todo!()
}

/// The metric signature stored beside a candidate.
fn report_signature(report: &TrainingReport) -> [f32; SIGNATURE_DIM] {
    todo!()
}

/// Commit or roll back the transaction `result` was produced inside.
async fn finish_transaction(connection: &Connection, result: Result<()>) -> Result<()> {
    todo!()
}

/// Count the rows of one registry table.
async fn count(connection: &Connection, table: &str) -> Result<u64> {
    todo!()
}

/// Build the pointer an activation publishes.
fn generation_pointer(generation: u64, candidate: &CandidateRecord) -> GenerationPointer {
    todo!()
}

/// Read and verify a generation pointer against its immutable checkpoint.
pub fn read_generation(path: &Path) -> Result<GenerationPointer> {
    todo!()
}

/// Write a generation pointer with an fsync and a rename.
pub fn write_generation_atomic(path: &Path, pointer: &GenerationPointer) -> Result<()> {
    todo!()
}

/// Whether a checkpoint id is a safe path component.
fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Whether a string is a lowercase-or-uppercase hex sha256.
fn sha256_text(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Hex sha256 of a byte slice.
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Narrow a millisecond timestamp into the SQLite integer range.
fn to_i64(value: u128) -> Result<i64> {
    i64::try_from(value).context("registry integer exceeds SQLite range")
}

/// Wall-clock milliseconds since the Unix epoch.
pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_jepa_dataset::{
        AppliedActionAggregate, DatasetAudit, DatasetConfig, EventRef, TransitionQuality,
        TransitionSample,
    };
    use qualia_jepa_model::evaluation::{CalibrationReport, EffectiveRankReport, OccupancyReport};
    use qualia_jepa_model::{
        ActionSupport, BaselineGate, GroundingGeometry, HeldOutMetrics, SplitEvaluation,
        TRAINING_REPORT_SCHEMA,
    };

    fn metrics(transition_nll: f64, rollout_error: f64) -> HeldOutMetrics {
        HeldOutMetrics {
            transition_nll,
            rollout_error,
        }
    }

    fn calibration() -> CalibrationReport {
        CalibrationReport {
            dimensions: 8_192,
            transition_nll: 0.45,
            mean_standardized_squared_residual: 1.02,
            coverage_50: 0.505,
            coverage_90: 0.902,
            coverage_95: 0.948,
            calibration_slope: 0.98,
            clamp_fraction: 0.002,
            nonfinite_values: 0,
            passes: true,
        }
    }

    fn occupancy() -> OccupancyReport {
        OccupancyReport {
            observed_cells: 6_400,
            occupied_cells: 1_280,
            occupied_prevalence: 0.2,
            intersection_over_union: 0.72,
            pr_auc: 0.81,
            trivial_iou: 0.2,
            trivial_pr_auc: 0.2,
            passes: true,
        }
    }

    fn support() -> ActionSupport {
        ActionSupport {
            sample_count: 50_000,
            min_left: -1.0,
            max_left: 1.0,
            min_right: -0.5,
            max_right: 0.5,
            min_effective_forward: -0.75,
            max_effective_forward: 0.75,
            min_effective_turn: -0.5,
            max_effective_turn: 0.5,
            min_speed_scale: 0.0,
            max_speed_scale: 1.0,
            min_delta_seconds: 0.05,
            max_delta_seconds: 0.4,
        }
    }

    fn geometry() -> GroundingGeometry {
        GroundingGeometry {
            width: 64,
            height: 64,
            resolution_m: 0.05,
        }
    }

    fn training_report(now: u128) -> TrainingReport {
        let held_out = SplitEvaluation {
            samples: 4_096,
            sessions: 4,
            constant: metrics(3.0, 2.0),
            flat_mlp: metrics(2.0, 1.5),
            tiny_cnn: metrics(1.0, 0.8),
            calibration: calibration(),
            occupancy: occupancy(),
        };
        TrainingReport {
            schema_version: TRAINING_REPORT_SCHEMA.to_string(),
            created_at_ms: now,
            checkpoint_id: "checkpoint-a".to_string(),
            dataset_digest: "d".repeat(64),
            backend: "metal".to_string(),
            seed: 42,
            epochs: 2,
            batch_size: 32,
            cnn_steps: 256,
            flat_steps: 256,
            skipped_singletons: 0,
            action_support: support(),
            grounding_geometry: geometry(),
            validation: held_out.clone(),
            test: held_out,
            effective_rank: EffectiveRankReport {
                sample_count: 4_096,
                dimensions: 256,
                effective_rank: 80.0,
                trace: 100.0,
                converged: true,
                sweeps: 12,
            },
            llm_priors_ablated: true,
            baseline_gate_passed: true,
            grounding_calibration_gate_passed: true,
            all_gates_passed: true,
        }
    }

    fn checkpoint_manifest() -> CheckpointManifest {
        CheckpointManifest {
            schema_version: CHECKPOINT_SCHEMA.to_string(),
            architecture_id: ARCHITECTURE_ID.to_string(),
            checkpoint_id: "checkpoint-a".to_string(),
            dataset_digest: "d".repeat(64),
            training_seed: 42,
            backend: "metal".to_string(),
            dtype: "F32".to_string(),
            target_encoder: "ema".to_string(),
            parameter_count: 10,
            weights_sha256: "a".repeat(64),
            training_report_path: "report.json".to_string(),
            training_report_sha256: "b".repeat(64),
            baseline_gate: BaselineGate {
                dataset_digest: "d".repeat(64),
                valid_transitions: 50_000,
                sessions: 12,
                conditions: 3,
                environments: 3,
                constant: metrics(3.0, 2.0),
                flat_mlp: metrics(2.0, 1.5),
                tiny_cnn: metrics(1.0, 0.8),
            },
            action_support: support(),
            grounding_geometry: geometry(),
            status: "candidate".to_string(),
        }
    }

    fn transition_sample(
        sample_id: String,
        session_id: &str,
        split: DatasetSplit,
        sequence: u64,
    ) -> TransitionSample {
        let observed = EventRef {
            topic: "camera".to_string(),
            entity: "guard".to_string(),
            source_sequence: sequence,
            timestamp_ns: 1_000_000_000 + sequence * 200_000_000,
        };
        let following = EventRef {
            source_sequence: sequence + 1,
            timestamp_ns: observed.timestamp_ns + 100_000_000,
            ..observed.clone()
        };
        TransitionSample {
            sample_id,
            session_id: session_id.to_string(),
            environment_id: "arena".to_string(),
            condition: "day".to_string(),
            split,
            mcap_sha256: "d".repeat(64),
            observation_camera: observed.clone(),
            observation_lidar: EventRef {
                topic: "lidar".to_string(),
                ..observed.clone()
            },
            observation_pose: EventRef {
                topic: "pose".to_string(),
                ..observed.clone()
            },
            target_camera: following.clone(),
            target_lidar: EventRef {
                topic: "lidar".to_string(),
                ..following.clone()
            },
            target_pose: EventRef {
                topic: "pose".to_string(),
                ..following
            },
            action: AppliedActionAggregate {
                source_sequences: vec![sequence],
                left: 0.0,
                right: 0.0,
                speed_scale: 1.0,
                interval_start_ns: observed.timestamp_ns,
                interval_end_ns: observed.timestamp_ns + 100_000_000,
                coverage: 1.0,
                safety_flags: 0,
            },
            quality: TransitionQuality {
                camera_luminance_mean: 0.5,
                camera_luminance_stddev: 0.1,
                target_camera_luminance_mean: 0.5,
                target_camera_luminance_stddev: 0.1,
                lidar_valid_fraction: 1.0,
                target_lidar_valid_fraction: 1.0,
                pose_confidence: 1.0,
                target_pose_confidence: 1.0,
                max_sensor_skew_ns: 0,
                action_coverage: 1.0,
                calibration_id: "calibration".to_string(),
            },
        }
    }

    /// A candidate whose checkpoint files exist on disk, so a pointer written
    /// for it can be verified the way the runner verifies one.
    fn staged_candidate(
        root: &Path,
        id: &str,
        transition_nll: f64,
        rollout_error: f64,
        now: u128,
    ) -> EvidenceCandidate {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let weights = format!("staged-weights-{id}").into_bytes();
        fs::write(dir.join("weights.safetensors"), &weights).unwrap();
        let weights_sha256 = sha256_hex(&weights);
        let manifest = CheckpointManifest {
            checkpoint_id: id.to_string(),
            weights_sha256: weights_sha256.clone(),
            training_report_path: format!("{id}-report.json"),
            ..checkpoint_manifest()
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        EvidenceCandidate {
            record: CandidateRecord {
                checkpoint_id: id.to_string(),
                checkpoint_dir: dir.display().to_string(),
                dataset_digest: "d".repeat(64),
                weights_sha256,
                manifest_sha256: "e".repeat(64),
                training_report_sha256: "b".repeat(64),
                backend: "metal".to_string(),
                status: CANDIDATE_ELIGIBLE.to_string(),
                report_created_at_ms: now,
                registered_at_ms: now,
                transition_nll,
                rollout_error,
                occupancy_iou: 0.68,
                occupancy_pr_auc: 0.79,
                clamp_fraction: 0.003,
                nonfinite_values: 0,
            },
            manifest_json: "{}".to_string(),
            report_json: "{}".to_string(),
            dataset_manifest_path: "evidence/dataset.json".to_string(),
            signature: [0.0; SIGNATURE_DIM],
        }
    }

    #[test]
    fn dataset_quota_metadata_without_evidence_is_refused() {
        let mut audit = DatasetAudit {
            valid_transitions: 50_000,
            sessions: 12,
            environments: 3,
            ..DatasetAudit::default()
        };
        audit.conditions.insert("day".to_string(), 20_000);
        audit.conditions.insert("dusk".to_string(), 18_000);
        audit.conditions.insert("night".to_string(), 12_000);
        audit.split_samples.insert(DatasetSplit::Train, 50_000);
        let mut dataset = DatasetManifest {
            schema_version: DATASET_SCHEMA.to_string(),
            digest: String::new(),
            config: DatasetConfig::default(),
            sources: Vec::new(),
            samples: Vec::new(),
            audit,
        };
        dataset.digest = manifest_digest(&dataset).unwrap();
        let error = verify_dataset(&dataset, &dataset.digest)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("close over"),
            "unexpected rejection reason: {error}"
        );
    }

    #[test]
    fn alignment_recomputes_split_counts_action_support_and_steps() {
        let mut samples = (0..4_096)
            .map(|index| {
                transition_sample(
                    format!("train-{index}"),
                    "train-session",
                    DatasetSplit::Train,
                    index + 1,
                )
            })
            .collect::<Vec<_>>();
        samples.push(transition_sample(
            "validation-a".to_string(),
            "validation-session-a",
            DatasetSplit::Validation,
            10_000,
        ));
        samples.push(transition_sample(
            "validation-b".to_string(),
            "validation-session-b",
            DatasetSplit::Validation,
            10_010,
        ));
        samples.push(transition_sample(
            "test-a".to_string(),
            "test-session-a",
            DatasetSplit::Test,
            10_020,
        ));
        let dataset = DatasetManifest {
            schema_version: DATASET_SCHEMA.to_string(),
            digest: String::new(),
            config: DatasetConfig::default(),
            sources: Vec::new(),
            samples,
            audit: DatasetAudit::default(),
        };

        let mut aligned = training_report(10_000);
        aligned.validation.samples = 2;
        aligned.validation.sessions = 2;
        aligned.test.samples = 1;
        aligned.test.sessions = 1;
        aligned.cnn_steps = 256;
        aligned.flat_steps = 256;
        aligned.skipped_singletons = 0;
        aligned.action_support = measured_action_support(&dataset).unwrap();
        aligned.grounding_geometry.resolution_m = dataset.config.grounding_resolution_m;
        validate_report_dataset_alignment(&aligned, &dataset).unwrap();

        let mut forged_sessions = aligned.clone();
        forged_sessions.validation.sessions = 1;
        assert!(validate_report_dataset_alignment(&forged_sessions, &dataset).is_err());

        let mut forged_envelope = aligned.clone();
        forged_envelope.action_support.max_left = 0.5;
        assert!(validate_report_dataset_alignment(&forged_envelope, &dataset).is_err());

        let mut forged_steps = aligned.clone();
        forged_steps.cnn_steps += 1;
        assert!(validate_report_dataset_alignment(&forged_steps, &dataset).is_err());
    }

    #[tokio::test]
    async fn register_refuses_weights_that_do_not_match_the_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let registry = CandidateRegistry::open(temp.path().join("registry.turso"))
            .await
            .unwrap();
        let dir = temp.path().join("checkpoint-tampered");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("weights.safetensors"), b"not the declared bytes").unwrap();
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&checkpoint_manifest()).unwrap(),
        )
        .unwrap();
        let error = registry
            .register(
                &dir,
                temp.path().join("dataset.json"),
                now_ms(),
                DEFAULT_MAX_REPORT_AGE_MS,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("weights digest"),
            "unexpected rejection reason: {error}"
        );
    }

    #[tokio::test]
    async fn promotion_health_failure_and_reconcile_publish_new_generations() {
        let temp = tempfile::tempdir().unwrap();
        let registry = CandidateRegistry::open(temp.path().join("registry.turso"))
            .await
            .unwrap();
        let now = now_ms();
        registry
            .insert_verified(&staged_candidate(temp.path(), "checkpoint-a", 1.0, 0.8, now))
            .await
            .unwrap();
        registry
            .insert_verified(&staged_candidate(temp.path(), "checkpoint-b", 0.9, 0.7, now))
            .await
            .unwrap();
        let pointer_file = temp.path().join("active-generation.json");

        let active_a = registry
            .promote("checkpoint-a", &pointer_file, now)
            .await
            .unwrap();
        assert_eq!(active_a.generation, 1);
        assert_eq!(active_a.checkpoint_id, "checkpoint-a");
        assert_eq!(active_a.mode, OBSERVE_ONLY_MODE);
        assert!(active_a.approved_observe_only);
        assert_eq!(read_generation(&pointer_file).unwrap(), active_a);
        assert_eq!(
            registry
                .candidate("checkpoint-a")
                .await
                .unwrap()
                .unwrap()
                .status,
            CANDIDATE_ACTIVE
        );
        registry
            .record_health(&pointer_file, 1, true, now + 1, "soak passed")
            .await
            .unwrap();

        // A crash after the pointer rename but before the activation commit
        // leaves a prepared generation the registry has to finish.
        let pending = registry.candidate("checkpoint-b").await.unwrap().unwrap();
        registry
            .connection
            .execute(
                "INSERT INTO jepa_generations(generation, checkpoint_id, checkpoint_dir,
                    weights_sha256, state, prepared_at_ms, health_state, previous_generation,
                    previous_checkpoint_id, rollback_of, reason)
                 VALUES (2, ?1, ?2, ?3, 'prepared', ?4, 'pending', 1, 'checkpoint-a', NULL, ?5)",
                params![
                    pending.checkpoint_id.clone(),
                    pending.checkpoint_dir.clone(),
                    pending.weights_sha256.clone(),
                    to_i64(now + 2).unwrap(),
                    REASON_CATALOGUE.to_string(),
                ],
            )
            .await
            .unwrap();
        let active_b = generation_pointer(2, &pending);
        write_generation_atomic(&pointer_file, &active_b).unwrap();
        assert_eq!(
            registry.reconcile(&pointer_file, now + 2).await.unwrap(),
            active_b
        );

        let rolled_back = registry
            .record_health(&pointer_file, 2, false, now + 3, "rollout regression")
            .await
            .unwrap();
        assert_eq!(rolled_back.generation, 3);
        assert_eq!(rolled_back.checkpoint_id, "checkpoint-a");
        assert_eq!(read_generation(&pointer_file).unwrap(), rolled_back);

        let status = registry.status(Some(&pointer_file)).await.unwrap();
        assert_eq!(status.schema_version, REGISTRY_SCHEMA);
        assert_eq!(status.generation_count, 3);
        assert_eq!(status.active.unwrap().generation, 3);

        // A pointer this registry never prepared is refused.
        let mut foreign = rolled_back;
        foreign.generation = 42;
        write_generation_atomic(&pointer_file, &foreign).unwrap();
        assert!(registry.reconcile(&pointer_file, now + 4).await.is_err());
    }

    #[tokio::test]
    async fn an_active_candidate_refuses_a_predictive_regression() {
        let temp = tempfile::tempdir().unwrap();
        let registry = CandidateRegistry::open(temp.path().join("registry.turso"))
            .await
            .unwrap();
        let now = now_ms();
        registry
            .insert_verified(&staged_candidate(temp.path(), "checkpoint-a", 1.0, 0.8, now))
            .await
            .unwrap();
        registry
            .promote("checkpoint-a", temp.path().join("active-generation.json"), now)
            .await
            .unwrap();

        let regressed = staged_candidate(temp.path(), "checkpoint-worse", 1.2, 0.85, now);
        assert!(registry.insert_verified(&regressed).await.is_err());
        assert!(registry.candidate("checkpoint-worse").await.unwrap().is_none());

        let matched = staged_candidate(temp.path(), "checkpoint-equal", 1.0, 0.8, now);
        registry.insert_verified(&matched).await.unwrap();
        assert_eq!(registry.status(None).await.unwrap().candidate_count, 2);
    }

    #[tokio::test]
    async fn candidate_records_and_similarity_survive_a_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("registry.turso");
        let now = now_ms();
        let (alpha, beta) = {
            let registry = CandidateRegistry::open(&db).await.unwrap();
            let mut alpha = staged_candidate(temp.path(), "checkpoint-alpha", 1.25, 0.75, now);
            alpha.record.occupancy_iou = 0.625;
            alpha.signature[0] = 1.0;
            let mut beta = staged_candidate(temp.path(), "checkpoint-beta", 1.5, 0.9, now);
            beta.signature[1] = 1.0;
            registry.insert_verified(&alpha).await.unwrap();
            registry.insert_verified(&beta).await.unwrap();
            assert_eq!(registry.path(), db.as_path());
            (alpha.record, beta.record)
        };

        let registry = CandidateRegistry::open(&db).await.unwrap();
        assert_eq!(
            registry.candidate("checkpoint-alpha").await.unwrap(),
            Some(alpha)
        );
        assert_eq!(
            registry.candidate("checkpoint-beta").await.unwrap(),
            Some(beta)
        );
        assert_eq!(
            registry
                .nearest_candidates("checkpoint-alpha", 2)
                .await
                .unwrap(),
            vec!["checkpoint-alpha".to_string(), "checkpoint-beta".to_string()]
        );
        let status = registry.status(None).await.unwrap();
        assert_eq!(status.candidate_count, 2);
        assert_eq!(status.generation_count, 0);
        assert!(status.active.is_none());
    }

    #[test]
    fn generation_pointer_round_trips_and_rejects_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("checkpoint-a");
        fs::create_dir_all(&dir).unwrap();
        let weights = b"immutable-candidate-weights".to_vec();
        fs::write(dir.join("weights.safetensors"), &weights).unwrap();
        let manifest = CheckpointManifest {
            weights_sha256: sha256_hex(&weights),
            ..checkpoint_manifest()
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let pointer = GenerationPointer {
            schema_version: GENERATION_SCHEMA.to_string(),
            generation: 7,
            checkpoint_dir: dir.display().to_string(),
            checkpoint_id: manifest.checkpoint_id.clone(),
            weights_sha256: manifest.weights_sha256.clone(),
            mode: OBSERVE_ONLY_MODE.to_string(),
            approved_observe_only: true,
        };
        let file = temp.path().join("generation.json");
        write_generation_atomic(&file, &pointer).unwrap();
        assert_eq!(read_generation(&file).unwrap(), pointer);

        let mut unapproved = pointer.clone();
        unapproved.approved_observe_only = false;
        assert!(unapproved.validate().is_err());
        let mut wrong_mode = pointer.clone();
        wrong_mode.mode = "read-write".to_string();
        assert!(wrong_mode.validate().is_err());
        let mut zero = pointer.clone();
        zero.generation = 0;
        assert!(zero.validate().is_err());

        fs::write(dir.join("weights.safetensors"), b"tampered bytes").unwrap();
        assert!(read_generation(&file).is_err());
    }
}
