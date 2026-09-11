//! The observe-only JEPA runtime binary.
//!
//! Starting the runner is an explicit, separate operation: it refuses to run
//! unless the operator has set the two enable variables, and it only ever
//! attaches to an existing shared region and an installed generation pointer.

use qualia_braid::BraidEvent;
use qualia_jepa_runtime::{GenerationChange, ObserveOnlyConfig, ObserveOnlyRunner, TickOutcome};
use qualia_shm::ShmRegion;
use std::env;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a braid report may take before the runner stops waiting for it.
const BRAID_TIMEOUT: Duration = Duration::from_millis(250);

/// The reason a swap back to the parked checkpoint is reported with. The
/// registry's rollback record is what renders it to an operator (T22, #37).
const ROLLBACK_REASON: &str = "generation pointer returned to the parked checkpoint";

fn main() {
    if let Err(error) = observe() {
        eprintln!("qualia-jepa-runtime: {error}");
        std::process::exit(1);
    }
}

/// Everything the binary does, so `main` only owns the exit code.
fn observe() -> Result<(), Box<dyn Error + Send + Sync>> {
    require_env("QUALIA_JEPA_ENABLE", "1", "refusing to start: set QUALIA_JEPA_ENABLE=1 explicitly")?;
    require_env(
        "QUALIA_JEPA_MODE",
        "observe-only",
        "refusing to start outside QUALIA_JEPA_MODE=observe-only",
    )?;
    let generation_file = env::var("QUALIA_JEPA_GENERATION_FILE")
        .map_err(|_| "QUALIA_JEPA_GENERATION_FILE is required")?;
    let backend = env::var("QUALIA_JEPA_BACKEND").unwrap_or_else(|_| "cpu".to_string());
    let shm_name = env::var("QUALIA_SHM_NAME").unwrap_or_else(|_| "/qualia_body".to_string());
    let calibration_valid = env::var("QUALIA_CALIBRATION_ID")
        .is_ok_and(|value| !value.is_empty() && value != "unavailable");
    let poll_ms = env_u64("QUALIA_JEPA_POLL_MS", 10)?;
    let config = ObserveOnlyConfig {
        max_source_age_ns: millis_to_nanos(env_u64("QUALIA_JEPA_MAX_SOURCE_AGE_MS", 500)?)?,
        max_source_skew_ns: millis_to_nanos(env_u64("QUALIA_JEPA_MAX_SOURCE_SKEW_MS", 150)?)?,
        max_action_edge_slop_ns: millis_to_nanos(env_u64("QUALIA_JEPA_ACTION_EDGE_SLOP_MS", 50)?)?,
        min_action_coverage: env_f32("QUALIA_JEPA_MIN_ACTION_COVERAGE", 0.8)?,
        min_lidar_valid_fraction: env_f32("QUALIA_JEPA_MIN_LIDAR_VALID_FRACTION", 0.1)?,
        min_pose_confidence: env_f32("QUALIA_JEPA_MIN_POSE_CONFIDENCE", 0.1)?,
        lidar_max_range_m: env_f32("QUALIA_JEPA_LIDAR_MAX_RANGE_M", 8.0)?,
        calibration_valid,
    };

    let shm = ShmRegion::open(&shm_name)?;
    let producer_epoch = now_ns();
    let mut runner = ObserveOnlyRunner::open(generation_file, &backend, config, producer_epoch)?;
    let running = Arc::new(AtomicBool::new(true));
    let stop = Arc::clone(&running);
    ctrlc::set_handler(move || stop.store(false, Ordering::Release))?;
    eprintln!(
        "qualia-jepa-runtime: observe-only backend={} generation={} checkpoint={} shm={}",
        backend,
        runner.active_generation(),
        runner.active_checkpoint_id(),
        shm_name
    );

    let reporter = BraidReporter::from_env();
    while running.load(Ordering::Acquire) {
        match runner.tick(&shm, now_ns()) {
            Ok(TickOutcome::Published { inference_seq }) if inference_seq % 100 == 0 => {
                eprintln!("qualia-jepa-runtime: inference_seq={inference_seq}");
            }
            Ok(TickOutcome::GenerationSwapped { generation }) => {
                eprintln!(
                    "qualia-jepa-runtime: swapped generation={} checkpoint={}",
                    generation,
                    runner.active_checkpoint_id()
                );
                // The pointer moved, so the promotion strand reports it: a
                // pointer that named the parked checkpoint is a rollback, and
                // anything else is a generation loaded for the first time.
                if let Some(reporter) = reporter.as_ref() {
                    reporter.report(promotion_event(runner.last_generation_change(), generation));
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("qualia-jepa-runtime: tick rejected: {error}"),
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
    Ok(())
}

/// The braid's HTTP edge, when the operator named an agent.
///
/// The runtime's own records are the pointer file and the shared-memory slots;
/// this reports the pointer's movement so the braid's view is not a second copy
/// of them. A report the agent does not take is a gap in that view, not a reason
/// to stop scoring, so it is never retried and never logged.
struct BraidReporter {
    client: reqwest::blocking::Client,
    endpoint: String,
}

impl BraidReporter {
    /// Open the reporter against `QUALIA_AGENT_URL`, or `None` when it is unset
    /// or empty, which is how a stack runs when nobody is reading the view.
    fn from_env() -> Option<Self> {
        let agent_url = env::var("QUALIA_AGENT_URL").ok()?;
        let agent_url = agent_url.trim().to_string();
        if agent_url.is_empty() {
            return None;
        }
        // The agent serves TLS with the certificate it generates on first start,
        // so the runner cannot verify it; the URL is the operator's.
        let client = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(BRAID_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            client,
            endpoint: format!("{}/braid", agent_url.trim_end_matches('/')),
        })
    }

    /// Report one event, ignoring a delivery the agent did not take.
    fn report(&self, event: BraidEvent) {
        let _ = self.client.post(&self.endpoint).json(&event).send();
    }
}

/// The event a generation swap is reported to the braid as.
///
/// The braid's promotion strand distinguishes a first-time load from the stack
/// returning to the checkpoint it parked; everything the runner did not record
/// as a rollback is a promotion, so a swap the caller cannot classify is never
/// reported as a rollback.
fn promotion_event(change: Option<GenerationChange>, generation: u64) -> BraidEvent {
    match change {
        Some(GenerationChange::RolledBack) => BraidEvent::PromotionRolledBack {
            generation,
            reason: ROLLBACK_REASON.to_string(),
        },
        _ => BraidEvent::PromotionAccepted { generation },
    }
}

/// Refuse to start unless `name` is set to exactly `expected`.
fn require_env(name: &str, expected: &str, refusal: &'static str) -> Result<(), Box<dyn Error + Send + Sync>> {
    if env::var(name).as_deref() == Ok(expected) {
        Ok(())
    } else {
        Err(refusal.into())
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, Box<dyn Error + Send + Sync>> {
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("invalid {name}: {error}").into()),
        Err(_) => Ok(default),
    }
}

fn env_f32(name: &str, default: f32) -> Result<f32, Box<dyn Error + Send + Sync>> {
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("invalid {name}: {error}").into()),
        Err(_) => Ok(default),
    }
}

fn millis_to_nanos(value: u64) -> Result<u64, Box<dyn Error + Send + Sync>> {
    value
        .checked_mul(1_000_000)
        .ok_or_else(|| "millisecond configuration overflows nanoseconds".into())
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The braid's promotion strand reads these two variants apart, so what a
    /// swap is reported as — the variant and the reason's text — is pinned here
    /// rather than left to the loop.
    #[test]
    fn a_swap_is_reported_as_the_variant_the_pointer_moved() {
        assert_eq!(
            promotion_event(Some(GenerationChange::Promoted), 7),
            BraidEvent::PromotionAccepted { generation: 7 }
        );
        assert_eq!(
            promotion_event(Some(GenerationChange::RolledBack), 3),
            BraidEvent::PromotionRolledBack {
                generation: 3,
                reason: "generation pointer returned to the parked checkpoint".to_string(),
            }
        );
    }
}
