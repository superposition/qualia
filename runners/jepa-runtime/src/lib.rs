//! Observe-only JEPA inference.
//!
//! The runner owns exactly one output contract: read the coherent physical
//! tuple the rest of the stack has already published, score the transition
//! with the checkpoint a generation pointer names, and publish evidence and
//! telemetry into the JEPA slots. It holds no writer for the canonical world,
//! the beliefs or the motors, so a misconfiguration cannot turn inference into
//! motion.
//!
//! Every input path fails closed. A generation pointer is loaded only from a
//! complete document naming an approved observe-only checkpoint; a transition
//! is scored only when the camera, LiDAR and pose samples agree in time and
//! the Leash action history explains the interval between them.

use qualia_jepa::{
    camera_step_velocity, pack_observation, AppliedAction, ObservationQuality, PoseFeatures,
    CAMERA_HEIGHT, CAMERA_PIXELS, CAMERA_WIDTH, LIDAR_BINS,
};
use qualia_jepa_model::runtime::{CoherentJepaRuntime, RuntimeInput, RuntimeOutput};
use qualia_jepa_model::{device_for_backend, ARCHITECTURE_ID};
use qualia_shm::ShmRegion;
use qualia_types::{
    AppliedActionSnapshot, JepaEvidencePayload, JepaTelemetryPayload, NavPose, SnapshotError,
    ACTION_AUTHORITY_LEASH, APPLIED_ACTION_HISTORY_CAPACITY, JEPA_BACKEND_CPU, JEPA_BACKEND_CUDA,
    JEPA_BACKEND_METAL, JEPA_FLAG_ACTION_COVERED, JEPA_FLAG_CHECKPOINT_VERIFIED,
    JEPA_FLAG_GROUNDING_AVAILABLE, JEPA_FLAG_OUTPUT_FINITE, JEPA_FLAG_SOURCES_COHERENT,
    JEPA_FLAG_VALID, JEPA_ID_BYTES, JEPA_MODE_OBSERVE_ONLY, LEASH_ACTION_SAFETY_COLLISION_CLAMP,
};
#[cfg(feature = "sim")]
use qualia_types::{FlySimPayload, FLY_SIM_MAX_TYPES};
#[cfg(feature = "sim")]
use qualia_fly_circuit::{CircuitSim, SIM_ID};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, Ordering};

/// Result alias for every fallible entry point in this crate.
pub type RuntimeResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Schema tag a generation pointer must carry to be accepted.
pub const GENERATION_SCHEMA: &str = "qualia.jepa-generation.v1";

/// Latency samples retained for the published percentiles.
pub const MAX_LATENCY_SAMPLES: usize = 256;

/// The complete generation pointer installed beside a checkpoint directory.
///
/// The document is written to a staging path and renamed into place, so a
/// reader either sees the whole previous pointer or the whole next one. Every
/// field is part of the operator-visible contract: the identity digests are
/// re-checked against the checkpoint after it has been loaded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Reject anything that is not an explicitly approved observe-only
    /// generation with a filename-safe identity and a full SHA-256 digest.
    fn validate(&self) -> RuntimeResult<()> {
        let id_safe = !self.checkpoint_id.is_empty()
            && self
                .checkpoint_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        let digest_well_formed = self.weights_sha256.len() == 64
            && self.weights_sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
        let approved = self.mode == "observe-only" && self.approved_observe_only;
        let acceptable = self.schema_version == GENERATION_SCHEMA
            && self.generation != 0
            && !self.checkpoint_dir.is_empty()
            && id_safe
            && digest_well_formed
            && approved;
        if !acceptable {
            return Err("invalid or unapproved observe-only generation pointer".into());
        }
        Ok(())
    }
}

/// Frozen limits for live capture, correlation and preprocessing.
///
/// The default validity thresholds are the same ones dataset materialization
/// uses, so a live transition and an offline transition are admitted on the
/// same terms.
#[derive(Debug, Clone, Copy)]
pub struct ObserveOnlyConfig {
    pub max_source_age_ns: u64,
    pub max_source_skew_ns: u64,
    pub max_action_edge_slop_ns: u64,
    pub min_action_coverage: f32,
    pub min_lidar_valid_fraction: f32,
    pub min_pose_confidence: f32,
    pub lidar_max_range_m: f32,
    pub calibration_valid: bool,
}

impl Default for ObserveOnlyConfig {
    fn default() -> Self {
        Self {
            max_source_age_ns: 500_000_000,
            max_source_skew_ns: 150_000_000,
            max_action_edge_slop_ns: 50_000_000,
            min_action_coverage: 0.8,
            min_lidar_valid_fraction: 0.1,
            min_pose_confidence: 0.1,
            lidar_max_range_m: 8.0,
            calibration_valid: false,
        }
    }
}

impl ObserveOnlyConfig {
    /// Every threshold must be usable before a single frame is read.
    fn validate(self) -> RuntimeResult<Self> {
        let age_usable = self.max_source_age_ns > 0 && self.max_source_skew_ns > 0;
        let unit_usable = |value: f32| value.is_finite() && (0.0..=1.0).contains(&value);
        let range_usable = self.lidar_max_range_m.is_finite() && self.lidar_max_range_m > 0.0;
        let acceptable = age_usable
            && unit_usable(self.min_action_coverage)
            && unit_usable(self.min_lidar_valid_fraction)
            && unit_usable(self.min_pose_confidence)
            && range_usable;
        if !acceptable {
            return Err("invalid observe-only runtime configuration".into());
        }
        Ok(self)
    }
}

/// What one tick did, as observed by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// No new coherent tuple, or a refusal that was published as telemetry.
    Idle,
    /// The first coherent frame was stored; there is nothing to score yet.
    Primed,
    /// Evidence and telemetry for one scored transition were published.
    Published { inference_seq: u64 },
    /// An increasing generation was loaded and swapped in between ticks.
    GenerationSwapped { generation: u64 },
}

/// One checkpoint held resident with its verified pointer.
struct LoadedGeneration {
    pointer: GenerationPointer,
    runtime: CoherentJepaRuntime,
}

/// The coherent physical tuple that one inference consumes.
#[derive(Clone)]
struct SensorFrame {
    observation: Vec<f32>,
    timestamp_ns: u64,
    camera_seq: u64,
    lidar_seq: u64,
    pose_seq: u64,
    camera_age_ms: f32,
    lidar_age_ms: f32,
    pose_age_ms: f32,
    source_skew_ns: u64,
    quality: f32,
    pose: NavPose,
}

/// The duration-weighted action applied across one transition.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActionAggregate {
    left: f32,
    right: f32,
    speed_scale: f32,
    source_sequence: u64,
    interval_end_ns: u64,
    coverage: f32,
}

/// Counters that are published verbatim in the telemetry payload.
#[derive(Default)]
struct Counters {
    inferences: u64,
    dropped: u64,
    stale: u64,
    non_finite: u64,
    swaps: u64,
    last_inference_ns: u64,
}

/// The fly rate model, held at rest and published observe-only.
///
/// Built only when `QUALIA_FLY_MODE=sim`, `QUALIA_FLY_PRIOR_PATH` names a
/// loadable prior artifact and the `sim` feature is compiled in; a missing or
/// rejected artifact disables the simulator without failing the runner, the
/// way the belief runners treat the same keys. Nothing in the JEPA path is a
/// drive for the prior's type graph, so a tick advances the model one fixed
/// step with a zero drive and publishes the resulting rate vector. The slot is
/// written only while the active generation pointer carries
/// `approved_observe_only`, and nothing reads it back into the belief or motor
/// paths.
#[cfg(feature = "sim")]
struct FlySimPublisher {
    sim: CircuitSim,
    drive: Vec<f32>,
    step: u64,
    producer_epoch: u64,
}

#[cfg(feature = "sim")]
impl FlySimPublisher {
    /// Fixed integration step of one published state, seconds.
    const STEP_SECONDS: f32 = 0.001;

    /// Build from the fly environment, or `None` when the mode is not `sim`.
    fn from_env(producer_epoch: u64) -> Option<Self> {
        Self::from_parts(
            std::env::var("QUALIA_FLY_MODE").ok().as_deref(),
            std::env::var("QUALIA_FLY_PRIOR_PATH").ok().as_deref(),
            producer_epoch,
        )
    }

    /// The mode and artifact decision, without reading the environment.
    fn from_parts(mode: Option<&str>, path: Option<&str>, producer_epoch: u64) -> Option<Self> {
        if mode != Some("sim") {
            return None;
        }
        let path = path.unwrap_or_default();
        if path.trim().is_empty() {
            eprintln!("fly sim: disabled (QUALIA_FLY_PRIOR_PATH is unset)");
            return None;
        }
        let sim = match CircuitSim::load(Path::new(path)) {
            Ok(sim) => sim,
            Err(error) => {
                eprintln!("fly sim: disabled ({error})");
                return None;
            }
        };
        let type_count = sim.type_count();
        if type_count > FLY_SIM_MAX_TYPES {
            eprintln!(
                "fly sim: disabled (prior carries {type_count} types, the slot holds {FLY_SIM_MAX_TYPES})"
            );
            return None;
        }
        Some(Self {
            sim,
            drive: vec![0.0; type_count],
            step: 0,
            producer_epoch,
        })
    }

    /// Step the model once and publish the state, if observing is approved.
    fn tick(
        &mut self,
        shm: &ShmRegion,
        now_ns: u64,
        approved_observe_only: bool,
        runner_epoch: u64,
    ) -> RuntimeResult<()> {
        if !approved_observe_only {
            return Ok(());
        }
        let rates = self.sim.step(&self.drive, Self::STEP_SECONDS);
        self.step = self.step.saturating_add(1);
        let mut payload = FlySimPayload {
            type_count: rates.len() as u32,
            producer_epoch: self.producer_epoch,
            runner_epoch,
            sim_step: self.step,
            timestamp_ns: now_ns,
            dt: Self::STEP_SECONDS,
            ..FlySimPayload::default()
        };
        write_id(&mut payload.sim_id, SIM_ID);
        let finite = rates.iter().all(|rate| rate.is_finite());
        if finite {
            payload.flags = JEPA_FLAG_VALID | JEPA_FLAG_OUTPUT_FINITE;
        } else {
            payload.last_error[..15].copy_from_slice(b"non-finite rate");
            payload.last_error_len = 15;
        }
        payload.state[..rates.len()].copy_from_slice(&rates);
        shm.fly_sim().publish(payload)?;
        Ok(())
    }
}

/// Flag-gated observe-only runner.
///
/// One instance keeps the active generation resident, parks the immediately
/// previous one for rollback, and publishes only the JEPA evidence and
/// telemetry slots.
pub struct ObserveOnlyRunner {
    generation_file: PathBuf,
    backend_name: String,
    backend_code: u32,
    config: ObserveOnlyConfig,
    producer_epoch: u64,
    live: LoadedGeneration,
    parked: Option<LoadedGeneration>,
    last_frame: Option<SensorFrame>,
    last_key: Option<(u64, u64, u64)>,
    counters: Counters,
    latencies: VecDeque<u64>,
    #[cfg(feature = "sim")]
    fly: Option<FlySimPublisher>,
}

impl ObserveOnlyRunner {
    /// Load and verify the generation the pointer names, then run it.
    pub fn open(
        generation_file: impl AsRef<Path>,
        backend_name: impl Into<String>,
        config: ObserveOnlyConfig,
        producer_epoch: u64,
    ) -> RuntimeResult<Self> {
        let generation_file = generation_file.as_ref().to_path_buf();
        let backend_name = backend_name.into();
        let backend_code = backend_code(&backend_name)?;
        let config = config.validate()?;
        let live = load_generation(read_generation(&generation_file)?, &backend_name)?;
        #[cfg(feature = "sim")]
        let fly = FlySimPublisher::from_env(producer_epoch);
        Ok(Self {
            generation_file,
            backend_name,
            backend_code,
            config,
            producer_epoch,
            live,
            parked: None,
            last_frame: None,
            last_key: None,
            counters: Counters::default(),
            latencies: VecDeque::with_capacity(MAX_LATENCY_SAMPLES),
            #[cfg(feature = "sim")]
            fly,
        })
    }

    /// Generation number of the loaded checkpoint.
    pub fn active_generation(&self) -> u64 {
        self.live.pointer.generation
    }

    /// Checkpoint identity of the loaded generation.
    pub fn active_checkpoint_id(&self) -> &str {
        &self.live.pointer.checkpoint_id
    }

    /// Advance the fly simulator one step and publish it.
    ///
    /// The publisher is dropped, loudly, if a publication fails: the slot is a
    /// monitor, and the monitor never fails the inference it watches.
    #[cfg(feature = "sim")]
    fn step_fly_sim(&mut self, shm: &ShmRegion, now_ns: u64) {
        let Some(fly) = self.fly.as_mut() else {
            return;
        };
        let published = fly.tick(
            shm,
            now_ns,
            self.live.pointer.approved_observe_only,
            self.live.pointer.generation,
        );
        if let Err(error) = published {
            eprintln!("fly sim: disabled ({error})");
            self.fly = None;
        }
    }

    /// Advance the runner by at most one inference.
    ///
    /// A generation change takes precedence over inference: the new pointer is
    /// read, loaded and digest-checked before it replaces the active one, and
    /// the previous generation is parked for rollback. Every other refusal
    /// publishes telemetry that carries the diagnostic.
    pub fn tick(&mut self, shm: &ShmRegion, now_ns: u64) -> RuntimeResult<TickOutcome> {
        match self.reconcile_generation() {
            Ok(true) => {
                self.publish_telemetry(shm, None, None)?;
                return Ok(TickOutcome::GenerationSwapped {
                    generation: self.live.pointer.generation,
                });
            }
            Ok(false) => {}
            Err(error) => {
                self.counters.dropped = self.counters.dropped.saturating_add(1);
                self.publish_telemetry(shm, None, Some(&error.to_string()))?;
                return Ok(TickOutcome::Idle);
            }
        }

        #[cfg(feature = "sim")]
        self.step_fly_sim(shm, now_ns);

        let frame = match capture_observation(shm, now_ns, self.config, self.last_frame.as_ref()) {
            Ok(frame) => frame,
            Err(error) => {
                self.counters.stale = self.counters.stale.saturating_add(1);
                self.publish_telemetry(shm, None, Some(&error.to_string()))?;
                return Ok(TickOutcome::Idle);
            }
        };
        let key = (frame.camera_seq, frame.lidar_seq, frame.pose_seq);
        if self.last_key == Some(key) {
            return Ok(TickOutcome::Idle);
        }
        self.last_key = Some(key);

        let previous = match self.last_frame.replace(frame.clone()) {
            Some(previous) => previous,
            None => {
                self.publish_telemetry(shm, Some(&frame), None)?;
                return Ok(TickOutcome::Primed);
            }
        };
        let delta_ns = frame.timestamp_ns.saturating_sub(previous.timestamp_ns);
        if delta_ns == 0 || delta_ns > self.config.max_source_age_ns {
            self.counters.stale = self.counters.stale.saturating_add(1);
            self.publish_telemetry(shm, Some(&frame), Some("invalid frame interval"))?;
            return Ok(TickOutcome::Idle);
        }

        let action = match correlate_action_history(
            shm,
            previous.timestamp_ns,
            frame.timestamp_ns,
            self.config,
        ) {
            Ok(action) => action,
            Err(error) => {
                self.counters.stale = self.counters.stale.saturating_add(1);
                self.publish_telemetry(shm, Some(&frame), Some(&error.to_string()))?;
                return Ok(TickOutcome::Idle);
            }
        };
        let applied_action = pack_action(action.as_ref())?;
        let input = RuntimeInput {
            observation: previous.observation,
            applied_action,
            delta_seconds: delta_ns as f32 / 1_000_000_000.0,
        };
        let output = match self.live.runtime.observe_transition(&input, &frame.observation) {
            Ok(output) => output,
            Err(error) => {
                self.counters.dropped = self.counters.dropped.saturating_add(1);
                self.publish_telemetry(shm, Some(&frame), Some(&error.to_string()))?;
                return Ok(TickOutcome::Idle);
            }
        };
        if !output.is_finite() {
            self.counters.non_finite = self.counters.non_finite.saturating_add(1);
            self.publish_telemetry(shm, Some(&frame), Some("non-finite runtime output"))?;
            return Ok(TickOutcome::Idle);
        }

        self.counters.inferences = self.counters.inferences.saturating_add(1);
        self.counters.last_inference_ns = now_ns;
        remember_latency(&mut self.latencies, output.synchronized_latency_us);
        self.publish_evidence(shm, now_ns, &frame, action.as_ref(), &output)?;
        self.publish_telemetry(shm, Some(&frame), None)?;
        Ok(TickOutcome::Published {
            inference_seq: self.counters.inferences,
        })
    }

    /// Read the pointer again and adopt an increasing generation.
    ///
    /// Returns `true` when the active generation changed. A pointer that names
    /// the parked checkpoint and digest is reused without touching the disk
    /// again, which is what makes an immediate rollback cheap.
    fn reconcile_generation(&mut self) -> RuntimeResult<bool> {
        let next = read_generation(&self.generation_file)?;
        if next.generation == self.live.pointer.generation {
            if next != self.live.pointer {
                return Err("active generation was mutated in place".into());
            }
            return Ok(false);
        }
        if next.generation < self.live.pointer.generation {
            return Err("generation counter moved backwards".into());
        }
        let recycled = match self.parked.as_ref() {
            Some(parked)
                if parked.pointer.checkpoint_id == next.checkpoint_id
                    && parked.pointer.weights_sha256 == next.weights_sha256 =>
            {
                self.parked.take()
            }
            _ => None,
        };
        let loaded = match recycled {
            Some(mut parked) => {
                parked.pointer = next;
                parked
            }
            None => load_generation(next, &self.backend_name)?,
        };
        let previous = std::mem::replace(&mut self.live, loaded);
        self.parked = Some(previous);
        self.last_frame = None;
        self.last_key = None;
        self.counters.swaps = self.counters.swaps.saturating_add(1);
        Ok(true)
    }

    /// Publish one scored transition into the evidence slot.
    fn publish_evidence(
        &self,
        shm: &ShmRegion,
        now_ns: u64,
        frame: &SensorFrame,
        action: Option<&ActionAggregate>,
        output: &RuntimeOutput,
    ) -> RuntimeResult<()> {
        let mut payload = JepaEvidencePayload {
            backend: self.backend_code,
            mode: JEPA_MODE_OBSERVE_ONLY,
            flags: evidence_flags(action.is_some(), frame.quality),
            producer_epoch: self.producer_epoch,
            runner_epoch: self.live.pointer.generation,
            inference_seq: self.counters.inferences,
            timestamp_ns: now_ns,
            camera_seq: frame.camera_seq,
            lidar_seq: frame.lidar_seq,
            pose_seq: frame.pose_seq,
            action_seq: action.map_or(0, |action| action.source_sequence),
            camera_age_ms: frame.camera_age_ms,
            lidar_age_ms: frame.lidar_age_ms,
            pose_age_ms: frame.pose_age_ms,
            action_age_ms: action.map_or(f32::INFINITY, |action| {
                age_ms(now_ns, action.interval_end_ns)
            }),
            source_skew_ns: frame.source_skew_ns.min(i64::MAX as u64) as i64,
            observation_quality: frame.quality,
            transition_nll: output.transition_nll,
            occupancy_confidence: occupancy_confidence(&output.occupancy_logits),
            ..JepaEvidencePayload::default()
        };
        write_id(&mut payload.model_id, ARCHITECTURE_ID);
        write_id(
            &mut payload.checkpoint_id,
            &self.live.pointer.checkpoint_id,
        );
        payload.latent.copy_from_slice(&output.latent);
        payload.predicted_mean.copy_from_slice(&output.predicted_mean);
        payload
            .predicted_log_variance
            .copy_from_slice(&output.predicted_log_variance);
        payload.evidence.copy_from_slice(&output.evidence);
        payload
            .occupancy_logits
            .copy_from_slice(&output.occupancy_logits);
        shm.jepa_evidence().publish(payload)?;
        Ok(())
    }

    /// Publish the running counters, ages and the last refusal, if any.
    fn publish_telemetry(
        &self,
        shm: &ShmRegion,
        frame: Option<&SensorFrame>,
        error: Option<&str>,
    ) -> RuntimeResult<()> {
        let samples = &self.latencies;
        let mut payload = JepaTelemetryPayload {
            backend: self.backend_code,
            mode: JEPA_MODE_OBSERVE_ONLY,
            flags: JEPA_FLAG_CHECKPOINT_VERIFIED,
            producer_epoch: self.producer_epoch,
            runner_epoch: self.live.pointer.generation,
            inference_count: self.counters.inferences,
            dropped_frames: self.counters.dropped,
            stale_frames: self.counters.stale,
            non_finite_outputs: self.counters.non_finite,
            hot_swaps: self.counters.swaps,
            last_inference_ns: self.counters.last_inference_ns,
            latency_p50_us: percentile(samples, 50),
            latency_p95_us: percentile(samples, 95),
            latency_last_us: samples
                .back()
                .copied()
                .unwrap_or(0)
                .min(u64::from(u32::MAX)) as u32,
            latency_max_us: samples
                .iter()
                .copied()
                .max()
                .unwrap_or(0)
                .min(u64::from(u32::MAX)) as u32,
            camera_age_ms: frame.map_or(0.0, |frame| frame.camera_age_ms),
            lidar_age_ms: frame.map_or(0.0, |frame| frame.lidar_age_ms),
            pose_age_ms: frame.map_or(0.0, |frame| frame.pose_age_ms),
            source_skew_ns: frame
                .map_or(0, |frame| frame.source_skew_ns.min(i64::MAX as u64) as i64),
            ..JepaTelemetryPayload::default()
        };
        write_id(&mut payload.model_id, ARCHITECTURE_ID);
        write_id(
            &mut payload.checkpoint_id,
            &self.live.pointer.checkpoint_id,
        );
        if let Some(error) = error {
            payload.last_error_code = 1;
            let bytes = error.as_bytes();
            let copied = bytes.len().min(payload.last_error.len());
            payload.last_error[..copied].copy_from_slice(&bytes[..copied]);
            payload.last_error_len = copied as u32;
        }
        shm.jepa_telemetry().publish(payload)?;
        Ok(())
    }
}

/// The evidence flags a fully verified observation earns.
fn evidence_flags(action_covered: bool, quality: f32) -> u32 {
    let mut flags = JEPA_FLAG_SOURCES_COHERENT
        | JEPA_FLAG_CHECKPOINT_VERIFIED
        | JEPA_FLAG_OUTPUT_FINITE
        | JEPA_FLAG_GROUNDING_AVAILABLE;
    if action_covered {
        flags |= JEPA_FLAG_ACTION_COVERED;
        if quality > 0.0 {
            flags |= JEPA_FLAG_VALID;
        }
    }
    flags
}

/// Read and validate the installed generation pointer.
fn read_generation(path: &Path) -> RuntimeResult<GenerationPointer> {
    let bytes = fs::read(path)?;
    let pointer: GenerationPointer = serde_json::from_slice(&bytes)?;
    pointer.validate()?;
    Ok(pointer)
}

/// Load the checkpoint the pointer names and re-check its identity.
fn load_generation(
    pointer: GenerationPointer,
    backend_name: &str,
) -> RuntimeResult<LoadedGeneration> {
    let runtime = CoherentJepaRuntime::from_checkpoint(
        &pointer.checkpoint_dir,
        device_for_backend(backend_name)?,
    )?;
    let identity_matches = runtime.checkpoint_id() == pointer.checkpoint_id
        && runtime.weights_sha256() == pointer.weights_sha256;
    if !identity_matches {
        return Err("generation pointer does not match the checkpoint".into());
    }
    Ok(LoadedGeneration { pointer, runtime })
}

/// Map a backend name onto its published code.
fn backend_code(name: &str) -> RuntimeResult<u32> {
    match name {
        "cpu" => Ok(JEPA_BACKEND_CPU),
        "metal" => Ok(JEPA_BACKEND_METAL),
        "cuda" => Ok(JEPA_BACKEND_CUDA),
        _ => Err(format!("unsupported JEPA backend {name}").into()),
    }
}

/// Read one coherent camera, LiDAR and pose tuple and preprocess it.
///
/// The frame is refused when any source is missing, when the tuple spans more
/// than the source-skew budget, when the oldest sample exceeds the age budget,
/// or when any sample is further ahead of the caller's clock than the budget
/// allows. A finite positive LiDAR return beyond the model range saturates to
/// normalized range `1.0` rather than being reclassified as missing.
fn capture_observation(
    shm: &ShmRegion,
    now_ns: u64,
    config: ObserveOnlyConfig,
    previous: Option<&SensorFrame>,
) -> RuntimeResult<SensorFrame> {
    let camera = shm.camera_frame().snapshot(8)?;
    let lidar = shm.lidar_scan().snapshot(8)?;
    let (pose_seq, pose) = coherent_pose(shm, 8)?;

    let shape_ok = camera.thumb_width as usize == CAMERA_WIDTH
        && camera.thumb_height as usize == CAMERA_HEIGHT
        && camera.thumbnail_luma.len() == CAMERA_PIXELS;
    if camera.seq == 0 || lidar.seq == 0 || pose_seq == 0 || !shape_ok {
        return Err("physical observation is incomplete".into());
    }

    let lidar_timestamp_ns = lidar.scan_end_ns.max(lidar.scan_start_ns);
    let oldest_ns = camera
        .timestamp_ns
        .min(lidar_timestamp_ns)
        .min(pose.timestamp_ns);
    let newest_ns = camera
        .timestamp_ns
        .max(lidar_timestamp_ns)
        .max(pose.timestamp_ns);
    let source_skew_ns = newest_ns - oldest_ns;
    let oldest_age_ns = now_ns.saturating_sub(oldest_ns);
    if oldest_ns == 0
        || newest_ns > now_ns.saturating_add(config.max_source_skew_ns)
        || source_skew_ns > config.max_source_skew_ns
        || oldest_age_ns > config.max_source_age_ns
    {
        return Err("physical observation is stale or sensor-skewed".into());
    }

    let luma = camera
        .thumbnail_luma
        .iter()
        .map(|value| f32::from(*value) / 255.0)
        .collect::<Vec<_>>();
    let mut ranges = vec![0.0_f32; LIDAR_BINS];
    let mut valid = vec![false; LIDAR_BINS];
    let returned = (lidar.point_count as usize).min(LIDAR_BINS);
    for index in 0..returned {
        let point = &lidar.points[index];
        let usable =
            point.intensity != 0 && point.distance_m.is_finite() && point.distance_m > 0.0;
        if usable {
            ranges[index] = (point.distance_m / config.lidar_max_range_m).clamp(0.0, 1.0);
            valid[index] = true;
        }
    }
    let valid_fraction = valid.iter().filter(|flag| **flag).count() as f32 / LIDAR_BINS as f32;

    let (linear_mps, angular_rps) = match previous {
        Some(previous) => {
            let elapsed_s =
                camera.timestamp_ns.saturating_sub(previous.timestamp_ns) as f32 / 1_000_000_000.0;
            if elapsed_s > 0.0 {
                camera_step_velocity(
                    [pose.x_m, pose.z_m],
                    pose.yaw_rad,
                    [previous.pose.x_m, previous.pose.z_m],
                    previous.pose.yaw_rad,
                    elapsed_s,
                )?
            } else {
                (0.0, 0.0)
            }
        }
        None => (0.0, 0.0),
    };
    let confidence_ok = pose.confidence.is_finite() && pose.confidence >= config.min_pose_confidence;
    let pose_camera_skew_ns = pose.timestamp_ns.abs_diff(camera.timestamp_ns);
    let features = PoseFeatures {
        x_m: pose.x_m,
        z_m: pose.z_m,
        yaw_rad: pose.yaw_rad,
        linear_mps,
        angular_rps,
        normalized_age: (pose_camera_skew_ns as f32 / config.max_source_skew_ns as f32)
            .clamp(0.0, 1.0),
        valid: confidence_ok,
    };
    let observation = pack_observation(&luma, &ranges, &valid, features)?.to_vec();

    let freshness =
        (1.0 - oldest_age_ns as f32 / config.max_source_age_ns as f32).clamp(0.0, 1.0);
    let exposure = if (0.05..=0.95).contains(&camera.luminance_mean) {
        (1.0 - (camera.luminance_mean - 0.5).abs() / 0.45).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let modalities_usable =
        camera.valid && valid_fraction >= config.min_lidar_valid_fraction && confidence_ok;
    let modality_validity = if modalities_usable {
        valid_fraction * pose.confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let quality = ObservationQuality::new(
        freshness,
        exposure,
        f32::from(config.calibration_valid),
        modality_validity,
    )?
    .precision_scale();

    Ok(SensorFrame {
        observation,
        timestamp_ns: camera.timestamp_ns,
        camera_seq: camera.seq,
        lidar_seq: lidar.seq,
        pose_seq,
        camera_age_ms: age_ms(now_ns, camera.timestamp_ns),
        lidar_age_ms: age_ms(now_ns, lidar_timestamp_ns),
        pose_age_ms: age_ms(now_ns, pose.timestamp_ns),
        source_skew_ns,
        quality,
        pose,
    })
}

/// Read the robot pose under the world-model seqlock.
///
/// A pair of equal acquire sequence reads brackets the pose copy, so a
/// concurrent publisher either lands wholly before or wholly after it.
fn coherent_pose(shm: &ShmRegion, max_attempts: usize) -> Result<(u64, NavPose), SnapshotError> {
    let world = shm.world_model();
    for _ in 0..max_attempts.max(1) {
        let before = world.nav_seq.load(Ordering::Acquire);
        // SAFETY: `NavPose` is plain C-layout data inside the mapping, and the
        // sequence pair rejects any write that completed between the loads.
        let pose = unsafe { std::ptr::read_volatile(&world.robot_pose) };
        fence(Ordering::Acquire);
        let after = world.nav_seq.load(Ordering::Acquire);
        if before == after {
            return Ok((after, pose));
        }
    }
    Err(SnapshotError::TornRead)
}

/// Pack an aggregate, or an explicitly invalid zero action, for the encoder.
fn pack_action(aggregate: Option<&ActionAggregate>) -> RuntimeResult<[f32; 4]> {
    let (left, right, speed_scale) = aggregate.map_or((0.0, 0.0, 0.0), |aggregate| {
        (aggregate.left, aggregate.right, aggregate.speed_scale)
    });
    Ok(AppliedAction {
        left,
        right,
        speed_scale,
        valid: aggregate.is_some(),
    }
    .packed()?)
}

/// Drain the lossless Leash history for every interval that overlaps the
/// transition, oldest first.
fn correlate_action_history(
    shm: &ShmRegion,
    previous_timestamp_ns: u64,
    current_timestamp_ns: u64,
    config: ObserveOnlyConfig,
) -> RuntimeResult<Option<ActionAggregate>> {
    if previous_timestamp_ns >= current_timestamp_ns {
        return Ok(None);
    }
    let history = shm.applied_action_history();
    let newest = history.committed_seq();
    if newest == 0 {
        return Ok(None);
    }
    let oldest = newest
        .saturating_sub(APPLIED_ACTION_HISTORY_CAPACITY as u64)
        .saturating_add(1);
    let window_start = previous_timestamp_ns.saturating_sub(config.max_action_edge_slop_ns);
    let window_end = current_timestamp_ns.saturating_add(config.max_action_edge_slop_ns);
    let mut overlapping = Vec::new();
    for sequence in (oldest..=newest).rev() {
        let interval = history
            .snapshot(sequence, 8)
            .map_err(|error| format!("applied-action history read failed: {error}"))?;
        if interval.interval_end_ns < window_start {
            break;
        }
        if interval.interval_start_ns > window_end {
            continue;
        }
        overlapping.push(interval);
    }
    overlapping.reverse();
    aggregate_action_intervals(
        &overlapping,
        previous_timestamp_ns,
        current_timestamp_ns,
        config,
    )
}

/// Whether two chronologically adjacent history entries continue one another.
fn intervals_are_ordered(prior: &AppliedActionSnapshot, next: &AppliedActionSnapshot) -> bool {
    if prior.slot_seq.checked_add(1) != Some(next.slot_seq) {
        return false;
    }
    if next.interval_start_ns < prior.interval_end_ns {
        return false;
    }
    if next.interval_end_ns <= prior.interval_end_ns {
        return false;
    }
    if prior.producer_epoch == next.producer_epoch {
        next.action_sequence > prior.action_sequence
    } else {
        next.producer_epoch > prior.producer_epoch
    }
}

/// Weight every interval by the time it actually overlaps the transition.
///
/// The aggregate is admitted only when the history is contiguous, finite,
/// bounded, Leash-authored, edge-aligned and covers at least the configured
/// fraction of the transition. Anything else yields no action at all, which is
/// how a missing or inconsistent history becomes invalid evidence instead of a
/// silently applied zero.
fn aggregate_action_intervals(
    intervals: &[AppliedActionSnapshot],
    previous_timestamp_ns: u64,
    current_timestamp_ns: u64,
    config: ObserveOnlyConfig,
) -> RuntimeResult<Option<ActionAggregate>> {
    let duration_ns = match current_timestamp_ns.checked_sub(previous_timestamp_ns) {
        Some(duration) if duration > 0 => duration,
        _ => return Ok(None),
    };
    let mut weighted = [0.0_f64; 3];
    let mut covered_ns = 0_u64;
    let mut first_covered_ns = None::<u64>;
    let mut last_covered_ns = None::<u64>;
    let mut prior = None::<&AppliedActionSnapshot>;
    let mut closing = None::<&AppliedActionSnapshot>;
    for interval in intervals {
        if !valid_action_interval(interval) {
            return Ok(None);
        }
        if let Some(prior) = prior {
            if !intervals_are_ordered(prior, interval) {
                return Ok(None);
            }
        }
        prior = Some(interval);

        let overlap_start = previous_timestamp_ns.max(interval.interval_start_ns);
        let overlap_end = current_timestamp_ns.min(interval.interval_end_ns);
        let overlap_ns = overlap_end.saturating_sub(overlap_start);
        if overlap_ns == 0 {
            continue;
        }
        if last_covered_ns.is_some_and(|end| overlap_start < end) {
            return Ok(None);
        }
        if first_covered_ns.is_none() {
            first_covered_ns = Some(overlap_start);
        }
        last_covered_ns = Some(overlap_end);
        covered_ns = covered_ns.saturating_add(overlap_ns);
        if covered_ns > duration_ns {
            return Ok(None);
        }
        let weight = overlap_ns as f64;
        weighted[0] += f64::from(interval.applied_left) * weight;
        weighted[1] += f64::from(interval.applied_right) * weight;
        weighted[2] += f64::from(interval.speed_scale) * weight;
        closing = Some(interval);
    }
    if covered_ns == 0 {
        return Ok(None);
    }
    let coverage = covered_ns as f32 / duration_ns as f32;
    let leading_aligned = first_covered_ns.is_some_and(|start| {
        start <= previous_timestamp_ns.saturating_add(config.max_action_edge_slop_ns)
    });
    let trailing_aligned = last_covered_ns.is_some_and(|end| {
        end.saturating_add(config.max_action_edge_slop_ns) >= current_timestamp_ns
    });
    if coverage < config.min_action_coverage || !leading_aligned || !trailing_aligned {
        return Ok(None);
    }
    let closing = closing.expect("a covered overlap records the interval that closed it");
    let divisor = covered_ns as f64;
    let aggregate = ActionAggregate {
        left: (weighted[0] / divisor) as f32,
        right: (weighted[1] / divisor) as f32,
        speed_scale: (weighted[2] / divisor) as f32,
        source_sequence: closing.action_sequence,
        interval_end_ns: closing.interval_end_ns,
        coverage,
    };
    if !aggregate.left.is_finite()
        || !aggregate.right.is_finite()
        || !aggregate.speed_scale.is_finite()
    {
        return Ok(None);
    }
    Ok(Some(aggregate))
}

/// Whether one history entry is a complete, finite, Leash-authored interval.
fn valid_action_interval(interval: &AppliedActionSnapshot) -> bool {
    let values = [
        interval.requested_left,
        interval.requested_right,
        interval.clamped_left,
        interval.clamped_right,
        interval.applied_left,
        interval.applied_right,
        interval.speed_scale,
    ];
    let applied = [
        interval.clamped_left,
        interval.clamped_right,
        interval.applied_left,
        interval.applied_right,
    ];
    let stamped =
        interval.producer_epoch > 0 && interval.slot_seq > 0 && interval.action_sequence > 0;
    let span_ok =
        interval.interval_start_ns > 0 && interval.interval_end_ns > interval.interval_start_ns;
    let collision_consistent = interval.collision_clamped
        == (interval.safety_flags & LEASH_ACTION_SAFETY_COLLISION_CLAMP != 0);
    stamped
        && interval.valid
        && interval.authority == ACTION_AUTHORITY_LEASH
        && span_ok
        && values.iter().all(|value| value.is_finite())
        && applied.iter().all(|value| value.abs() <= 1.0)
        && (0.0..=1.0).contains(&interval.speed_scale)
        && collision_consistent
}

/// Age of a source timestamp at publication, in milliseconds.
fn age_ms(now_ns: u64, timestamp_ns: u64) -> f32 {
    now_ns.saturating_sub(timestamp_ns) as f32 / 1_000_000.0
}

/// Mean decisiveness of the occupancy head, in `[0, 1]`.
fn occupancy_confidence(logits: &[f32]) -> f32 {
    if logits.is_empty() {
        return 0.0;
    }
    let total: f32 = logits
        .iter()
        .map(|logit| (sigmoid(*logit) - 0.5).abs() * 2.0)
        .sum();
    total / logits.len() as f32
}

/// Logistic function, evaluated on the branch that cannot overflow.
fn sigmoid(logit: f32) -> f32 {
    if logit >= 0.0 {
        1.0 / (1.0 + (-logit).exp())
    } else {
        let exp = logit.exp();
        exp / (1.0 + exp)
    }
}

/// Copy a UTF-8 identifier into a fixed, null-terminated field.
fn write_id(field: &mut [u8; JEPA_ID_BYTES], value: &str) {
    let bytes = value.as_bytes();
    let copied = bytes.len().min(field.len() - 1);
    field[..copied].copy_from_slice(&bytes[..copied]);
}

/// Keep at most [`MAX_LATENCY_SAMPLES`] latency samples, newest last.
fn remember_latency(samples: &mut VecDeque<u64>, latency_us: u64) {
    while samples.len() >= MAX_LATENCY_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(latency_us);
}

/// Nearest-rank percentile of the retained latency samples.
fn percentile(values: &VecDeque<u64>, percentile: usize) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u64> = values.iter().copied().collect();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) * percentile)
        .div_ceil(100)
        .min(sorted.len() - 1);
    sorted[index].min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{DType, Device};
    use candle_nn::{VarBuilder, VarMap};
    use qualia_jepa::{OBS_DIM, POSE_DIM};
    use qualia_jepa_model::{
        empty_candidate_manifest, initialize_deterministic, write_candidate_checkpoint, ActionSupport,
        BaselineGate, CheckpointManifest, GroundingGeometry, HeldOutMetrics, JepaCandidateModel,
    };
    use qualia_types::{
        CameraFrameSnapshot, LidarPoint, LidarScanSnapshot, LIDAR_MAX_POINTS,
    };
    use tempfile::TempDir;

    /// A gate result that clears the frozen dataset scale and both baselines.
    fn passing_gate() -> BaselineGate {
        BaselineGate {
            dataset_digest: "c".repeat(64),
            valid_transitions: 60_000,
            sessions: 20,
            conditions: 4,
            environments: 4,
            constant: HeldOutMetrics {
                transition_nll: 3.0,
                rollout_error: 3.0,
            },
            flat_mlp: HeldOutMetrics {
                transition_nll: 2.0,
                rollout_error: 2.0,
            },
            tiny_cnn: HeldOutMetrics {
                transition_nll: 0.5,
                rollout_error: 0.5,
            },
        }
    }

    /// A measured action envelope that clears the minimum sample count.
    fn passing_support() -> ActionSupport {
        ActionSupport {
            sample_count: 60_000,
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
            min_delta_seconds: 0.02,
            max_delta_seconds: 0.6,
        }
    }

    fn passing_geometry() -> GroundingGeometry {
        GroundingGeometry {
            width: 64,
            height: 64,
            resolution_m: 0.05,
        }
    }

    /// Materialize a candidate checkpoint and return the pointer naming it.
    fn checkpoint_pointer(root: &Path, id: &str, seed: u64) -> GenerationPointer {
        let variables = VarMap::new();
        JepaCandidateModel::load(VarBuilder::from_varmap(&variables, DType::F32, &Device::Cpu))
            .expect("register the candidate tensors");
        initialize_deterministic(&variables, seed).expect("seed the candidate tensors");
        let manifest = empty_candidate_manifest(
            id,
            &"c".repeat(64),
            seed,
            "cpu",
            passing_gate(),
            passing_support(),
            passing_geometry(),
        );
        let (_, manifest_path) =
            write_candidate_checkpoint(root, id, &variables, &variables, manifest)
                .expect("write the candidate checkpoint");
        let written: CheckpointManifest =
            serde_json::from_slice(&fs::read(manifest_path).expect("read the manifest"))
                .expect("parse the manifest");
        GenerationPointer {
            schema_version: GENERATION_SCHEMA.to_string(),
            generation: 1,
            checkpoint_dir: root.join(id).display().to_string(),
            checkpoint_id: id.to_string(),
            weights_sha256: written.weights_sha256,
            mode: "observe-only".to_string(),
            approved_observe_only: true,
        }
    }

    /// Install a pointer the way an operator does: stage, then rename.
    fn install_pointer(path: &Path, pointer: &GenerationPointer) {
        let staging = path.with_extension("partial");
        fs::write(
            &staging,
            serde_json::to_vec_pretty(pointer).expect("serialize the pointer"),
        )
        .expect("stage the pointer");
        fs::rename(&staging, path).expect("rename the pointer into place");
    }

    /// A fresh region with a name unique to this test process.
    fn fresh_region(tag: &str) -> ShmRegion {
        let name = format!("/qualia_jepa_c42_{tag}_{}", std::process::id());
        #[cfg(unix)]
        unsafe {
            if let Ok(c_name) = std::ffi::CString::new(name.clone()) {
                libc::shm_unlink(c_name.as_ptr());
            }
        }
        ShmRegion::create(&name).expect("create the JEPA test region")
    }

    /// Publish one coherent camera, LiDAR and pose tuple.
    fn publish_sources(shm: &ShmRegion, timestamp_ns: u64, token: u64) {
        let mut camera = CameraFrameSnapshot {
            timestamp_ns,
            source_width: 640,
            source_height: 480,
            thumb_width: 64,
            thumb_height: 48,
            luminance_mean: 0.5,
            luminance_stddev: 0.2,
            valid: true,
            ..CameraFrameSnapshot::default()
        };
        for (index, value) in camera.thumbnail_luma.iter_mut().enumerate() {
            *value = ((index as u64 + token) % 255) as u8;
        }
        shm.camera_frame_mut()
            .publish(&camera)
            .expect("publish the camera frame");

        let mut scan = LidarScanSnapshot {
            scan_start_ns: timestamp_ns.saturating_sub(1_000_000),
            scan_end_ns: timestamp_ns,
            point_count: LIDAR_MAX_POINTS as u32,
            ..LidarScanSnapshot::default()
        };
        for point in &mut scan.points {
            *point = LidarPoint {
                angle_rad: 0.0,
                distance_m: 2.0,
                intensity: 10,
                _pad: [0; 3],
            };
        }
        shm.lidar_scan_mut()
            .publish(&scan)
            .expect("publish the lidar scan");

        shm.set_robot_pose(NavPose {
            x_m: token as f32 * 0.001,
            y_m: 0.0,
            z_m: 0.0,
            yaw_rad: 0.0,
            pitch_rad: 0.0,
            roll_rad: 0.0,
            confidence: 1.0,
            _pad0: 0.0,
            timestamp_ns,
        });
    }

    /// Append one completed, Leash-authored interval to the lossless history.
    fn publish_interval(
        shm: &ShmRegion,
        sequence: u64,
        start_ns: u64,
        end_ns: u64,
        left: f32,
        right: f32,
        speed_scale: f32,
    ) {
        let interval = AppliedActionSnapshot {
            producer_epoch: 1,
            action_sequence: sequence,
            interval_start_ns: start_ns,
            interval_end_ns: end_ns,
            requested_left: left,
            requested_right: right,
            clamped_left: left,
            clamped_right: right,
            applied_left: left,
            applied_right: right,
            speed_scale,
            authority: ACTION_AUTHORITY_LEASH,
            valid: true,
            ..AppliedActionSnapshot::default()
        };
        shm.applied_action()
            .publish(interval)
            .expect("publish the applied action");
        shm.applied_action_history()
            .append(interval)
            .expect("append the lossless action interval");
    }

    fn read_id(field: &[u8]) -> &str {
        let end = field
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(field.len());
        std::str::from_utf8(&field[..end]).expect("identifiers are UTF-8")
    }

    #[test]
    fn coherent_tick_publishes_evidence_and_rollback_keeps_foreign_slots() {
        let artifacts = TempDir::new().expect("artifact directory");
        let mut alpha = checkpoint_pointer(artifacts.path(), "gen-alpha", 11);
        let mut beta = checkpoint_pointer(artifacts.path(), "gen-beta", 22);
        beta.generation = 2;
        let pointer_path = artifacts.path().join("current.json");
        install_pointer(&pointer_path, &alpha);

        let shm = fresh_region("evidence");
        let mut runner = ObserveOnlyRunner::open(
            &pointer_path,
            "cpu",
            ObserveOnlyConfig {
                calibration_valid: true,
                ..ObserveOnlyConfig::default()
            },
            4_242,
        )
        .expect("open the runner");

        let first_ns = 1_000_000_000;
        publish_sources(&shm, first_ns, 1);
        assert_eq!(
            runner.tick(&shm, first_ns + 10_000_000).expect("tick"),
            TickOutcome::Primed
        );

        let second_ns = first_ns + 100_000_000;
        publish_sources(&shm, second_ns, 2);
        publish_interval(&shm, 1, first_ns, second_ns, 0.25, -0.25, 0.5);
        let camera_seq = shm.camera_frame().snapshot(8).expect("camera").seq;
        let lidar_seq = shm.lidar_scan().snapshot(8).expect("lidar").seq;
        let nav_seq = shm.world_model().nav_seq.load(Ordering::Acquire);
        assert_eq!(
            runner.tick(&shm, second_ns + 10_000_000).expect("tick"),
            TickOutcome::Published { inference_seq: 1 }
        );

        let evidence = shm.jepa_evidence().snapshot(8).expect("evidence");
        assert_eq!(evidence.inference_seq, 1);
        assert_eq!(evidence.backend, JEPA_BACKEND_CPU);
        assert_eq!(evidence.mode, JEPA_MODE_OBSERVE_ONLY);
        assert_eq!(evidence.action_seq, 1);
        assert_eq!(evidence.camera_seq, camera_seq);
        assert_eq!(evidence.lidar_seq, lidar_seq);
        assert_eq!(read_id(&evidence.model_id), ARCHITECTURE_ID);
        assert_eq!(read_id(&evidence.checkpoint_id), "gen-alpha");
        assert_eq!(
            evidence.flags & (JEPA_FLAG_VALID | JEPA_FLAG_ACTION_COVERED),
            JEPA_FLAG_VALID | JEPA_FLAG_ACTION_COVERED
        );
        assert!(evidence.observation_quality > 0.0);
        assert!(evidence.latent.iter().any(|value| *value != 0.0));
        assert_eq!(evidence.producer_epoch, 4_242);

        let telemetry = shm.jepa_telemetry().snapshot(8).expect("telemetry");
        assert_eq!(telemetry.inference_count, 1);
        assert_eq!(telemetry.hot_swaps, 0);
        assert_eq!(telemetry.last_inference_ns, second_ns + 10_000_000);
        assert_eq!(telemetry.last_error_code, 0);

        // A corrupt pointer is refused without disturbing the active model.
        fs::write(&pointer_path, b"{ truncated").expect("write a partial pointer");
        assert_eq!(
            runner.tick(&shm, second_ns + 15_000_000).expect("tick"),
            TickOutcome::Idle
        );
        assert_eq!(runner.active_checkpoint_id(), "gen-alpha");
        let telemetry = shm.jepa_telemetry().snapshot(8).expect("telemetry");
        assert_eq!(telemetry.last_error_code, 1);
        assert!(telemetry.last_error_len > 0);
        assert_eq!(telemetry.dropped_frames, 1);

        // An increasing generation is loaded and swapped between ticks.
        install_pointer(&pointer_path, &beta);
        assert_eq!(
            runner.tick(&shm, second_ns + 20_000_000).expect("tick"),
            TickOutcome::GenerationSwapped { generation: 2 }
        );
        assert_eq!(runner.active_generation(), 2);
        assert_eq!(runner.active_checkpoint_id(), "gen-beta");

        // The parked generation is reused when the pointer names it again.
        alpha.generation = 3;
        install_pointer(&pointer_path, &alpha);
        assert_eq!(
            runner.tick(&shm, second_ns + 30_000_000).expect("tick"),
            TickOutcome::GenerationSwapped { generation: 3 }
        );
        assert_eq!(runner.active_checkpoint_id(), "gen-alpha");
        assert_eq!(shm.jepa_telemetry().snapshot(8).expect("telemetry").hot_swaps, 2);

        // The runner never wrote outside its own slots.
        assert_eq!(
            shm.camera_frame().snapshot(8).expect("camera").seq,
            camera_seq
        );
        assert_eq!(shm.lidar_scan().snapshot(8).expect("lidar").seq, lidar_seq);
        assert_eq!(
            shm.applied_action().snapshot(8).expect("action").action_sequence,
            1
        );
        assert_eq!(shm.world_model().nav_seq.load(Ordering::Acquire), nav_seq);
    }

    #[test]
    fn generation_pointer_requires_explicit_observe_only_approval() {
        let mut pointer = GenerationPointer {
            schema_version: GENERATION_SCHEMA.to_string(),
            generation: 1,
            checkpoint_dir: "/tmp/checkpoint".to_string(),
            checkpoint_id: "checkpoint".to_string(),
            weights_sha256: "a".repeat(64),
            mode: "observe-only".to_string(),
            approved_observe_only: true,
        };
        assert!(pointer.validate().is_ok());

        pointer.approved_observe_only = false;
        assert_eq!(
            pointer.validate().expect_err("refused").to_string(),
            "invalid or unapproved observe-only generation pointer"
        );

        pointer.approved_observe_only = true;
        let mut cases = Vec::new();
        let mut wrong_schema = pointer.clone();
        wrong_schema.schema_version = "qualia.jepa-generation.v2".to_string();
        cases.push(wrong_schema);
        let mut zero_generation = pointer.clone();
        zero_generation.generation = 0;
        cases.push(zero_generation);
        let mut empty_directory = pointer.clone();
        empty_directory.checkpoint_dir = String::new();
        cases.push(empty_directory);
        let mut unsafe_id = pointer.clone();
        unsafe_id.checkpoint_id = "../escape".to_string();
        cases.push(unsafe_id);
        let mut short_digest = pointer.clone();
        short_digest.weights_sha256 = "abc".to_string();
        cases.push(short_digest);
        let mut uppercase_mode = pointer.clone();
        uppercase_mode.mode = "canonical".to_string();
        cases.push(uppercase_mode);
        for case in cases {
            assert!(case.validate().is_err(), "accepted {case:?}");
        }
    }

    #[test]
    fn incoherent_or_stale_sources_never_reach_the_model() {
        let shm = fresh_region("staleness");
        let config = ObserveOnlyConfig {
            calibration_valid: true,
            ..ObserveOnlyConfig::default()
        };
        let now_ns = 5_000_000_000;

        // Nothing has been published yet.
        assert_eq!(
            capture_observation(&shm, now_ns, config, None)
                .err()
                .expect("empty sources are refused")
                .to_string(),
            "physical observation is incomplete"
        );

        // A tuple dated beyond the source-skew budget is refused.
        publish_sources(&shm, now_ns + config.max_source_skew_ns + 1, 1);
        assert_eq!(
            capture_observation(&shm, now_ns, config, None)
                .err()
                .expect("future sources are refused")
                .to_string(),
            "physical observation is stale or sensor-skewed"
        );

        // The same refusal is published as telemetry by a tick.
        let artifacts = TempDir::new().expect("artifact directory");
        let pointer = checkpoint_pointer(artifacts.path(), "gen-stale", 33);
        let pointer_path = artifacts.path().join("current.json");
        install_pointer(&pointer_path, &pointer);
        let mut runner = ObserveOnlyRunner::open(&pointer_path, "cpu", config, 1).expect("open");
        assert_eq!(runner.tick(&shm, now_ns).expect("tick"), TickOutcome::Idle);
        let telemetry = shm.jepa_telemetry().snapshot(8).expect("telemetry");
        assert_eq!(telemetry.stale_frames, 1);
        assert_eq!(telemetry.last_error_code, 1);
        assert!(telemetry.last_error_len > 0);

        // An uncompiled backend never opens.
        let error = ObserveOnlyRunner::open(&pointer_path, "tpu", config, 1)
            .err()
            .expect("unknown backend is refused");
        assert_eq!(error.to_string(), "unsupported JEPA backend tpu");
    }

    #[test]
    fn action_correlation_weights_overlaps_and_demands_coverage() {
        let shm = fresh_region("correlation");
        let config = ObserveOnlyConfig {
            calibration_valid: true,
            ..ObserveOnlyConfig::default()
        };
        let start_ns = 2_000_000_000;
        publish_interval(&shm, 1, start_ns, start_ns + 100_000_000, 0.2, -0.2, 0.5);
        publish_interval(
            &shm,
            2,
            start_ns + 100_000_000,
            start_ns + 200_000_000,
            0.4,
            -0.4,
            1.0,
        );
        let aggregate = correlate_action_history(&shm, start_ns, start_ns + 200_000_000, config)
            .expect("history read")
            .expect("a fully covered transition is correlated");
        assert!((aggregate.left - 0.3).abs() < 1.0e-6);
        assert!((aggregate.right + 0.3).abs() < 1.0e-6);
        assert!((aggregate.speed_scale - 0.75).abs() < 1.0e-6);
        assert_eq!(aggregate.source_sequence, 2);
        assert_eq!(aggregate.interval_end_ns, start_ns + 200_000_000);
        assert!((aggregate.coverage - 1.0).abs() < 1.0e-6);
        let packed = pack_action(Some(&aggregate)).expect("pack the aggregate");
        assert!((packed[0] - 0.3).abs() < 1.0e-6);
        assert_eq!(packed[3], 1.0);

        // Half the transition is uncovered, so the aggregate is refused.
        let gapped = vec![
            AppliedActionSnapshot {
                slot_seq: 1,
                producer_epoch: 1,
                action_sequence: 1,
                interval_start_ns: start_ns,
                interval_end_ns: start_ns + 50_000_000,
                applied_left: 0.2,
                applied_right: -0.2,
                speed_scale: 0.5,
                authority: ACTION_AUTHORITY_LEASH,
                valid: true,
                ..AppliedActionSnapshot::default()
            },
            AppliedActionSnapshot {
                slot_seq: 2,
                producer_epoch: 1,
                action_sequence: 2,
                interval_start_ns: start_ns + 150_000_000,
                interval_end_ns: start_ns + 200_000_000,
                applied_left: 0.4,
                applied_right: -0.4,
                speed_scale: 1.0,
                authority: ACTION_AUTHORITY_LEASH,
                valid: true,
                ..AppliedActionSnapshot::default()
            },
        ];
        assert!(
            aggregate_action_intervals(&gapped, start_ns, start_ns + 200_000_000, config)
                .expect("aggregate")
                .is_none()
        );

        // A non-Leash interval, or one whose collision flag contradicts its
        // safety bits, is not a usable action.
        let mut foreign = gapped[0];
        foreign.authority = ACTION_AUTHORITY_LEASH + 1;
        assert!(!valid_action_interval(&foreign));
        let mut contradictory = gapped[0];
        contradictory.collision_clamped = true;
        assert!(!valid_action_interval(&contradictory));
        contradictory.safety_flags = LEASH_ACTION_SAFETY_COLLISION_CLAMP;
        assert!(valid_action_interval(&contradictory));

        // An interval that runs backwards is refused even when it is stamped.
        let mut backwards = gapped[0];
        backwards.interval_end_ns = backwards.interval_start_ns;
        assert!(!valid_action_interval(&backwards));
    }

    #[test]
    fn quality_gates_match_materialization_and_range_saturates() {
        let shm = fresh_region("quality");
        let config = ObserveOnlyConfig {
            calibration_valid: true,
            ..ObserveOnlyConfig::default()
        };
        let now_ns = 7_000_000_000;
        let sensor_ns = now_ns - 100_000_000;
        publish_sources(&shm, sensor_ns, 1);

        let frame = capture_observation(&shm, now_ns, config, None).expect("first frame");
        assert!(frame.quality > 0.0);
        assert_eq!(frame.observation[OBS_DIM - 1], 1.0);
        assert_eq!(frame.observation[OBS_DIM - 2], 0.0);

        // A second frame one cadence later reports the measured velocity.
        let next_sensor_ns = sensor_ns + 100_000_000;
        publish_sources(&shm, next_sensor_ns, 101);
        let next_frame =
            capture_observation(&shm, now_ns, config, Some(&frame)).expect("second frame");
        let pose_start = OBS_DIM - POSE_DIM;
        assert!((next_frame.observation[pose_start + 4] - 1.0).abs() < 1.0e-6);
        assert!(next_frame.observation[pose_start + 5].abs() < 1.0e-6);

        // Finite returns beyond the model range stay valid and saturate.
        let mut far_scan = LidarScanSnapshot {
            scan_start_ns: sensor_ns.saturating_sub(1_000_000),
            scan_end_ns: sensor_ns,
            point_count: LIDAR_MAX_POINTS as u32,
            ..LidarScanSnapshot::default()
        };
        for point in &mut far_scan.points {
            *point = LidarPoint {
                angle_rad: 0.0,
                distance_m: config.lidar_max_range_m * 1.5,
                intensity: 10,
                _pad: [0; 3],
            };
        }
        shm.lidar_scan_mut().publish(&far_scan).expect("publish");
        let frame = capture_observation(&shm, now_ns, config, None).expect("far frame");
        assert!(frame.quality > 0.0);
        assert_eq!(frame.observation[CAMERA_PIXELS], 1.0);
        assert_eq!(frame.observation[CAMERA_PIXELS + LIDAR_BINS], 1.0);

        // Pose confidence below the floor invalidates the frame and the pose.
        let mut weak_pose = shm.world_model().robot_pose;
        weak_pose.confidence = config.min_pose_confidence / 2.0;
        shm.set_robot_pose(weak_pose);
        let frame = capture_observation(&shm, now_ns, config, None).expect("weak pose frame");
        assert_eq!(frame.quality, 0.0);
        assert_eq!(frame.observation[OBS_DIM - 1], 0.0);

        // Too few valid LiDAR returns invalidate the frame as well.
        publish_sources(&shm, sensor_ns, 2);
        let mut sparse_scan = LidarScanSnapshot {
            scan_start_ns: sensor_ns.saturating_sub(1_000_000),
            scan_end_ns: sensor_ns,
            point_count: LIDAR_MAX_POINTS as u32,
            ..LidarScanSnapshot::default()
        };
        sparse_scan.points[0] = LidarPoint {
            angle_rad: 0.0,
            distance_m: 2.0,
            intensity: 10,
            _pad: [0; 3],
        };
        shm.lidar_scan_mut().publish(&sparse_scan).expect("publish");
        assert_eq!(
            capture_observation(&shm, now_ns, config, None)
                .expect("sparse frame")
                .quality,
            0.0
        );
    }
}

#[cfg(all(test, feature = "sim"))]
mod fly_sim_tests {
    use super::*;
    use qualia_types::FLY_SIM_ABI_VERSION;
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;

    /// A three-type cycle prior, written the way the builder writes it:
    /// `manifest.json` carries the schema and counts, `graph.bin` the `u64`
    /// rowptr followed by the `u32` columns and weights.
    fn write_prior(directory: &Path) {
        let manifest = serde_json::json!({
            "schema": "qualia.connectome-prior.v1",
            "type_count": 3,
            "edge_count": 3,
            "source_sha256": "0".repeat(64),
            "rowptr_sha256": "0".repeat(64),
            "cols_sha256": "0".repeat(64),
            "weights_sha256": "0".repeat(64),
            "created_at_ms": 0,
            "attribution": {
                "dataset": "male-cns:v1.0",
                "licence": "CC-BY-4.0",
                "url": "https://male-cns.janelia.org",
                "citation": "Berg et al. 2026, Cell",
            },
        });
        fs::write(directory.join("manifest.json"), manifest.to_string()).expect("manifest");
        let mut graph = Vec::new();
        for row in [0u64, 1, 2, 3] {
            graph.extend_from_slice(&row.to_le_bytes());
        }
        for column in [1u32, 2, 0] {
            graph.extend_from_slice(&column.to_le_bytes());
        }
        for weight in [2u32, 1, 3] {
            graph.extend_from_slice(&weight.to_le_bytes());
        }
        fs::write(directory.join("graph.bin"), graph).expect("graph");
    }

    #[test]
    fn fly_sim_is_absent_unless_the_mode_is_sim() {
        let directory = TempDir::new().expect("temporary directory");
        write_prior(directory.path());
        let path = directory.path().to_str().expect("utf-8 path");
        for mode in [None, Some("off"), Some("prior")] {
            assert!(FlySimPublisher::from_parts(mode, Some(path), 1).is_none());
        }
    }

    #[test]
    fn fly_sim_disables_itself_when_the_artifact_does_not_load() {
        let directory = TempDir::new().expect("temporary directory");
        write_prior(directory.path());
        let missing = directory.path().join("absent");
        assert!(FlySimPublisher::from_parts(Some("sim"), None, 1).is_none());
        assert!(FlySimPublisher::from_parts(Some("sim"), Some("  "), 1).is_none());
        assert!(
            FlySimPublisher::from_parts(Some("sim"), missing.to_str(), 1).is_none(),
            "a directory without a prior disables the simulator"
        );
    }

    #[test]
    fn fly_sim_publishes_only_while_observe_only_is_approved() {
        let directory = TempDir::new().expect("temporary directory");
        write_prior(directory.path());
        let mut publisher = FlySimPublisher::from_parts(
            Some("sim"),
            directory.path().to_str(),
            7,
        )
        .expect("publisher");
        let shm = ShmRegion::create("qualia_t15_fly_slot").expect("region");

        publisher
            .tick(&shm, 1_000, false, 3)
            .expect("unapproved tick");
        assert_eq!(
            shm.fly_sim().snapshot(4).expect("snapshot").sim_step,
            0,
            "an unapproved pointer publishes nothing"
        );

        publisher.tick(&shm, 2_000, true, 3).expect("approved tick");
        let read = shm.fly_sim().snapshot(4).expect("snapshot");
        assert_eq!(read.abi_version, FLY_SIM_ABI_VERSION);
        assert_eq!(read.type_count, 3);
        assert_eq!(read.flags, JEPA_FLAG_VALID | JEPA_FLAG_OUTPUT_FINITE);
        assert_eq!(read.producer_epoch, 7);
        assert_eq!(read.runner_epoch, 3);
        assert_eq!(read.sim_step, 1);
        assert_eq!(read.timestamp_ns, 2_000);
        assert_eq!(read.dt, FlySimPublisher::STEP_SECONDS);
        assert_eq!(&read.sim_id[..SIM_ID.len()], SIM_ID.as_bytes());
        assert_eq!(read.state[..3], [0.0, 0.0, 0.0]);

        publisher.tick(&shm, 3_000, true, 3).expect("approved tick");
        assert_eq!(
            shm.fly_sim().snapshot(4).expect("snapshot").sim_step,
            2,
            "each approved tick advances the model once"
        );
        assert_eq!(shm.fly_sim().seq.load(Ordering::Acquire) & 1, 0);
    }
}
