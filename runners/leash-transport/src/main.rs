//! `qualia-leash-transport`: the applied-action ledger of a robot nobody drives.
//!
//! An applied action is *what reached the wire*: the dataset leg reads the
//! `transport_accepted` intervals and accepts only those whose `authority` is
//! the transport's own (`ACTION_AUTHORITY_LEASH`, `crates/jepa-dataset/src/lib.rs:1288-1301`).
//! On this robot the transport is the leash — it owns `/dev/ttyTHS1` — so
//! `runners/drive` cannot reach the wire and its records carry drive authority.
//! What is left for a bench session is the command that keeps the robot
//! stopped: the leash's own `stop` tool ("Send a non-latching zero-speed motor
//! stop", `safety: physical-stop`, no token or approval in its input schema).
//! This runner calls it, and every acknowledgement becomes one interval whose
//! fields are copied from the leash's own reply:
//!
//! ```text
//! {"left":0.0,"max_speed":0.35,"ok":true,"right":0.0,"soft_odometry_limited":false,
//!  "speed_mode":"medium","stopped_by_deadman":false}
//! ```
//!
//! **No motion is commanded.** The command is a stop, and the record carries
//! what the leash answered: `requested_*` is the zero this runner asked for,
//! `clamped_*` and `applied_*` are the leash's own `left`/`right`. A refusal
//! (`ok: false`) is recorded as `valid: false` rather than dropped, so the
//! ledger carries the transport's answer either way.
//!
//! `valid` is therefore not this runner's opinion: it is the leash's `ok`.
//! `armed` is copied from the leash's `health` at startup
//! (`mode == "live" && deadman_ok && !estop`) so the record carries the
//! harness's own state rather than a claim about it, and `safety_flags` uses the
//! leash's own vocabulary (`LEASH_ACTION_SAFETY_*`) for the two flags its reply
//! reports.
//!
//! Intervals tile: each starts where the previous acknowledgement ended, so a
//! transition window is covered rather than sampled. A failed call appends
//! nothing, which leaves a real gap — the honest record of a transport that did
//! not answer.
//!
//! Configuration:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `QUALIA_LEASH_BASE_URL` | The leash's HTTP root; default `http://127.0.0.1:8000`. |
//! | `QUALIA_SHM_NAME` | The arena to attach to; default `/qualia_body`. |
//! | `QUALIA_LEASH_TRANSPORT_POLL_MS` | Command interval; default 100. |
//! | `QUALIA_LEASH_TRANSPORT_LOG_EVERY` | Intervals between summary lines; default 50. |
//! | `QUALIA_LEASH_SENSORS_TIMEOUT_MS` | Request timeout; default 2000. |
//!
//! Exit codes: 1 an arena that cannot be opened.

use qualia_leash_sensors::{
    call, BASE_URL_ENV, DEFAULT_BASE_URL, DEFAULT_SHM_NAME, DEFAULT_TIMEOUT_MS, SHM_NAME_ENV,
    TIMEOUT_MS_ENV,
};
use qualia_shm::ShmRegion;
use qualia_types::{
    AppliedActionSnapshot, ACTION_AUTHORITY_LEASH, LEASH_ACTION_SAFETY_DEADMAN,
    LEASH_ACTION_SAFETY_SOFT_ODOMETRY_LIMIT,
};
use serde::Deserialize;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `QUALIA_LEASH_TRANSPORT_POLL_MS`, default `100`.
pub const POLL_MS_ENV: &str = "QUALIA_LEASH_TRANSPORT_POLL_MS";
/// `QUALIA_LEASH_TRANSPORT_LOG_EVERY`, default `50`.
pub const LOG_EVERY_ENV: &str = "QUALIA_LEASH_TRANSPORT_LOG_EVERY";
/// Command interval when none is named.
pub const DEFAULT_POLL_MS: u64 = 100;
/// Intervals between summary lines once the first has been logged.
pub const DEFAULT_LOG_EVERY: u64 = 50;
/// Characters of the leash's acknowledgement a log line carries.
const ACK_PREVIEW: usize = 160;

/// Everything the process learns from the environment before the loop starts.
#[derive(Debug, Clone)]
struct Settings {
    base_url: String,
    shm_name: String,
    poll: Duration,
    log_every: u64,
    timeout: Duration,
}

impl Settings {
    fn from_env() -> Self {
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
        Self {
            base_url: text(BASE_URL_ENV, DEFAULT_BASE_URL),
            shm_name: text(SHM_NAME_ENV, DEFAULT_SHM_NAME),
            poll: Duration::from_millis(number(POLL_MS_ENV, DEFAULT_POLL_MS).max(1)),
            log_every: number(LOG_EVERY_ENV, DEFAULT_LOG_EVERY).max(1),
            timeout: Duration::from_millis(number(TIMEOUT_MS_ENV, DEFAULT_TIMEOUT_MS).max(1)),
        }
    }
}

/// The leash's own report of one stop, as `DriveOutcome`.
#[derive(Debug, Clone, Deserialize, Default)]
struct DriveOutcome {
    /// The leash's verdict: the command was accepted and applied.
    #[serde(default)]
    ok: bool,
    /// The left speed that reached the wire, as the leash reports it.
    #[serde(default)]
    left: f32,
    /// The right speed that reached the wire, as the leash reports it.
    #[serde(default)]
    right: f32,
    /// The harness's speed ceiling at the time.
    #[serde(default)]
    max_speed: f32,
    /// The active speed mode.
    #[serde(default)]
    speed_mode: String,
    /// The leash's own soft-odometry limit flag.
    #[serde(default)]
    soft_odometry_limited: bool,
    /// The leash's own deadman flag.
    #[serde(default)]
    stopped_by_deadman: bool,
}

impl DriveOutcome {
    /// The leash's flags, in the leash's own vocabulary.
    fn safety_flags(&self) -> u32 {
        u32::from(self.soft_odometry_limited) * LEASH_ACTION_SAFETY_SOFT_ODOMETRY_LIMIT
            | u32::from(self.stopped_by_deadman) * LEASH_ACTION_SAFETY_DEADMAN
    }
}

fn main() {
    let settings = Settings::from_env();

    let region = match ShmRegion::open(&settings.shm_name) {
        Ok(region) => region,
        Err(error) => {
            eprintln!(
                "qualia-leash-transport: failed to open shm '{}': {error}",
                settings.shm_name
            );
            std::process::exit(1);
        }
    };

    let agent = qualia_leash_sensors::agent(settings.timeout);

    println!(
        "qualia-leash-transport: recording the leash's acknowledged zero-speed stop into {} every {}ms; authority=leash",
        settings.shm_name,
        settings.poll.as_millis()
    );

    // The harness's own state, read once: the field this process copies into
    // `armed` is the leash's claim, not this runner's.
    let armed = match call(&agent, &settings.base_url, "health") {
        Ok(text) => {
            let armed = health_armed(&text);
            println!(
                "qualia-leash-transport: leash health mode={} deadman_ok={} estop={} -> armed={armed}",
                json_text(&text, "mode").unwrap_or_else(|| "unknown".to_string()),
                json_bool(&text, "deadman_ok")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                json_bool(&text, "estop")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
            );
            armed
        }
        Err(error) => {
            eprintln!("qualia-leash-transport: leash health unavailable: {error}");
            false
        }
    };

    let history = region.applied_action_history();
    let producer_epoch = now_ns();
    let mut last_end_ns = producer_epoch;
    let mut sequence = 0u64;
    let mut accepted = 0u64;
    let mut refused = 0u64;
    let mut failures = 0u64;
    let mut reported_ack = false;

    loop {
        let attempted_ns = now_ns();
        match call(&agent, &settings.base_url, "stop") {
            Ok(ack) => {
                failures = 0;
                let outcome: DriveOutcome = serde_json::from_str(&ack).unwrap_or_default();
                let end_ns = now_ns();
                if end_ns <= last_end_ns {
                    continue;
                }
                let start_ns = last_end_ns;
                let next = sequence + 1;
                let snapshot = AppliedActionSnapshot {
                    producer_epoch,
                    action_sequence: next,
                    interval_start_ns: start_ns,
                    interval_end_ns: end_ns,
                    requested_left: 0.0,
                    requested_right: 0.0,
                    clamped_left: outcome.left,
                    clamped_right: outcome.right,
                    applied_left: outcome.left,
                    applied_right: outcome.right,
                    speed_scale: 1.0,
                    safety_flags: outcome.safety_flags(),
                    authority: ACTION_AUTHORITY_LEASH,
                    valid: outcome.ok,
                    armed,
                    deadman_active: armed,
                    collision_clamped: false,
                    ..AppliedActionSnapshot::default()
                };
                match history.append(snapshot) {
                    Ok(_) => {
                        sequence = next;
                        last_end_ns = end_ns;
                        if outcome.ok {
                            accepted += 1;
                        } else {
                            refused += 1;
                        }
                        if !reported_ack {
                            reported_ack = true;
                            println!(
                                "qualia-leash-transport: leash acknowledged the stop: {}",
                                preview(&ack)
                            );
                        }
                        if sequence == 1 || sequence % settings.log_every == 0 {
                            println!(
                                "qualia-leash-transport: interval {} {}..{} ok={} left={:.3} right={:.3} max_speed={:.3} speed_mode={} flags=0x{:x} authority=leash armed={} accepted={} refused={}",
                                sequence,
                                start_ns,
                                end_ns,
                                outcome.ok,
                                outcome.left,
                                outcome.right,
                                outcome.max_speed,
                                outcome.speed_mode,
                                outcome.safety_flags(),
                                armed,
                                accepted,
                                refused,
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "qualia-leash-transport: applied-action append failed at {next}: {error}"
                        );
                    }
                }
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures == 1 || failures % 20 == 0 {
                    eprintln!(
                        "qualia-leash-transport: stop unavailable (failure {failures}): {error}"
                    );
                }
            }
        }
        let elapsed = Duration::from_nanos(now_ns().saturating_sub(attempted_ns));
        thread::sleep(settings.poll.saturating_sub(elapsed));
    }
}

/// Whether the leash's own state says the harness is live and its deadman ok.
fn health_armed(text: &str) -> bool {
    let live = json_text(text, "mode").is_some_and(|mode| mode == "live");
    let deadman = json_bool(text, "deadman_ok").unwrap_or(false);
    let estop = json_bool(text, "estop").unwrap_or(true);
    live && deadman && !estop
}

/// One string field of a JSON payload, when the payload parses.
fn json_text(text: &str, key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get(key)?.as_str().map(|value| value.to_owned())
}

/// One boolean field of a JSON payload, when the payload parses.
fn json_bool(text: &str, key: &str) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get(key)?.as_bool()
}

/// The acknowledgement, on one line and bounded, for the runner's log.
fn preview(ack: &str) -> String {
    let flat = ack.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= ACK_PREVIEW {
        return flat;
    }
    flat.chars().take(ACK_PREVIEW).collect::<String>() + "…"
}

/// Wall-clock nanoseconds since the Unix epoch, or zero if the clock is set
/// before it.
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}
