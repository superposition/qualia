//! Step 29's two assertions, as one checker (ticket #45, EPIC-10 #15), ported
//! from `scripts/mission_check.py`.
//!
//! * `braid-terminal` — one bounded exploration mission is delivered to the
//!   agent's broker and then cancelled; `GET /braid`'s `open_missions` rises by
//!   one and returns to where it started with the mission record terminal.
//! * `gpu-memory` — the peak memory the run used stays under the 8 GB bound;
//!   `nvidia-smi` first, `tegrastats` (the board's own sampler) second, and a
//!   reading that cannot be taken is `CANNOT-ASSERT`, never a pass.
//!
//! The transitions and the envelope are pure functions, so `--self-test`
//! exercises them here with no board and no stack — including the agent's
//! `MissionEnvelopeV1::validate()` bounds (`crates/sync-types/src/mission.rs`),
//! which a generated envelope must satisfy or the broker refuses it with `400`.
//! The port keeps the verdicts, the summary lines (`mission: OK` / `FAIL` /
//! `CANNOT-ASSERT`) and the exit codes (0/1/2).

use clap::Args;
use regex::Regex;
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BRAID_PATH: &str = "/braid";
const MISSIONS_PATH: &str = "/mission-control/missions";
const ENVELOPES_PATH: &str = "/mission-control/envelopes";

// `crates/sync-types/src/mission.rs` fixes these; the envelope this checker
// builds has to satisfy the same `validate()` the broker runs, or the delivery
// is a 400.
const MISSION_ENVELOPE_SCHEMA: &str = "qualia.mission-envelope.v1";
const MAX_DEADLINE_MS: i64 = 120_000;
const MIN_RUNTIME_MS: i64 = 100;
const MAX_RUNTIME_MS: i64 = 120_000;
const MIN_SPEED_MPS: f64 = 0.01;
const MAX_SPEED_MPS: f64 = 0.25;
const MIN_DISTANCE_M: f64 = 0.05;
const MAX_DISTANCE_M: f64 = 5.0;
const MAX_REPLANS: i64 = 8;
const MIN_EVIDENCE_MAX_AGE_MS: i64 = 100;
const MAX_EVIDENCE_MAX_AGE_MS: i64 = 5_000;
const MAX_AREA_SPAN_M: f64 = 20.0;

const JSON_SAFE_INTEGER_MAX: i64 = (1 << 53) - 1;
const IDENTIFIER_EXTRA: [char; 4] = ['-', '_', '.', ':'];
const ENVELOPE_KEYS: [&str; 13] = [
    "schema_version",
    "broker_id",
    "producer_epoch",
    "sequence",
    "mission_id",
    "idempotency_key",
    "command",
    "issued_at_ms",
    "deadline_ms",
    "objective",
    "constraints",
    "evidence_refs",
    "fly_governed",
];
const OBJECTIVE_KEYS: [&str; 5] = ["kind", "summary", "target_x_m", "target_y_m", "tolerance_m"];
const CONSTRAINT_KEYS: [&str; 6] = [
    "operating_area",
    "speed_ceiling_mps",
    "max_distance_m",
    "max_runtime_ms",
    "max_replans",
    "evidence_max_age_ms",
];
const AREA_KEYS: [&str; 5] = ["frame_id", "min_x_m", "min_y_m", "max_x_m", "max_y_m"];
const COMMANDS: [&str; 4] = ["start", "pause", "resume", "cancel"];
const POINT_KINDS: [&str; 2] = ["observe_point", "navigate_to"];

/// Step 29's bound: "under 8 GB used".
const DEFAULT_BUDGET_MIB: i64 = 8_192;
const DEFAULT_DEADLINE_S: i64 = 20;
const DEFAULT_OPEN_TIMEOUT_S: f64 = 60.0;
const DEFAULT_CLOSE_TIMEOUT_S: f64 = 120.0;
/// The mission is held open this long before the cancel, so the memory sampler
/// sees the stack working rather than one instant of it.
const DEFAULT_HOLD_S: f64 = 3.0;
const POLL_INTERVAL_S: f64 = 0.5;
const MEMORY_SAMPLE_INTERVAL_S: f64 = 1.0;
const TERMINAL_STATUSES: [&str; 3] = ["completed", "failed", "cancelled"];

#[derive(Args)]
pub struct MissionArgs {
    /// Agent base URL
    #[arg(long = "agent-url", default_value = "https://127.0.0.1:8081")]
    pub agent_url: String,

    /// Mission-broker bearer token (default: $QUALIA_MISSION_BROKER_TOKEN)
    #[arg(long)]
    pub token: Option<String>,

    /// Mission id (default: t29-board-<epoch>)
    #[arg(long = "mission-id")]
    pub mission_id: Option<String>,

    /// Mission deadline in seconds
    #[arg(long = "deadline-s", default_value_t = DEFAULT_DEADLINE_S)]
    pub deadline_s: i64,

    /// Seconds to wait for the mission to open
    #[arg(long = "open-timeout-s", default_value_t = DEFAULT_OPEN_TIMEOUT_S)]
    pub open_timeout_s: f64,

    /// Seconds to wait for the mission to close
    #[arg(long = "close-timeout-s", default_value_t = DEFAULT_CLOSE_TIMEOUT_S)]
    pub close_timeout_s: f64,

    /// Seconds to hold the mission open before cancelling it
    #[arg(long = "hold-s", default_value_t = DEFAULT_HOLD_S)]
    pub hold_s: f64,

    /// The memory bound in MiB (step 29's 8 GB)
    #[arg(long = "budget-mib", default_value_t = DEFAULT_BUDGET_MIB)]
    pub budget_mib: i64,

    /// Where memory is read
    #[arg(
        long = "memory-source",
        default_value = "auto",
        value_parser = ["auto", "nvidia-smi", "tegrastats"]
    )]
    pub memory_source: String,

    /// Write the observation record here
    #[arg(long = "json")]
    pub json_out: Option<String>,

    /// Run the pure-function fixtures; no board needed
    #[arg(long = "self-test")]
    pub self_test: bool,
}

// --------------------------------------------------------------------------
// The transition, as a pure function
// --------------------------------------------------------------------------

/// The counts with runs collapsed, so a failure detail line stays readable.
fn sequence(counts: &[i64]) -> String {
    let mut distinct: Vec<i64> = Vec::new();
    for count in counts {
        if distinct.last() != Some(count) {
            distinct.push(*count);
        }
    }
    distinct
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Judge the `open_missions` 1 -> 0 transition from the observed samples.
/// Returns `(verdict, detail)` with verdict `OK`, `FAIL` or `CANNOT-ASSERT`.
pub fn evaluate_transition(
    samples: &[(f64, i64)],
    before: i64,
    open_timeout_s: f64,
    close_timeout_s: f64,
) -> (String, String) {
    if samples.is_empty() {
        return ("CANNOT-ASSERT".into(), "the braid was never read".into());
    }
    let counts: Vec<i64> = samples.iter().map(|(_, count)| *count).collect();
    let last = samples.last().expect("non-empty").0;
    let Some(opened) = samples.iter().find(|(_, count)| *count > before).map(|(elapsed, _)| *elapsed)
    else {
        return (
            "FAIL".into(),
            format!(
                "open_missions never rose above {before} (saw {} over {last:.1} s)",
                sequence(&counts)
            ),
        );
    };
    if opened > open_timeout_s {
        return (
            "FAIL".into(),
            format!(
                "open_missions took {opened:.1} s to rise above {before} (limit {open_timeout_s:.1} s)"
            ),
        );
    }
    let Some(closed) = samples
        .iter()
        .find(|(elapsed, count)| *elapsed >= opened && *count <= before)
        .map(|(elapsed, _)| *elapsed)
    else {
        return (
            "FAIL".into(),
            format!(
                "open_missions never returned to {before} after opening above it (saw {} over {last:.1} s)",
                sequence(&counts)
            ),
        );
    };
    if closed - opened > close_timeout_s {
        return (
            "FAIL".into(),
            format!(
                "open_missions took {:.1} s to return to {before} (limit {close_timeout_s:.1} s)",
                closed - opened
            ),
        );
    }
    (
        "OK".into(),
        format!(
            "open_missions {} (started at {before}; rose {opened:.1} s in, returned {closed:.1} s in)",
            sequence(&counts)
        ),
    )
}

/// Judge the memory bound. Step 29 says *under* 8 GB, so the bound is strict.
pub fn evaluate_memory(peak_mib: Option<i64>, budget_mib: i64) -> (String, String) {
    match peak_mib {
        None => (
            "CANNOT-ASSERT".into(),
            "neither nvidia-smi nor tegrastats produced a number".into(),
        ),
        Some(peak) if peak < budget_mib => (
            "OK".into(),
            format!("peak {peak} MiB of {budget_mib} MiB budget"),
        ),
        Some(peak) => (
            "FAIL".into(),
            format!("peak {peak} MiB is not under the {budget_mib} MiB budget"),
        ),
    }
}

// --------------------------------------------------------------------------
// The envelope, as a pure function
// --------------------------------------------------------------------------

/// A bounded `explore_frontier` envelope the broker's `validate()` accepts. The
/// deadline is clamped into the documented window: the broker rejects an
/// envelope whose deadline is more than 120 s after issue, and `ingest` rejects
/// one already past, so `deadline_s` is clamped to 1..=120.
pub fn build_envelope(
    mission_id: &str,
    issued_at_ms: i64,
    deadline_s: i64,
    idempotency_key: Option<&str>,
) -> Value {
    let deadline_s = deadline_s.clamp(1, MAX_DEADLINE_MS / 1000);
    let runtime_ms = (deadline_s * 1000).clamp(MIN_RUNTIME_MS, MAX_RUNTIME_MS);
    json!({
        "schema_version": MISSION_ENVELOPE_SCHEMA,
        "broker_id": "board-t29",
        "producer_epoch": 1,
        "sequence": 1,
        "mission_id": mission_id,
        "idempotency_key": idempotency_key.unwrap_or(mission_id),
        "command": "start",
        "issued_at_ms": issued_at_ms,
        "deadline_ms": issued_at_ms + deadline_s * 1000,
        "objective": {
            "kind": "explore_frontier",
            "summary": "board end-to-end: bounded frontier sweep (T29, step 29)",
            "target_x_m": Value::Null,
            "target_y_m": Value::Null,
            "tolerance_m": Value::Null,
        },
        "constraints": {
            "operating_area": {
                "frame_id": "odom",
                "min_x_m": -10.0,
                "min_y_m": -10.0,
                "max_x_m": 10.0,
                "max_y_m": 10.0,
            },
            "speed_ceiling_mps": 0.05,
            "max_distance_m": 1.0,
            "max_runtime_ms": runtime_ms,
            "max_replans": 2,
            "evidence_max_age_ms": 1_000,
        },
        // A `start` with no evidence reference is refused by `validate()`.
        "evidence_refs": [
            "docs/decisions.md#d-010--the-deploy-target-pinkie-is-attached-to-the-dev-host"
        ],
        "fly_governed": true,
    })
}

/// The second envelope of the pair: same mission, `cancel`, sequence 2.
pub fn build_cancel_envelope(mission_id: &str, issued_at_ms: i64, deadline_s: i64) -> Value {
    let mut envelope = build_envelope(
        mission_id,
        issued_at_ms,
        deadline_s,
        Some(&format!("{mission_id}-cancel")),
    );
    envelope["sequence"] = json!(2);
    envelope["command"] = json!("cancel");
    envelope
}

/// The broker's `validate()` bounds, restated as a list of violations.
pub fn envelope_bounds_errors(envelope: &Value) -> Vec<String> {
    let mut errors = Vec::new();

    let identifier = |field: &str, value: &str, errors: &mut Vec<String>| {
        if value.is_empty()
            || value.chars().count() > 160
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || IDENTIFIER_EXTRA.contains(&c))
        {
            errors.push(format!("{field} must contain 1..=160 URL-safe characters"));
        }
    };

    let object_keys = |value: &Value| -> Vec<String> {
        value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    };
    for key in object_keys(envelope) {
        if !ENVELOPE_KEYS.contains(&key.as_str()) {
            errors.push(format!("unexpected envelope field {}", crate::pytext::py_repr(&key)));
        }
    }
    if envelope.get("schema_version").and_then(Value::as_str) != Some(MISSION_ENVELOPE_SCHEMA) {
        errors.push("unsupported envelope schema".to_string());
    }
    for field in ["broker_id", "mission_id", "idempotency_key"] {
        let value = envelope.get(field).and_then(Value::as_str).unwrap_or("");
        identifier(field, value, &mut errors);
    }
    let epoch = envelope.get("producer_epoch").and_then(Value::as_i64).unwrap_or(0);
    let sequence = envelope.get("sequence").and_then(Value::as_i64).unwrap_or(0);
    if !(0 < epoch && epoch <= JSON_SAFE_INTEGER_MAX)
        || !(0 < sequence && sequence <= JSON_SAFE_INTEGER_MAX)
    {
        errors.push("producer epoch and sequence must be non-zero JSON-safe integers".to_string());
    }
    let issued = envelope.get("issued_at_ms").and_then(Value::as_i64).unwrap_or(0);
    let deadline = envelope.get("deadline_ms").and_then(Value::as_i64).unwrap_or(0);
    if issued == 0
        || issued > JSON_SAFE_INTEGER_MAX
        || deadline > JSON_SAFE_INTEGER_MAX
        || deadline <= issued
        || deadline - issued > MAX_DEADLINE_MS
    {
        errors.push(format!("deadline must be within {MAX_DEADLINE_MS} ms of issue"));
    }
    if !envelope
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| COMMANDS.contains(&command))
    {
        errors.push(format!("command must be one of {}", COMMANDS.join(", ")));
    }

    let objective = envelope.get("objective").cloned().unwrap_or(json!({}));
    for key in object_keys(&objective) {
        if !OBJECTIVE_KEYS.contains(&key.as_str()) {
            errors.push(format!("unexpected objective field {}", crate::pytext::py_repr(&key)));
        }
    }
    let summary = objective.get("summary").and_then(Value::as_str).unwrap_or("");
    if summary.trim().is_empty() || summary.chars().count() > 512 {
        errors.push("objective summary must contain 1..=512 characters".to_string());
    }
    let x = objective.get("target_x_m").filter(|value| !value.is_null());
    let y = objective.get("target_y_m").filter(|value| !value.is_null());
    let kind = objective.get("kind").and_then(Value::as_str).unwrap_or("");
    if POINT_KINDS.contains(&kind) && (x.is_none() || y.is_none()) {
        errors.push("point objectives require target_x_m and target_y_m".to_string());
    }
    if x.is_some() != y.is_some() {
        errors.push("mission target coordinates must be supplied together".to_string());
    }
    for value in [x, y].into_iter().flatten() {
        match value.as_f64() {
            Some(number) if number.is_finite() => {}
            _ => errors.push("mission target coordinates must be finite".to_string()),
        }
    }
    let tolerance = objective.get("tolerance_m").filter(|value| !value.is_null());
    if let Some(tolerance) = tolerance {
        let valid = tolerance
            .as_f64()
            .is_some_and(|number| number.is_finite() && (0.05..=1.0).contains(&number));
        if !valid {
            errors.push("mission tolerance_m must be in 0.05..=1.0".to_string());
        }
    }

    let constraints = envelope.get("constraints").cloned().unwrap_or(json!({}));
    for key in object_keys(&constraints) {
        if !CONSTRAINT_KEYS.contains(&key.as_str()) {
            errors.push(format!("unexpected constraint field {}", crate::pytext::py_repr(&key)));
        }
    }
    let area = constraints.get("operating_area").cloned().unwrap_or(json!({}));
    for key in object_keys(&area) {
        if !AREA_KEYS.contains(&key.as_str()) {
            errors.push(format!("unexpected operating-area field {}", crate::pytext::py_repr(&key)));
        }
    }
    let corner = |name: &str| {
        area.get(name)
            .and_then(Value::as_f64)
            .filter(|number| number.is_finite())
    };
    let corners = ["min_x_m", "min_y_m", "max_x_m", "max_y_m"].map(|name| area.get(name).cloned());
    let (min_x, min_y, max_x, max_y) = (corner("min_x_m"), corner("min_y_m"), corner("max_x_m"), corner("max_y_m"));
    if area.get("frame_id").and_then(Value::as_str) != Some("odom")
        || corners.iter().any(|value| match value {
            None | Some(Value::Null) => true,
            Some(value) => !value.as_f64().is_some_and(f64::is_finite),
        })
        || min_x.unwrap_or(0.0) >= max_x.unwrap_or(0.0)
        || min_y.unwrap_or(0.0) >= max_y.unwrap_or(0.0)
        || max_x.unwrap_or(0.0) - min_x.unwrap_or(0.0) > MAX_AREA_SPAN_M
        || max_y.unwrap_or(0.0) - min_y.unwrap_or(0.0) > MAX_AREA_SPAN_M
    {
        errors.push(format!(
            "operating area must be a finite odom rectangle no larger than {MAX_AREA_SPAN_M:.0} m"
        ));
    }
    if let (Some(x), Some(y)) = (x.and_then(Value::as_f64), y.and_then(Value::as_f64)) {
        // The Python comparison defaulted a missing corner to 0.
        let inside = min_x.unwrap_or(0.0) <= x
            && x <= max_x.unwrap_or(0.0)
            && min_y.unwrap_or(0.0) <= y
            && y <= max_y.unwrap_or(0.0);
        if !inside {
            errors.push("mission target is outside the bounded operating area".to_string());
        }
    }
    let speed = constraints.get("speed_ceiling_mps").and_then(Value::as_f64);
    let distance = constraints.get("max_distance_m").and_then(Value::as_f64);
    let runtime = constraints.get("max_runtime_ms").and_then(Value::as_i64);
    let replans = constraints.get("max_replans").and_then(Value::as_i64);
    let age = constraints.get("evidence_max_age_ms").and_then(Value::as_i64);
    if !speed.is_some_and(|number| number.is_finite() && (MIN_SPEED_MPS..=MAX_SPEED_MPS).contains(&number))
        || !distance
            .is_some_and(|number| number.is_finite() && (MIN_DISTANCE_M..=MAX_DISTANCE_M).contains(&number))
        || !runtime.is_some_and(|number| (MIN_RUNTIME_MS..=MAX_RUNTIME_MS).contains(&number))
        || !replans.is_some_and(|number| number <= MAX_REPLANS)
        || !age.is_some_and(|number| (MIN_EVIDENCE_MAX_AGE_MS..=MAX_EVIDENCE_MAX_AGE_MS).contains(&number))
    {
        errors.push("mission constraints exceed the bounded low-speed limits".to_string());
    }

    let refs = envelope.get("evidence_refs").and_then(Value::as_array);
    match refs {
        None => errors.push("mission evidence_refs are invalid".to_string()),
        Some(refs) => {
            let invalid = refs.len() > 64
                || refs.iter().any(|reference| {
                    let text = reference.as_str().unwrap_or("");
                    text.trim().is_empty() || text.chars().count() > 256
                });
            if invalid {
                errors.push("mission evidence_refs are invalid".to_string());
            } else if envelope.get("command").and_then(Value::as_str) == Some("start")
                && refs.is_empty()
            {
                errors.push("a mission start requires at least one evidence reference".to_string());
            }
        }
    }
    errors
}

/// The mission id of one `/mission-control/missions` record: `MissionRecordV1`
/// nests the accepted envelope, and a flat `mission_id` is accepted too.
pub fn record_mission_id(record: &Value) -> Option<String> {
    if let Some(envelope) = record.get("envelope") {
        if envelope.is_object() {
            if let Some(id) = envelope.get("mission_id").and_then(Value::as_str) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    record.get("mission_id").and_then(Value::as_str).map(str::to_string)
}

// --------------------------------------------------------------------------
// The two memory sources
// --------------------------------------------------------------------------

/// `nvidia-smi` memory.used in MiB, or `None` when it is not a number. A Jetson
/// reports `[N/A]` here, which is a reading that cannot be judged — not a
/// passing zero.
pub fn parse_nvidia_smi_used_mib(text: &str) -> Option<i64> {
    let regex = Regex::new(r"^(\d+)(?:\s|$)").expect("nvidia-smi regex");
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        return regex.captures(line).and_then(|found| found[1].parse().ok());
    }
    None
}

/// The `RAM <used>/<total>MB` field of a `tegrastats` line, or `None`.
pub fn parse_tegrastats_used_mib(text: &str) -> Option<i64> {
    Regex::new(r"RAM (\d+)/(\d+)MB")
        .expect("tegrastats regex")
        .captures(text)
        .and_then(|found| found[1].parse().ok())
}

/// One bounded subprocess: `(status, stdout, stderr)` or an error string. The
/// Python gates gave `subprocess.run` a timeout; a hung sampler must not hang
/// the gate on the board, so the child is killed at the deadline.
fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Result<(i32, String, String), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().map_err(|error| error.to_string())?;
                return Ok((
                    output.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&output.stdout).to_string(),
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// `(mib, source, note)` from `nvidia-smi`, keeping the raw reading on failure.
fn sample_nvidia_smi() -> (Option<i64>, String, String) {
    let done = run_bounded(
        "nvidia-smi",
        &["--query-gpu=memory.used", "--format=csv,noheader,nounits"],
        Duration::from_secs(10),
    );
    let (status, stdout, stderr) = match done {
        Ok(done) => done,
        Err(error) => return (None, "nvidia-smi".into(), format!("nvidia-smi did not run: {error}")),
    };
    let raw = stdout.trim().to_string();
    if status != 0 {
        let detail = if stderr.trim().is_empty() { "no output".to_string() } else { stderr.trim().to_string() };
        return (None, "nvidia-smi".into(), format!("nvidia-smi exit {status}: {detail}"));
    }
    match parse_nvidia_smi_used_mib(&raw) {
        Some(value) => (Some(value), "nvidia-smi".into(), String::new()),
        None => (
            None,
            "nvidia-smi".into(),
            format!("nvidia-smi reported {}", crate::pytext::py_repr(&raw)),
        ),
    }
}

/// `(mib, source, note)` from `tegrastats`, the board's own sampler.
fn sample_tegrastats() -> (Option<i64>, String, String) {
    let done = run_bounded(
        "timeout",
        &["3", "tegrastats", "--interval", "500"],
        Duration::from_secs(15),
    );
    let (status, stdout, stderr) = match done {
        Ok(done) => done,
        Err(error) => return (None, "tegrastats".into(), format!("tegrastats did not run: {error}")),
    };
    if status != 0 && status != 124 {
        let detail = if stderr.trim().is_empty() { "no output".to_string() } else { stderr.trim().to_string() };
        return (None, "tegrastats".into(), format!("tegrastats exit {status}: {detail}"));
    }
    match parse_tegrastats_used_mib(&stdout) {
        Some(value) => (Some(value), "tegrastats".into(), String::new()),
        None => (None, "tegrastats".into(), "tegrastats printed no RAM line".into()),
    }
}

/// One reading from the requested source, or the documented fallback chain.
fn sample_memory(source: &str) -> (Option<i64>, String, String) {
    if source == "nvidia-smi" {
        return sample_nvidia_smi();
    }
    if source == "tegrastats" {
        return sample_tegrastats();
    }
    let (mib, origin, note) = sample_nvidia_smi();
    if mib.is_some() {
        return (mib, origin, note);
    }
    let (fallback_mib, fallback_origin, fallback_note) = sample_tegrastats();
    if fallback_mib.is_some() {
        return (
            fallback_mib,
            fallback_origin,
            format!("nvidia-smi unusable ({note}); tegrastats supplied the reading"),
        );
    }
    (None, "auto".into(), format!("{note}; {fallback_note}"))
}

// --------------------------------------------------------------------------
// HTTP
// --------------------------------------------------------------------------

/// A refused request (Python's `HTTPError`) or a transport failure.
enum RequestError {
    Refused { status: u16, body: String },
    Failed(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Refused { status, body } => {
                write!(formatter, "HTTP {status}: {}", body.trim())
            }
            RequestError::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

/// One bounded exchange. Returns `(status, decoded_body)`.
fn request(
    url: &str,
    payload: Option<&Value>,
    token: Option<&str>,
    timeout: u64,
) -> Result<(u16, Value), RequestError> {
    // The agent's certificate is self-signed and the checker is the only client
    // on that hop, as `runners/watch` does for `GET /braid`.
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(timeout))
        .build()
        .map_err(|error| RequestError::Failed(error.to_string()))?;
    let mut builder = client.get(url).header("Accept", "application/json");
    if let Some(payload) = payload {
        builder = client.post(url).header("Accept", "application/json").json(payload);
    }
    if let Some(token) = token {
        builder = builder.bearer_auth(token);
    }
    let response = builder.send().map_err(|error| RequestError::Failed(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response.text().map_err(|error| RequestError::Failed(error.to_string()))?;
    if status >= 400 {
        return Err(RequestError::Refused { status, body });
    }
    let decoded = if body.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&body).unwrap_or(Value::Null)
    };
    Ok((status, decoded))
}

fn read_braid(base_url: &str, token: Option<&str>) -> Result<Value, RequestError> {
    request(&format!("{base_url}{BRAID_PATH}"), None, token, 10).map(|(_, view)| view)
}

fn deliver_envelope(base_url: &str, envelope: &Value, token: Option<&str>) -> Result<(u16, Value), RequestError> {
    request(&format!("{base_url}{ENVELOPES_PATH}"), Some(envelope), token, 10)
}

fn read_missions(base_url: &str, token: Option<&str>) -> Result<Vec<Value>, RequestError> {
    let (_, body) = request(&format!("{base_url}{MISSIONS_PATH}"), None, token, 10)?;
    Ok(body
        .get("missions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

// --------------------------------------------------------------------------
// The live run
// --------------------------------------------------------------------------

/// Python's `%s` on a decoded JSON value: a string prints bare, and `None` is
/// how Python renders a missing key, not JSON's `null`.
fn py_value(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// A leg that ended before the assertions could run: record it and report.
fn finish(args: &MissionArgs, record: &mut Value, verdicts: Vec<(&str, String, String)>) -> i32 {
    let mut collect = Vec::new();
    for (leg, verdict, detail) in verdicts {
        record["legs"][leg] = json!({ "verdict": verdict, "detail": detail });
        eprintln!("mission-check: {leg} {verdict} ({detail})");
        collect.push((leg.to_string(), verdict));
    }
    outcome(args, record, collect)
}

fn outcome(args: &MissionArgs, record: &mut Value, verdicts: Vec<(String, String)>) -> i32 {
    record["verdict"] = json!(verdicts
        .iter()
        .map(|(leg, verdict)| (leg.clone(), json!(verdict)))
        .collect::<serde_json::Map<String, Value>>());
    if let Some(json_out) = &args.json_out {
        let path = std::path::Path::new(json_out);
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(record) {
            let _ = std::fs::write(path, format!("{text}\n"));
        }
    }
    if verdicts.iter().any(|(_, verdict)| verdict == "FAIL") {
        let failed: Vec<&str> = verdicts
            .iter()
            .filter(|(_, verdict)| verdict == "FAIL")
            .map(|(leg, _)| leg.as_str())
            .collect();
        println!("mission: FAIL ({})", failed.join(", "));
        return 1;
    }
    if verdicts.iter().any(|(_, verdict)| verdict == "CANNOT-ASSERT") {
        let unjudged: Vec<&str> = verdicts
            .iter()
            .filter(|(_, verdict)| verdict == "CANNOT-ASSERT")
            .map(|(leg, _)| leg.as_str())
            .collect();
        println!("mission: CANNOT-ASSERT ({})", unjudged.join(", "));
        return 2;
    }
    println!("mission: OK");
    0
}

type MissionReader<'a> = dyn Fn(&str, Option<&str>) -> Result<Vec<Value>, RequestError> + 'a;

fn terminal_record(
    read: &MissionReader<'_>,
    base_url: &str,
    token: Option<&str>,
    mission_id: &str,
) -> Option<Value> {
    let missions = match read(base_url, token) {
        Ok(missions) => missions,
        Err(error) => {
            eprintln!("mission-check: could not read {MISSIONS_PATH}: {error}");
            return None;
        }
    };
    missions
        .into_iter()
        .find(|mission| record_mission_id(mission).as_deref() == Some(mission_id))
}

/// Poll the braid through the mission's life, sampling memory alongside it. The
/// mission is ended the way Step 28 ends it: once `open_missions` has risen, the
/// second envelope cancels it; the start envelope's deadline is the backstop.
fn observe(
    base_url: &str,
    token: Option<&str>,
    before: i64,
    args: &MissionArgs,
    mission_id: &str,
    cancel: &Value,
) -> (Vec<(f64, i64)>, Vec<(f64, i64, String)>, Vec<String>, String) {
    let start = Instant::now();
    let mut samples: Vec<(f64, i64)> = Vec::new();
    let mut memories: Vec<(f64, i64, String)> = Vec::new();
    let mut read_failures: Vec<String> = Vec::new();
    let mut memory_note = String::new();
    let mut next_memory_at = 0.0f64;
    let deadline = args.open_timeout_s + args.close_timeout_s;
    let mut cancelled = false;
    let mut hold_until: Option<f64> = None;

    loop {
        let elapsed = start.elapsed().as_secs_f64();
        let mut count: Option<i64> = None;
        match read_braid(base_url, token) {
            Ok(view) => {
                let value = view.get("open_missions").and_then(Value::as_i64).unwrap_or(0);
                samples.push((elapsed, value));
                count = Some(value);
            }
            Err(error) => {
                if read_failures.len() < 3 {
                    read_failures.push(error.to_string());
                }
            }
        }
        if elapsed >= next_memory_at {
            let (mib, source, note) = sample_memory(&args.memory_source);
            match mib {
                None => {
                    if memory_note.is_empty() {
                        memory_note = note;
                    }
                }
                Some(mib) => memories.push((elapsed, mib, source)),
            }
            next_memory_at = elapsed + MEMORY_SAMPLE_INTERVAL_S;
        }
        if let Some(count) = count {
            let opened = samples.iter().any(|(_, value)| *value > before);
            if opened && count <= before {
                break;
            }
            if !opened && elapsed > args.open_timeout_s {
                break;
            }
            if opened && !cancelled {
                if hold_until.is_none() {
                    hold_until = Some(elapsed + args.hold_s);
                }
                if elapsed >= hold_until.unwrap_or(elapsed) {
                    cancelled = true;
                    let cancel_note = match deliver_envelope(base_url, cancel, token) {
                        Ok((status, _)) => format!("cancel answered HTTP {status}"),
                        Err(error) => format!(
                            "cancel delivery failed: {error} (the start deadline closes it)"
                        ),
                    };
                    println!("mission-check: mission {mission_id} {cancel_note}");
                }
            }
        }
        if elapsed > deadline {
            break;
        }
        std::thread::sleep(Duration::from_secs_f64(POLL_INTERVAL_S));
    }
    (samples, memories, read_failures, memory_note)
}

fn execute(args: &MissionArgs) -> i32 {
    let base_url = args.agent_url.trim_end_matches('/').to_string();
    let token = args
        .token
        .clone()
        .filter(|token| !token.is_empty())
        .or_else(|| {
            std::env::var("QUALIA_MISSION_BROKER_TOKEN")
                .ok()
                .filter(|token| !token.is_empty())
        });
    let mut record = json!({
        "agent_url": base_url,
        "budget_mib": args.budget_mib,
        "legs": {},
    });

    // Leg: the agent answers and reports a braid at all.
    let before_view = match read_braid(&base_url, token.as_deref()) {
        Ok(view) => view,
        Err(error) => {
            return finish(
                args,
                &mut record,
                vec![(
                    "agent",
                    "CANNOT-ASSERT".to_string(),
                    format!("{base_url}{BRAID_PATH} did not answer: {error}"),
                )],
            );
        }
    };

    let before = before_view.get("open_missions").and_then(Value::as_i64).unwrap_or(0);
    let session = before_view.get("session_id").cloned().unwrap_or(Value::Null);
    let generation = before_view.get("generation").cloned().unwrap_or(Value::Null);
    println!(
        "mission-check: agent {base_url} answered; braid before: open_missions={before} session={} generation={}",
        py_value(&session),
        py_value(&generation)
    );

    let Some(token) = token else {
        return finish(
            args,
            &mut record,
            vec![(
                "mission-drive",
                "CANNOT-ASSERT".to_string(),
                "no mission-broker token (--token or QUALIA_MISSION_BROKER_TOKEN); the agent never trusts loopback for mission intake".to_string(),
            )],
        );
    };

    let mission_id = args
        .mission_id
        .clone()
        .unwrap_or_else(|| format!("t29-board-{}", now_ms() / 1000));
    let envelope = build_envelope(&mission_id, now_ms(), args.deadline_s, Some(&mission_id));
    let cancel = build_cancel_envelope(&mission_id, now_ms(), args.deadline_s);
    let mut violations = envelope_bounds_errors(&envelope);
    violations.extend(envelope_bounds_errors(&cancel));
    if !violations.is_empty() {
        return finish(
            args,
            &mut record,
            vec![(
                "mission-drive",
                "CANNOT-ASSERT".to_string(),
                format!("generated envelope is invalid: {}", violations.join("; ")),
            )],
        );
    }

    match deliver_envelope(&base_url, &envelope, Some(&token)) {
        Err(RequestError::Refused { status, body }) => {
            return finish(
                args,
                &mut record,
                vec![(
                    "mission-drive",
                    "FAIL".to_string(),
                    format!("envelope refused with HTTP {status}: {}", body.trim()),
                )],
            );
        }
        Err(RequestError::Failed(message)) => {
            return finish(
                args,
                &mut record,
                vec![(
                    "mission-drive",
                    "FAIL".to_string(),
                    format!("envelope delivery failed: {message}"),
                )],
            );
        }
        Ok((status, _ack)) if status != 200 && status != 202 => {
            return finish(
                args,
                &mut record,
                vec![(
                    "mission-drive",
                    "FAIL".to_string(),
                    format!("envelope answered HTTP {status}"),
                )],
            );
        }
        Ok((status, ack)) => {
            println!(
                "mission-check: mission {mission_id} accepted (HTTP {status}, idempotent_replay={})",
                py_value(&ack.get("idempotent_replay").cloned().unwrap_or(Value::Null))
            );
        }
    }

    let (samples, memories, read_failures, memory_note) =
        observe(&base_url, Some(&token), before, args, &mission_id, &cancel);

    // Assertion 1: the braid transition, then the mission record behind it.
    let (mut verdict, mut detail) = evaluate_transition(
        &samples,
        before,
        args.open_timeout_s,
        args.close_timeout_s,
    );
    if verdict == "FAIL" && !read_failures.is_empty() {
        detail = format!("{detail}; braid reads failed: {}", read_failures[0]);
    }
    let mut terminal: Option<Value> = None;
    if verdict == "OK" {
        let reader = |base: &str, token: Option<&str>| read_missions(base, token);
        terminal = terminal_record(&reader, &base_url, Some(&token), &mission_id);
        match &terminal {
            None => {
                verdict = "FAIL".to_string();
                detail = format!("mission {mission_id} has no record on {MISSIONS_PATH}");
            }
            Some(record) => {
                let status = record.get("status").and_then(Value::as_str).unwrap_or("");
                if !TERMINAL_STATUSES.contains(&status) {
                    verdict = "FAIL".to_string();
                    detail = format!(
                        "mission {mission_id} is not terminal: status={} stage={}",
                        py_value(&record.get("status").cloned().unwrap_or(Value::Null)),
                        py_value(&record.get("stage").cloned().unwrap_or(Value::Null))
                    );
                } else {
                    println!(
                        "mission-check: mission record terminal: status={} stage={} code={} detail={}",
                        py_value(&record.get("status").cloned().unwrap_or(Value::Null)),
                        py_value(&record.get("stage").cloned().unwrap_or(Value::Null)),
                        py_value(&record.get("last_code").cloned().unwrap_or(Value::Null)),
                        py_value(&record.get("last_detail").cloned().unwrap_or(Value::Null))
                    );
                }
            }
        }
    }

    // Assertion 2: the memory bound over the same window.
    let peak = memories.iter().map(|(_, mib, _)| *mib).max();
    let mut sources: Vec<String> = memories.iter().map(|(_, _, source)| source.clone()).collect();
    sources.sort();
    sources.dedup();
    let (memory_verdict, mut memory_detail) = evaluate_memory(peak, args.budget_mib);
    if !memories.is_empty() {
        memory_detail = format!(
            "{memory_detail} over {} sample(s) from {}",
            memories.len(),
            sources.join(", ")
        );
    } else if !memory_note.is_empty() {
        memory_detail = format!("{memory_detail} ({memory_note})");
    }

    record["legs"]["braid-terminal"] = json!({
        "before": before,
        "mission_id": mission_id,
        "samples": samples.iter().map(|(elapsed, count)| json!([round2(*elapsed), count])).collect::<Vec<_>>(),
        "record": terminal,
        "verdict": verdict,
        "detail": detail,
    });
    record["legs"]["gpu-memory"] = json!({
        "peak_mib": peak,
        "samples": memories.iter().map(|(elapsed, mib, source)| json!([round2(*elapsed), mib, source])).collect::<Vec<_>>(),
        "sources": sources,
        "note": memory_note,
        "verdict": memory_verdict,
        "detail": memory_detail,
    });

    if verdict == "OK" {
        println!(
            "mission-check: braid-terminal OK ({detail}; session={} generation={})",
            py_value(&session),
            py_value(&generation)
        );
    } else {
        eprintln!("mission-check: braid-terminal {verdict} ({detail})");
    }
    if memory_verdict == "OK" {
        println!("mission-check: gpu-memory OK ({memory_detail})");
    } else {
        eprintln!("mission-check: gpu-memory {memory_verdict} ({memory_detail})");
    }

    outcome(
        args,
        &mut record,
        vec![
            ("braid-terminal".to_string(), verdict),
            ("gpu-memory".to_string(), memory_verdict),
        ],
    )
}

/// `round(x, 2)`, as the record writer used it.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The gate. 0 both assertions hold, 1 an assertion failed, 2 a leg that cannot
/// be judged.
pub fn run(args: &MissionArgs) -> i32 {
    if args.self_test {
        return self_test();
    }
    if args.deadline_s < 1 {
        eprintln!("usage: qualia-gates mission [--agent-url URL] [--token TOKEN] [--self-test] ...");
        eprintln!("mission-check: usage error - --deadline-s must be at least 1");
        return 2;
    }
    execute(args)
}

// --------------------------------------------------------------------------
// The self-test: every pure function, on the host, without a board
// --------------------------------------------------------------------------

struct Cases {
    ran: usize,
    failures: Vec<String>,
}

impl Cases {
    fn case(&mut self, label: &str, condition: bool, detail: &str) {
        self.ran += 1;
        if !condition {
            self.failures.push(if detail.is_empty() {
                label.to_string()
            } else {
                format!("{label}: {detail}")
            });
        }
    }
}

pub fn self_test() -> i32 {
    let mut cases = Cases { ran: 0, failures: Vec::new() };

    // The 1 -> 0 transition, and the ways it does not happen.
    let (verdict, detail) = evaluate_transition(&[(0.0, 0), (1.0, 1), (2.0, 1), (21.0, 0)], 0, 10.0, 30.0);
    cases.case("transition 0 -> 1 -> 0 is OK", verdict == "OK", &detail);
    cases.case("the transition detail names the sequence", detail.contains("0 -> 1 -> 0"), &detail);
    let (verdict, _) = evaluate_transition(&[(0.0, 0), (30.0, 0)], 0, 10.0, 30.0);
    cases.case("a mission that never opens fails", verdict == "FAIL", "");
    let (verdict, _) = evaluate_transition(&[(0.0, 0), (1.0, 1), (60.0, 1)], 0, 10.0, 30.0);
    cases.case("a mission that never closes fails", verdict == "FAIL", "");
    let (verdict, detail) = evaluate_transition(&[(0.0, 2), (1.0, 3), (9.0, 2)], 2, 10.0, 30.0);
    cases.case(
        "a repeat run with two missions already open still transitions",
        verdict == "OK",
        &detail,
    );
    let (verdict, _) = evaluate_transition(&[(0.0, 0), (12.0, 1), (13.0, 0)], 0, 10.0, 30.0);
    cases.case("an open past the open window fails", verdict == "FAIL", "");
    let (verdict, detail) = evaluate_transition(&[(0.0, 0), (1.0, 1), (50.0, 0)], 0, 10.0, 30.0);
    cases.case("a close past the close window fails", verdict == "FAIL", &detail);
    let (verdict, _) = evaluate_transition(&[], 0, 10.0, 30.0);
    cases.case("no braid reads cannot be asserted", verdict == "CANNOT-ASSERT", "");

    // The memory bound, including its boundary and the unreadable case.
    cases.case("4096 MiB is under the 8192 MiB budget", evaluate_memory(Some(4096), 8192).0 == "OK", "");
    cases.case("8191 MiB is under the 8192 MiB budget", evaluate_memory(Some(8191), 8192).0 == "OK", "");
    cases.case("8192 MiB is not under the 8192 MiB budget", evaluate_memory(Some(8192), 8192).0 == "FAIL", "");
    cases.case(
        "an unread memory value cannot be asserted",
        evaluate_memory(None, 8192).0 == "CANNOT-ASSERT",
        "",
    );

    // Both parsers, against the readings the two tools actually print.
    cases.case("nvidia-smi's 412 parses", parse_nvidia_smi_used_mib("412") == Some(412), "");
    cases.case("nvidia-smi's [N/A] does not parse", parse_nvidia_smi_used_mib("[N/A]").is_none(), "");
    cases.case("an empty nvidia-smi reading does not parse", parse_nvidia_smi_used_mib("").is_none(), "");
    cases.case(
        "tegrastats' RAM line parses",
        parse_tegrastats_used_mib("RAM 1573/3601MB (lfb 1x2MB) SWAP 109/14088MB (cached 0MB)")
            == Some(1573),
        "",
    );
    cases.case(
        "a tegrastats line with no RAM does not parse",
        parse_tegrastats_used_mib("GR3D_FREQ 0%").is_none(),
        "",
    );

    // The envelope must satisfy the bounds the broker validates.
    let issued = 1_700_000_000_000i64;
    let envelope = build_envelope("t29-board-selftest", issued, DEFAULT_DEADLINE_S, None);
    cases.case(
        "the generated envelope satisfies the broker's bounds",
        envelope_bounds_errors(&envelope).is_empty(),
        &envelope_bounds_errors(&envelope).join("; "),
    );
    cases.case(
        "the generated envelope opens a frontier mission",
        envelope["objective"]["kind"] == json!("explore_frontier"),
        "",
    );
    cases.case(
        "the generated envelope is a start with evidence",
        envelope["command"] == json!("start")
            && envelope["evidence_refs"].as_array().is_some_and(|refs| !refs.is_empty()),
        "",
    );
    cases.case(
        "an over-long deadline is clamped",
        envelope_bounds_errors(&build_envelope("m", issued, 9_999, None)).is_empty(),
        "",
    );
    let mut unclamped = build_envelope("m", issued, 1, None);
    unclamped["deadline_ms"] = json!(unclamped["issued_at_ms"].as_i64().unwrap_or(0) + MAX_DEADLINE_MS + 1);
    cases.case(
        "the bounds check catches an over-long deadline",
        envelope_bounds_errors(&unclamped).iter().any(|error| error.contains("deadline")),
        "",
    );
    let mut no_evidence = build_envelope("m", issued, 1, None);
    no_evidence["evidence_refs"] = json!([]);
    cases.case(
        "the bounds check catches a start with no evidence",
        envelope_bounds_errors(&no_evidence)
            .iter()
            .any(|error| error.contains("evidence reference")),
        "",
    );
    let mut wide_area = build_envelope("m", issued, 1, None);
    wide_area["constraints"]["operating_area"]["max_x_m"] = json!(100.0);
    cases.case(
        "the bounds check catches an over-wide area",
        envelope_bounds_errors(&wide_area)
            .iter()
            .any(|error| error.contains("operating area")),
        "",
    );

    let cancel = build_cancel_envelope("t29-board-selftest", issued, DEFAULT_DEADLINE_S);
    cases.case(
        "the cancel envelope satisfies the broker's bounds",
        envelope_bounds_errors(&cancel).is_empty(),
        &envelope_bounds_errors(&cancel).join("; "),
    );
    cases.case(
        "the cancel envelope is sequence 2 of the same mission",
        cancel["sequence"] == json!(2)
            && cancel["command"] == json!("cancel")
            && cancel["mission_id"] == envelope["mission_id"]
            && cancel["producer_epoch"] == envelope["producer_epoch"],
        "",
    );
    cases.case(
        "the cancel envelope carries its own idempotency key",
        cancel["idempotency_key"] != envelope["idempotency_key"],
        "",
    );

    // Bounds beyond the ranges: the key sets, the identifier alphabet and the
    // point-objective rules.
    let mut extra = build_envelope("m", issued, 1, None);
    extra["surprise"] = json!(1);
    cases.case(
        "the bounds check catches an unknown envelope field",
        envelope_bounds_errors(&extra)
            .iter()
            .any(|error| error.contains("unexpected envelope field")),
        "",
    );
    let mut bad_id = build_envelope("m", issued, 1, None);
    bad_id["mission_id"] = json!("not url safe/at all");
    cases.case(
        "the bounds check catches a non-URL-safe identifier",
        envelope_bounds_errors(&bad_id).iter().any(|error| error.contains("URL-safe")),
        "",
    );
    let mut point = build_envelope("m", issued, 1, None);
    point["objective"]["kind"] = json!("navigate_to");
    cases.case(
        "the bounds check catches a point objective with no target",
        envelope_bounds_errors(&point)
            .iter()
            .any(|error| error.contains("point objective")),
        "",
    );
    let mut outside = build_envelope("m", issued, 1, None);
    outside["objective"]["target_x_m"] = json!(40.0);
    outside["objective"]["target_y_m"] = json!(0.0);
    cases.case(
        "the bounds check catches a target outside the area",
        envelope_bounds_errors(&outside)
            .iter()
            .any(|error| error.contains("outside the bounded operating area")),
        "",
    );
    let mut inside = build_envelope("m", issued, 1, None);
    inside["objective"]["kind"] = json!("navigate_to");
    inside["objective"]["target_x_m"] = json!(1.0);
    inside["objective"]["target_y_m"] = json!(-2.0);
    cases.case(
        "a target inside the area is legal",
        envelope_bounds_errors(&inside).is_empty(),
        &envelope_bounds_errors(&inside).join("; "),
    );
    let mut no_command = build_envelope("m", issued, 1, None);
    no_command["command"] = json!("teleport");
    cases.case(
        "the bounds check catches an unknown command",
        envelope_bounds_errors(&no_command)
            .iter()
            .any(|error| error.contains("command must be one of")),
        "",
    );

    // The record shape: MissionRecordV1 nests the envelope.
    let nested = json!({
        "envelope": {"mission_id": "m-nested"},
        "status": "cancelled",
        "stage": "terminal",
        "last_code": "cancelled",
        "last_detail": "broker cancelled the mission",
    });
    let flat = json!({
        "mission_id": "m-flat",
        "status": "failed",
        "stage": "terminal",
        "last_code": "deadline_exceeded",
    });
    cases.case(
        "a nested record reports its envelope's mission id",
        record_mission_id(&nested).as_deref() == Some("m-nested"),
        "",
    );
    cases.case(
        "a flat record reports its own mission id",
        record_mission_id(&flat).as_deref() == Some("m-flat"),
        "",
    );
    cases.case(
        "a record with no id reports none",
        record_mission_id(&json!({"status": "cancelled"})).is_none(),
        "",
    );
    cases.case(
        "the terminal record shapes are the statuses the checker accepts",
        TERMINAL_STATUSES.contains(&nested["status"].as_str().unwrap_or(""))
            && TERMINAL_STATUSES.contains(&flat["status"].as_str().unwrap_or("")),
        "",
    );

    // The lookup itself, not a re-implementation of it: the fixture reader
    // stands in for the missions route and calls the real `terminal_record`.
    let nested_reader = |_base: &str, _token: Option<&str>| {
        Ok(vec![json!({"envelope": {"mission_id": "other"}}), nested.clone()])
    };
    cases.case(
        "terminal_record finds a nested record",
        terminal_record(&nested_reader, "http://unit", Some("token"), "m-nested").as_ref() == Some(&nested),
        "",
    );
    let flat_reader = |_base: &str, _token: Option<&str>| Ok(vec![flat.clone()]);
    cases.case(
        "terminal_record finds a flat record",
        terminal_record(&flat_reader, "http://unit", Some("token"), "m-flat").as_ref() == Some(&flat),
        "",
    );
    cases.case(
        "terminal_record reports no record for an id it does not hold",
        terminal_record(&flat_reader, "http://unit", Some("token"), "m-missing").is_none(),
        "",
    );

    if !cases.failures.is_empty() {
        for failure in &cases.failures {
            eprintln!("mission-check: self-test FAIL - {failure}");
        }
        return 1;
    }
    println!("mission-check: self-test OK ({} cases)", cases.ran);
    0
}
