//! `qualia-arena-recorder`: turn coherent shared-memory state into crash-safe
//! MCAP evidence.
//!
//! One recorder owns one session file. It attaches to every configured region,
//! replays each committed source state as a JSON record on the matching topic,
//! and narrates the session on stderr. Any failure ends the process with
//! status 1; a clean stop (signal or a bounded run) seals the session and
//! records it in the session store when one is configured.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use qualia_mcap::{
    quarantine_partials, McapReference, McapSessionWriter, TOPIC_ACTION_APPLIED,
    TOPIC_ACTION_CLAMPED, TOPIC_ACTION_REQUESTED, TOPIC_BELIEF, TOPIC_CAMERA, TOPIC_HEALTH,
    TOPIC_JEPA, TOPIC_LIDAR, TOPIC_PLANNER, TOPIC_POSE, TOPIC_PRIOR, TOPIC_VSLAM,
};
use qualia_session_store::SessionStore;
use qualia_shm::{LayerReader, ShmRegion};
use qualia_types::{
    AppliedActionSnapshot, BeliefSlot, JepaEvidencePayload, JepaTelemetryPayload,
    LidarOccupancyGridSnapshot, LidarScanSnapshot, NUM_LAYERS,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

/// Session directory used when `QUALIA_MCAP_ROOT` is unset.
const DEFAULT_ROOT: &str = "artifacts/arena";
/// Poll cadence used when `QUALIA_MCAP_POLL_MS` is unset or not a number.
const DEFAULT_POLL_MS: u64 = 50;
/// Seqlock retries granted to every shared-memory snapshot.
const SNAPSHOT_ATTEMPTS: usize = 8;
/// Prior poller: per-request timeout and pause between two attempts.
const PRIOR_TIMEOUT: Duration = Duration::from_millis(250);
const PRIOR_INTERVAL: Duration = Duration::from_millis(500);

/// `schema_version` stamped into each topic's evidence document.
const CAMERA_DOC: &str = "qualia.camera-evidence.v1";
const LIDAR_DOC: &str = "qualia.lidar-evidence.v1";
const POSE_DOC: &str = "qualia.pose-evidence.v1";
const VSLAM_DOC: &str = "qualia.vslam-evidence.v1";
const ACTION_DOC: &str = "qualia.action-evidence.v1";
const JEPA_DOC: &str = "qualia.jepa-evidence.v1";
const PRIOR_DOC: &str = "qualia.prior-evidence.v1";
const BELIEF_DOC: &str = "qualia.belief-evidence.v1";
const HEALTH_DOC: &str = "qualia.health-evidence.v1";
const PLANNER_DOC: &str = "qualia.planner-evidence.v1";

type Failure = Box<dyn std::error::Error + Send + Sync>;
type Fallible = Result<(), Failure>;

fn main() {
    if let Err(error) = run() {
        eprintln!("qualia-arena-recorder: {error}");
        std::process::exit(1);
    }
}

/// One configured entity: where its state lives and how to label it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct EntitySourceConfig {
    entity: String,
    shm_name: String,
    calibration_id: String,
    #[serde(default)]
    primary: bool,
}

/// A configured entity plus the versions already captured from it.
struct SourceState {
    config: EntitySourceConfig,
    region: ShmRegion,
    seen_camera: u64,
    seen_lidar: u64,
    seen_vslam: u64,
    seen_nav: u64,
    seen_actions: u64,
}

/// Everything the process learns from the environment before the loop starts.
struct Settings {
    root: String,
    session: String,
    producer_epoch: u64,
    poll: Duration,
    limit: Option<Duration>,
    agent_url: Option<String>,
}

impl Settings {
    fn from_env() -> Result<Self, Failure> {
        let producer_epoch = now_ns();
        let root = std::env::var("QUALIA_MCAP_ROOT").unwrap_or_else(|_| DEFAULT_ROOT.to_owned());
        let session = std::env::var("QUALIA_ARENA_SESSION")
            .unwrap_or_else(|_| format!("arena-{producer_epoch}"));
        let poll_ms = std::env::var("QUALIA_MCAP_POLL_MS")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(DEFAULT_POLL_MS);
        let limit = std::env::var("QUALIA_MCAP_DURATION_SECONDS")
            .ok()
            .map(|raw| raw.parse::<u64>())
            .transpose()?
            .map(Duration::from_secs);
        let agent_url = std::env::var("QUALIA_AGENT_URL").ok();
        Ok(Self {
            root,
            session,
            producer_epoch,
            poll: Duration::from_millis(poll_ms),
            limit,
            agent_url,
        })
    }
}

fn run() -> Fallible {
    let settings = Settings::from_env()?;
    let configs = load_source_configs()?;
    let primary = configs
        .iter()
        .position(|source| source.primary)
        .ok_or("exactly one primary entity source is required")?;

    for quarantined in quarantine_partials(&settings.root)? {
        eprintln!(
            "qualia-arena-recorder: quarantined incomplete file {} -> {}",
            quarantined.original_path, quarantined.quarantine_path
        );
    }

    let mut sources = open_sources(configs)?;
    let primary_entity = sources[primary].config.entity.clone();
    let mut recorder = Recorder::new(
        McapSessionWriter::create(&settings.root, &settings.session)?,
        settings.producer_epoch,
        primary,
        primary_entity,
    )?;

    let running = Arc::new(AtomicBool::new(true));
    let signal = Arc::clone(&running);
    ctrlc::set_handler(move || signal.store(false, Ordering::Release))?;
    let (priors, prior_thread) = start_prior_poller(settings.agent_url, Arc::clone(&running));

    eprintln!(
        "qualia-arena-recorder: recording session={} sources={} root={}",
        settings.session,
        summarize_sources(&sources),
        settings.root
    );

    let started = Instant::now();
    while running.load(Ordering::Acquire)
        && settings.limit.is_none_or(|limit| started.elapsed() < limit)
    {
        recorder.tick(&mut sources, &priors, now_ns())?;
        thread::sleep(settings.poll);
    }

    running.store(false, Ordering::Release);
    if let Some(thread) = prior_thread {
        let _ = thread.join();
    }

    let reference = recorder.seal()?;
    if let (Ok(store_path), Ok(session_id)) = (
        std::env::var("QUALIA_SESSION_STORE"),
        std::env::var("QUALIA_SESSION_ID"),
    ) {
        let session_id = session_id.parse::<i64>()?;
        SessionStore::open(&store_path)?.upsert_mcap_reference(session_id, &reference)?;
    }
    eprintln!(
        "qualia-arena-recorder: completed {} sha256={} channels={}",
        reference.path,
        reference.sha256,
        reference.channels.len()
    );
    Ok(())
}

fn load_source_configs() -> Result<Vec<EntitySourceConfig>, Failure> {
    let configs = match std::env::var("QUALIA_MCAP_SOURCES_JSON") {
        Ok(encoded) => serde_json::from_str::<Vec<EntitySourceConfig>>(&encoded)?,
        Err(_) => vec![EntitySourceConfig {
            entity: std::env::var("QUALIA_CAMERA_ENTITY").unwrap_or_else(|_| "unknown".to_owned()),
            shm_name: std::env::var("QUALIA_SHM_NAME")
                .unwrap_or_else(|_| "/qualia_body".to_owned()),
            calibration_id: std::env::var("QUALIA_CALIBRATION_ID")
                .unwrap_or_else(|_| "unavailable".to_owned()),
            primary: true,
        }],
    };
    validate_source_configs(&configs)?;
    Ok(configs)
}

fn validate_source_configs(configs: &[EntitySourceConfig]) -> Fallible {
    if configs.is_empty() {
        return Err("at least one MCAP entity source is required".into());
    }
    if configs.iter().filter(|source| source.primary).count() != 1 {
        return Err("exactly one MCAP entity source must be primary".into());
    }
    if configs.iter().any(|source| {
        source.entity.trim().is_empty()
            || source.shm_name.trim().is_empty()
            || source.calibration_id.trim().is_empty()
    }) {
        return Err("MCAP entity, SHM name, and calibration ID must be non-empty".into());
    }
    let entities = configs
        .iter()
        .map(|source| source.entity.as_str())
        .collect::<BTreeSet<_>>();
    let names = configs
        .iter()
        .map(|source| source.shm_name.as_str())
        .collect::<BTreeSet<_>>();
    if entities.len() != configs.len() || names.len() != configs.len() {
        return Err("MCAP entity names and SHM names must be unique".into());
    }
    Ok(())
}

fn open_sources(configs: Vec<EntitySourceConfig>) -> Result<Vec<SourceState>, Failure> {
    let mut sources = Vec::with_capacity(configs.len());
    for config in configs {
        let region = ShmRegion::open(&config.shm_name)?;
        // A session owns only the intervals that complete while it runs, so the
        // history a producer finished before attach is deliberately skipped.
        let seen_actions = region.applied_action_history().committed_seq();
        sources.push(SourceState {
            config,
            region,
            seen_camera: 0,
            seen_lidar: 0,
            seen_vslam: 0,
            seen_nav: 0,
            seen_actions,
        });
    }
    Ok(sources)
}

fn summarize_sources(sources: &[SourceState]) -> String {
    sources
        .iter()
        .map(|source| {
            format!(
                "{}:{}{}",
                source.config.entity,
                source.config.shm_name,
                if source.config.primary { "(primary)" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The recording state of one session: the open writer and every version mark.
struct Recorder {
    writer: McapSessionWriter,
    producer_epoch: u64,
    primary: usize,
    primary_entity: String,
    clock: u32,
    last_jepa: (u64, u64),
    prior_hashes: BTreeSet<String>,
    belief_marks: [u64; NUM_LAYERS],
}

impl Recorder {
    fn new(
        mut writer: McapSessionWriter,
        producer_epoch: u64,
        primary: usize,
        primary_entity: String,
    ) -> Result<Self, Failure> {
        writer.register_required_topics()?;
        Ok(Self {
            writer,
            producer_epoch,
            primary,
            primary_entity,
            clock: 0,
            last_jepa: (0, 0),
            prior_hashes: BTreeSet::new(),
            belief_marks: [0; NUM_LAYERS],
        })
    }

    /// Seals the session and reports what was written.
    fn seal(self) -> qualia_mcap::McapResult<McapReference> {
        self.writer.finish()
    }

    /// One poll of every lane, in a fixed order: physical evidence per entity
    /// first, then the primary entity's inference, prior and belief lanes.
    fn tick(
        &mut self,
        sources: &mut [SourceState],
        priors: &Receiver<serde_json::Value>,
        publish_time: u64,
    ) -> Fallible {
        for source in sources.iter_mut() {
            self.capture_physical(source, publish_time)?;
        }
        self.capture_jepa(&sources[self.primary].region, publish_time)?;
        self.capture_priors(priors, publish_time)?;
        self.capture_beliefs(&sources[self.primary].region, publish_time)?;
        Ok(())
    }

    fn next_clock(&mut self) -> u32 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    fn capture_physical(&mut self, source: &mut SourceState, publish_time: u64) -> Fallible {
        self.capture_camera(source, publish_time)?;
        self.capture_lidar(source, publish_time)?;
        self.drain_actions(source, publish_time)?;
        self.capture_nav(source, publish_time)?;
        self.capture_vslam(source, publish_time)?;
        Ok(())
    }

    fn capture_camera(&mut self, source: &mut SourceState, publish_time: u64) -> Fallible {
        let Ok(frame) = source.region.camera_frame().snapshot(SNAPSHOT_ATTEMPTS) else {
            return Ok(());
        };
        if frame.seq == 0 || frame.seq == source.seen_camera {
            return Ok(());
        }
        let entity = source.config.entity.as_str();
        let calibration_id = source.config.calibration_id.as_str();
        let sequence = self.next_clock();
        self.writer.write_json(
            TOPIC_CAMERA,
            sequence,
            frame.timestamp_ns,
            publish_time,
            &json!({
                "schema_version": CAMERA_DOC,
                "producer_epoch": self.producer_epoch,
                "entity": entity,
                "source_sequence": frame.seq,
                "timestamp_ns": frame.timestamp_ns,
                "calibration_id": calibration_id,
                "source_width": frame.source_width,
                "source_height": frame.source_height,
                "thumb_width": frame.thumb_width,
                "thumb_height": frame.thumb_height,
                "luminance_mean": frame.luminance_mean,
                "luminance_stddev": frame.luminance_stddev,
                "valid": frame.valid,
                "thumbnail_luma": frame.thumbnail_luma.to_vec(),
            }),
        )?;
        source.seen_camera = frame.seq;
        Ok(())
    }

    fn capture_lidar(&mut self, source: &mut SourceState, publish_time: u64) -> Fallible {
        let Ok(scan) = source.region.lidar_scan().snapshot(SNAPSHOT_ATTEMPTS) else {
            return Ok(());
        };
        if scan.seq == 0 || scan.seq == source.seen_lidar {
            return Ok(());
        }
        let visible = scan.point_count as usize;
        let points = scan
            .points
            .iter()
            .take(visible)
            .map(|point| {
                json!({
                    "angle_rad": point.angle_rad,
                    "distance_m": point.distance_m,
                    "intensity": point.intensity,
                })
            })
            .collect::<Vec<_>>();
        let validity_mask = scan
            .points
            .iter()
            .take(visible)
            .map(|point| {
                point.intensity != 0 && point.distance_m.is_finite() && point.distance_m > 0.0
            })
            .collect::<Vec<_>>();
        let grid = source.region.lidar_grid().snapshot(SNAPSHOT_ATTEMPTS).ok();
        let observed = grid.as_ref().map(|grid| project_observed_cells(&scan, grid));
        let timestamp_ns = scan.scan_end_ns.max(scan.scan_start_ns);
        let entity = source.config.entity.as_str();
        let sequence = self.next_clock();
        self.writer.write_json(
            TOPIC_LIDAR,
            sequence,
            timestamp_ns,
            publish_time,
            &json!({
                "schema_version": LIDAR_DOC,
                "producer_epoch": self.producer_epoch,
                "entity": entity,
                "source_sequence": scan.seq,
                "timestamp_ns": timestamp_ns,
                "scan_start_ns": scan.scan_start_ns,
                "scan_end_ns": scan.scan_end_ns,
                "points": points,
                "validity_mask": validity_mask,
                "occupancy": grid.map(|grid| json!({
                    "source_sequence": grid.seq,
                    "width": grid.width,
                    "height": grid.height,
                    "resolution_m": grid.resolution_m,
                    "origin_x_m": grid.origin_x_m,
                    "origin_y_m": grid.origin_y_m,
                    "cells": grid.cells.to_vec(),
                    "observed": observed,
                })),
            }),
        )?;
        source.seen_lidar = scan.seq;
        Ok(())
    }

    /// Drains every interval committed since the last poll, oldest first.
    fn drain_actions(&mut self, source: &mut SourceState, publish_time: u64) -> Fallible {
        let history = source.region.applied_action_history();
        let committed = history.committed_seq();
        if committed < source.seen_actions {
            return Err(format!(
                "{} applied-action history rolled back from {} to {}",
                source.config.entity, source.seen_actions, committed
            )
            .into());
        }
        for sequence in source.seen_actions + 1..=committed {
            let action = history.snapshot(sequence, SNAPSHOT_ATTEMPTS).map_err(|error| {
                format!(
                    "{} applied-action history read failed at {sequence}: {error}",
                    source.config.entity
                )
            })?;
            self.capture_action(source.config.entity.as_str(), action, publish_time)?;
        }
        source.seen_actions = committed;
        Ok(())
    }

    /// One completed interval, recorded once per safety stage.
    fn capture_action(
        &mut self,
        entity: &str,
        action: AppliedActionSnapshot,
        publish_time: u64,
    ) -> Fallible {
        for (topic, left, right, stage) in [
            (
                TOPIC_ACTION_REQUESTED,
                action.requested_left,
                action.requested_right,
                "requested",
            ),
            (
                TOPIC_ACTION_CLAMPED,
                action.clamped_left,
                action.clamped_right,
                "safety_clamped",
            ),
            (
                TOPIC_ACTION_APPLIED,
                action.applied_left,
                action.applied_right,
                "transport_accepted",
            ),
        ] {
            let sequence = self.next_clock();
            self.writer.write_json(
                topic,
                sequence,
                action.interval_end_ns,
                publish_time.max(action.interval_end_ns),
                &json!({
                    "schema_version": ACTION_DOC,
                    "producer_epoch": action.producer_epoch,
                    "entity": entity,
                    "history_sequence": action.slot_seq,
                    "source_sequence": action.action_sequence,
                    "timestamp_ns": action.interval_end_ns,
                    "interval_start_ns": action.interval_start_ns,
                    "interval_end_ns": action.interval_end_ns,
                    "stage": stage,
                    "left": left,
                    "right": right,
                    "requested_left": action.requested_left,
                    "requested_right": action.requested_right,
                    "clamped_left": action.clamped_left,
                    "clamped_right": action.clamped_right,
                    "applied_left": action.applied_left,
                    "applied_right": action.applied_right,
                    "speed_scale": action.speed_scale,
                    "safety_flags": action.safety_flags,
                    "authority": action.authority,
                    "valid": action.valid,
                    "armed": action.armed,
                    "deadman_active": action.deadman_active,
                    "collision_clamped": action.collision_clamped,
                }),
            )?;
        }
        Ok(())
    }

    /// The canonical pose, plus the primary entity's observe-only planner view.
    fn capture_nav(&mut self, source: &mut SourceState, publish_time: u64) -> Fallible {
        let world = source.region.world_model();
        let nav_seq = world.nav_seq.load(Ordering::Acquire);
        if nav_seq == 0 || nav_seq == source.seen_nav {
            return Ok(());
        }
        let pose = world.robot_pose;
        let goal = world.nav_goal;
        let entity = source.config.entity.as_str();
        let primary = source.config.primary;
        let sequence = self.next_clock();
        self.writer.write_json(
            TOPIC_POSE,
            sequence,
            pose.timestamp_ns,
            publish_time,
            &json!({
                "schema_version": POSE_DOC,
                "producer_epoch": self.producer_epoch,
                "entity": entity,
                "source_sequence": nav_seq,
                "timestamp_ns": pose.timestamp_ns,
                "x_m": pose.x_m,
                "y_m": pose.y_m,
                "z_m": pose.z_m,
                "yaw_rad": pose.yaw_rad,
                "pitch_rad": pose.pitch_rad,
                "roll_rad": pose.roll_rad,
                "confidence": pose.confidence,
            }),
        )?;
        if primary {
            let sequence = self.next_clock();
            self.writer.write_json(
                TOPIC_PLANNER,
                sequence,
                publish_time,
                publish_time,
                &json!({
                    "schema_version": PLANNER_DOC,
                    "producer_epoch": self.producer_epoch,
                    "entity": entity,
                    "source_sequence": nav_seq,
                    "timestamp_ns": publish_time,
                    "goal": {
                        "active": goal.active != 0,
                        "cell_x": goal.cell_x,
                        "cell_z": goal.cell_z,
                        "x_m": goal.x_m,
                        "y_m": goal.y_m,
                        "z_m": goal.z_m,
                        "yaw_rad": goal.yaw_rad,
                        "selected_at_ns": goal.timestamp_ns,
                    },
                    "authority": "existing-planner-observe-only",
                }),
            )?;
        }
        source.seen_nav = nav_seq;
        Ok(())
    }

    fn capture_vslam(&mut self, source: &mut SourceState, publish_time: u64) -> Fallible {
        let vslam = source.region.vslam_frontend();
        let seq = vslam.seq.load(Ordering::Acquire);
        if seq == 0 || seq == source.seen_vslam {
            return Ok(());
        }
        let entity = source.config.entity.as_str();
        let calibration_id = source.config.calibration_id.as_str();
        let sequence = self.next_clock();
        self.writer.write_json(
            TOPIC_VSLAM,
            sequence,
            vslam.timestamp_ns,
            publish_time,
            &json!({
                "schema_version": VSLAM_DOC,
                "producer_epoch": self.producer_epoch,
                "entity": entity,
                "source_sequence": seq,
                "timestamp_ns": vslam.timestamp_ns,
                "calibration_id": calibration_id,
                "frame_seq": vslam.frame_seq,
                "tracking": vslam.tracking_ok.load(Ordering::Acquire),
                "tracking_confidence": vslam.tracking_confidence,
                "pose_confidence": vslam.pose_confidence,
                "feature_count": vslam.feature_count,
                "keyframe_count": vslam.keyframe_count,
                "loop_closure_count": vslam.loop_closure_count,
            }),
        )?;
        source.seen_vslam = seq;
        Ok(())
    }

    /// Records one inference result per `(runner_epoch, inference_seq)` pair.
    fn capture_jepa(&mut self, region: &ShmRegion, publish_time: u64) -> Fallible {
        let (Ok(evidence), Ok(telemetry)) = (
            region.jepa_evidence().snapshot(SNAPSHOT_ATTEMPTS),
            region.jepa_telemetry().snapshot(SNAPSHOT_ATTEMPTS),
        ) else {
            return Ok(());
        };
        let key = (evidence.runner_epoch, evidence.inference_seq);
        if evidence.producer_epoch == 0 || evidence.inference_seq == 0 || key == self.last_jepa {
            return Ok(());
        }
        let entity = self.primary_entity.clone();
        let sequence = self.next_clock();
        self.writer.write_json(
            TOPIC_JEPA,
            sequence,
            evidence.timestamp_ns,
            publish_time,
            &jepa_record(&entity, &evidence, &telemetry),
        )?;
        self.last_jepa = key;
        Ok(())
    }

    /// Forwards each distinct prior the agent reports, hashing it for identity.
    fn capture_priors(
        &mut self,
        priors: &Receiver<serde_json::Value>,
        publish_time: u64,
    ) -> Fallible {
        while let Ok(prior) = priors.try_recv() {
            let encoded = serde_json::to_vec(&prior)?;
            let digest = format!("{:x}", Sha256::digest(&encoded));
            if !self.prior_hashes.insert(digest.clone()) {
                continue;
            }
            let entity = self.primary_entity.clone();
            let sequence = self.next_clock();
            let timestamp_ns = prior_timestamp_ns(&prior, publish_time);
            self.writer.write_json(
                TOPIC_PRIOR,
                sequence,
                timestamp_ns,
                publish_time,
                &json!({
                    "schema_version": PRIOR_DOC,
                    "producer_epoch": self.producer_epoch,
                    "entity": entity,
                    "source_sequence": sequence,
                    "timestamp_ns": timestamp_ns,
                    "prior_sha256": digest,
                    "prior": prior,
                }),
            )?;
        }
        Ok(())
    }

    /// Records every layer that advanced, then one health summary for the poll.
    fn capture_beliefs(&mut self, region: &ShmRegion, publish_time: u64) -> Fallible {
        let mut health = Vec::with_capacity(NUM_LAYERS);
        let mut observed_any = false;
        for layer in 0..NUM_LAYERS {
            let Some(belief) = coherent_belief(region, layer) else {
                continue;
            };
            health.push(json!({
                "layer": layer,
                "timestamp_ns": belief.timestamp_ns,
                "vfe": belief.vfe,
                "challenge_vfe": belief.challenge_vfe,
                "cycle_us": belief.cycle_us,
                "compression": belief.compression,
            }));
            observed_any |= belief.timestamp_ns != 0;
            if belief.timestamp_ns == 0 || belief.timestamp_ns == self.belief_marks[layer] {
                continue;
            }
            let entity = self.primary_entity.clone();
            let sequence = self.next_clock();
            self.writer.write_json(
                TOPIC_BELIEF,
                sequence,
                belief.timestamp_ns,
                publish_time,
                &json!({
                    "schema_version": BELIEF_DOC,
                    "producer_epoch": self.producer_epoch,
                    "entity": entity,
                    "source_sequence": belief.timestamp_ns,
                    "timestamp_ns": belief.timestamp_ns,
                    "layer": layer,
                    "mean": belief.mean.to_vec(),
                    "precision": belief.precision.to_vec(),
                    "prediction": belief.prediction.to_vec(),
                    "residual": belief.residual.to_vec(),
                    "vfe": belief.vfe,
                }),
            )?;
            self.belief_marks[layer] = belief.timestamp_ns;
        }
        if observed_any {
            let entity = self.primary_entity.clone();
            let sequence = self.next_clock();
            self.writer.write_json(
                TOPIC_HEALTH,
                sequence,
                publish_time,
                publish_time,
                &json!({
                    "schema_version": HEALTH_DOC,
                    "producer_epoch": self.producer_epoch,
                    "entity": entity,
                    "source_sequence": sequence,
                    "timestamp_ns": publish_time,
                    "layers": health,
                }),
            )?;
        }
        Ok(())
    }
}

/// Polls the agent's belief status on its own thread so a slow agent cannot
/// stall the capture loop; each distinct prior is forwarded on the channel.
fn start_prior_poller(
    agent_url: Option<String>,
    running: Arc<AtomicBool>,
) -> (Receiver<serde_json::Value>, Option<JoinHandle<()>>) {
    let (sender, receiver) = mpsc::channel();
    let Some(agent_url) = agent_url.filter(|url| !url.trim().is_empty()) else {
        return (receiver, None);
    };
    let thread = thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(PRIOR_TIMEOUT)
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                eprintln!("qualia-arena-recorder: prior poller client failed: {error}");
                return;
            }
        };
        let endpoint = format!("{}/belief/status", agent_url.trim_end_matches('/'));
        while running.load(Ordering::Acquire) {
            if let Ok(response) = client.get(&endpoint).send() {
                if let Ok(status) = response
                    .error_for_status()
                    .and_then(|value| value.json::<serde_json::Value>())
                {
                    if let Some(active) = status
                        .get("active_priors")
                        .and_then(serde_json::Value::as_array)
                    {
                        for prior in active {
                            if sender.send(prior.clone()).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            thread::sleep(PRIOR_INTERVAL);
        }
    });
    (receiver, Some(thread))
}

fn jepa_record(
    entity: &str,
    evidence: &JepaEvidencePayload,
    telemetry: &JepaTelemetryPayload,
) -> serde_json::Value {
    json!({
        "schema_version": JEPA_DOC,
        "producer_epoch": evidence.producer_epoch,
        "entity": entity,
        "source_sequence": evidence.inference_seq,
        "timestamp_ns": evidence.timestamp_ns,
        "runner_epoch": evidence.runner_epoch,
        "inference_seq": evidence.inference_seq,
        "camera_seq": evidence.camera_seq,
        "lidar_seq": evidence.lidar_seq,
        "pose_seq": evidence.pose_seq,
        "action_seq": evidence.action_seq,
        "backend": evidence.backend,
        "mode": evidence.mode,
        "flags": evidence.flags,
        "model_id": fixed_id(&evidence.model_id),
        "checkpoint_id": fixed_id(&evidence.checkpoint_id),
        "observation_quality": evidence.observation_quality,
        "transition_nll": evidence.transition_nll,
        "occupancy_confidence": evidence.occupancy_confidence,
        "latent": evidence.latent.to_vec(),
        "predicted_mean": evidence.predicted_mean.to_vec(),
        "predicted_log_variance": evidence.predicted_log_variance.to_vec(),
        "evidence": evidence.evidence.to_vec(),
        "occupancy_logits": evidence.occupancy_logits.to_vec(),
        "telemetry": {
            "correlated": telemetry.producer_epoch == evidence.producer_epoch
                && telemetry.runner_epoch == evidence.runner_epoch
                && telemetry.inference_count == evidence.inference_seq,
            "inference_count": telemetry.inference_count,
            "dropped_frames": telemetry.dropped_frames,
            "stale_frames": telemetry.stale_frames,
            "non_finite_outputs": telemetry.non_finite_outputs,
            "hot_swaps": telemetry.hot_swaps,
            "latency_last_us": telemetry.latency_last_us,
            "latency_p50_us": telemetry.latency_p50_us,
            "latency_p95_us": telemetry.latency_p95_us,
            "latency_max_us": telemetry.latency_max_us,
        }
    })
}

/// Decodes a fixed-size, NUL-padded identifier as the string it carries.
fn fixed_id(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

/// Prior creation time in nanoseconds, or the poll time when it is absent or
/// does not fit the nanosecond range.
fn prior_timestamp_ns(prior: &serde_json::Value, fallback: u64) -> u64 {
    prior
        .get("created_at_ms")
        .and_then(serde_json::Value::as_u64)
        .and_then(|milliseconds| milliseconds.checked_mul(1_000_000))
        .unwrap_or(fallback)
}

/// Reads one layer without accepting a half-written belief.
fn coherent_belief(region: &ShmRegion, layer: usize) -> Option<BeliefSlot> {
    let slot = region.layer_slot(layer);
    for _ in 0..SNAPSHOT_ATTEMPTS {
        let before = slot.write_idx.load(Ordering::Acquire);
        let belief = *LayerReader::new(slot).read();
        let after = slot.write_idx.load(Ordering::Acquire);
        if before == after {
            return Some(belief);
        }
    }
    None
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Marks the grid cells a scan's rays cross, leaving unseen cells unknown.
fn project_observed_cells(
    scan: &LidarScanSnapshot,
    grid: &LidarOccupancyGridSnapshot,
) -> Vec<bool> {
    let width = grid.width as usize;
    let height = grid.height as usize;
    let cell_count = width.saturating_mul(height).min(grid.cells.len());
    let mut observed = vec![false; cell_count];
    if width == 0
        || height == 0
        || cell_count != width.saturating_mul(height)
        || !grid.resolution_m.is_finite()
        || grid.resolution_m <= 0.0
    {
        return observed;
    }
    let origin_x = ((-grid.origin_x_m) / grid.resolution_m).floor() as i32;
    let origin_y = ((-grid.origin_y_m) / grid.resolution_m).floor() as i32;
    for point in scan.points.iter().take(scan.point_count as usize) {
        if point.intensity == 0
            || !point.distance_m.is_finite()
            || point.distance_m <= 0.0
            || !point.angle_rad.is_finite()
        {
            continue;
        }
        let x_m = point.angle_rad.cos() * point.distance_m;
        let y_m = point.angle_rad.sin() * point.distance_m;
        let end_x = ((x_m - grid.origin_x_m) / grid.resolution_m).floor() as i32;
        let end_y = ((y_m - grid.origin_y_m) / grid.resolution_m).floor() as i32;
        stamp_ray(
            &mut observed,
            width,
            height,
            origin_x,
            origin_y,
            end_x,
            end_y,
        );
    }
    observed
}

/// Walks the integer line from sensor to hit, marking each cell it enters.
fn stamp_ray(
    observed: &mut [bool],
    width: usize,
    height: usize,
    mut x: i32,
    mut y: i32,
    end_x: i32,
    end_y: i32,
) {
    let dx = (end_x - x).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let dy = -(end_y - y).abs();
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
            observed[y as usize * width + x as usize] = true;
        }
        if x == end_x && y == end_y {
            break;
        }
        let twice_error = 2 * error;
        if twice_error >= dy {
            error += dy;
            x += step_x;
        }
        if twice_error <= dx {
            error += dx;
            y += step_y;
        }
        if left_the_grid(x, y, end_x, end_y, step_x, step_y, width, height) {
            break;
        }
    }
}

/// Whether the walk has left the grid for good, i.e. cannot reach the hit.
fn left_the_grid(
    x: i32,
    y: i32,
    end_x: i32,
    end_y: i32,
    step_x: i32,
    step_y: i32,
    width: usize,
    height: usize,
) -> bool {
    (x < 0 && x != end_x && step_x < 0)
        || (y < 0 && y != end_y && step_y < 0)
        || (x as usize >= width && x != end_x && step_x > 0)
        || (y as usize >= height && y != end_y && step_y > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_mcap::read_window;
    use qualia_shm::LayerWriter;
    use qualia_types::{CameraFrameSnapshot, LidarPoint, ACTION_AUTHORITY_LEASH};

    fn scratch_region(tag: &str) -> (ShmRegion, String) {
        let name = format!("/qualia-arena-unit-{tag}-{}", std::process::id());
        let region = ShmRegion::create(&name).expect("region");
        (region, name)
    }

    fn config(entity: &str, shm_name: &str, primary: bool) -> EntitySourceConfig {
        EntitySourceConfig {
            entity: entity.to_owned(),
            shm_name: shm_name.to_owned(),
            calibration_id: format!("{entity}-cal"),
            primary,
        }
    }

    fn open_recorder(root: &std::path::Path, session: &str, entity: &str) -> Recorder {
        Recorder::new(
            McapSessionWriter::create(root, session).expect("writer"),
            7,
            0,
            entity.to_owned(),
        )
        .expect("recorder")
    }

    fn record_values(path: &str, topic: &str) -> Vec<serde_json::Value> {
        read_window(path, Some(topic), 0, u64::MAX)
            .expect("session replays")
            .iter()
            .map(|message| serde_json::from_slice(&message.data).expect("json record"))
            .collect()
    }

    fn frame(seq: u64, stamp: u64) -> CameraFrameSnapshot {
        CameraFrameSnapshot {
            seq,
            timestamp_ns: stamp,
            source_width: 64,
            source_height: 48,
            thumb_width: 64,
            thumb_height: 48,
            luminance_mean: 0.4,
            luminance_stddev: 0.2,
            valid: true,
            ..CameraFrameSnapshot::default()
        }
    }

    #[test]
    fn validation_requires_one_primary_and_distinct_names() {
        let pair = vec![
            config("guard", "/qualia_guard", true),
            config("pinkie", "/qualia_pinkie", false),
        ];
        validate_source_configs(&pair).expect("a valid pair");

        let mut headless = pair.clone();
        headless[0].primary = false;
        assert!(validate_source_configs(&headless).is_err());

        let mut shared_region = pair.clone();
        shared_region[1].shm_name = shared_region[0].shm_name.clone();
        assert!(validate_source_configs(&shared_region).is_err());

        let mut anonymous = pair;
        anonymous[0].entity = "   ".to_owned();
        assert!(validate_source_configs(&anonymous).is_err());
    }

    #[test]
    fn a_tick_records_both_entities_and_every_committed_interval() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (guard, guard_name) = scratch_region("dual-guard");
        let (pinkie, pinkie_name) = scratch_region("dual-pinkie");
        let stamp = now_ns();
        guard
            .camera_frame_mut()
            .publish(&frame(1, stamp))
            .expect("guard frame");
        pinkie
            .camera_frame_mut()
            .publish(&frame(1, stamp))
            .expect("pinkie frame");

        let mut sources = open_sources(vec![
            config("guard", &guard_name, true),
            config("pinkie", &pinkie_name, false),
        ])
        .expect("sources open");

        // Both entities commit two intervals after the recorder attached.
        let first = AppliedActionSnapshot {
            producer_epoch: 3,
            action_sequence: 1,
            interval_start_ns: stamp.saturating_sub(2_000_000),
            interval_end_ns: stamp.saturating_sub(1_000_000),
            speed_scale: 1.0,
            authority: ACTION_AUTHORITY_LEASH,
            valid: true,
            ..AppliedActionSnapshot::default()
        };
        let second = AppliedActionSnapshot {
            action_sequence: 2,
            interval_start_ns: first.interval_end_ns,
            interval_end_ns: stamp,
            ..first
        };
        for region in [&guard, &pinkie] {
            region.applied_action_history().append(first).expect("first");
            region
                .applied_action_history()
                .append(second)
                .expect("second");
        }

        let mut recorder = open_recorder(temp.path(), "dual", "guard");
        let (_sender, priors) = mpsc::channel();
        recorder.tick(&mut sources, &priors, stamp).expect("tick");
        let reference = recorder.seal().expect("seal");

        let cameras = record_values(&reference.path, TOPIC_CAMERA);
        let entities = cameras
            .iter()
            .map(|camera| camera["entity"].as_str().expect("entity").to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(entities, BTreeSet::from(["guard".into(), "pinkie".into()]));

        let applied = record_values(&reference.path, TOPIC_ACTION_APPLIED);
        assert_eq!(applied.len(), 4, "both entities drain both intervals");
        let sequences = applied
            .iter()
            .map(|action| action["source_sequence"].as_u64().expect("sequence"))
            .collect::<BTreeSet<_>>();
        assert_eq!(sequences, BTreeSet::from([1, 2]));
        for action in &applied {
            assert_eq!(action["schema_version"], ACTION_DOC);
            assert_eq!(action["stage"], "transport_accepted");
            assert!(action["history_sequence"].as_u64().expect("slot") > 0);
            assert_eq!(action["timestamp_ns"], action["interval_end_ns"]);
            assert_eq!(action["left"], action["applied_left"]);
            assert_eq!(action["right"], action["applied_right"]);
            for field in [
                "requested_left",
                "requested_right",
                "clamped_left",
                "clamped_right",
                "applied_left",
                "applied_right",
            ] {
                assert_eq!(action[field].as_f64(), Some(0.0));
            }
            assert_eq!(action["speed_scale"].as_f64(), Some(1.0));
            assert_eq!(action["authority"].as_u64(), Some(1));
            assert_eq!(action["valid"].as_bool(), Some(true));
        }
    }

    #[test]
    fn beliefs_are_recorded_once_and_health_once_per_poll() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (region, name) = scratch_region("belief");
        let stamp = now_ns();
        let belief = LayerWriter::new(region.layer_slot(0));
        belief.back_buffer().timestamp_ns = stamp;
        belief.back_buffer().vfe = 1.5;
        belief.back_buffer().challenge_vfe = 0.25;
        belief.back_buffer().cycle_us = 900;
        belief.back_buffer().compression = 7;
        belief.publish();

        let mut sources = open_sources(vec![config("guard", &name, true)]).expect("sources");
        let mut recorder = open_recorder(temp.path(), "belief", "guard");
        let (_sender, priors) = mpsc::channel();
        recorder.tick(&mut sources, &priors, stamp).expect("first tick");
        recorder.tick(&mut sources, &priors, stamp).expect("second tick");
        let reference = recorder.seal().expect("seal");

        let beliefs = record_values(&reference.path, TOPIC_BELIEF);
        assert_eq!(beliefs.len(), 1, "an unchanged layer is not re-recorded");
        assert_eq!(beliefs[0]["layer"], 0);
        assert_eq!(beliefs[0]["vfe"], 1.5);
        assert_eq!(beliefs[0]["source_sequence"], stamp);

        let health = record_values(&reference.path, TOPIC_HEALTH);
        assert_eq!(health.len(), 2, "health summarizes every poll");
        let layers = health[0]["layers"].as_array().expect("layers");
        assert_eq!(layers.len(), NUM_LAYERS);
        assert_eq!(layers[0]["cycle_us"], 900);
        assert_eq!(layers[0]["compression"], 7);
        assert_eq!(layers[1]["layer"], 1);
    }

    #[test]
    fn jepa_records_each_sequence_once_with_its_telemetry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (region, name) = scratch_region("jepa");
        let mut evidence = JepaEvidencePayload {
            producer_epoch: 7,
            runner_epoch: 9,
            inference_seq: 11,
            timestamp_ns: 13,
            ..JepaEvidencePayload::default()
        };
        evidence.model_id[..8].copy_from_slice(b"model-v1");
        evidence.checkpoint_id[..12].copy_from_slice(b"checkpoint-a");
        let telemetry = JepaTelemetryPayload {
            producer_epoch: 7,
            runner_epoch: 9,
            inference_count: 11,
            latency_last_us: 123,
            ..JepaTelemetryPayload::default()
        };
        region
            .jepa_evidence()
            .publish(evidence)
            .expect("evidence");
        region
            .jepa_telemetry()
            .publish(telemetry)
            .expect("telemetry");

        let mut sources = open_sources(vec![config("guard", &name, true)]).expect("sources");
        let mut recorder = open_recorder(temp.path(), "jepa", "guard");
        let (_sender, priors) = mpsc::channel();
        recorder.tick(&mut sources, &priors, 99).expect("first tick");
        recorder.tick(&mut sources, &priors, 99).expect("second tick");
        let reference = recorder.seal().expect("seal");

        let jepa = record_values(&reference.path, TOPIC_JEPA);
        assert_eq!(jepa.len(), 1, "one record per inference sequence");
        assert_eq!(jepa[0]["entity"], "guard");
        assert_eq!(jepa[0]["model_id"], "model-v1");
        assert_eq!(jepa[0]["checkpoint_id"], "checkpoint-a");
        assert_eq!(jepa[0]["telemetry"]["correlated"], true);
        assert_eq!(jepa[0]["telemetry"]["latency_last_us"], 123);
    }

    #[test]
    fn uncorrelated_telemetry_is_reported_uncorrelated() {
        let evidence = JepaEvidencePayload {
            producer_epoch: 7,
            runner_epoch: 9,
            inference_seq: 11,
            ..JepaEvidencePayload::default()
        };
        let telemetry = JepaTelemetryPayload {
            producer_epoch: 8,
            runner_epoch: 9,
            inference_count: 11,
            ..JepaTelemetryPayload::default()
        };
        let record = jepa_record("guard", &evidence, &telemetry);
        assert_eq!(record["telemetry"]["correlated"], false);
    }

    #[test]
    fn a_rollback_stops_the_capture_instead_of_gapping_the_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (region, name) = scratch_region("rollback");
        region
            .applied_action_history()
            .append(AppliedActionSnapshot::default())
            .expect("append");

        let mut sources = open_sources(vec![config("guard", &name, true)]).expect("sources");
        sources[0].seen_actions = 5;
        let mut recorder = open_recorder(temp.path(), "rollback", "guard");
        let error = recorder
            .drain_actions(&mut sources[0], now_ns())
            .expect_err("rollback is fatal");
        assert_eq!(
            error.to_string(),
            "guard applied-action history rolled back from 5 to 1"
        );
    }

    #[test]
    fn project_observed_cells_traces_the_ray_and_leaves_the_rest_unknown() {
        let mut scan = LidarScanSnapshot {
            point_count: 1,
            ..LidarScanSnapshot::default()
        };
        scan.points[0] = LidarPoint {
            angle_rad: 0.0,
            distance_m: 1.0,
            intensity: 1,
            _pad: [0; 3],
        };
        let mut grid = LidarOccupancyGridSnapshot::default();
        grid.resolution_m = 0.05;
        grid.origin_x_m = -(grid.width as f32 * grid.resolution_m * 0.5);
        grid.origin_y_m = -(grid.height as f32 * grid.resolution_m * 0.5);

        let observed = project_observed_cells(&scan, &grid);
        let width = grid.width as usize;
        let centre_x = width / 2;
        let centre_y = grid.height as usize / 2;
        assert!(observed[centre_y * width + centre_x], "sensor cell");
        assert!(observed[centre_y * width + centre_x + 20], "hit cell");
        assert!(!observed[0], "a cell off the ray stays unknown");
    }

    #[test]
    fn fixed_identifiers_stop_at_the_first_nul() {
        let mut bytes = [0u8; 8];
        bytes[..3].copy_from_slice(b"abc");
        assert_eq!(fixed_id(&bytes), "abc");
        assert_eq!(fixed_id(b"abcd"), "abcd");
        assert_eq!(fixed_id(&[]), "");
    }

    #[test]
    fn prior_timestamps_convert_milliseconds_and_survive_overflow() {
        assert_eq!(
            prior_timestamp_ns(&json!({"created_at_ms": 123}), 9),
            123_000_000
        );
        assert_eq!(prior_timestamp_ns(&json!({"created_at_ms": u64::MAX}), 9), 9);
        assert_eq!(prior_timestamp_ns(&json!({}), 9), 9);
    }
}
