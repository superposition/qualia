//! Immutable, audited transition manifests for offline Qualia JEPA training.
//!
//! A manifest is a reference-only record. It names the sealed MCAP segments a
//! dataset was drawn from, the transition samples that survived the physical
//! quality gates, and the audit trail of everything that was rejected, so a
//! trainer checks the same evidence the builder saw.
//!
//! The crate has two halves:
//!
//! * [`build_manifest`] walks sealed segments and emits transition references.
//! * [`materialize_session`] turns those references back into packed encoder
//!   observations one session at a time, so a trainer never decodes the same
//!   segment once per sample.
//!
//! Splits are assigned per environment, never per sample: an environment is
//! wholly training, validation or test, so held-out evidence cannot leak.

use qualia_jepa::{
    camera_step_velocity, pack_observation, AppliedAction, PoseFeatures, CAMERA_PIXELS,
    GROUNDING_CELLS, GROUNDING_HEIGHT, GROUNDING_WIDTH, LIDAR_BINS, OBS_DIM,
};
use qualia_mcap::{
    read_window_topics, LoggedMessage, TOPIC_ACTION_APPLIED, TOPIC_CAMERA, TOPIC_LIDAR, TOPIC_POSE,
};
use qualia_types::{ACTION_AUTHORITY_LEASH, LEASH_ACTION_SAFETY_COLLISION_CLAMP};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Schema tag every manifest written by this crate carries.
const MANIFEST_SCHEMA: &str = "qualia.jepa-dataset.v3";
/// Envelope contract an applied-action record must satisfy to be admissible.
const ACTION_SCHEMA: &str = "qualia.action-evidence.v1";
/// Transport stage that proves the action reached the physical layer.
const ACTION_STAGE: &str = "transport_accepted";
/// Camera thumbnail geometry frozen into the encoder observation.
const THUMB_WIDTH: u64 = 64;
const THUMB_HEIGHT: u64 = 48;
/// Physical streams a single transition can reference.
const TRANSITION_TOPICS: [&str; 4] = [TOPIC_CAMERA, TOPIC_LIDAR, TOPIC_POSE, TOPIC_ACTION_APPLIED];

/// Result alias for every fallible operation in this crate.
pub type DatasetResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// One sealed MCAP segment offered as evidence for a dataset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionEvidence {
    pub session_id: String,
    pub environment_id: String,
    pub condition: String,
    /// Entity whose physical trajectory is materialized from this segment.
    ///
    /// One arena recording may carry several robots, but a transition must
    /// never pair one entity's camera with another entity's pose or action.
    pub primary_entity: String,
    pub path: String,
    pub sha256: String,
}

/// Physical gates a candidate transition must clear before it is admitted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DatasetConfig {
    pub max_sensor_skew_ns: u64,
    pub max_frame_gap_ns: u64,
    pub min_action_coverage: f32,
    pub min_lidar_valid_fraction: f32,
    pub min_pose_confidence: f32,
    pub min_luminance_mean: f32,
    pub max_luminance_mean: f32,
    pub lidar_max_range_m: f32,
    pub grounding_resolution_m: f32,
}

impl Default for DatasetConfig {
    fn default() -> Self {
        Self {
            max_sensor_skew_ns: 150_000_000,
            max_frame_gap_ns: 500_000_000,
            min_action_coverage: 0.8,
            min_lidar_valid_fraction: 0.1,
            min_pose_confidence: 0.1,
            min_luminance_mean: 0.05,
            max_luminance_mean: 0.95,
            lidar_max_range_m: 8.0,
            grounding_resolution_m: 0.05,
        }
    }
}

/// Which half of the held-out boundary an environment belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Train,
    Validation,
    Test,
}

/// Address of one physical event inside a sealed MCAP segment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRef {
    pub topic: String,
    pub entity: String,
    pub source_sequence: u64,
    pub timestamp_ns: u64,
}

/// Measured quality of the two frames a transition joins.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransitionQuality {
    pub camera_luminance_mean: f32,
    pub camera_luminance_stddev: f32,
    pub target_camera_luminance_mean: f32,
    pub target_camera_luminance_stddev: f32,
    pub lidar_valid_fraction: f32,
    pub target_lidar_valid_fraction: f32,
    pub pose_confidence: f32,
    pub target_pose_confidence: f32,
    pub max_sensor_skew_ns: u64,
    pub action_coverage: f32,
    pub calibration_id: String,
}

/// Duration-weighted action actually applied across a transition interval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppliedActionAggregate {
    pub source_sequences: Vec<u64>,
    pub left: f32,
    pub right: f32,
    pub speed_scale: f32,
    pub interval_start_ns: u64,
    pub interval_end_ns: u64,
    pub coverage: f32,
    pub safety_flags: u32,
}

/// One admitted transition, stored by reference to its MCAP evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransitionSample {
    pub sample_id: String,
    pub session_id: String,
    pub environment_id: String,
    pub condition: String,
    pub split: DatasetSplit,
    pub mcap_sha256: String,
    pub observation_camera: EventRef,
    pub observation_lidar: EventRef,
    pub observation_pose: EventRef,
    pub target_camera: EventRef,
    pub target_lidar: EventRef,
    pub target_pose: EventRef,
    pub action: AppliedActionAggregate,
    pub quality: TransitionQuality,
}

/// Rejection ledger and aggregate statistics for one build.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DatasetAudit {
    pub candidate_transitions: u64,
    pub valid_transitions: u64,
    pub rejected: BTreeMap<String, u64>,
    pub sessions: u64,
    pub environments: u64,
    pub conditions: BTreeMap<String, u64>,
    pub split_samples: BTreeMap<DatasetSplit, u64>,
    pub mean_sensor_skew_ns: f64,
    pub p95_sensor_skew_ns: u64,
    pub mean_action_coverage: f64,
    pub overexposed_candidate_fraction: f64,
}

/// The complete, digest-addressed record of one dataset build.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasetManifest {
    pub schema_version: String,
    pub digest: String,
    pub config: DatasetConfig,
    pub sources: Vec<SessionEvidence>,
    pub samples: Vec<TransitionSample>,
    pub audit: DatasetAudit,
}

/// Recompute manifest metadata from transition provenance.
///
/// This is independent of promotion quotas so a diagnostic manifest built from
/// rejected sources stays writable. Training and registry admission call both
/// integrity and quota validation.
pub fn validate_dataset_manifest_integrity(manifest: &DatasetManifest) -> DatasetResult<()> {
    validate_unique_sources(&manifest.sources)?;
    if manifest.audit.valid_transitions != manifest.samples.len() as u64 {
        return Err("dataset valid-transition count does not match samples".into());
    }

    let sources = manifest
        .sources
        .iter()
        .map(|source| (source.session_id.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let expected_splits = assign_environment_splits(
        manifest
            .samples
            .iter()
            .map(|sample| sample.environment_id.as_str()),
    );
    let mut sample_ids = BTreeSet::new();
    let mut sessions = BTreeSet::new();
    let mut environments = BTreeSet::new();
    let mut conditions = BTreeMap::<String, u64>::new();
    let mut split_samples = BTreeMap::<DatasetSplit, u64>::new();
    for sample in &manifest.samples {
        if !sample_ids.insert(sample.sample_id.as_str()) {
            return Err(format!("duplicate dataset sample id: {}", sample.sample_id).into());
        }
        let source = sources
            .get(sample.session_id.as_str())
            .ok_or("dataset sample references an unknown source session")?;
        if sample.environment_id != source.environment_id
            || sample.condition != source.condition
            || sample.mcap_sha256 != source.sha256
        {
            return Err("dataset sample provenance does not match its source session".into());
        }
        for (reference, expected_topic) in [
            (&sample.observation_camera, TOPIC_CAMERA),
            (&sample.observation_lidar, TOPIC_LIDAR),
            (&sample.observation_pose, TOPIC_POSE),
            (&sample.target_camera, TOPIC_CAMERA),
            (&sample.target_lidar, TOPIC_LIDAR),
            (&sample.target_pose, TOPIC_POSE),
        ] {
            if reference.topic != expected_topic
                || reference.entity != source.primary_entity
                || reference.source_sequence == 0
                || reference.timestamp_ns == 0
            {
                return Err("dataset sample contains an invalid physical event reference".into());
            }
        }
        if sample.action.source_sequences.is_empty()
            || sample.action.source_sequences.contains(&0)
            || sample.action.interval_start_ns == 0
            || sample.action.interval_end_ns <= sample.action.interval_start_ns
        {
            return Err("dataset sample contains invalid applied-action provenance".into());
        }
        let expected_split = expected_splits
            .get(&sample.environment_id)
            .ok_or("dataset sample environment has no split assignment")?;
        if &sample.split != expected_split {
            return Err("dataset sample split is not the whole-environment assignment".into());
        }
        sessions.insert(sample.session_id.as_str());
        environments.insert(sample.environment_id.as_str());
        *conditions.entry(sample.condition.clone()).or_default() += 1;
        *split_samples.entry(sample.split.clone()).or_default() += 1;
    }
    if manifest.audit.sessions != sessions.len() as u64
        || manifest.audit.environments != environments.len() as u64
        || manifest.audit.conditions != conditions
        || manifest.audit.split_samples != split_samples
    {
        return Err("dataset audit metadata does not match transition provenance".into());
    }
    Ok(())
}

/// Apply the evidence boundary that gates offline training.
///
/// The two axes are not interchangeable: conditions measure how the capture
/// varied, environments measure where it happened. A long recording inside one
/// room therefore still fails, however many transitions, sessions and
/// condition labels it carries, because it cannot demonstrate a leakage
/// boundary.
pub fn validate_dataset_promotion_gate(manifest: &DatasetManifest) -> DatasetResult<()> {
    validate_dataset_manifest_integrity(manifest)?;
    if manifest.audit.sessions != manifest.sources.len() as u64 {
        return Err("every promoted MCAP source must contribute valid transitions".into());
    }
    validate_promotion_audit(&manifest.audit)
}

fn validate_promotion_audit(audit: &DatasetAudit) -> DatasetResult<()> {
    if audit.valid_transitions < 50_000
        || audit.sessions < 12
        || audit.environments < 3
        || audit.conditions.len() < 3
    {
        return Err(
            "dataset does not meet the 50k/12-session/3-condition/3-environment gate".into(),
        );
    }
    let minimum_condition_samples = audit.valid_transitions.div_ceil(20);
    let balanced_conditions = audit
        .conditions
        .values()
        .filter(|samples| **samples >= minimum_condition_samples)
        .count();
    if balanced_conditions < 3 {
        return Err(format!(
            "dataset needs three conditions with at least 5% of valid transitions ({minimum_condition_samples} samples each)"
        )
        .into());
    }
    if audit.conditions.values().sum::<u64>() != audit.valid_transitions
        || audit.split_samples.values().sum::<u64>() != audit.valid_transitions
    {
        return Err("dataset audit counts do not sum to valid transitions".into());
    }
    for split in [
        DatasetSplit::Train,
        DatasetSplit::Validation,
        DatasetSplit::Test,
    ] {
        if audit.split_samples.get(&split).copied().unwrap_or(0) == 0 {
            return Err(format!(
                "dataset environment split {split:?} is empty; held-out evidence is required"
            )
            .into());
        }
    }
    let training_samples = audit
        .split_samples
        .get(&DatasetSplit::Train)
        .copied()
        .unwrap_or(0);
    if training_samples < 4_096 {
        return Err(format!(
            "dataset environment split Train needs at least 4096 transitions, found {training_samples}"
        )
        .into());
    }
    for split in [DatasetSplit::Validation, DatasetSplit::Test] {
        let samples = audit.split_samples.get(&split).copied().unwrap_or(0);
        if samples < 4_096 {
            return Err(format!(
                "dataset environment split {split:?} needs at least 4096 transitions, found {samples}"
            )
            .into());
        }
    }
    Ok(())
}

/// A future occupancy grid decoded from one LiDAR record.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializedOccupancy {
    pub width: usize,
    pub height: usize,
    pub resolution_m: f32,
    pub origin_x_m: f32,
    pub origin_y_m: f32,
    pub occupied: Vec<f32>,
    pub observed: Vec<bool>,
}

/// The occupancy window centred on the robot, in the grounding frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RobotCentricGrounding {
    pub occupied: Vec<f32>,
    pub observed: Vec<f32>,
}

impl MaterializedOccupancy {
    /// Crop the grid to the fixed robot-centric window the loss consumes.
    pub fn robot_centric_grounding(&self) -> DatasetResult<RobotCentricGrounding> {
        if self.width.checked_mul(self.height) != Some(self.occupied.len())
            || self.observed.len() != self.occupied.len()
            || !self.resolution_m.is_finite()
            || self.resolution_m <= 0.0
        {
            return Err("invalid materialized occupancy geometry".into());
        }
        let robot_x = ((-self.origin_x_m) / self.resolution_m).floor() as isize;
        let robot_y = ((-self.origin_y_m) / self.resolution_m).floor() as isize;
        let start_x = robot_x - GROUNDING_WIDTH as isize / 2;
        let start_y = robot_y - GROUNDING_HEIGHT as isize / 2;
        let mut occupied = vec![0.0; GROUNDING_CELLS];
        let mut observed = vec![0.0; GROUNDING_CELLS];
        for target_y in 0..GROUNDING_HEIGHT {
            for target_x in 0..GROUNDING_WIDTH {
                let source_x = start_x + target_x as isize;
                let source_y = start_y + target_y as isize;
                if source_x < 0
                    || source_y < 0
                    || source_x as usize >= self.width
                    || source_y as usize >= self.height
                {
                    continue;
                }
                let source = source_y as usize * self.width + source_x as usize;
                let target = target_y * GROUNDING_WIDTH + target_x;
                occupied[target] = self.occupied[source];
                observed[target] = f32::from(self.observed[source]);
            }
        }
        Ok(RobotCentricGrounding { occupied, observed })
    }
}

/// One transition resolved back into packed tensors.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterializedTransition {
    pub sample_id: String,
    pub observation_sequence: u64,
    pub observation_timestamp_ns: u64,
    pub target_sequence: u64,
    pub target_timestamp_ns: u64,
    pub observation: Vec<f32>,
    pub target_observation: Vec<f32>,
    pub applied_action: [f32; 4],
    pub delta_seconds: f32,
    pub future_occupancy: MaterializedOccupancy,
    pub quality: TransitionQuality,
}

#[derive(Debug, Clone)]
struct CameraEvent {
    reference: EventRef,
    valid: bool,
    luminance_mean: f32,
    luminance_stddev: f32,
    calibration_id: String,
}

#[derive(Debug, Clone)]
struct LidarEvent {
    reference: EventRef,
    valid_fraction: f32,
    has_grounded_occupancy: bool,
}

#[derive(Debug, Clone)]
struct PoseEvent {
    reference: EventRef,
    confidence: f32,
}

#[derive(Debug, Clone)]
struct ActionEvent {
    history_sequence: u64,
    producer_epoch: u64,
    source_sequence: u64,
    start_ns: u64,
    end_ns: u64,
    left: f32,
    right: f32,
    speed_scale: f32,
    safety_flags: u32,
    valid: bool,
    leash_authority: bool,
}

struct ParsedEvents {
    cameras: Vec<CameraEvent>,
    lidars: Vec<LidarEvent>,
    poses: Vec<PoseEvent>,
    actions: Vec<ActionEvent>,
}

#[derive(Debug, Clone)]
struct MaterializedPose {
    reference: EventRef,
    x_m: f32,
    z_m: f32,
    yaw_rad: f32,
    confidence: f32,
}

/// Walk every offered session and emit the transition references that clear
/// the physical gates.
///
/// The manifest that comes back is reference-only and digest-addressed; the
/// rejected map records why each candidate was refused so a short dataset is
/// explained rather than mysterious.
pub fn build_manifest(
    mut sources: Vec<SessionEvidence>,
    config: DatasetConfig,
) -> DatasetResult<DatasetManifest> {
    validate_config(config)?;
    sources.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    validate_unique_sources(&sources)?;
    let mut samples = Vec::new();
    let mut audit = DatasetAudit::default();
    let mut skew_values = Vec::new();
    let mut action_coverages = Vec::new();
    let mut overexposed = 0u64;

    for source in &sources {
        verify_source(source)?;
        let messages = read_window_topics(&source.path, &TRANSITION_TOPICS, 0, u64::MAX)?;
        let ParsedEvents {
            mut cameras,
            mut lidars,
            mut poses,
            mut actions,
        } = parse_events(&messages, &source.primary_entity)?;
        cameras.sort_by_key(|event| event.reference.timestamp_ns);
        lidars.sort_by_key(|event| event.reference.timestamp_ns);
        poses.sort_by_key(|event| event.reference.timestamp_ns);
        actions.sort_by_key(|event| event.start_ns);
        validate_action_provenance(&actions)?;

        for pair in cameras.windows(2) {
            audit.candidate_transitions += 1;
            let current = &pair[0];
            let target = &pair[1];
            let gap = target
                .reference
                .timestamp_ns
                .saturating_sub(current.reference.timestamp_ns);
            let is_overexposed = current.luminance_mean > config.max_luminance_mean;
            overexposed += u64::from(is_overexposed);
            if gap == 0 || gap > config.max_frame_gap_ns {
                reject(&mut audit, "frame_gap");
                continue;
            }
            if !current.valid || !target.valid {
                reject(&mut audit, "camera_invalid");
                continue;
            }
            if current.calibration_id == "unavailable" || current.calibration_id.is_empty() {
                reject(&mut audit, "calibration_missing");
                continue;
            }
            if target.calibration_id == "unavailable"
                || target.calibration_id.is_empty()
                || target.calibration_id != current.calibration_id
            {
                reject(&mut audit, "target_calibration_mismatch");
                continue;
            }
            if !(config.min_luminance_mean..=config.max_luminance_mean)
                .contains(&current.luminance_mean)
            {
                reject(&mut audit, "exposure");
                continue;
            }
            if !(config.min_luminance_mean..=config.max_luminance_mean)
                .contains(&target.luminance_mean)
            {
                reject(&mut audit, "target_exposure");
                continue;
            }
            let Some((lidar, lidar_skew)) = nearest(&lidars, current.reference.timestamp_ns, |e| {
                e.reference.timestamp_ns
            }) else {
                reject(&mut audit, "lidar_missing");
                continue;
            };
            let Some((target_lidar, target_lidar_skew)) =
                nearest(&lidars, target.reference.timestamp_ns, |e| {
                    e.reference.timestamp_ns
                })
            else {
                reject(&mut audit, "target_lidar_missing");
                continue;
            };
            let Some((pose, pose_skew)) = nearest(&poses, current.reference.timestamp_ns, |e| {
                e.reference.timestamp_ns
            }) else {
                reject(&mut audit, "pose_missing");
                continue;
            };
            let Some((target_pose, target_pose_skew)) =
                nearest(&poses, target.reference.timestamp_ns, |e| {
                    e.reference.timestamp_ns
                })
            else {
                reject(&mut audit, "target_pose_missing");
                continue;
            };
            let max_skew = lidar_skew
                .max(target_lidar_skew)
                .max(pose_skew)
                .max(target_pose_skew);
            if max_skew > config.max_sensor_skew_ns {
                reject(&mut audit, "sensor_skew");
                continue;
            }
            if lidar.valid_fraction < config.min_lidar_valid_fraction {
                reject(&mut audit, "lidar_invalid");
                continue;
            }
            if target_lidar.valid_fraction < config.min_lidar_valid_fraction {
                reject(&mut audit, "target_lidar_invalid");
                continue;
            }
            if !target_lidar.has_grounded_occupancy {
                reject(&mut audit, "future_occupancy_ungrounded");
                continue;
            }
            if pose.confidence < config.min_pose_confidence {
                reject(&mut audit, "pose_confidence");
                continue;
            }
            if target_pose.confidence < config.min_pose_confidence {
                reject(&mut audit, "target_pose_confidence");
                continue;
            }
            let Some(action) = aggregate_actions(
                &actions,
                current.reference.timestamp_ns,
                target.reference.timestamp_ns,
            ) else {
                reject(&mut audit, "applied_action_missing");
                continue;
            };
            if action.coverage < config.min_action_coverage {
                reject(&mut audit, "action_coverage");
                continue;
            }

            let action_coverage = action.coverage;
            let sample_id = sample_id(
                &source.sha256,
                &source.primary_entity,
                current.reference.source_sequence,
                target.reference.source_sequence,
            );
            skew_values.push(max_skew);
            action_coverages.push(action.coverage);
            *audit
                .conditions
                .entry(source.condition.clone())
                .or_default() += 1;
            samples.push(TransitionSample {
                sample_id,
                session_id: source.session_id.clone(),
                environment_id: source.environment_id.clone(),
                condition: source.condition.clone(),
                // Environment-held-out assignment happens after every source
                // has been audited, so an empty catalog row cannot steer it.
                split: DatasetSplit::Train,
                mcap_sha256: source.sha256.clone(),
                observation_camera: current.reference.clone(),
                observation_lidar: lidar.reference.clone(),
                observation_pose: pose.reference.clone(),
                target_camera: target.reference.clone(),
                target_lidar: target_lidar.reference.clone(),
                target_pose: target_pose.reference.clone(),
                action,
                quality: TransitionQuality {
                    camera_luminance_mean: current.luminance_mean,
                    camera_luminance_stddev: current.luminance_stddev,
                    target_camera_luminance_mean: target.luminance_mean,
                    target_camera_luminance_stddev: target.luminance_stddev,
                    lidar_valid_fraction: lidar.valid_fraction,
                    target_lidar_valid_fraction: target_lidar.valid_fraction,
                    pose_confidence: pose.confidence,
                    target_pose_confidence: target_pose.confidence,
                    max_sensor_skew_ns: max_skew,
                    action_coverage,
                    calibration_id: current.calibration_id.clone(),
                },
            });
        }
    }

    let contributing_sessions = samples
        .iter()
        .map(|sample| sample.session_id.clone())
        .collect::<BTreeSet<_>>();
    let environment_splits =
        assign_environment_splits(samples.iter().map(|sample| sample.environment_id.as_str()));
    for sample in &mut samples {
        sample.split = environment_splits
            .get(&sample.environment_id)
            .cloned()
            .ok_or("valid transition environment is missing its split assignment")?;
        *audit.split_samples.entry(sample.split.clone()).or_default() += 1;
    }
    audit.valid_transitions = samples.len() as u64;
    audit.sessions = contributing_sessions.len() as u64;
    audit.environments = environment_splits.len() as u64;
    audit.mean_sensor_skew_ns = mean_u64(&skew_values);
    audit.p95_sensor_skew_ns = percentile95(&mut skew_values);
    audit.mean_action_coverage = mean_f32(&action_coverages);
    audit.overexposed_candidate_fraction = if audit.candidate_transitions == 0 {
        0.0
    } else {
        overexposed as f64 / audit.candidate_transitions as f64
    };

    let mut manifest = DatasetManifest {
        schema_version: MANIFEST_SCHEMA.to_string(),
        digest: String::new(),
        config,
        sources,
        samples,
        audit,
    };
    validate_dataset_manifest_integrity(&manifest)?;
    manifest.digest = manifest_digest(&manifest)?;
    Ok(manifest)
}

/// Write `manifest` under `root` at a name derived from its digest.
///
/// The write is temp-then-rename, and a rerun that lands on the same digest
/// must match byte for byte; anything else is a collision the caller has to
/// look at rather than an overwrite.
pub fn write_immutable_manifest(
    root: impl AsRef<Path>,
    manifest: &DatasetManifest,
) -> DatasetResult<PathBuf> {
    validate_dataset_manifest_integrity(manifest)?;
    if manifest.digest != manifest_digest(manifest)? {
        return Err("dataset manifest digest does not match its contents".into());
    }
    fs::create_dir_all(root.as_ref())?;
    let path = root
        .as_ref()
        .join(format!("jepa-dataset-{}.json", manifest.digest));
    let bytes = serde_json::to_vec_pretty(manifest)?;
    if path.exists() {
        if fs::read(&path)? != bytes {
            return Err(format!("immutable manifest collision at {}", path.display()).into());
        }
        return Ok(path);
    }
    let partial = path.with_extension("json.partial");
    let mut file = File::create(&partial)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&partial, &path)?;
    #[cfg(unix)]
    File::open(root.as_ref())?.sync_all()?;
    Ok(path)
}

/// SHA-256 over the manifest with the digest field cleared.
pub fn manifest_digest(manifest: &DatasetManifest) -> DatasetResult<String> {
    let mut unsigned = manifest.clone();
    unsigned.digest.clear();
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&unsigned)?)
    ))
}

/// Resolve every transition for one session from its immutable MCAP source.
///
/// Session-at-a-time materialization avoids reopening and decoding the segment
/// once per sample while keeping the manifest itself reference-only.
pub fn materialize_session(
    manifest: &DatasetManifest,
    session_id: &str,
) -> DatasetResult<Vec<MaterializedTransition>> {
    if manifest.schema_version != MANIFEST_SCHEMA {
        return Err("unsupported dataset manifest schema".into());
    }
    if manifest.digest != manifest_digest(manifest)? {
        return Err("dataset manifest digest does not match its contents".into());
    }
    validate_dataset_manifest_integrity(manifest)?;
    let source = manifest
        .sources
        .iter()
        .find(|source| source.session_id == session_id)
        .ok_or("dataset session is not present in the manifest")?;
    verify_source(source)?;
    let messages = read_window_topics(&source.path, &TRANSITION_TOPICS, 0, u64::MAX)?;

    // Index the three physical streams by full identity: a same-sequence
    // frame from another entity must not answer for the primary entity.
    let mut events = BTreeMap::<(String, String, u64, u64), Value>::new();
    for message in &messages {
        if !matches!(
            message.topic.as_str(),
            TOPIC_CAMERA | TOPIC_LIDAR | TOPIC_POSE
        ) {
            continue;
        }
        let value: Value = serde_json::from_slice(&message.data)?;
        let entity = string_field(&value, "entity")?;
        if entity != source.primary_entity {
            continue;
        }
        let source_sequence = u64_field(&value, "source_sequence")?;
        let timestamp_ns = u64_field(&value, "timestamp_ns")?;
        let key = (
            message.topic.clone(),
            entity.to_string(),
            source_sequence,
            timestamp_ns,
        );
        if events.insert(key, value).is_some() {
            return Err("duplicate sensor event identity in MCAP".into());
        }
    }
    let cameras = materialized_camera_refs(&events);
    let poses = materialized_poses(&events)?;
    let mut actions = parse_events(&messages, &source.primary_entity)?.actions;
    actions.sort_by_key(|action| action.start_ns);
    validate_action_provenance(&actions)?;

    manifest
        .samples
        .iter()
        .filter(|sample| sample.session_id == session_id)
        .map(|sample| {
            materialize_sample(
                manifest, source, sample, &events, &cameras, &poses, &actions,
            )
        })
        .collect()
}

fn materialize_sample(
    manifest: &DatasetManifest,
    source: &SessionEvidence,
    sample: &TransitionSample,
    events: &BTreeMap<(String, String, u64, u64), Value>,
    cameras: &[EventRef],
    poses: &[MaterializedPose],
    actions: &[ActionEvent],
) -> DatasetResult<MaterializedTransition> {
    if sample.mcap_sha256 != source.sha256
        || sample.environment_id != source.environment_id
        || sample.condition != source.condition
    {
        return Err("sample provenance does not match its source session".into());
    }
    let observation = materialize_observation(
        &sample.observation_camera,
        &sample.observation_lidar,
        &sample.observation_pose,
        events,
        cameras,
        poses,
        manifest.config,
    )?;
    let target_observation = materialize_observation(
        &sample.target_camera,
        &sample.target_lidar,
        &sample.target_pose,
        events,
        cameras,
        poses,
        manifest.config,
    )?;
    let target_lidar = event_value(events, &sample.target_lidar)?;
    let future_occupancy = materialize_occupancy(target_lidar)?;
    if (future_occupancy.resolution_m - manifest.config.grounding_resolution_m).abs() > 1.0e-6 {
        return Err(
            "future occupancy resolution does not match the frozen dataset geometry".into(),
        );
    }
    let recorded_action = aggregate_actions(
        actions,
        sample.observation_camera.timestamp_ns,
        sample.target_camera.timestamp_ns,
    )
    .ok_or("applied-action evidence cannot be rematerialized from MCAP")?;
    if recorded_action != sample.action {
        return Err("dataset applied-action aggregate does not match MCAP evidence".into());
    }
    let applied_action = AppliedAction {
        left: recorded_action.left,
        right: recorded_action.right,
        speed_scale: recorded_action.speed_scale,
        valid: recorded_action.coverage >= manifest.config.min_action_coverage,
    }
    .packed()?;
    let interval_ns = recorded_action
        .interval_end_ns
        .checked_sub(recorded_action.interval_start_ns)
        .ok_or("applied action interval is inverted")?;
    let delta_seconds = interval_ns as f32 / 1_000_000_000.0;
    if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
        return Err("applied action interval is empty or non-finite".into());
    }
    Ok(MaterializedTransition {
        sample_id: sample.sample_id.clone(),
        observation_sequence: sample.observation_camera.source_sequence,
        observation_timestamp_ns: sample.observation_camera.timestamp_ns,
        target_sequence: sample.target_camera.source_sequence,
        target_timestamp_ns: sample.target_camera.timestamp_ns,
        observation,
        target_observation,
        applied_action,
        delta_seconds,
        future_occupancy,
        quality: sample.quality.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn materialize_observation(
    camera_ref: &EventRef,
    lidar_ref: &EventRef,
    pose_ref: &EventRef,
    events: &BTreeMap<(String, String, u64, u64), Value>,
    cameras: &[EventRef],
    poses: &[MaterializedPose],
    config: DatasetConfig,
) -> DatasetResult<Vec<f32>> {
    let camera = event_value(events, camera_ref)?;
    let lidar = event_value(events, lidar_ref)?;
    let thumbnail = camera
        .get("thumbnail_luma")
        .and_then(Value::as_array)
        .ok_or("camera thumbnail_luma is missing")?;
    if thumbnail.len() != CAMERA_PIXELS
        || u64_field(camera, "thumb_width")? != THUMB_WIDTH
        || u64_field(camera, "thumb_height")? != THUMB_HEIGHT
    {
        return Err("camera thumbnail shape does not match 48x64".into());
    }
    let camera_luma = thumbnail
        .iter()
        .map(|entry| {
            let value = entry
                .as_u64()
                .filter(|value| *value <= u8::MAX as u64)
                .ok_or("camera thumbnail contains an invalid luma value")?;
            Ok(value as f32 / 255.0)
        })
        .collect::<DatasetResult<Vec<_>>>()?;

    let points = lidar
        .get("points")
        .and_then(Value::as_array)
        .ok_or("LiDAR points are missing")?;
    let validity = lidar
        .get("validity_mask")
        .and_then(Value::as_array)
        .ok_or("LiDAR validity mask is missing")?;
    if points.len() != validity.len() || points.len() > LIDAR_BINS {
        return Err("LiDAR points and validity have incompatible shapes".into());
    }
    let mut ranges = vec![0.0; LIDAR_BINS];
    let mut valid = vec![false; LIDAR_BINS];
    for (index, (point, validity)) in points.iter().zip(validity).enumerate() {
        let is_valid = validity
            .as_bool()
            .ok_or("LiDAR validity contains a non-boolean value")?;
        let distance = f32_field(point, "distance_m")?;
        if is_valid && distance > 0.0 {
            ranges[index] = (distance / config.lidar_max_range_m).clamp(0.0, 1.0);
            valid[index] = true;
        }
    }

    let pose = pose_features(poses, cameras, pose_ref, camera_ref, config)?;
    let packed = pack_observation(&camera_luma, &ranges, &valid, pose)?;
    debug_assert_eq!(packed.len(), OBS_DIM);
    Ok(packed.to_vec())
}

fn materialized_camera_refs(events: &BTreeMap<(String, String, u64, u64), Value>) -> Vec<EventRef> {
    let mut cameras = events
        .keys()
        .filter(|(topic, _, _, _)| topic == TOPIC_CAMERA)
        .map(|(topic, entity, source_sequence, timestamp_ns)| EventRef {
            topic: topic.clone(),
            entity: entity.clone(),
            source_sequence: *source_sequence,
            timestamp_ns: *timestamp_ns,
        })
        .collect::<Vec<_>>();
    cameras.sort_by_key(|camera| camera.timestamp_ns);
    cameras
}

fn materialized_poses(
    events: &BTreeMap<(String, String, u64, u64), Value>,
) -> DatasetResult<Vec<MaterializedPose>> {
    let mut poses = events
        .iter()
        .filter(|((topic, _, _, _), _)| topic == TOPIC_POSE)
        .map(|((topic, entity, source_sequence, timestamp_ns), value)| {
            Ok(MaterializedPose {
                reference: EventRef {
                    topic: topic.clone(),
                    entity: entity.clone(),
                    source_sequence: *source_sequence,
                    timestamp_ns: *timestamp_ns,
                },
                x_m: f32_field(value, "x_m")?,
                z_m: f32_field(value, "z_m")?,
                yaw_rad: f32_field(value, "yaw_rad")?,
                confidence: f32_field(value, "confidence")?,
            })
        })
        .collect::<DatasetResult<Vec<_>>>()?;
    poses.sort_by_key(|pose| pose.reference.timestamp_ns);
    Ok(poses)
}

fn pose_features(
    poses: &[MaterializedPose],
    cameras: &[EventRef],
    pose_reference: &EventRef,
    camera_reference: &EventRef,
    config: DatasetConfig,
) -> DatasetResult<PoseFeatures> {
    let pose = poses
        .iter()
        .find(|pose| pose.reference == *pose_reference)
        .ok_or("pose event is missing from MCAP")?;
    let camera_index = cameras
        .iter()
        .position(|camera| camera == camera_reference)
        .ok_or("camera event is missing from MCAP")?;
    let mut linear_mps = 0.0;
    let mut angular_rps = 0.0;
    if let Some(previous_camera) = camera_index
        .checked_sub(1)
        .and_then(|index| cameras.get(index))
    {
        let previous_pose = nearest(poses, previous_camera.timestamp_ns, |entry| {
            entry.reference.timestamp_ns
        });
        if let Some((previous_pose, _)) =
            previous_pose.filter(|(_, skew_ns)| *skew_ns <= config.max_sensor_skew_ns)
        {
            let elapsed_ns = camera_reference
                .timestamp_ns
                .saturating_sub(previous_camera.timestamp_ns);
            if elapsed_ns > 0 {
                let elapsed = elapsed_ns as f32 / 1_000_000_000.0;
                (linear_mps, angular_rps) = camera_step_velocity(
                    [pose.x_m, pose.z_m],
                    pose.yaw_rad,
                    [previous_pose.x_m, previous_pose.z_m],
                    previous_pose.yaw_rad,
                    elapsed,
                )?;
            }
        }
    }
    let normalized_age = pose
        .reference
        .timestamp_ns
        .abs_diff(camera_reference.timestamp_ns) as f32
        / config.max_sensor_skew_ns as f32;
    Ok(PoseFeatures {
        x_m: pose.x_m,
        z_m: pose.z_m,
        yaw_rad: pose.yaw_rad,
        linear_mps,
        angular_rps,
        normalized_age: normalized_age.clamp(0.0, 1.0),
        valid: pose.confidence >= config.min_pose_confidence,
    })
}

fn materialize_occupancy(value: &Value) -> DatasetResult<MaterializedOccupancy> {
    let occupancy = value
        .get("occupancy")
        .filter(|entry| !entry.is_null())
        .ok_or("future occupancy is missing")?;
    let width = u64_field(occupancy, "width")? as usize;
    let height = u64_field(occupancy, "height")? as usize;
    let cell_count = width
        .checked_mul(height)
        .ok_or("future occupancy dimensions overflow")?;
    let cells = occupancy
        .get("cells")
        .and_then(Value::as_array)
        .ok_or("future occupancy cells are missing")?;
    let observed = occupancy
        .get("observed")
        .and_then(Value::as_array)
        .ok_or("future occupancy observed mask is missing")?;
    if cell_count == 0 || cells.len() != cell_count || observed.len() != cell_count {
        return Err("future occupancy arrays do not match their dimensions".into());
    }
    let occupied = cells
        .iter()
        .map(|cell| {
            cell.as_u64()
                .filter(|value| *value <= u8::MAX as u64)
                .map(|value| f32::from(value > 0))
                .ok_or_else(|| "future occupancy contains an invalid cell".into())
        })
        .collect::<DatasetResult<Vec<_>>>()?;
    let observed = observed
        .iter()
        .map(|cell| {
            cell.as_bool()
                .ok_or_else(|| "future occupancy contains an invalid observed value".into())
        })
        .collect::<DatasetResult<Vec<_>>>()?;
    Ok(MaterializedOccupancy {
        width,
        height,
        resolution_m: f32_field(occupancy, "resolution_m")?,
        origin_x_m: f32_field(occupancy, "origin_x_m")?,
        origin_y_m: f32_field(occupancy, "origin_y_m")?,
        occupied,
        observed,
    })
}

fn event_value<'a>(
    events: &'a BTreeMap<(String, String, u64, u64), Value>,
    reference: &EventRef,
) -> DatasetResult<&'a Value> {
    events
        .get(&(
            reference.topic.clone(),
            reference.entity.clone(),
            reference.source_sequence,
            reference.timestamp_ns,
        ))
        .ok_or_else(|| format!("missing MCAP event for {}", reference.topic).into())
}

fn validate_config(config: DatasetConfig) -> DatasetResult<()> {
    if config.max_sensor_skew_ns == 0 || config.max_frame_gap_ns == 0 {
        return Err("dataset time gates must be nonzero".into());
    }
    for (name, value) in [
        ("min_action_coverage", config.min_action_coverage),
        ("min_lidar_valid_fraction", config.min_lidar_valid_fraction),
        ("min_pose_confidence", config.min_pose_confidence),
        ("min_luminance_mean", config.min_luminance_mean),
        ("max_luminance_mean", config.max_luminance_mean),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!("{name} must be finite and within [0,1]").into());
        }
    }
    if config.min_luminance_mean >= config.max_luminance_mean {
        return Err("luminance bounds are inverted".into());
    }
    if !config.lidar_max_range_m.is_finite()
        || config.lidar_max_range_m <= 0.0
        || !config.grounding_resolution_m.is_finite()
        || config.grounding_resolution_m <= 0.0
    {
        return Err("LiDAR range and grounding resolution must be finite and positive".into());
    }
    Ok(())
}

fn validate_unique_sources(sources: &[SessionEvidence]) -> DatasetResult<()> {
    let mut session_ids = BTreeSet::new();
    let mut evidence_entities = BTreeSet::new();
    for source in sources {
        if !session_ids.insert(source.session_id.as_str()) {
            return Err(format!("duplicate dataset session id: {}", source.session_id).into());
        }
        if !evidence_entities.insert((source.sha256.as_str(), source.primary_entity.as_str())) {
            return Err(format!(
                "duplicate MCAP evidence for primary entity {}: {}",
                source.primary_entity, source.sha256
            )
            .into());
        }
    }
    Ok(())
}

fn verify_source(source: &SessionEvidence) -> DatasetResult<()> {
    if source.session_id.is_empty()
        || source.environment_id.is_empty()
        || source.condition.is_empty()
        || source.primary_entity.is_empty()
    {
        return Err("session, environment, condition, and primary entity must be non-empty".into());
    }
    let bytes = fs::read(&source.path)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != source.sha256 {
        return Err(format!("MCAP digest mismatch for session {}", source.session_id).into());
    }
    Ok(())
}

fn parse_events(messages: &[LoggedMessage], primary_entity: &str) -> DatasetResult<ParsedEvents> {
    let mut cameras = Vec::new();
    let mut lidars = Vec::new();
    let mut poses = Vec::new();
    let mut actions = Vec::new();
    for message in messages {
        if !matches!(
            message.topic.as_str(),
            TOPIC_CAMERA | TOPIC_LIDAR | TOPIC_POSE | TOPIC_ACTION_APPLIED
        ) {
            continue;
        }
        let value: Value = serde_json::from_slice(&message.data)?;
        let entity = string_field(&value, "entity")?;
        if entity != primary_entity {
            continue;
        }
        let source_sequence = u64_field(&value, "source_sequence")?;
        let timestamp_ns = u64_field(&value, "timestamp_ns")?;
        let reference = EventRef {
            topic: message.topic.clone(),
            entity: entity.to_string(),
            source_sequence,
            timestamp_ns,
        };
        match message.topic.as_str() {
            TOPIC_CAMERA => cameras.push(CameraEvent {
                reference,
                valid: bool_field(&value, "valid")?,
                luminance_mean: f32_field(&value, "luminance_mean")?,
                luminance_stddev: f32_field(&value, "luminance_stddev")?,
                calibration_id: string_field(&value, "calibration_id")?.to_string(),
            }),
            TOPIC_LIDAR => {
                let mask = value
                    .get("validity_mask")
                    .and_then(Value::as_array)
                    .ok_or("LiDAR validity_mask is missing")?;
                let valid = mask
                    .iter()
                    .filter(|entry| entry.as_bool() == Some(true))
                    .count();
                let valid_fraction = if mask.is_empty() {
                    0.0
                } else {
                    valid as f32 / mask.len() as f32
                };
                let occupancy = value.get("occupancy").filter(|entry| !entry.is_null());
                let has_grounded_occupancy = occupancy.is_some_and(|occupancy| {
                    let cells = occupancy.get("cells").and_then(Value::as_array);
                    let observed = occupancy.get("observed").and_then(Value::as_array);
                    matches!((cells, observed), (Some(cells), Some(observed)) if !cells.is_empty() && cells.len() == observed.len())
                });
                lidars.push(LidarEvent {
                    reference,
                    valid_fraction,
                    has_grounded_occupancy,
                });
            }
            TOPIC_POSE => poses.push(PoseEvent {
                reference,
                confidence: f32_field(&value, "confidence")?,
            }),
            TOPIC_ACTION_APPLIED => actions.push(parse_action_event(
                message,
                &value,
                source_sequence,
                timestamp_ns,
            )?),
            _ => {}
        }
    }
    Ok(ParsedEvents {
        cameras,
        lidars,
        poses,
        actions,
    })
}

fn parse_action_event(
    message: &LoggedMessage,
    value: &Value,
    source_sequence: u64,
    timestamp_ns: u64,
) -> DatasetResult<ActionEvent> {
    if string_field(value, "schema_version")? != ACTION_SCHEMA
        || string_field(value, "stage")? != ACTION_STAGE
    {
        return Err("unsupported applied-action evidence contract".into());
    }
    let producer_epoch = u64_field(value, "producer_epoch")?;
    let history_sequence = u64_field(value, "history_sequence")?;
    let start_ns = u64_field(value, "interval_start_ns")?;
    let end_ns = u64_field(value, "interval_end_ns")?;
    let requested_left = f32_field(value, "requested_left")?;
    let requested_right = f32_field(value, "requested_right")?;
    let clamped_left = f32_field(value, "clamped_left")?;
    let clamped_right = f32_field(value, "clamped_right")?;
    let applied_left = f32_field(value, "applied_left")?;
    let applied_right = f32_field(value, "applied_right")?;
    let left = f32_field(value, "left")?;
    let right = f32_field(value, "right")?;
    let speed_scale = f32_field(value, "speed_scale")?;
    let safety_flags = u32_field(value, "safety_flags")?;
    let valid = bool_field(value, "valid")?;
    let _armed = bool_field(value, "armed")?;
    let _deadman_active = bool_field(value, "deadman_active")?;
    let collision_clamped = bool_field(value, "collision_clamped")?;
    let leash_authority = match value.get("authority") {
        Some(Value::Number(authority)) => {
            authority.as_u64() == Some(u64::from(ACTION_AUTHORITY_LEASH))
        }
        Some(Value::String(authority)) => authority == "leash",
        _ => return Err("applied-action authority is missing or malformed".into()),
    };
    if history_sequence == 0
        || producer_epoch == 0
        || source_sequence == 0
        || start_ns == 0
        || end_ns <= start_ns
        || timestamp_ns != end_ns
        || message.log_time_ns != end_ns
        || message.publish_time_ns < message.log_time_ns
        || left != applied_left
        || right != applied_right
        || [requested_left, requested_right]
            .iter()
            .any(|value| !value.is_finite())
        || [clamped_left, clamped_right, applied_left, applied_right]
            .iter()
            .any(|value| value.abs() > 1.0)
        || !(0.0..=1.0).contains(&speed_scale)
        || collision_clamped != (safety_flags & LEASH_ACTION_SAFETY_COLLISION_CLAMP != 0)
        || (!valid && (applied_left != 0.0 || applied_right != 0.0))
    {
        return Err("invalid applied-action evidence contract".into());
    }
    Ok(ActionEvent {
        history_sequence,
        producer_epoch,
        source_sequence,
        start_ns,
        end_ns,
        left,
        right,
        speed_scale,
        safety_flags,
        valid,
        leash_authority,
    })
}

fn validate_action_provenance(actions: &[ActionEvent]) -> DatasetResult<()> {
    for pair in actions.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let history_invalid =
            previous.history_sequence.checked_add(1) != Some(current.history_sequence);
        let same_epoch_invalid = current.producer_epoch == previous.producer_epoch
            && (current.source_sequence <= previous.source_sequence
                || current.end_ns <= previous.end_ns);
        let restart_invalid = current.producer_epoch != previous.producer_epoch
            && (current.producer_epoch < previous.producer_epoch
                || current.end_ns <= previous.end_ns);
        if history_invalid
            || current.start_ns < previous.end_ns
            || same_epoch_invalid
            || restart_invalid
        {
            return Err("applied-action provenance overlaps, rolls back, or mutates".into());
        }
    }
    Ok(())
}

/// Average the applied actions over `start_ns..end_ns`, weighted by overlap.
///
/// An action that is not valid, not leash-authoritative or empty is skipped,
/// and a gap or an overlap inside the window makes the whole aggregate
/// untrustworthy, so the window yields nothing rather than partial numbers.
fn aggregate_actions(
    actions: &[ActionEvent],
    start_ns: u64,
    end_ns: u64,
) -> Option<AppliedActionAggregate> {
    let duration = end_ns.checked_sub(start_ns)?;
    if duration == 0 {
        return None;
    }
    let mut covered = 0u64;
    let mut left = 0.0f64;
    let mut right = 0.0f64;
    let mut scale = 0.0f64;
    let mut flags = 0u32;
    let mut sequences = Vec::new();
    let mut last_overlap_end = None;
    for action in actions {
        if !action.valid || !action.leash_authority || action.end_ns <= action.start_ns {
            continue;
        }
        let overlap_start = start_ns.max(action.start_ns);
        let overlap_end = end_ns.min(action.end_ns);
        let overlap = overlap_end.saturating_sub(overlap_start);
        if overlap == 0 {
            continue;
        }
        if last_overlap_end.is_some_and(|previous| overlap_start < previous) {
            return None;
        }
        last_overlap_end = Some(overlap_end);
        covered = covered.saturating_add(overlap);
        let weight = overlap as f64;
        left += f64::from(action.left) * weight;
        right += f64::from(action.right) * weight;
        scale += f64::from(action.speed_scale) * weight;
        flags |= action.safety_flags;
        sequences.push(action.source_sequence);
    }
    if covered == 0 || covered > duration {
        return None;
    }
    let divisor = covered as f64;
    Some(AppliedActionAggregate {
        source_sequences: sequences,
        left: (left / divisor) as f32,
        right: (right / divisor) as f32,
        speed_scale: (scale / divisor) as f32,
        interval_start_ns: start_ns,
        interval_end_ns: end_ns,
        coverage: covered as f32 / duration as f32,
        safety_flags: flags,
    })
}

/// Nearest item to `timestamp_ns`, preferring the earlier one on a tie.
fn nearest<T, F>(items: &[T], timestamp_ns: u64, timestamp: F) -> Option<(&T, u64)>
where
    F: Fn(&T) -> u64,
{
    if items.is_empty() {
        return None;
    }
    let next = items.partition_point(|item| timestamp(item) < timestamp_ns);
    match (next.checked_sub(1), items.get(next)) {
        (Some(previous), Some(current)) => {
            let previous = &items[previous];
            let previous_skew = timestamp(previous).abs_diff(timestamp_ns);
            let current_skew = timestamp(current).abs_diff(timestamp_ns);
            if previous_skew <= current_skew {
                Some((previous, previous_skew))
            } else {
                Some((current, current_skew))
            }
        }
        (Some(previous), None) => {
            let previous = &items[previous];
            Some((previous, timestamp(previous).abs_diff(timestamp_ns)))
        }
        (None, Some(current)) => Some((current, timestamp(current).abs_diff(timestamp_ns))),
        (None, None) => None,
    }
}

/// Assign whole environments to train, validation and test, deterministically.
///
/// Ranks environments by a hash of the identifier so the assignment does not
/// follow catalog order, holds out up to a tenth of them (at least one and at
/// most half) for validation and the same for test, and keeps the environment
/// carrying the most physical evidence in the training split so a large
/// primary collection is never the one held out.
fn assign_environment_splits<'a>(
    environment_ids: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, DatasetSplit> {
    let mut sample_counts = BTreeMap::<String, u64>::new();
    for environment_id in environment_ids {
        *sample_counts.entry(environment_id.to_string()).or_default() += 1;
    }
    let mut ranked = sample_counts.keys().cloned().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        Sha256::digest(left.as_bytes())
            .cmp(&Sha256::digest(right.as_bytes()))
            .then_with(|| left.cmp(right))
    });

    let held_out_per_split = if ranked.len() < 3 {
        0
    } else {
        (ranked.len() / 10).max(1).min((ranked.len() - 1) / 2)
    };
    if held_out_per_split > 0 {
        // BTreeMap iteration is ascending by identifier, so an explicit fold
        // gives "most evidence, then smallest identifier" without depending on
        // the tie-breaking quirks of an iterator adapter.
        let mut largest: Option<(&String, u64)> = None;
        for (environment_id, count) in &sample_counts {
            let better = match largest {
                None => true,
                Some((current_id, current_count)) => {
                    *count > current_count
                        || (*count == current_count && environment_id < current_id)
                }
            };
            if better {
                largest = Some((environment_id, *count));
            }
        }
        if let Some(index) = largest
            .and_then(|(environment_id, _)| ranked.iter().position(|item| item == environment_id))
            .filter(|index| *index < held_out_per_split * 2)
        {
            let environment_id = ranked.remove(index);
            ranked.push(environment_id);
        }
    }
    ranked
        .into_iter()
        .enumerate()
        .map(|(index, environment_id)| {
            let split = if index < held_out_per_split {
                DatasetSplit::Validation
            } else if index < held_out_per_split * 2 {
                DatasetSplit::Test
            } else {
                DatasetSplit::Train
            };
            (environment_id, split)
        })
        .collect()
}

/// Stable identity for a transition, derived from its evidence, never a counter.
fn sample_id(digest: &str, entity: &str, current_sequence: u64, target_sequence: u64) -> String {
    let input = format!("{digest}:{entity}:{current_sequence}:{target_sequence}");
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

fn reject(audit: &mut DatasetAudit, reason: &str) {
    *audit.rejected.entry(reason.to_string()).or_default() += 1;
}

fn mean_u64(values: &[u64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64
    }
}

fn mean_f32(values: &[f32]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64
    }
}

fn percentile95(values: &mut [u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * 0.95).round() as usize;
    values[index]
}

fn u64_field(value: &Value, field: &str) -> DatasetResult<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing unsigned field {field}").into())
}

fn u32_field(value: &Value, field: &str) -> DatasetResult<u32> {
    u64_field(value, field)?
        .try_into()
        .map_err(|_| format!("field {field} exceeds u32").into())
}

fn f32_field(value: &Value, field: &str) -> DatasetResult<f32> {
    let number = match value.get(field).and_then(Value::as_f64) {
        Some(number) => number as f32,
        None => return Err(format!("missing numeric field {field}").into()),
    };
    if !number.is_finite() {
        return Err(format!("field {field} is non-finite").into());
    }
    Ok(number)
}

fn bool_field(value: &Value, field: &str) -> DatasetResult<bool> {
    value
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing boolean field {field}").into())
}

fn string_field<'a>(value: &'a Value, field: &str) -> DatasetResult<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {field}").into())
}

#[cfg(test)]
mod tests;








