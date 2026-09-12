//! `qualia-leash-transport`: the applied-action ledger of a robot nobody drives.
//!
//! An applied action is *what reached the wire*: the dataset leg reads the
//! `transport_accepted` intervals and accepts only those whose `authority` is
//! the transport's own (`ACTION_AUTHORITY_LEASH`, `crates/jepa-dataset/src/lib.rs:1288-1301`).
//! On this robot the transport is the leash — it owns `/dev/ttyTHS1` — so
//! `runners/drive` cannot reach the wire and its records carry drive authority.
//! What is left for a bench session is the command that keeps the robot
//! stopped: the leash's own `stop` tool ("Send a non-latching zero-speed motor
//! stop"). This runner calls it, and each acknowledgement becomes one interval
//! with `authority: leash`, `applied_left = applied_right = 0.0`.
//!
//! **No motion is commanded.** Every interval this runner records says the
//! leash accepted a zero-speed stop; the fields a reader would look for to see
//! motion — `requested_*`, `clamped_*`, `applied_*` — are all `0.0`, and
//! `speed_scale` is `1.0` because nothing scaled a zero command down. The
//! ledger's value is that the *transport path* is exercised end to end and
//! acknowledged by the body's owner; it is not evidence of driving, and the
//! evidence README says so beside the numbers.
//!
//! `valid` is this runner's own field meaning "the leash acknowledged this
//! interval's command" — it is what the dataset leg reads as the transport's
//! acceptance. `armed` is copied from the leash's `health` at startup
//! (`deadman_ok && !estop && mode == "live"`) so the record carries the
//! harness's own state rather than a claim about it.
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
use qualia_types::{AppliedActionSnapshot, ACTION_AUTHORITY_LEASH};
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
        "qualia-leash-transport: recording the leash's acknowledged zero-speed stop into {} every {}ms; authority=leash, applied speed 0.0/0.0",
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
    let mut failures = 0u64;
    let mut reported_ack = false;

    loop {
        let attempted_ns = now_ns();
        match call(&agent, &settings.base_url, "stop") {
            Ok(ack) => {
                failures = 0;
                let end_ns = now_ns();
                if end_ns > last_end_ns {
                    let start_ns = last_end_ns;
                    let next = sequence + 1;
                    let snapshot = AppliedActionSnapshot {
                        producer_epoch,
                        action_sequence: next,
                        interval_start_ns: start_ns,
                        interval_end_ns: end_ns,
                        requested_left: 0.0,
                        requested_right: 0.0,
                        clamped_left: 0.0,
                        clamped_right: 0.0,
                        applied_left: 0.0,
                        applied_right: 0.0,
                        speed_scale: 1.0,
                        safety_flags: 0,
                        authority: ACTION_AUTHORITY_LEASH,
                        valid: true,
                        armed,
                        deadman_active: armed,
                        collision_clamped: false,
                        ..AppliedActionSnapshot::default()
                    };
                    match history.append(snapshot) {
                        Ok(_) => {
                            sequence = next;
                            last_end_ns = end_ns;
                            if !reported_ack {
                                reported_ack = true;
                                println!(
                                    "qualia-leash-transport: leash acknowledged the stop: {}",
                                    preview(&ack)
                                );
                            }
                            if sequence == 1 || sequence % settings.log_every == 0 {
                                println!(
                                    "qualia-leash-transport: interval {} accepted {}..{} applied=0.000/0.000 speed_scale=1.000 authority=leash armed={}",
                                    sequence, start_ns, end_ns, armed,
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
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures == 1 || failures % 20 == 0 {
                    eprintln!("qualia-leash-transport: stop unavailable (failure {failures}): {error}");
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
