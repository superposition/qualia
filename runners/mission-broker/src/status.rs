//! The status surface: the operator's console reads the coach's state and its
//! newest decisions from one read-only loopback JSON endpoint.
//!
//! `GET /coach` returns `qualia.coach-state.v1`. It is a plain `std::net`
//! listener — one thread, one request per connection, `Connection: close` — so
//! this crate carries no async runtime for a three-field read. The bind is
//! loopback or it does not happen: the broker refuses any other address rather
//! than exposing the surface off-host.
//!
//! This is a *second* source beside the agent: the agent owns missions, and
//! when it grows a coach/decision stream of its own the console should read
//! that instead of this endpoint (see the crate's README).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::redact;
use crate::{now_ms, COACH_STATE_SCHEMA};

/// How many decisions the surface keeps.
pub const MAX_DECISIONS: usize = 32;
/// How much of a request line is read before the surface answers.
const REQUEST_BYTES: usize = 8 * 1024;

/// The model's state, as the panel's header shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelState {
    pub configured: bool,
    pub model_id: String,
    pub base_url: String,
    /// Presence and provenance of the credential — `<present via
    /// DEEPSEEK_API_KEY>`, or absent. Never a character of the key.
    pub key_presence: Option<String>,
    /// `ok` | `no_key` | `timeout` | `error`.
    pub status: String,
    pub reason: Option<String>,
    pub last_latency_ms: Option<u64>,
    pub last_error: Option<String>,
}

/// One decision, from the model's answer to the envelope it justified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRow {
    pub decision_id: String,
    pub decision_kind: String,
    pub target_proposal_ids: Vec<String>,
    pub output_ids: Vec<String>,
    pub reason: Option<String>,
    pub llm_priors_ablated: bool,
    pub model_id: Option<String>,
    pub prompt_digest: Option<String>,
    pub response_id: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub usage_reported: bool,
    pub latency_ms: Option<u64>,
    pub decided_at_ms: u64,
    /// What the broker did with it: `posted`, `recorded`, or a refusal reason.
    pub disposition: String,
}

/// One mission delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionRow {
    pub mission_id: String,
    pub idempotency_key: String,
    pub command: String,
    pub accepted: bool,
    pub ack_http_status: Option<u16>,
    pub detail: String,
}

/// The whole payload the console reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachState {
    pub schema_version: String,
    pub observed_at_ms: u64,
    pub model: ModelState,
    pub decisions: Vec<DecisionRow>,
    pub missions: Vec<MissionRow>,
    /// One named line per degradation, newest last.
    pub degradations: Vec<String>,
}

/// The shared state the broker writes and the surface serves.
#[derive(Clone)]
pub struct StatusHandle {
    inner: Arc<Mutex<CoachState>>,
}

impl StatusHandle {
    /// Open the surface with the model's state.
    pub fn new(model: ModelState) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CoachState {
                schema_version: COACH_STATE_SCHEMA.to_string(),
                observed_at_ms: now_ms() as u64,
                model,
                decisions: Vec::new(),
                missions: Vec::new(),
                degradations: Vec::new(),
            })),
        }
    }

    /// The current payload.
    pub fn snapshot(&self) -> CoachState {
        self.inner.lock().expect("coach state lock").clone()
    }

    /// Update the model's state.
    pub fn set_model(&self, change: impl FnOnce(&mut ModelState)) {
        let mut state = self.inner.lock().expect("coach state lock");
        change(&mut state.model);
        state.observed_at_ms = now_ms() as u64;
    }

    /// Record one decision, newest first.
    pub fn push_decision(&self, row: DecisionRow) {
        let mut state = self.inner.lock().expect("coach state lock");
        state.decisions.insert(0, row);
        state.decisions.truncate(MAX_DECISIONS);
        state.observed_at_ms = now_ms() as u64;
    }

    /// Record one mission delivery, newest first.
    pub fn push_mission(&self, row: MissionRow) {
        let mut state = self.inner.lock().expect("coach state lock");
        state.missions.insert(0, row);
        state.missions.truncate(MAX_DECISIONS);
        state.observed_at_ms = now_ms() as u64;
    }

    /// Record one named degradation line.
    pub fn push_degradation(&self, line: String) {
        let mut state = self.inner.lock().expect("coach state lock");
        state.degradations.push(redact::secrets(&line));
        state.degradations.truncate(MAX_DECISIONS);
        state.observed_at_ms = now_ms() as u64;
    }

    /// Stamp the payload's observation time.
    pub fn observe(&self) {
        self.inner.lock().expect("coach state lock").observed_at_ms = now_ms() as u64;
    }

    /// Serve `GET /coach` on a loopback address, on its own thread.
    ///
    /// Returns an error for a non-loopback host or a bind that fails; the
    /// caller reports it and keeps working, because a status surface is not
    /// what the broker's decisions depend on.
    pub fn serve(&self, host: &str, port: u16) -> Result<(), String> {
        let address: IpAddr = host
            .parse()
            .map_err(|_| format!("the coach status host {host} is not an IP address"))?;
        if !address.is_loopback() {
            return Err(format!(
                "the coach status surface binds loopback only; {host} is not loopback"
            ));
        }
        let listener = TcpListener::bind(SocketAddr::new(address, port))
            .map_err(|error| format!("bind the coach status surface to {host}:{port}: {error}"))?;
        let handle = self.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let mut request = [0_u8; REQUEST_BYTES];
                let read = stream.read(&mut request).unwrap_or(0);
                let head = String::from_utf8_lossy(&request[..read]);
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, body) = match path {
                    "/coach" => (
                        "200 OK",
                        serde_json::to_string(&handle.snapshot()).unwrap_or_else(|_| "{}".to_string()),
                    ),
                    "/health" => ("200 OK", r#"{"status":"ok"}"#.to_string()),
                    _ => (
                        "404 Not Found",
                        r#"{"error":"the coach status surface serves GET /coach"}"#.to_string(),
                    ),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Ok(())
    }
}
