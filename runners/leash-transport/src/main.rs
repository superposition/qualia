//! `qualia-leash-transport`: our wheel commands, applied through the leash.
//!
//! On this robot the body's actuator has one owner: `leash serve http` holds
//! `/dev/ttyTHS1` (D-025), so `runners/drive` cannot open the link and nothing
//! it writes reaches the wheels. The leash republishes the actuator on its own
//! surface, and this runner is the transport between the two: it reads our
//! wheel commands, applies each one through the leash's `drive`, and sends a
//! non-latching zero-speed stop whenever a command fails or the stream goes
//! quiet for longer than the deadman.
//!
//! # The command stream
//!
//! One command per line on stdin, in the frame `runners/drive` puts on the
//! wire — the same three fields, so a producer that speaks the firmware's
//! protocol needs no adapter:
//!
//! ```text
//! {"T":41,"L":0.16,"R":0.16}
//! ```
//!
//! `L` and `R` are the left and right wheel speeds `runners/drive`'s control
//! law resolves from the active [`qualia_types::NavGoal`]; the connectome loop's
//! left/right decision carries the same two numbers. `T` is the producer's tick
//! and is carried for the log only. A line the parser cannot read is not a
//! command: the transport sends zero and says so, because a malformed frame
//! must never leave the previous command standing. End of input is the quiet
//! stream: the transport stays stopped (and keeps publishing the stop records
//! the dataset leg reads) rather than exiting.
//!
//! # The deadman
//!
//! Every applied command carries an expiry (`QUALIA_LEASH_TRANSPORT_DEADMAN_MS`,
//! default 500). If the next command does not arrive inside it the transport
//! sends zero speed — the leash's `stop` — and logs the expiry with the age of
//! the last command, so "the stream stopped and the wheels stopped" is a line
//! with a number in it. The clock runs from the frame's **arrival**, not from
//! the leash's answer, so the budget is the producer's cadence rather than the
//! cadence plus a round trip. A transport error is a stop too: a failed call
//! never leaves the last command standing.
//!
//! # `stop` and `estop`
//!
//! The leash latches an `estop` until `estop_reset`, and this transport watches
//! the leash's own `health` for it. **Only a live, armed harness receives
//! commands.** While `health` reports a latched `estop`, a mode other than
//! `live`, or a deadman it is not satisfied with, the transport refuses every
//! command locally — it calls no `drive`, logs the refusal with the harness's
//! own mode/deadman/estop and the command it dropped, and polls `health` until
//! the harness is armed again, at which point it says so and takes commands
//! again. A leash in `replay` can answer about motion that did not happen
//! (`valid: true, armed: false`), which is exactly the state where a command
//! must not be sent.
//!
//! A non-latching operator `stop` is not observable by a client: leash's stop
//! receipts coalesce (measured `coalesced: 3865, through_request_sequence:
//! 3866` on the deployed build), so nothing on the surface distinguishes the
//! operator's zero from this transport's own. What the transport guarantees
//! instead is that no command ever stands: between two commands there is always
//! either the next command inside the deadman or a zero, and the operator's hard
//! stop is the latching `estop`.
//!
//! A zero ends the lease. Measured on the deployed harness: after
//! `POST /motors/stop/verified` the next `drive` with the same pilot token is
//! refused `{"error":"invalid pilot token","ok":false}`, and a fresh
//! `authorize` makes the following `drive` carry again — a verified stop ends
//! the pilot's authority. The transport re-registers a lease after every zero,
//! so the command after a stop is not refused for a reason this runner could
//! have avoided. And a stop the leash does not acknowledge sets a hold: the
//! transport keeps asking for zero and applies no command until one is
//! acknowledged, because an unacknowledged zero is not a zero.
//!
//! # The record
//!
//! The applied command is the leash's, not this runner's: every field of the
//! published [`AppliedActionSnapshot`] is copied from the leash's reply
//! (`applied_*` from its `left`/`right`, `valid` from its `ok`, `safety_flags`
//! from its own flags), and the authoritative record of what reached the wheels
//! is the leash's own ledger, quoted from `GET /action-evidence` and
//! `GET /evidence/action/applied`. This runner keeps no ledger of its own; what
//! it publishes into the arena is the dataset leg's `transport_accepted`
//! interface, carrying leash authority.
//!
//! # Routes
//!
//! Two interfaces, one seam, chosen by `QUALIA_LEASH_TRANSPORT_ROUTE`:
//!
//! | Route | Authorize | Drive | Zero |
//! |---|---|---|---|
//! | `http` (default) | `POST /pilot/authorize`, bearer-authenticated | `POST /motors/drive` | `POST /motors/stop/verified` |
//! | `mcp` | `tools/call` `invoke_capability {capability:"authorize"}` | `…{capability:"drive"}` | `tools/call` `stop` |
//!
//! `http` is the default because it is the route the deployed leash serves:
//! `44fa250/src/http.rs:3713-3727` refuses the MCP `invoke_capability` physical
//! capabilities unless the deployment sets `LEASH_ALLOW_LEGACY_PHYSICAL_CONTROL`
//! *and* the caller is a loopback peer, and the running harness has that key
//! unset. The `mcp` route is the ticket's `tools/call` interface, kept one
//! variable away (and needing the transport to run on the robot, since that
//! gate is a peer check). Both routes carry the same
//! `left`/`right`/`speed_mode` command; only the envelope differs.
//!
//! The pilot token is a short lease: `authorize` is called with the operator's
//! bearer secret, read from the file `QUALIA_LEASH_OPERATOR_TOKEN_FILE` names,
//! and the lease is refreshed at half its TTL. The session token this runner
//! registers is generated here and is deliberately *not* the bearer secret
//! (leash refuses that: "pilot session token must be distinct from the operator
//! bearer token"). Neither value is ever logged: the log carries the file's
//! path and one eight-hex label derived once at startup from the session token,
//! the same label on every line of the run, so a session can be followed through
//! the log.
//! The bearer secret is never written to the arena, a log line, an evidence file
//! or a commit.
//!
//! Configuration:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `QUALIA_LEASH_BASE_URL` | The leash's HTTP root; default `http://127.0.0.1:8000`. |
//! | `QUALIA_SHM_NAME` | The arena to attach to; default `/qualia_body`. |
//! | `QUALIA_LEASH_TRANSPORT_ROUTE` | `http` (default) or `mcp`. |
//! | `QUALIA_LEASH_TRANSPORT_DEADMAN_MS` | Command expiry; default 500. |
//! | `QUALIA_LEASH_TRANSPORT_LEASE_TTL_SECS` | Pilot lease TTL; default 20. |
//! | `QUALIA_LEASH_TRANSPORT_HEALTH_MS` | `health` poll interval; default 500. |
//! | `QUALIA_LEASH_TRANSPORT_POLL_MS` | Idle stop interval; default 100. |
//! | `QUALIA_LEASH_TRANSPORT_LOG_EVERY` | Idle intervals between summary lines; default 50. |
//! | `QUALIA_LEASH_SPEED_MODE` | The operator's speed mode; default `low`. |
//! | `QUALIA_LEASH_OPERATOR_TOKEN_FILE` | The operator bearer secret's file. |
//! | `QUALIA_LEASH_SENSORS_TIMEOUT_MS` | Request timeout; default 2000. |
//!
//! Exit codes: 1 an arena that cannot be opened; 2 a route or speed mode the
//! transport does not know, or no usable operator token file.

use qualia_leash_sensors::{
    agent, call, call_with, BASE_URL_ENV, DEFAULT_BASE_URL, DEFAULT_SHM_NAME, DEFAULT_TIMEOUT_MS,
    SHM_NAME_ENV, TIMEOUT_MS_ENV,
};
use qualia_shm::ShmRegion;
use qualia_types::{
    AppliedActionHistory, AppliedActionSnapshot, ACTION_AUTHORITY_LEASH,
    LEASH_ACTION_SAFETY_DEADMAN, LEASH_ACTION_SAFETY_ESTOP, LEASH_ACTION_SAFETY_SOFT_ODOMETRY_LIMIT,
    LEASH_ACTION_SAFETY_VERIFIED_ZERO,
};
use serde::Deserialize;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `QUALIA_LEASH_TRANSPORT_ROUTE`, default `http`.
pub const ROUTE_ENV: &str = "QUALIA_LEASH_TRANSPORT_ROUTE";
/// `QUALIA_LEASH_TRANSPORT_DEADMAN_MS`, default `500`.
pub const DEADMAN_MS_ENV: &str = "QUALIA_LEASH_TRANSPORT_DEADMAN_MS";
/// `QUALIA_LEASH_TRANSPORT_LEASE_TTL_SECS`, default `20`.
pub const LEASE_TTL_SECS_ENV: &str = "QUALIA_LEASH_TRANSPORT_LEASE_TTL_SECS";
/// `QUALIA_LEASH_TRANSPORT_HEALTH_MS`, default `500`.
pub const HEALTH_MS_ENV: &str = "QUALIA_LEASH_TRANSPORT_HEALTH_MS";
/// `QUALIA_LEASH_TRANSPORT_POLL_MS`, default `100`.
pub const POLL_MS_ENV: &str = "QUALIA_LEASH_TRANSPORT_POLL_MS";
/// `QUALIA_LEASH_TRANSPORT_LOG_EVERY`, default `50`.
pub const LOG_EVERY_ENV: &str = "QUALIA_LEASH_TRANSPORT_LOG_EVERY";
/// `QUALIA_LEASH_SPEED_MODE`, default `low`.
pub const SPEED_MODE_ENV: &str = "QUALIA_LEASH_SPEED_MODE";
/// `QUALIA_LEASH_OPERATOR_TOKEN_FILE`, the operator bearer secret's file.
pub const OPERATOR_TOKEN_FILE_ENV: &str = "QUALIA_LEASH_OPERATOR_TOKEN_FILE";
/// Command expiry when none is named.
pub const DEFAULT_DEADMAN_MS: u64 = 500;
/// Pilot lease TTL when none is named. Leash limits a bearer-authenticated
/// physical lease to 1–30 s.
pub const DEFAULT_LEASE_TTL_SECS: u64 = 20;
/// Health poll interval when none is named.
pub const DEFAULT_HEALTH_MS: u64 = 500;
/// Idle stop interval when none is named.
pub const DEFAULT_POLL_MS: u64 = 100;
/// Idle intervals between summary lines once the first has been logged.
pub const DEFAULT_LOG_EVERY: u64 = 50;
/// The speed mode this transport asks for when none is named.
pub const DEFAULT_SPEED_MODE: &str = "low";
/// Characters of the leash's acknowledgement a log line carries.
const ACK_PREVIEW: usize = 200;

/// Everything the process learns from the environment before the loop starts.
#[derive(Debug, Clone)]
struct Settings {
    base_url: String,
    shm_name: String,
    route: Route,
    poll: Duration,
    deadman: Duration,
    health_every: Duration,
    ttl: Duration,
    log_every: u64,
    timeout: Duration,
    speed_mode: String,
    token_file: PathBuf,
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
        let route = Route::parse(&text(ROUTE_ENV, "http"))?;
        let speed_mode = text(SPEED_MODE_ENV, DEFAULT_SPEED_MODE);
        if !matches!(speed_mode.as_str(), "low" | "medium" | "high") {
            return Err(format!(
                "{SPEED_MODE_ENV} must be low, medium or high, not '{speed_mode}'"
            ));
        }
        let token_file = std::env::var_os(OPERATOR_TOKEN_FILE_ENV)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| format!("{OPERATOR_TOKEN_FILE_ENV} is not set"))?;
        Ok(Self {
            base_url: text(BASE_URL_ENV, DEFAULT_BASE_URL),
            shm_name: text(SHM_NAME_ENV, DEFAULT_SHM_NAME),
            route,
            poll: Duration::from_millis(number(POLL_MS_ENV, DEFAULT_POLL_MS).max(1)),
            deadman: Duration::from_millis(number(DEADMAN_MS_ENV, DEFAULT_DEADMAN_MS).max(1)),
            health_every: Duration::from_millis(number(HEALTH_MS_ENV, DEFAULT_HEALTH_MS).max(1)),
            ttl: Duration::from_secs(number(LEASE_TTL_SECS_ENV, DEFAULT_LEASE_TTL_SECS).max(1)),
            log_every: number(LOG_EVERY_ENV, DEFAULT_LOG_EVERY).max(1),
            timeout: Duration::from_millis(number(TIMEOUT_MS_ENV, DEFAULT_TIMEOUT_MS).max(1)),
            speed_mode,
            token_file,
        })
    }
}

/// Which of the leash's two control interfaces this transport speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The leash's canonical operator HTTP surface.
    Http,
    /// The leash's MCP `tools/call` surface.
    Mcp,
}

impl Route {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "http" => Ok(Self::Http),
            "mcp" => Ok(Self::Mcp),
            other => Err(format!("{ROUTE_ENV} must be http or mcp, not '{other}'")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Mcp => "mcp",
        }
    }
}

/// One wheel command, in the frame `runners/drive` writes to the wire.
#[derive(Debug, Clone, Copy, Deserialize)]
struct WheelFrame {
    /// The producer's tick, carried for the log only.
    #[serde(rename = "T", default)]
    tick: u64,
    /// The left wheel speed.
    #[serde(rename = "L")]
    left: f32,
    /// The right wheel speed.
    #[serde(rename = "R")]
    right: f32,
}

/// The leash's own report of one applied command or one verified stop.
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
    /// The leash's own refusal, when it declined the command.
    #[serde(default)]
    error: Option<String>,
    /// The verified-zero evidence's `acknowledged`, on the HTTP stop route.
    #[serde(default)]
    acknowledged: bool,
    /// The verified-zero evidence's own sentence, on the HTTP stop route.
    #[serde(default)]
    statement: String,
}

impl DriveOutcome {
    /// Whether the leash took what was sent: its `ok`, or a verified zero's
    /// own acknowledgement.
    fn accepted(&self) -> bool {
        self.ok || self.acknowledged
    }

    /// The leash's flags, in the leash's own vocabulary.
    fn safety_flags(&self) -> u32 {
        u32::from(self.soft_odometry_limited) * LEASH_ACTION_SAFETY_SOFT_ODOMETRY_LIMIT
            | u32::from(self.stopped_by_deadman) * LEASH_ACTION_SAFETY_DEADMAN
            | u32::from(!self.statement.is_empty()) * LEASH_ACTION_SAFETY_VERIFIED_ZERO
    }

    /// The leash's own words for this receipt, for the log.
    fn detail(&self) -> String {
        match self.error.as_deref() {
            Some(error) => error.to_owned(),
            None if !self.statement.is_empty() => self.statement.clone(),
            None => preview(
                &serde_json::json!({
                    "ok": self.ok,
                    "left": self.left,
                    "right": self.right,
                    "max_speed": self.max_speed,
                    "speed_mode": self.speed_mode,
                })
                .to_string(),
            ),
        }
    }
}

/// The leash's own safety state, as `health` reports it.
#[derive(Debug, Clone, Deserialize, Default)]
struct Health {
    /// `live`, `replay`, …
    #[serde(default)]
    mode: String,
    /// The harness's own deadman state.
    #[serde(default)]
    deadman_ok: bool,
    /// The latching emergency stop.
    #[serde(default)]
    estop: bool,
}

impl Health {
    /// Whether the harness is live, its deadman is satisfied and it is not
    /// latched in an emergency stop.
    fn armed(&self) -> bool {
        self.mode == "live" && self.deadman_ok && !self.estop
    }
}

/// The transport's client of the leash's control surface.
struct Leash {
    agent: ureq::Agent,
    base_url: String,
    route: Route,
    /// The operator's bearer secret. Never logged.
    bearer: String,
    /// The short pilot session token this process registers. Never logged.
    session: String,
    /// This process's session label: an eight-hex digest derived once, in
    /// [`Leash::new`], so every line of the run's log carries the same one and a
    /// session can be followed through the log. Derived from the session token,
    /// so it names the session without being it.
    label: String,
    speed_mode: String,
    ttl: Duration,
    /// When the current lease must be refreshed.
    refresh_at: Instant,
}

impl Leash {
    /// Reads the operator's bearer secret and mints this process's session
    /// token. The session token is derived from the secret and a per-process
    /// nonce, so it cannot be the secret leash refuses to see reused as a
    /// session token, and it is not reversible to the secret.
    fn new(
        agent: ureq::Agent,
        base_url: String,
        route: Route,
        speed_mode: String,
        ttl: Duration,
        token_file: &std::path::Path,
    ) -> Result<Self, String> {
        let bearer = std::fs::read_to_string(token_file)
            .map_err(|error| format!("read {}: {error}", token_file.display()))?
            .trim()
            .to_owned();
        if bearer.is_empty() {
            return Err(format!("{} is empty", token_file.display()));
        }
        let session = mint_session_token(&bearer);
        let label = digest(&session)[..8].to_owned();
        Ok(Self {
            agent,
            base_url,
            route,
            label,
            session,
            bearer,
            speed_mode,
            ttl,
            refresh_at: Instant::now(),
        })
    }

    /// This process's session label, the same on every line it logs.
    fn session_label(&self) -> &str {
        &self.label
    }

    /// Registers or refreshes the pilot lease when it is due.
    fn ensure_lease(&mut self) -> Result<(), String> {
        if Instant::now() < self.refresh_at {
            return Ok(());
        }
        match self.route {
            Route::Http => {
                let body = serde_json::json!({
                    "token": self.session,
                    "ttl_secs": self.ttl.as_secs(),
                    "speed_mode": self.speed_mode,
                });
                self.post_json("/pilot/authorize", &body)?;
            }
            Route::Mcp => {
                let arguments = serde_json::json!({
                    "capability": "authorize",
                    "token": self.session,
                    "ttl_secs": self.ttl.as_secs(),
                    "speed_mode": self.speed_mode,
                });
                call_with(&self.agent, &self.base_url, "invoke_capability", &arguments)?;
            }
        }
        // Refresh at half the lease, so a slow round trip still lands inside it.
        self.refresh_at = Instant::now() + self.ttl / 2;
        Ok(())
    }

    /// Applies one wheel command through the leash's `drive`.
    fn drive(&mut self, left: f32, right: f32) -> Result<DriveOutcome, String> {
        self.ensure_lease()?;
        let body = serde_json::json!({
            "token": self.session,
            "left": left,
            "right": right,
            "speed_mode": self.speed_mode,
            "approval": true,
        });
        let text = match self.route {
            Route::Http => self.post_json("/motors/drive", &body)?,
            Route::Mcp => {
                let mut arguments = body;
                arguments["capability"] = serde_json::Value::from("drive");
                call_with(&self.agent, &self.base_url, "invoke_capability", &arguments)?
            }
        };
        let outcome: DriveOutcome = serde_json::from_str(&text)
            .map_err(|error| format!("decode drive payload '{text}': {error}"))?;
        if outcome.accepted() {
            Ok(outcome)
        } else {
            Err(outcome.detail())
        }
    }

    /// Sends the leash's zero-speed stop.
    ///
    /// A verified stop ends the pilot's authority on this harness — measured:
    /// after `POST /motors/stop/verified` the next `drive` with the same token
    /// is refused `{"error":"invalid pilot token","ok":false}`, and a fresh
    /// `authorize` makes the following `drive` carry again. The lease is
    /// therefore dropped here, so the command after a zero registers a new one
    /// rather than failing with a refusal this runner could have avoided.
    fn zero(&mut self) -> Result<DriveOutcome, String> {
        let text = match self.route {
            Route::Http => self.post_json(
                "/motors/stop/verified",
                &serde_json::json!({ "reason": "operator-request" }),
            )?,
            Route::Mcp => call(&self.agent, &self.base_url, "stop")?,
        };
        let outcome: DriveOutcome = serde_json::from_str(&text)
            .map_err(|error| format!("decode stop payload '{text}': {error}"))?;
        if outcome.accepted() {
            self.refresh_at = Instant::now();
            Ok(outcome)
        } else {
            Err(outcome.detail())
        }
    }

    /// The leash's own safety state.
    fn health(&self) -> Result<Health, String> {
        let text = match self.route {
            Route::Http => self.get_json("/health")?,
            Route::Mcp => call(&self.agent, &self.base_url, "health")?,
        };
        serde_json::from_str(&text)
            .map_err(|error| format!("decode health payload '{text}': {error}"))
    }

    /// One authenticated JSON request, with the leash's own refusal quoted.
    fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<String, String> {
        let endpoint = format!("{}{path}", self.base_url.trim_end_matches('/'));
        let payload =
            serde_json::to_string(body).map_err(|error| format!("encode {path}: {error}"))?;
        let request = self
            .agent
            .post(&endpoint)
            .set("content-type", "application/json")
            .set("authorization", &format!("Bearer {}", self.bearer));
        match request.send_string(&payload) {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("read {endpoint} reply: {error}")),
            Err(ureq::Error::Status(code, response)) => {
                let text = response.into_string().unwrap_or_default();
                Err(format!(
                    "POST {endpoint} refused HTTP {code}: {}",
                    preview(&text)
                ))
            }
            Err(error) => Err(format!("POST {endpoint}: {error}")),
        }
    }

    /// One authenticated GET, with the leash's own refusal quoted.
    fn get_json(&self, path: &str) -> Result<String, String> {
        let endpoint = format!("{}{path}", self.base_url.trim_end_matches('/'));
        let request = self
            .agent
            .get(&endpoint)
            .set("authorization", &format!("Bearer {}", self.bearer));
        match request.call() {
            Ok(response) => response
                .into_string()
                .map_err(|error| format!("read {endpoint} reply: {error}")),
            Err(ureq::Error::Status(code, response)) => {
                let text = response.into_string().unwrap_or_default();
                Err(format!(
                    "GET {endpoint} refused HTTP {code}: {}",
                    preview(&text)
                ))
            }
            Err(error) => Err(format!("GET {endpoint}: {error}")),
        }
    }
}

/// The intervals this runner publishes into the arena, tiled so each starts
/// where the previous acknowledgement ended.
struct Ledger<'a> {
    history: &'a AppliedActionHistory,
    producer_epoch: u64,
    sequence: u64,
    last_end_ns: u64,
}

impl Ledger<'_> {
    /// Publishes one interval for what the leash just acknowledged.
    #[allow(clippy::too_many_arguments)]
    fn publish(
        &mut self,
        requested_left: f32,
        requested_right: f32,
        outcome: &DriveOutcome,
        armed: bool,
        estop: bool,
    ) -> bool {
        let end_ns = now_ns();
        if end_ns <= self.last_end_ns {
            return false;
        }
        let next = self.sequence + 1;
        let snapshot = AppliedActionSnapshot {
            producer_epoch: self.producer_epoch,
            action_sequence: next,
            interval_start_ns: self.last_end_ns,
            interval_end_ns: end_ns,
            requested_left,
            requested_right,
            clamped_left: outcome.left,
            clamped_right: outcome.right,
            applied_left: outcome.left,
            applied_right: outcome.right,
            // This transport applies no scale of its own; the leash's own
            // ceiling for the chosen mode is in its reply (`max_speed`) and in
            // its ledger.
            speed_scale: 1.0,
            safety_flags: outcome.safety_flags() | u32::from(estop) * LEASH_ACTION_SAFETY_ESTOP,
            authority: ACTION_AUTHORITY_LEASH,
            valid: outcome.accepted(),
            armed,
            deadman_active: armed,
            collision_clamped: false,
            ..AppliedActionSnapshot::default()
        };
        match self.history.append(snapshot) {
            Ok(_) => {
                self.sequence = next;
                self.last_end_ns = end_ns;
                true
            }
            Err(error) => {
                eprintln!("qualia-leash-transport: applied-action append failed at {next}: {error}");
                false
            }
        }
    }
}

/// The counters one operator line carries.
#[derive(Debug, Default)]
struct Counters {
    accepted: u64,
    refused: u64,
    failures: u64,
    dropped: u64,
    idle: u64,
    reported_stop: bool,
}

/// The transport's loop state.
struct Transport<'a> {
    settings: Settings,
    leash: Leash,
    ledger: Ledger<'a>,
    counters: Counters,
    /// The leash's own safety state, as the harness last reported it.
    health: Health,
    /// A stop this transport has sent but the leash has not acknowledged. While
    /// held, no command is applied: an unacknowledged zero is not a zero.
    hold: bool,
    /// When the last accepted command reached the leash; zero while no command
    /// is standing and there is no expiry to enforce.
    last_command_ns: u64,
    last_health: Instant,
}

impl Transport<'_> {
    /// One wheel command, applied or refused without ever leaving a gap in the
    /// record. `arrived_ns` is when the frame reached this process, which is
    /// what the deadman's expiry is measured from.
    fn command(&mut self, frame: WheelFrame, arrived_ns: u64) {
        if !self.health.armed() || self.hold {
            // Only a live, armed harness receives commands. Nothing of ours
            // stands either way: a leash in `replay` can answer `valid: true,
            // armed: false` about motion that did not happen, and under a hold
            // the last stop was not acknowledged.
            self.last_command_ns = 0;
            self.counters.dropped = self.counters.dropped.saturating_add(1);
            if self.counters.dropped == 1 || self.counters.dropped % self.settings.log_every == 0 {
                println!(
                    "qualia-leash-transport: leash mode={} deadman_ok={} estop={} hold={}; refused T={} left={:.3} right={:.3} (dropped={})",
                    self.health.mode,
                    self.health.deadman_ok,
                    self.health.estop,
                    self.hold,
                    frame.tick,
                    frame.left,
                    frame.right,
                    self.counters.dropped
                );
            }
            return;
        }
        let outcome = self.leash.drive(frame.left, frame.right);
        match outcome {
            Ok(outcome) => {
                self.counters.failures = 0;
                self.counters.accepted = self.counters.accepted.saturating_add(1);
                // Every applied command carries the deadman's expiry, zero
                // commands included: what the leash accepted is what the expiry
                // promises to take back down. The clock starts at the frame's
                // arrival, so the expiry is the producer's cadence, not this
                // round trip plus the cadence.
                self.last_command_ns = arrived_ns;
                println!(
                    "qualia-leash-transport: applied T={} requested_left={:.3} requested_right={:.3} applied_left={:.3} applied_right={:.3} max_speed={:.3} speed_mode={} flags=0x{:x} ok={} session={}…",
                    frame.tick,
                    frame.left,
                    frame.right,
                    outcome.left,
                    outcome.right,
                    outcome.max_speed,
                    outcome.speed_mode,
                    outcome.safety_flags(),
                    outcome.ok,
                    self.leash.session_label(),
                );
                let armed = self.health.armed();
                let estop = self.health.estop;
                self.ledger
                    .publish(frame.left, frame.right, &outcome, armed, estop);
            }
            Err(error) => {
                // A refused or unreachable drive is a stop, not a coast.
                self.counters.failures = self.counters.failures.saturating_add(1);
                self.last_command_ns = 0;
                eprintln!(
                    "qualia-leash-transport: drive refused (T={} left={:.3} right={:.3}) session={}…: {error}; sending zero",
                    frame.tick,
                    frame.left,
                    frame.right,
                    self.leash.session_label()
                );
                if error.to_ascii_lowercase().contains("estop") {
                    self.health.estop = true;
                    eprintln!(
                        "qualia-leash-transport: the refusal names the estop; refusing to drive until leash reports estop=false"
                    );
                }
                self.stop("after a refused drive");
            }
        }
    }

    /// Applies a zero-speed stop and publishes the leash's own receipt. A stop
    /// the leash does not acknowledge sets the hold: the transport keeps asking
    /// for zero and applies no command until one is acknowledged.
    fn stop(&mut self, because: &str) {
        match self.leash.zero() {
            Ok(outcome) => {
                self.counters.failures = 0;
                if self.hold {
                    self.hold = false;
                    println!("qualia-leash-transport: the stop is acknowledged again; holding released");
                }
                if !self.counters.reported_stop {
                    self.counters.reported_stop = true;
                    println!(
                        "qualia-leash-transport: leash acknowledged the stop: {}",
                        outcome.detail()
                    );
                }
                if outcome.accepted() {
                    self.counters.accepted = self.counters.accepted.saturating_add(1);
                } else {
                    self.counters.refused = self.counters.refused.saturating_add(1);
                }
                let armed = self.health.armed();
                let estop = self.health.estop;
                let published = self.ledger.publish(0.0, 0.0, &outcome, armed, estop);
                if !published && self.counters.idle == 0 {
                    eprintln!(
                        "qualia-leash-transport: stop after {because}: nothing appended (clock did not advance)"
                    );
                }
            }
            Err(error) => {
                self.hold = true;
                self.counters.failures = self.counters.failures.saturating_add(1);
                if self.counters.failures == 1 || self.counters.failures % 20 == 0 {
                    eprintln!(
                        "qualia-leash-transport: stop after {because} unavailable (failure {}): {error}; holding, no command will be applied",
                        self.counters.failures
                    );
                }
            }
        }
    }

    /// Polls the leash's own safety state at most every `health_every`.
    fn poll_health(&mut self) {
        if self.last_health.elapsed() < self.settings.health_every {
            return;
        }
        self.last_health = Instant::now();
        match self.leash.health() {
            Ok(health) => {
                if health.estop != self.health.estop {
                    if health.estop {
                        println!(
                            "qualia-leash-transport: leash reports estop latched; refusing to drive until it reports estop=false (dropped {} command(s) so far)",
                            self.counters.dropped
                        );
                    } else {
                        println!(
                            "qualia-leash-transport: leash reports estop=false; accepting commands again"
                        );
                    }
                }
                self.health = health;
            }
            Err(error) => eprintln!("qualia-leash-transport: health unavailable: {error}"),
        }
    }

    /// How long to wait for the next command: the deadman's remainder while a
    /// command stands, the idle stop interval when none does.
    fn wait(&self) -> Duration {
        match self.last_command_ns {
            0 => self.settings.poll,
            start => self
                .settings
                .deadman
                .saturating_sub(Duration::from_nanos(now_ns().saturating_sub(start)))
                .max(Duration::from_millis(1)),
        }
    }

    fn run(&mut self, commands: &mpsc::Receiver<(u64, Result<WheelFrame, String>)>) {
        loop {
            self.poll_health();
            match commands.recv_timeout(self.wait()) {
                Ok((_, Err(error))) => {
                    // A frame the parser could not read is not a command: stop.
                    self.last_command_ns = 0;
                    self.counters.failures = self.counters.failures.saturating_add(1);
                    eprintln!(
                        "qualia-leash-transport: unreadable command frame ({error}); sending zero and refusing to coast"
                    );
                    self.stop("an unreadable frame");
                }
                Ok((arrived_ns, Ok(frame))) => self.command(frame, arrived_ns),
                Err(mpsc::RecvTimeoutError::Timeout) if self.last_command_ns != 0 => {
                    let age_ms = now_ns().saturating_sub(self.last_command_ns) / 1_000_000;
                    println!(
                        "qualia-leash-transport: deadman expired after {age_ms}ms without a command (expiry {}ms); sending zero speed",
                        self.settings.deadman.as_millis()
                    );
                    self.last_command_ns = 0;
                    self.stop("the deadman");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // The quiet stream: the same non-latching zero this runner
                    // has always published, and the interval the dataset leg
                    // reads.
                    self.counters.idle = self.counters.idle.saturating_add(1);
                    self.stop("the quiet stream");
                    if self.counters.idle == 1
                        || self.counters.idle % self.settings.log_every == 0
                    {
                        println!(
                            "qualia-leash-transport: idle {} stop intervals; estop={} accepted={} refused={} failures={}",
                            self.counters.idle,
                            self.health.estop,
                            self.counters.accepted,
                            self.counters.refused,
                            self.counters.failures,
                        );
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // The producer is gone. The deadman's rule applies to the end
                    // of a stream as much as to a gap in it.
                    if self.last_command_ns != 0 {
                        self.last_command_ns = 0;
                        println!("qualia-leash-transport: command stream ended; sending zero speed");
                    }
                    self.stop("the end of the stream");
                    thread::sleep(self.settings.poll);
                }
            }
        }
    }
}

fn main() {
    let settings = match Settings::from_env() {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("qualia-leash-transport: {error}");
            std::process::exit(2);
        }
    };

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

    let leash = match Leash::new(
        agent(settings.timeout),
        settings.base_url.clone(),
        settings.route,
        settings.speed_mode.clone(),
        settings.ttl,
        &settings.token_file,
    ) {
        Ok(leash) => leash,
        Err(error) => {
            eprintln!("qualia-leash-transport: {error}");
            std::process::exit(2);
        }
    };

    println!(
        "qualia-leash-transport: carrying our wheel commands to {} over {}; deadman={}ms speed_mode={} lease_ttl={}s session={}… token_file={} authority=leash arena={}",
        settings.base_url,
        settings.route.as_str(),
        settings.deadman.as_millis(),
        settings.speed_mode,
        settings.ttl.as_secs(),
        leash.session_label(),
        settings.token_file.display(),
        settings.shm_name,
    );

    // The harness's own state at startup, so the operator line carries leash's
    // claim rather than this runner's.
    let health = match leash.health() {
        Ok(health) => {
            println!(
                "qualia-leash-transport: leash health mode={} deadman_ok={} estop={} -> armed={}",
                health.mode,
                health.deadman_ok,
                health.estop,
                health.armed()
            );
            health
        }
        Err(error) => {
            eprintln!("qualia-leash-transport: leash health unavailable: {error}");
            Health::default()
        }
    };
    if health.estop {
        println!(
            "qualia-leash-transport: leash reports estop latched; refusing to drive until it reports estop=false"
        );
    }

    let ledger = Ledger {
        history: region.applied_action_history(),
        producer_epoch: now_ns(),
        sequence: 0,
        last_end_ns: now_ns(),
    };
    let mut transport = Transport {
        settings,
        leash,
        ledger,
        counters: Counters::default(),
        health,
        hold: false,
        last_command_ns: 0,
        last_health: Instant::now(),
    };
    transport.run(&spawn_command_reader());
}

/// Reads the command stream from stdin on its own thread and timestamps each
/// frame when it arrives, so the deadman measures the gap between commands
/// rather than the gap between calls to the leash.
fn spawn_command_reader() -> mpsc::Receiver<(u64, Result<WheelFrame, String>)> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let at = now_ns();
            let parsed = match line {
                Ok(text) => {
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    serde_json::from_str::<WheelFrame>(text)
                        .map_err(|error| format!("'{text}': {error}"))
                }
                Err(error) => Err(format!("stdin: {error}")),
            };
            if sender.send((at, parsed)).is_err() {
                return;
            }
        }
    });
    receiver
}

/// A digest of one string with a fixed seed, so the same input gives the same
/// digest on every call in this process. The log's session label needs one value
/// for the whole run: a label that changed per line could not be used to follow
/// a session.
fn digest(text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(text.as_bytes());
    format!("{:016x}", hasher.finish())
}

/// A per-process session token that is not the operator's bearer secret: the
/// secret is hashed with a fresh OS-seeded state and the process's own clock,
/// and leash is shown only the hash. The secret itself is never registered as a
/// session token, which leash refuses, and the direction back is not
/// recoverable.
fn mint_session_token(secret: &str) -> String {
    let mut text = String::with_capacity(64);
    for domain in [
        "qualia-leash-transport-session-v1",
        "qualia-leash-transport-session-v2",
    ] {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write(domain.as_bytes());
        hasher.write(secret.as_bytes());
        hasher.write_u64(now_ns());
        hasher.write_u32(std::process::id());
        text.push_str(&format!("{:016x}", hasher.finish()));
    }
    text
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
