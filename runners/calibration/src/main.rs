//! `qualia-calibration`: observe the live rig and emit the identity the dataset gate reads.
//!
//! The dataset leg refuses every transition until each camera record carries a
//! `calibration_id` that is non-empty, is not the recorder's `"unavailable"`
//! sentinel, and is the *same* on both frames of the pair
//! (`crates/jepa-dataset/src/lib.rs:526-534`). That gate is an identity check:
//! it asks whether the two frames came from one calibrated rig, not what the
//! intrinsics are. This runner answers it by measuring the rig it can see and
//! hashing what it measured.
//!
//! What it observes, in one bounded window:
//!
//! * the leash's `observe` — the ranging device's own name, frame and angular
//!   sampling, and the camera surface the frames arrive on (D-025: the leash
//!   owns both devices, so this runner reads them the way every other runner
//!   does, and opens no device node);
//! * the arena's camera slot — the frame geometry the camera runner actually
//!   publishes, and the frame rate it achieved;
//! * the arena's lidar slot — the beam count and the scan rate the stack
//!   actually publishes.
//!
//! What it does **not** do is invent the parameters it cannot measure. Camera
//! intrinsics and the camera↔lidar extrinsics are recorded as `unmeasured`
//! with the reason: nothing in this window carries a target or a known
//! correspondence, and a fabricated focal length would be a number no
//! measurement stands behind. The identity digest therefore covers exactly the
//! fields this runner measured — entity, camera surface and frame geometry, and
//! the ranging device's name, frame, beam count and angular sampling — and
//! deliberately excludes the measured *rates*, which drift run to run and would
//! make two sessions of one unchanged rig disagree.
//!
//! Configuration:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `QUALIA_LEASH_BASE_URL` | The leash's HTTP root; default `http://127.0.0.1:8000`. |
//! | `QUALIA_SHM_NAME` | The arena to attach to; default `/qualia_body`. |
//! | `QUALIA_CALIBRATION_OUT` | **Required.** Where the document is written. |
//! | `QUALIA_CALIBRATION_WINDOW_SECONDS` | Observation window; default 5. |
//! | `QUALIA_CALIBRATION_POLL_MS` | Sampling interval; default 100. |
//! | `QUALIA_CALIBRATION_ENTITY` | The entity the record names; default `pinkie`. |
//! | `QUALIA_LEASH_SENSORS_TIMEOUT_MS` | Request timeout; default 2000. |
//!
//! The last line of a successful run carries `calibration_id=<digest>` for the
//! session script to hand to the recorder's documented `QUALIA_CALIBRATION_ID`.
//!
//! Exit codes: 1 bad configuration or an arena that cannot be opened, 2 no
//! camera frames observed, 3 no lidar scans observed.

use qualia_leash_sensors::{
    agent, observe, RangeScan, BASE_URL_ENV, DEFAULT_BASE_URL, DEFAULT_SHM_NAME, DEFAULT_TIMEOUT_MS,
    SHM_NAME_ENV, TIMEOUT_MS_ENV,
};
use qualia_shm::ShmRegion;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The document's schema name, written into every file.
pub const SCHEMA: &str = "qualia.calibration.v1";
/// The identity record's schema name; it is part of the digest, so the meaning
/// of an id can never silently change.
pub const IDENTITY_SCHEMA: &str = "qualia.calibration-identity.v1";
/// `QUALIA_CALIBRATION_OUT`.
pub const OUT_ENV: &str = "QUALIA_CALIBRATION_OUT";
/// `QUALIA_CALIBRATION_WINDOW_SECONDS`.
pub const WINDOW_ENV: &str = "QUALIA_CALIBRATION_WINDOW_SECONDS";
/// `QUALIA_CALIBRATION_POLL_MS`.
pub const POLL_ENV: &str = "QUALIA_CALIBRATION_POLL_MS";
/// `QUALIA_CALIBRATION_ENTITY`.
pub const ENTITY_ENV: &str = "QUALIA_CALIBRATION_ENTITY";
/// Observation window when none is named.
pub const DEFAULT_WINDOW_SECONDS: u64 = 5;
/// Sampling interval when none is named.
pub const DEFAULT_POLL_MS: u64 = 100;
/// The entity named when the environment names none.
pub const DEFAULT_ENTITY: &str = "pinkie";
/// Seqlock retries granted to every shared-memory snapshot.
const SNAPSHOT_ATTEMPTS: usize = 8;

/// Everything the process learns from the environment before the loop starts.
#[derive(Debug, Clone)]
struct Settings {
    base_url: String,
    shm_name: String,
    out: PathBuf,
    window: Duration,
    poll: Duration,
    entity: String,
    timeout: Duration,
}

impl Settings {
    fn from_env() -> Result<Self, String> {
        let text = |key: &str, fallback: &str| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| fallback.to_owned())
        };
        let number = |key: &str, fallback: u64| {
            std::env::var(key)
                .ok()
                .and_then(|raw| raw.trim().parse().ok())
                .unwrap_or(fallback)
        };
        let out = std::env::var(OUT_ENV)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{OUT_ENV} must name the document to write"))?;
        Ok(Self {
            base_url: text(BASE_URL_ENV, DEFAULT_BASE_URL),
            shm_name: text(SHM_NAME_ENV, DEFAULT_SHM_NAME),
            out: PathBuf::from(out),
            window: Duration::from_millis(number(WINDOW_ENV, DEFAULT_WINDOW_SECONDS).max(1) * 1_000),
            poll: Duration::from_millis(number(POLL_ENV, DEFAULT_POLL_MS).max(1)),
            entity: text(ENTITY_ENV, DEFAULT_ENTITY),
            timeout: Duration::from_millis(number(TIMEOUT_MS_ENV, DEFAULT_TIMEOUT_MS).max(1)),
        })
    }
}

/// The calibration document.
#[derive(Debug, Serialize)]
struct Document {
    schema_version: &'static str,
    calibration_id: String,
    entity: String,
    measured_at_ns: u64,
    observation: Observation,
    sensors: SensorCalibration,
    identity: Identity,
}

/// How much live traffic the measurements stand on.
#[derive(Debug, Serialize)]
struct Observation {
    window_ms: u64,
    leash_polls: u64,
    leash_failures: u64,
    camera_frames: u64,
    lidar_scans: u64,
}

/// The measured rig.
#[derive(Debug, Serialize)]
struct SensorCalibration {
    camera: CameraCalibration,
    lidar: LidarCalibration,
}

/// The camera as the stack sees it.
#[derive(Debug, Serialize)]
struct CameraCalibration {
    /// The owner's own name for the surface the frames arrive on.
    surface: String,
    source_width: u32,
    source_height: u32,
    thumb_width: u32,
    thumb_height: u32,
    frames_observed: u64,
    measured_rate_hz: f32,
    luminance_mean_min: f32,
    luminance_mean_max: f32,
    intrinsics: Unmeasured,
    extrinsics: Unmeasured,
}

/// The ranging device as both the owner and the arena describe it.
#[derive(Debug, Serialize)]
struct LidarCalibration {
    /// The device name the owner publishes, e.g. `waveshare-ugv-ld06`.
    source: String,
    /// The frame the owner expresses the rotation in.
    frame_id: String,
    /// Beams per rotation, as the arena's published scan carries them.
    beams: u32,
    angle_min_rad: f32,
    angle_increment_rad: f32,
    scans_observed: u64,
    /// The rate the stack achieved over the window.
    measured_rate_hz: f32,
    /// The rate the owner measures for the device itself.
    owner_rate_hz: f32,
    extrinsics: Unmeasured,
}

/// A parameter this window could not measure, named rather than guessed.
#[derive(Debug, Serialize)]
struct Unmeasured {
    status: &'static str,
    why: &'static str,
}

/// The identity the digest covers.
///
/// Field order is the struct's, so the digest is a deterministic function of
/// these values; rates and counts are deliberately absent (see the module doc).
#[derive(Debug, Serialize)]
struct Identity {
    identity_schema: &'static str,
    entity: String,
    camera: IdentityCamera,
    lidar: IdentityLidar,
}

/// The camera's identity fields.
#[derive(Debug, Serialize)]
struct IdentityCamera {
    surface: String,
    source_width: u32,
    source_height: u32,
    thumb_width: u32,
    thumb_height: u32,
}

/// The ranging device's identity fields.
#[derive(Debug, Serialize)]
struct IdentityLidar {
    source: String,
    frame_id: String,
    beams: u32,
    angle_min_rad: f32,
    angle_increment_rad: f32,
}

/// The camera slot, watched for new frames.
#[derive(Debug, Default)]
struct CameraWatch {
    seen_seq: Option<u64>,
    frames: u64,
    first_ns: Option<u64>,
    last_ns: Option<u64>,
    source_width: u32,
    source_height: u32,
    thumb_width: u32,
    thumb_height: u32,
    luminance_min: f32,
    luminance_max: f32,
}

impl CameraWatch {
    /// Counts one new frame. A frame whose sequence has not moved is the same
    /// frame as the last one and is not counted twice.
    fn note(&mut self, frame: &qualia_types::CameraFrameSnapshot) {
        if self.seen_seq == Some(frame.seq) {
            return;
        }
        self.seen_seq = Some(frame.seq);
        self.frames += 1;
        self.source_width = frame.source_width;
        self.source_height = frame.source_height;
        self.thumb_width = frame.thumb_width;
        self.thumb_height = frame.thumb_height;
        if self.frames == 1 {
            self.luminance_min = frame.luminance_mean;
            self.luminance_max = frame.luminance_mean;
        } else {
            self.luminance_min = self.luminance_min.min(frame.luminance_mean);
            self.luminance_max = self.luminance_max.max(frame.luminance_mean);
        }
        if frame.timestamp_ns > 0 {
            self.first_ns.get_or_insert(frame.timestamp_ns);
            self.last_ns = Some(frame.timestamp_ns);
        }
    }

    /// The rate the observed frames imply, or zero when one frame cannot.
    fn rate_hz(&self) -> f32 {
        match (self.first_ns, self.last_ns) {
            (Some(first), Some(last)) if last > first && self.frames > 1 => {
                let seconds = (last - first) as f64 / 1e9;
                ((self.frames - 1) as f64 / seconds) as f32
            }
            _ => 0.0,
        }
    }
}

/// The lidar slot and the owner's declaration of the ranging device.
#[derive(Debug, Default)]
struct LidarWatch {
    seen_seq: Option<u64>,
    scans: u64,
    first_ns: Option<u64>,
    last_ns: Option<u64>,
    beams: u32,
    source: String,
    frame_id: String,
    angle_min_rad: f32,
    angle_increment_rad: f32,
    owner_rate_hz: f32,
}

impl LidarWatch {
    /// Counts one new published scan.
    fn note_scan(&mut self, scan: &qualia_types::LidarScanSnapshot) {
        if self.seen_seq == Some(scan.seq) {
            return;
        }
        self.seen_seq = Some(scan.seq);
        self.scans += 1;
        self.beams = scan.point_count;
        let end = scan.scan_end_ns.max(scan.scan_start_ns);
        if end > 0 {
            self.first_ns.get_or_insert(end);
            self.last_ns = Some(end);
        }
    }

    /// Takes the device's own description from the current `observe` reply.
    fn note_owner(&mut self, scan: &RangeScan) {
        self.source = scan.source.clone();
        if let Some(sample) = &scan.sample {
            self.frame_id = sample.frame_id.clone();
            self.angle_min_rad = round_angle(sample.angle_min_rad);
            self.angle_increment_rad = round_angle(sample.angle_increment_rad);
            self.owner_rate_hz = sample.scan_rate_hz;
        }
    }

    /// The rate the published scans imply, or zero when one scan cannot.
    fn rate_hz(&self) -> f32 {
        match (self.first_ns, self.last_ns) {
            (Some(first), Some(last)) if last > first && self.scans > 1 => {
                let seconds = (last - first) as f64 / 1e9;
                ((self.scans - 1) as f64 / seconds) as f32
            }
            _ => 0.0,
        }
    }
}

fn main() {
    let settings = match Settings::from_env() {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("qualia-calibration: {error}");
            std::process::exit(1);
        }
    };

    let region = match ShmRegion::open(&settings.shm_name) {
        Ok(region) => region,
        Err(error) => {
            eprintln!(
                "qualia-calibration: failed to open shm '{}': {error}",
                settings.shm_name
            );
            std::process::exit(1);
        }
    };

    let agent = agent(settings.timeout);
    println!(
        "qualia-calibration: observing {}/mcp observe and shm {} for {}ms",
        settings.base_url,
        settings.shm_name,
        settings.window.as_millis()
    );

    let start = Instant::now();
    let mut camera = CameraWatch::default();
    let mut lidar = LidarWatch::default();
    let mut camera_surface = String::new();
    let mut polls = 0u64;
    let mut failures = 0u64;

    while start.elapsed() < settings.window {
        match observe(&agent, &settings.base_url) {
            Ok(sensors) => {
                polls += 1;
                if camera_surface.is_empty() {
                    if let Some(surface) = &sensors.camera {
                        camera_surface = surface.stream_url.clone();
                    }
                }
                if let Some(scan) = &sensors.range_scan {
                    if scan.is_available() {
                        lidar.note_owner(scan);
                    }
                }
            }
            Err(error) => {
                failures += 1;
                if failures == 1 || failures % 20 == 0 {
                    eprintln!("qualia-calibration: observe unavailable (failure {failures}): {error}");
                }
            }
        }
        if let Ok(frame) = region.camera_frame().snapshot(SNAPSHOT_ATTEMPTS) {
            camera.note(&frame);
        }
        if let Ok(scan) = region.lidar_scan().snapshot(SNAPSHOT_ATTEMPTS) {
            lidar.note_scan(&scan);
        }
        thread::sleep(settings.poll);
    }

    let window_ms = settings.window.as_millis() as u64;
    if camera.frames == 0 {
        eprintln!(
            "qualia-calibration: no camera frames on shm '{}' in {window_ms}ms; nothing to calibrate",
            settings.shm_name
        );
        std::process::exit(2);
    }
    if lidar.scans == 0 {
        eprintln!(
            "qualia-calibration: no lidar scans on shm '{}' in {window_ms}ms; nothing to calibrate",
            settings.shm_name
        );
        std::process::exit(3);
    }

    let identity = Identity {
        identity_schema: IDENTITY_SCHEMA,
        entity: settings.entity.clone(),
        camera: IdentityCamera {
            surface: camera_surface.clone(),
            source_width: camera.source_width,
            source_height: camera.source_height,
            thumb_width: camera.thumb_width,
            thumb_height: camera.thumb_height,
        },
        lidar: IdentityLidar {
            source: lidar.source.clone(),
            frame_id: lidar.frame_id.clone(),
            beams: lidar.beams,
            angle_min_rad: lidar.angle_min_rad,
            angle_increment_rad: lidar.angle_increment_rad,
        },
    };
    let calibration_id = match serde_json::to_string(&identity) {
        Ok(canonical) => hex(&Sha256::digest(canonical.as_bytes())),
        Err(error) => {
            eprintln!("qualia-calibration: cannot encode the identity: {error}");
            std::process::exit(1);
        }
    };

    let document = Document {
        schema_version: SCHEMA,
        calibration_id: calibration_id.clone(),
        entity: settings.entity.clone(),
        measured_at_ns: now_ns(),
        observation: Observation {
            window_ms,
            leash_polls: polls,
            leash_failures: failures,
            camera_frames: camera.frames,
            lidar_scans: lidar.scans,
        },
        sensors: SensorCalibration {
            camera: CameraCalibration {
                surface: camera_surface,
                source_width: camera.source_width,
                source_height: camera.source_height,
                thumb_width: camera.thumb_width,
                thumb_height: camera.thumb_height,
                frames_observed: camera.frames,
                measured_rate_hz: camera.rate_hz(),
                luminance_mean_min: camera.luminance_min,
                luminance_mean_max: camera.luminance_max,
                intrinsics: Unmeasured {
                    status: "unmeasured",
                    why: "no target or known correspondence is observed, so no focal length or \
                          principal point is measurable here",
                },
                extrinsics: Unmeasured {
                    status: "unmeasured",
                    why: "no camera/lidar correspondence is observed, so the transform between \
                          them is not measurable here",
                },
            },
            lidar: LidarCalibration {
                source: lidar.source.clone(),
                frame_id: lidar.frame_id.clone(),
                beams: lidar.beams,
                angle_min_rad: lidar.angle_min_rad,
                angle_increment_rad: lidar.angle_increment_rad,
                scans_observed: lidar.scans,
                measured_rate_hz: lidar.rate_hz(),
                owner_rate_hz: lidar.owner_rate_hz,
                extrinsics: Unmeasured {
                    status: "unmeasured",
                    why: "the device's mounting transform is not observable from its own \
                          rotation alone",
                },
            },
        },
        identity,
    };

    if let Some(parent) = settings.out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "qualia-calibration: cannot create {}: {error}",
                    parent.display()
                );
                std::process::exit(1);
            }
        }
    }
    let encoded = match serde_json::to_string_pretty(&document) {
        Ok(encoded) => encoded,
        Err(error) => {
            eprintln!("qualia-calibration: cannot encode the document: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = std::fs::write(&settings.out, format!("{encoded}\n")) {
        eprintln!(
            "qualia-calibration: cannot write {}: {error}",
            settings.out.display()
        );
        std::process::exit(1);
    }

    println!(
        "qualia-calibration: observed camera frames={} ({}x{}, thumb {}x{}, luma {:.3}..{:.3}, {:.3}Hz) scans={} ({} beams, {:.3}Hz) leash_polls={} failures={}",
        camera.frames,
        camera.source_width,
        camera.source_height,
        camera.thumb_width,
        camera.thumb_height,
        camera.luminance_min,
        camera.luminance_max,
        camera.rate_hz(),
        lidar.scans,
        lidar.beams,
        lidar.rate_hz(),
        polls,
        failures,
    );
    println!(
        "qualia-calibration: calibration_id={calibration_id} camera={}x{} lidar={} frame={} beams={} angle_min_rad={} angle_increment_rad={}",
        camera.source_width,
        camera.source_height,
        lidar.source,
        lidar.frame_id,
        lidar.beams,
        lidar.angle_min_rad,
        lidar.angle_increment_rad,
    );
    println!(
        "qualia-calibration: wrote {} (unmeasured, named in the document: camera intrinsics, camera->lidar extrinsics, lidar mount transform)",
        settings.out.display()
    );
}

/// Rounds an angle to a nanoradian: far below any sampling a real device
/// resolves, and stable against float noise in the JSON round trip.
fn round_angle(radians: f32) -> f32 {
    ((radians as f64) * 1e9).round() as f32 / 1e9
}

/// Lowercase hex, the encoding the recorder's `sha256=` line already uses.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Wall-clock nanoseconds since the Unix epoch, or zero if the clock is set
/// before it.
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}
