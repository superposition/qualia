//! `qualia-mission-broker` — the producing side of the mission wire.
//!
//! Nothing else in this repository calls a model. This crate does: it watches
//! the braid's proposals, asks the coach (an OpenAI-compatible chat surface,
//! DeepSeek by default) for one [`qualia_sync_types::CoachDecision`] per item
//! that needs one, and publishes the resulting
//! [`qualia_sync_types::MissionEnvelopeV1`] to the agent's operator surface —
//! `POST /mission-control/envelopes`, read back through
//! `/mission-control/missions` and `/mission-control/events` — with the bearer
//! scope the agent already defines (`QUALIA_MISSION_BROKER_TOKEN`).
//!
//! Two rules shape everything here:
//!
//! - **Honesty over theatre.** A run with no key, or a provider that does not
//!   answer inside the timeout, produces *no decision*: one named line, no
//!   envelope, and `llm_priors_ablated: true` with the reason. A decision a
//!   model actually produced records `false` plus the model id, the prompt
//!   digest, the response id, the token counts (or that the provider returned
//!   none) and a wall-clock stamp.
//! - **The key is environment only.** It is never a file in the tree, never in
//!   a log line, an error path, a status payload or an evidence file, and the
//!   redaction never emits the credential's own shape (see [`redact`]).
//!
//! The status surface ([`status`]) is a read-only loopback JSON endpoint the
//! operator's console reads for its Coach panel; it is a second source beside
//! the braid, and the console's README records why and what it would take to
//! retire it.

pub mod agent;
pub mod broker;
pub mod coach;
pub mod config;
pub mod envelope;
pub mod observer;
pub mod redact;
pub mod status;

/// The schema literal the broker's status surface carries.
pub const COACH_STATE_SCHEMA: &str = "qualia.coach-state.v1";
/// The schema of one line of the broker's own journal.
pub const BROKER_JOURNAL_SCHEMA: &str = "qualia.mission-broker-journal.v1";

/// Wall clock in milliseconds since the Unix epoch.
pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(1_700_000_000_000)
}

/// A producer epoch that fits a JSON number exactly and still orders restarts.
///
/// The same shape the agent's mission control seeds its own event epoch with:
/// a broker run is one epoch, its first delivery is sequence 1, and a restart
/// gets a new epoch rather than a gap the agent would refuse.
pub fn epoch_seed() -> u64 {
    const JSON_SAFE_INTEGER_MAX: u64 = (1_u64 << 53) - 1;
    let now = now_ms().min(JSON_SAFE_INTEGER_MAX as u128) as u64;
    let time_bits = now & (JSON_SAFE_INTEGER_MAX >> 12);
    ((time_bits << 12) ^ std::process::id() as u64).max(1)
}

/// Print one line of the run, with any credential shape scrubbed.
pub fn say(line: &str) {
    println!("{}", redact::secrets(line));
}

/// Print one line of the run's audit to stderr, with any credential shape
/// scrubbed.
pub fn warn(line: &str) {
    eprintln!("{}", redact::secrets(line));
}
