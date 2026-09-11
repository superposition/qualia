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
use std::fs::{self, OpenOptions};
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

const GENERATION_PREPARED: &str = "prepared";
const GENERATION_ACTIVE: &str = "active";

const REASON_CATALOGUE: &str = "promotion";

/// Registry tables. `jepa_candidates` holds the admitted evidence, `jepa_generations`
/// the activation journal, `jepa_registry_state` the singleton pointer row.
const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS jepa_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT, checkpoint_id TEXT NOT NULL UNIQUE,
    checkpoint_dir TEXT NOT NULL, dataset_digest TEXT NOT NULL,
    dataset_manifest_path TEXT NOT NULL, weights_sha256 TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL, training_report_sha256 TEXT NOT NULL,
    backend TEXT NOT NULL, status TEXT NOT NULL,
    report_created_at_ms INTEGER NOT NULL, registered_at_ms INTEGER NOT NULL,
    transition_nll REAL NOT NULL, rollout_error REAL NOT NULL,
    occupancy_iou REAL NOT NULL, occupancy_pr_auc REAL NOT NULL,
    clamp_fraction REAL NOT NULL, nonfinite_values INTEGER NOT NULL,
    signature_vector F32_BLOB(12) NOT NULL,
    manifest_json TEXT NOT NULL, report_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS jepa_candidates_status
    ON jepa_candidates(status, registered_at_ms DESC);
CREATE TABLE IF NOT EXISTS jepa_generations (
    generation INTEGER PRIMARY KEY, checkpoint_id TEXT NOT NULL,
    checkpoint_dir TEXT NOT NULL, weights_sha256 TEXT NOT NULL,
    state TEXT NOT NULL, prepared_at_ms INTEGER NOT NULL, activated_at_ms INTEGER,
    health_state TEXT NOT NULL, previous_generation INTEGER,
    previous_checkpoint_id TEXT, rollback_of INTEGER, reason TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS jepa_registry_state (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    active_generation INTEGER, active_checkpoint_id TEXT
);
INSERT OR IGNORE INTO jepa_registry_state(singleton) VALUES (1);";

const VECTOR_PROBE_SQL: &str =
    "SELECT vector_distance_cos(vector32('[1,0]'), vector32('[1,0]'))";

const INSERT_CANDIDATE_SQL: &str = "\
INSERT INTO jepa_candidates(checkpoint_id, checkpoint_dir, dataset_digest,
    dataset_manifest_path, weights_sha256, manifest_sha256, training_report_sha256,
    backend, status, report_created_at_ms, registered_at_ms, transition_nll,
    rollout_error, occupancy_iou, occupancy_pr_auc, clamp_fraction, nonfinite_values,
    signature_vector, manifest_json, report_json)
 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,vector32(?18),?19,?20)";

const INSERT_GENERATION_SQL: &str = "\
INSERT INTO jepa_generations(generation, checkpoint_id, checkpoint_dir, weights_sha256,
    state, prepared_at_ms, health_state, previous_generation, previous_checkpoint_id,
    rollback_of, reason)
 VALUES (?1,?2,?3,?4,'prepared',?5,'pending',?6,?7,?8,?9)";

const MARK_GENERATION_ACTIVE_SQL: &str = "\
UPDATE jepa_generations SET state = 'active', activated_at_ms = ?2
 WHERE generation = ?1 AND checkpoint_id = ?3 AND weights_sha256 = ?4
   AND state IN ('prepared','active')";

const SELECT_CANDIDATE_SQL: &str = "\
SELECT checkpoint_id, checkpoint_dir, dataset_digest, weights_sha256, manifest_sha256,
       training_report_sha256, backend, status, report_created_at_ms,
       registered_at_ms, transition_nll, rollout_error, occupancy_iou,
       occupancy_pr_auc, clamp_fraction, nonfinite_values
  FROM jepa_candidates WHERE checkpoint_id = ?1";

const SELECT_ROLLBACK_CANDIDATE_SQL: &str = "\
SELECT c.checkpoint_id, c.checkpoint_dir, c.dataset_digest, c.weights_sha256,
       c.manifest_sha256, c.training_report_sha256, c.backend, c.status,
       c.report_created_at_ms, c.registered_at_ms, c.transition_nll, c.rollout_error,
       c.occupancy_iou, c.occupancy_pr_auc, c.clamp_fraction, c.nonfinite_values
  FROM jepa_generations g JOIN jepa_candidates c ON c.checkpoint_id = g.checkpoint_id
 WHERE g.generation < ?1 AND g.checkpoint_id != ?2 AND g.state IN ('superseded','active')
 ORDER BY g.generation DESC LIMIT 1";

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
        let path = path.as_ref();
        if let Some(directory) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(directory)
                .with_context(|| format!("create registry directory {}", directory.display()))?;
        }
        let location = path
            .to_str()
            .ok_or_else(|| anyhow!("registry path is not UTF-8"))?;
        let database = Builder::new_local(location).build().await?;
        let connection = database.connect()?;
        connection.execute_batch(SCHEMA_SQL).await?;
        // Turso 0.7 ships native `F32_BLOB` vectors and distance kernels but no
        // ANN index method. Probe the pinned engine now and fail closed rather
        // than storing metric signatures the engine cannot search.
        let mut probe = connection.query(VECTOR_PROBE_SQL, ()).await?;
        let distance: Option<f64> = match probe.next().await? {
            Some(row) => Some(row.get(0)?),
            None => None,
        };
        match distance {
            Some(distance) if distance.is_finite() && distance.abs() <= 1e-5 => {}
            Some(distance) => bail!("Turso vector32 probe returned an invalid result: {distance}"),
            None => bail!("Turso vector probe returned no row"),
        }
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
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
        let candidate = verify_candidate(
            checkpoint_dir.as_ref(),
            dataset_manifest_path.as_ref(),
            now_ms,
            max_report_age_ms,
        )?;
        self.insert_verified(&candidate).await?;
        Ok(candidate.record)
    }

    /// Activate an already-admitted candidate as a new generation.
    pub async fn promote(
        &self,
        checkpoint_id: &str,
        generation_file: impl AsRef<Path>,
        now_ms: u128,
    ) -> Result<GenerationPointer> {
        let candidate = self
            .candidate(checkpoint_id)
            .await?
            .ok_or_else(|| anyhow!("candidate is not registered"))?;
        if candidate.status != CANDIDATE_ELIGIBLE && candidate.status != CANDIDATE_ACCEPTED {
            bail!("candidate is not eligible for activation");
        }
        if now_ms.saturating_sub(candidate.report_created_at_ms) > DEFAULT_MAX_REPORT_AGE_MS {
            bail!("candidate report is stale at promotion time");
        }
        self.reject_regression(&candidate).await?;
        self.activate(
            candidate,
            generation_file.as_ref(),
            now_ms,
            None,
            REASON_CATALOGUE,
        )
        .await
    }

    /// Finish an activation that a crash interrupted after the pointer rename.
    pub async fn reconcile(
        &self,
        generation_file: impl AsRef<Path>,
        now_ms: u128,
    ) -> Result<GenerationPointer> {
        let pointer = read_generation(generation_file.as_ref())?;
        let mut rows = self
            .connection
            .query(
                "SELECT state FROM jepa_generations WHERE generation = ?1
                   AND checkpoint_id = ?2 AND checkpoint_dir = ?3 AND weights_sha256 = ?4",
                params![
                    pointer.generation as i64,
                    pointer.checkpoint_id.clone(),
                    pointer.checkpoint_dir.clone(),
                    pointer.weights_sha256.clone(),
                ],
            )
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow!("generation pointer was not prepared by this registry"))?;
        let state: String = row.get(0)?;
        drop(rows);
        if state == GENERATION_PREPARED {
            self.finish_activation(&pointer, now_ms).await?;
        } else if state != GENERATION_ACTIVE {
            bail!("generation pointer references a non-active registry generation");
        }
        Ok(pointer)
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
        let active = read_generation(generation_file.as_ref())?;
        if active.generation != generation {
            bail!("health result does not target the active generation");
        }
        if healthy {
            self.connection
                .execute(
                    "UPDATE jepa_generations SET health_state = 'passed'
                     WHERE generation = ?1 AND state = 'active'",
                    [generation as i64],
                )
                .await?;
            return Ok(active);
        }
        self.connection
            .execute(
                "UPDATE jepa_generations SET health_state = 'failed', reason = ?2
                 WHERE generation = ?1 AND state = 'active'",
                params![generation as i64, reason.to_string()],
            )
            .await?;
        self.rollback(generation_file, now_ms, reason).await
    }

    /// Publish a new generation pointing at the previous accepted checkpoint.
    pub async fn rollback(
        &self,
        generation_file: impl AsRef<Path>,
        now_ms: u128,
        reason: &str,
    ) -> Result<GenerationPointer> {
        let active = read_generation(generation_file.as_ref())?;
        let mut rows = self
            .connection
            .query(
                SELECT_ROLLBACK_CANDIDATE_SQL,
                params![active.generation as i64, active.checkpoint_id.clone()],
            )
            .await?;
        let candidate = rows
            .next()
            .await?
            .map(candidate_from_row)
            .transpose()?
            .ok_or_else(|| anyhow!("no prior accepted generation is available for rollback"))?;
        drop(rows);
        self.activate(
            candidate,
            generation_file.as_ref(),
            now_ms,
            Some(active.generation),
            reason,
        )
        .await
    }

    /// Look up one candidate by checkpoint id.
    pub async fn candidate(&self, checkpoint_id: &str) -> Result<Option<CandidateRecord>> {
        let mut rows = self
            .connection
            .query(SELECT_CANDIDATE_SQL, [checkpoint_id.to_string()])
            .await?;
        rows.next().await?.map(candidate_from_row).transpose()
    }

    /// Checkpoint ids nearest to `checkpoint_id` by cosine distance.
    pub async fn nearest_candidates(&self, checkpoint_id: &str, limit: u64) -> Result<Vec<String>> {
        let mut probe = self
            .connection
            .query(
                "SELECT vector_extract(signature_vector) FROM jepa_candidates
                 WHERE checkpoint_id = ?1",
                [checkpoint_id.to_string()],
            )
            .await?;
        let signature: String = probe
            .next()
            .await?
            .ok_or_else(|| anyhow!("candidate is not registered"))?
            .get(0)?;
        drop(probe);
        let mut rows = self
            .connection
            .query(
                "SELECT checkpoint_id, vector_distance_cos(signature_vector, vector32(?1))
                   AS distance FROM jepa_candidates ORDER BY distance LIMIT ?2",
                params![signature, limit.clamp(1, 100) as i64],
            )
            .await?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    /// Totals, plus the active pointer when a generation file is given.
    pub async fn status(&self, generation_file: Option<&Path>) -> Result<RegistryStatus> {
        Ok(RegistryStatus {
            schema_version: REGISTRY_SCHEMA.to_string(),
            active: generation_file.map(read_generation).transpose()?,
            candidate_count: count(&self.connection, "jepa_candidates").await?,
            generation_count: count(&self.connection, "jepa_generations").await?,
        })
    }

    /// Insert a fully verified candidate, refusing a predictive regression.
    async fn insert_verified(&self, candidate: &EvidenceCandidate) -> Result<()> {
        self.reject_regression(&candidate.record).await?;
        let record = &candidate.record;
        let signature = serde_json::to_string(&candidate.signature)?;
        self.connection.execute("BEGIN IMMEDIATE", ()).await?;
        let inserted = self
            .connection
            .execute(
                INSERT_CANDIDATE_SQL,
                params![
                    record.checkpoint_id.clone(),
                    record.checkpoint_dir.clone(),
                    record.dataset_digest.clone(),
                    candidate.dataset_manifest_path.clone(),
                    record.weights_sha256.clone(),
                    record.manifest_sha256.clone(),
                    record.training_report_sha256.clone(),
                    record.backend.clone(),
                    record.status.clone(),
                    to_i64(record.report_created_at_ms)?,
                    to_i64(record.registered_at_ms)?,
                    record.transition_nll,
                    record.rollout_error,
                    record.occupancy_iou,
                    record.occupancy_pr_auc,
                    record.clamp_fraction,
                    to_i64(u128::from(record.nonfinite_values))?,
                    signature,
                    candidate.manifest_json.clone(),
                    candidate.report_json.clone(),
                ],
            )
            .await;
        finish_transaction(
            &self.connection,
            inserted.map(|_| ()).map_err(anyhow::Error::from),
        )
        .await
    }

    /// Refuse a candidate that predictively regresses the active model.
    async fn reject_regression(&self, candidate: &CandidateRecord) -> Result<()> {
        let mut rows = self
            .connection
            .query(
                "SELECT transition_nll, rollout_error FROM jepa_candidates
                 WHERE status = 'active' LIMIT 1",
                (),
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(());
        };
        let incumbent_nll: f64 = row.get(0)?;
        let incumbent_rollout: f64 = row.get(1)?;
        drop(rows);
        if candidate.transition_nll > incumbent_nll
            || candidate.rollout_error > incumbent_rollout
        {
            bail!("candidate regresses against the active sensor-only model");
        }
        Ok(())
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
        let (previous_generation, previous_checkpoint_id) = self.active_identity().await?;
        let generation = self.next_generation().await?;
        let pointer = generation_pointer(generation, &candidate);
        pointer.validate()?;
        self.connection.execute("BEGIN IMMEDIATE", ()).await?;
        let prepared = self
            .connection
            .execute(
                INSERT_GENERATION_SQL,
                params![
                    to_i64(u128::from(generation))?,
                    pointer.checkpoint_id.clone(),
                    pointer.checkpoint_dir.clone(),
                    pointer.weights_sha256.clone(),
                    to_i64(now_ms)?,
                    previous_generation.map(|value| value as i64),
                    previous_checkpoint_id.clone(),
                    rollback_of.map(|value| value as i64),
                    reason.to_string(),
                ],
            )
            .await;
        finish_transaction(
            &self.connection,
            prepared.map(|_| ()).map_err(anyhow::Error::from),
        )
        .await?;

        write_generation_atomic(generation_file, &pointer)?;
        self.finish_activation(&pointer, now_ms).await?;
        Ok(pointer)
    }

    /// Commit step three: mark the prepared generation active.
    async fn finish_activation(&self, pointer: &GenerationPointer, now_ms: u128) -> Result<()> {
        self.connection.execute("BEGIN IMMEDIATE", ()).await?;
        let committed = async {
            self.connection
                .execute(
                    "UPDATE jepa_generations SET state = 'superseded'
                     WHERE state = 'active' AND generation != ?1",
                    [pointer.generation as i64],
                )
                .await?;
            let promoted = self
                .connection
                .execute(
                    MARK_GENERATION_ACTIVE_SQL,
                    params![
                        pointer.generation as i64,
                        to_i64(now_ms)?,
                        pointer.checkpoint_id.clone(),
                        pointer.weights_sha256.clone(),
                    ],
                )
                .await?;
            if promoted != 1 {
                bail!("prepared generation does not match the active pointer");
            }
            self.connection
                .execute(
                    "UPDATE jepa_candidates SET status = 'accepted'
                     WHERE status = 'active' AND checkpoint_id != ?1",
                    [pointer.checkpoint_id.clone()],
                )
                .await?;
            self.connection
                .execute(
                    "UPDATE jepa_candidates SET status = 'active' WHERE checkpoint_id = ?1",
                    [pointer.checkpoint_id.clone()],
                )
                .await?;
            self.connection
                .execute(
                    "UPDATE jepa_registry_state
                     SET active_generation = ?1, active_checkpoint_id = ?2 WHERE singleton = 1",
                    params![pointer.generation as i64, pointer.checkpoint_id.clone()],
                )
                .await?;
            Ok(())
        }
        .await;
        finish_transaction(&self.connection, committed).await
    }

    /// The generation and checkpoint the registry currently considers active.
    async fn active_identity(&self) -> Result<(Option<u64>, Option<String>)> {
        let mut rows = self
            .connection
            .query(
                "SELECT active_generation, active_checkpoint_id FROM jepa_registry_state
                 WHERE singleton = 1",
                (),
            )
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow!("registry state is missing"))?;
        let generation: Option<i64> = row.get(0)?;
        let checkpoint: Option<String> = row.get(1)?;
        Ok((generation.map(|value| value.max(0) as u64), checkpoint))
    }

    /// One past the highest generation ever journalled.
    async fn next_generation(&self) -> Result<u64> {
        let mut rows = self
            .connection
            .query(
                "SELECT COALESCE(MAX(generation), 0) + 1 FROM jepa_generations",
                (),
            )
            .await?;
        let generation: i64 = rows
            .next()
            .await?
            .ok_or_else(|| anyhow!("generation query returned no row"))?
            .get(0)?;
        u64::try_from(generation).context("invalid next generation")
    }
}

/// Re-read and digest-verify a checkpoint, its report, and its dataset.
fn verify_candidate(
    checkpoint_dir: &Path,
    dataset_manifest_path: &Path,
    now_ms: u128,
    max_report_age_ms: u128,
) -> Result<EvidenceCandidate> {
    let manifest_bytes = fs::read(checkpoint_dir.join("manifest.json"))?;
    let manifest: CheckpointManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.schema_version != CHECKPOINT_SCHEMA
        || manifest.architecture_id != ARCHITECTURE_ID
        || manifest.status != "candidate"
        || manifest.target_encoder != "ema"
        || !safe_id(&manifest.checkpoint_id)
        || !manifest.baseline_gate.passes()
        || manifest.baseline_gate.dataset_digest != manifest.dataset_digest
        || !manifest.action_support.passes()
        || !manifest.grounding_geometry.passes()
    {
        bail!("checkpoint manifest does not pass immutable candidate gates");
    }
    let weights = fs::read(checkpoint_dir.join("weights.safetensors"))?;
    let weights_sha256 = sha256_hex(&weights);
    if weights_sha256 != manifest.weights_sha256 {
        bail!("checkpoint weights digest mismatch");
    }
    validate_checkpoint_weights(&weights, manifest.parameter_count)
        .map_err(|error| anyhow!(error.to_string()))?;
    let report_bytes = fs::read(&manifest.training_report_path)?;
    if sha256_hex(&report_bytes) != manifest.training_report_sha256 {
        bail!("training report digest mismatch");
    }
    let report: TrainingReport = serde_json::from_slice(&report_bytes)?;
    report
        .validate_for_promotion(&manifest, now_ms, max_report_age_ms)
        .map_err(|error| anyhow!(error.to_string()))?;

    let dataset_bytes = fs::read(dataset_manifest_path)?;
    let dataset: DatasetManifest = serde_json::from_slice(&dataset_bytes)?;
    verify_dataset(&dataset, &manifest.dataset_digest)?;
    validate_report_dataset_alignment(&report, &dataset)?;
    if manifest.baseline_gate.valid_transitions != dataset.audit.valid_transitions
        || manifest.baseline_gate.sessions != dataset.audit.sessions
        || manifest.baseline_gate.conditions != dataset.audit.conditions.len() as u64
        || manifest.baseline_gate.environments != dataset.audit.environments
    {
        bail!("checkpoint dataset counts do not match the immutable evidence manifest");
    }

    let record = CandidateRecord {
        checkpoint_id: manifest.checkpoint_id.clone(),
        checkpoint_dir: checkpoint_dir.canonicalize()?.display().to_string(),
        dataset_digest: manifest.dataset_digest.clone(),
        weights_sha256,
        manifest_sha256: sha256_hex(&manifest_bytes),
        training_report_sha256: manifest.training_report_sha256.clone(),
        backend: manifest.backend.clone(),
        status: CANDIDATE_ELIGIBLE.to_string(),
        report_created_at_ms: report.created_at_ms,
        registered_at_ms: now_ms,
        transition_nll: report.test.tiny_cnn.transition_nll,
        rollout_error: report.test.tiny_cnn.rollout_error,
        occupancy_iou: report.test.occupancy.intersection_over_union,
        occupancy_pr_auc: report.test.occupancy.pr_auc,
        clamp_fraction: report.test.calibration.clamp_fraction,
        nonfinite_values: report.test.calibration.nonfinite_values,
    };
    Ok(EvidenceCandidate {
        record,
        manifest_json: String::from_utf8(manifest_bytes)?,
        report_json: String::from_utf8(report_bytes)?,
        dataset_manifest_path: dataset_manifest_path.canonicalize()?.display().to_string(),
        signature: report_signature(&report),
    })
}

/// Re-derive a dataset manifest and re-run its promotion quotas.
fn verify_dataset(manifest: &DatasetManifest, expected_digest: &str) -> Result<()> {
    let recomputed = manifest_digest(manifest).map_err(|error| anyhow!(error.to_string()))?;
    validate_dataset_promotion_gate(manifest).map_err(|error| anyhow!(error.to_string()))?;
    let condition_total: u64 = manifest.audit.conditions.values().copied().sum();
    let split_total: u64 = manifest.audit.split_samples.values().copied().sum();
    if manifest.schema_version != DATASET_SCHEMA
        || manifest.digest != expected_digest
        || manifest.digest != recomputed
        || manifest.audit.valid_transitions != manifest.samples.len() as u64
        || manifest.audit.sessions != manifest.sources.len() as u64
        || condition_total != manifest.audit.valid_transitions
        || split_total != manifest.audit.valid_transitions
    {
        bail!("dataset is synthetic, incomplete, or below the real-evidence quota");
    }
    let rebuilt = build_manifest(manifest.sources.clone(), manifest.config)
        .map_err(|error| anyhow!(error.to_string()))?;
    if &rebuilt != manifest {
        bail!("dataset manifest does not exactly reproduce from immutable MCAP evidence");
    }
    Ok(())
}

/// Recompute split counts, action support and step accounting from evidence.
fn validate_report_dataset_alignment(
    report: &TrainingReport,
    dataset: &DatasetManifest,
) -> Result<()> {
    let measured = measured_action_support(dataset).map_err(|error| anyhow!(error.to_string()))?;
    if report.action_support != measured
        || report.grounding_geometry.resolution_m != dataset.config.grounding_resolution_m
    {
        bail!("training report support or grounding geometry does not match dataset evidence");
    }

    for (split, reported_samples, reported_sessions) in [
        (
            DatasetSplit::Validation,
            report.validation.samples,
            report.validation.sessions,
        ),
        (
            DatasetSplit::Test,
            report.test.samples,
            report.test.sessions,
        ),
    ] {
        let mut sessions = BTreeSet::new();
        let measured_samples = dataset
            .samples
            .iter()
            .filter(|sample| sample.split == split)
            .inspect(|sample| {
                sessions.insert(sample.session_id.as_str());
            })
            .count() as u64;
        if reported_samples != measured_samples || reported_sessions != sessions.len() as u64 {
            bail!("training report held-out counts do not match dataset provenance");
        }
    }

    let training_samples = measured.sample_count;
    let batch_size = u64::try_from(report.batch_size).context("training batch size overflows")?;
    let epochs = u64::try_from(report.epochs).context("training epoch count overflows")?;
    let complete_batches = training_samples / batch_size;
    let remainder = training_samples % batch_size;
    let steps_per_epoch = complete_batches + u64::from(remainder >= 2);
    let expected_steps = steps_per_epoch
        .checked_mul(epochs)
        .context("training step count overflows")?;
    let expected_singletons = u64::from(remainder == 1)
        .checked_mul(epochs)
        .context("training singleton count overflows")?;
    if report.cnn_steps != expected_steps
        || report.flat_steps != expected_steps
        || report.skipped_singletons != expected_singletons
    {
        bail!("training report step accounting does not match dataset and batch contract");
    }
    Ok(())
}

/// Rebuild a record from one `jepa_candidates` row.
fn candidate_from_row(row: turso::Row) -> Result<CandidateRecord> {
    let report_created_at_ms: i64 = row.get(8)?;
    let registered_at_ms: i64 = row.get(9)?;
    let nonfinite_values: i64 = row.get(15)?;
    let record = CandidateRecord {
        checkpoint_id: row.get(0)?,
        checkpoint_dir: row.get(1)?,
        dataset_digest: row.get(2)?,
        weights_sha256: row.get(3)?,
        manifest_sha256: row.get(4)?,
        training_report_sha256: row.get(5)?,
        backend: row.get(6)?,
        status: row.get(7)?,
        report_created_at_ms: report_created_at_ms.max(0) as u128,
        registered_at_ms: registered_at_ms.max(0) as u128,
        transition_nll: row.get(10)?,
        rollout_error: row.get(11)?,
        occupancy_iou: row.get(12)?,
        occupancy_pr_auc: row.get(13)?,
        clamp_fraction: row.get(14)?,
        nonfinite_values: nonfinite_values.max(0) as u64,
    };
    Ok(record)
}

/// The metric signature stored beside a candidate.
fn report_signature(report: &TrainingReport) -> [f32; SIGNATURE_DIM] {
    let held_out = &report.test;
    let calibration = &held_out.calibration;
    [
        held_out.tiny_cnn.transition_nll as f32,
        held_out.tiny_cnn.rollout_error as f32,
        held_out.occupancy.intersection_over_union as f32,
        held_out.occupancy.pr_auc as f32,
        calibration.transition_nll as f32,
        calibration.mean_standardized_squared_residual as f32,
        calibration.coverage_50 as f32,
        calibration.coverage_90 as f32,
        calibration.coverage_95 as f32,
        calibration.calibration_slope as f32,
        calibration.clamp_fraction as f32,
        report.effective_rank.effective_rank as f32,
    ]
}

/// Commit or roll back the transaction `result` was produced inside.
async fn finish_transaction(connection: &Connection, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => {
            connection.execute("COMMIT", ()).await?;
            Ok(())
        }
        Err(error) => {
            let _ = connection.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}

/// Count the rows of one registry table.
async fn count(connection: &Connection, table: &str) -> Result<u64> {
    let sql = match table {
        "jepa_candidates" => "SELECT COUNT(*) FROM jepa_candidates",
        "jepa_generations" => "SELECT COUNT(*) FROM jepa_generations",
        _ => bail!("unsupported registry count"),
    };
    let mut rows = connection.query(sql, ()).await?;
    let total: i64 = rows
        .next()
        .await?
        .ok_or_else(|| anyhow!("registry count returned no row"))?
        .get(0)?;
    Ok(total.max(0) as u64)
}

/// Build the pointer an activation publishes.
fn generation_pointer(generation: u64, candidate: &CandidateRecord) -> GenerationPointer {
    GenerationPointer {
        schema_version: GENERATION_SCHEMA.to_string(),
        generation,
        checkpoint_dir: candidate.checkpoint_dir.clone(),
        checkpoint_id: candidate.checkpoint_id.clone(),
        weights_sha256: candidate.weights_sha256.clone(),
        mode: OBSERVE_ONLY_MODE.to_string(),
        approved_observe_only: true,
    }
}

/// Read and verify a generation pointer against its immutable checkpoint.
pub fn read_generation(path: &Path) -> Result<GenerationPointer> {
    let pointer: GenerationPointer = serde_json::from_slice(&fs::read(path)?)?;
    pointer.validate()?;
    let checkpoint_dir = Path::new(&pointer.checkpoint_dir);
    let manifest: CheckpointManifest =
        serde_json::from_slice(&fs::read(checkpoint_dir.join("manifest.json"))?)?;
    let weights_digest = sha256_hex(&fs::read(checkpoint_dir.join("weights.safetensors"))?);
    if manifest.checkpoint_id != pointer.checkpoint_id
        || manifest.weights_sha256 != pointer.weights_sha256
        || weights_digest != pointer.weights_sha256
    {
        bail!("generation pointer does not match its immutable checkpoint");
    }
    Ok(pointer)
}

/// Write a generation pointer with an fsync and a rename.
pub fn write_generation_atomic(path: &Path, pointer: &GenerationPointer) -> Result<()> {
    pointer.validate()?;
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("generation pointer requires a parent directory"))?;
    fs::create_dir_all(directory)?;
    let marker = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("generation");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let partial = directory.join(format!(".{marker}.{}.{nonce}.partial", std::process::id()));
    let bytes = serde_json::to_vec_pretty(pointer)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&partial, path)?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
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
            samples: 5_120,
            sessions: 6,
            constant: metrics(2.75, 1.9),
            flat_mlp: metrics(1.8, 1.4),
            tiny_cnn: metrics(0.95, 0.7),
            calibration: calibration(),
            occupancy: occupancy(),
        };
        TrainingReport {
            schema_version: TRAINING_REPORT_SCHEMA.to_string(),
            created_at_ms: now,
            checkpoint_id: "cnn-echo-11".to_string(),
            dataset_digest: "3c".repeat(32),
            backend: "cpu".to_string(),
            seed: 7,
            epochs: 3,
            batch_size: 16,
            cnn_steps: 960,
            flat_steps: 960,
            skipped_singletons: 0,
            action_support: support(),
            grounding_geometry: geometry(),
            validation: held_out.clone(),
            test: held_out,
            effective_rank: EffectiveRankReport {
                sample_count: 4_096,
                dimensions: 256,
                effective_rank: 96.5,
                trace: 128.0,
                converged: true,
                sweeps: 9,
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
            checkpoint_id: "cnn-echo-11".to_string(),
            dataset_digest: "3c".repeat(32),
            training_seed: 7,
            backend: "cpu".to_string(),
            dtype: "F32".to_string(),
            target_encoder: "ema".to_string(),
            parameter_count: 4_096,
            weights_sha256: "5e".repeat(32),
            training_report_path: "evidence/training-report.json".to_string(),
            training_report_sha256: "b7".repeat(32),
            baseline_gate: BaselineGate {
                dataset_digest: "3c".repeat(32),
                valid_transitions: 51_200,
                sessions: 16,
                conditions: 4,
                environments: 5,
                constant: metrics(2.75, 1.9),
                flat_mlp: metrics(1.8, 1.4),
                tiny_cnn: metrics(0.95, 0.7),
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
            entity: "scout-2".to_string(),
            source_sequence: sequence,
            timestamp_ns: 1_700_000_000_000 + sequence * 250_000_000,
        };
        let following = EventRef {
            source_sequence: sequence + 1,
            timestamp_ns: observed.timestamp_ns + 120_000_000,
            ..observed.clone()
        };
        TransitionSample {
            sample_id,
            session_id: session_id.to_string(),
            environment_id: "arena".to_string(),
            condition: "daylight".to_string(),
            split,
            mcap_sha256: "9d".repeat(32),
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
                left: 0.12,
                right: -0.08,
                speed_scale: 0.85,
                interval_start_ns: observed.timestamp_ns,
                interval_end_ns: observed.timestamp_ns + 120_000_000,
                coverage: 0.95,
                safety_flags: 0,
            },
            quality: TransitionQuality {
                camera_luminance_mean: 0.42,
                camera_luminance_stddev: 0.08,
                target_camera_luminance_mean: 0.44,
                target_camera_luminance_stddev: 0.07,
                lidar_valid_fraction: 0.985,
                target_lidar_valid_fraction: 0.99,
                pose_confidence: 0.97,
                target_pose_confidence: 0.96,
                max_sensor_skew_ns: 1_200_000,
                action_coverage: 0.95,
                calibration_id: "cal-arena-2026-07".to_string(),
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
                dataset_digest: "3c".repeat(32),
                weights_sha256,
                manifest_sha256: "5e".repeat(32),
                training_report_sha256: "b7".repeat(32),
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
            valid_transitions: 60_000,
            sessions: 14,
            environments: 2,
            ..DatasetAudit::default()
        };
        audit.conditions.insert("day".to_string(), 30_000);
        audit.conditions.insert("dusk".to_string(), 18_000);
        audit.conditions.insert("night".to_string(), 12_000);
        audit.split_samples.insert(DatasetSplit::Train, 60_000);
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
            error.contains("valid-transition count does not match samples"),
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
        let train_samples = dataset
            .samples
            .iter()
            .filter(|sample| sample.split == DatasetSplit::Train)
            .count() as u64;
        let steps_per_epoch = train_samples / aligned.batch_size as u64;
        aligned.cnn_steps = steps_per_epoch * aligned.epochs as u64;
        aligned.flat_steps = aligned.cnn_steps;
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
            error.contains("checkpoint weights digest mismatch"),
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
            "active"
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
