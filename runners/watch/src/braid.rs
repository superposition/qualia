//! The Mission panel's source: the agent's `GET /braid`, and the lines the
//! Mission panel and the Braid line draw from it.
//!
//! The watch binary reads the engine's ABI straight out of shared memory; the
//! braid view is the one operator value that lives in another process, so it is
//! fetched over HTTP. The transport has the retired ops dashboard's shape
//! (`docs/frontend-lessons.md`, source 2): a blocking client on a worker thread
//! and a status the panel can draw when the agent is not there, so a slow or
//! dead agent degrades a line instead of stalling the render loop.
//!
//! The module holds no terminal code: [`Mission::lines`] is the whole panel, so
//! the panel is asserted without a TTY.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::view::format_clock;

/// The schema literal the agent's `GET /braid` carries.
pub const BRAID_STATE_SCHEMA: &str = "qualia.braid-state.v1";

/// The agent's base URL, when `QUALIA_AGENT_URL` names no other one. The agent
/// serves its web surface on `QUALIA_WEB_PORT`, default 8080.
pub const DEFAULT_AGENT_URL: &str = "http://127.0.0.1:8080";

/// The establish budget, from the ops dashboard: 700 ms to connect, 1200 ms
/// for the whole exchange.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(1200);

/// How often the worker asks for the braid view — the dashboard's poll and
/// retry interval, and slower than the 50 ms render loop it feeds.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The braid's view of a running stack: `GET /braid`'s body, field for field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BraidState {
    /// `qualia.braid-state.v1`.
    pub schema_version: String,
    /// The generation the promotion gate last accepted.
    pub generation: u64,
    /// The session this braid belongs to.
    pub session_id: String,
    /// Missions the broker has opened and not closed.
    pub open_missions: u32,
    /// When the generation pointer last moved forward.
    pub last_promotion_ns: u64,
    /// When recovery last quarantined a partial; absent when it never has.
    pub last_quarantine_ns: Option<u64>,
}

/// The agent's last answer, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionStatus {
    /// No answer yet; the first request is in flight.
    Pending,
    /// The agent answered.
    State(BraidState),
    /// The agent did not answer; `reason` is a short label.
    Unreachable(String),
}

/// The Mission panel's state: the agent it asks and the last answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission {
    url: String,
    status: MissionStatus,
}

impl Mission {
    /// The panel before the first answer.
    pub fn pending(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            status: MissionStatus::Pending,
        }
    }

    /// The last answer.
    pub fn status(&self) -> &MissionStatus {
        &self.status
    }

    /// Record a newer answer from the poller.
    pub fn set_status(&mut self, status: MissionStatus) {
        self.status = status;
    }

    /// Whether the panel is drawing a missing agent.
    pub fn degraded(&self) -> bool {
        matches!(self.status, MissionStatus::Unreachable(_))
    }

    /// The panel's rows, top to bottom.
    pub fn lines(&self) -> Vec<(String, String)> {
        let mut rows = vec![("agent".to_string(), self.url.clone())];
        match &self.status {
            MissionStatus::Pending => rows.push(("state".to_string(), "contacting".to_string())),
            MissionStatus::Unreachable(reason) => {
                rows.push(("state".to_string(), format!("unreachable: {reason}")));
            }
            MissionStatus::State(braid) => {
                rows.push(("session".to_string(), braid.session_id.clone()));
                rows.push(("generation".to_string(), braid.generation.to_string()));
                rows.push(("missions".to_string(), mission_row(braid.open_missions)));
                rows.push((
                    "promotion".to_string(),
                    format_clock(braid.last_promotion_ns),
                ));
                rows.push((
                    "quarantine".to_string(),
                    match braid.last_quarantine_ns {
                        Some(ns) => format_clock(ns),
                        None => "never".to_string(),
                    },
                ));
                rows.push(("schema".to_string(), braid.schema_version.clone()));
            }
        }
        rows
    }
}

/// The Braid line: the generation the agent last reported, then the braid's
/// clock — how far the newest belief commit trails the newest finished ledger
/// row, in whole milliseconds.
///
/// The agent itself is named in the Mission panel; the line names the
/// generation so the chrome says which stack is being watched even when
/// another panel is showing (`docs/frontend-lessons.md`, source 2).
pub fn braid_line(status: &MissionStatus, lag_ns: u64) -> String {
    let generation = match status {
        MissionStatus::State(braid) => format!("gen {}", braid.generation),
        MissionStatus::Pending | MissionStatus::Unreachable(_) => "gen unknown".to_string(),
    };
    format!("braid {generation} · belief lag: {} ms", lag_ns / 1_000_000)
}

/// The mission row. An empty broker is spelled out rather than drawn as a bare
/// zero, so "no mission" and "one mission" cannot be misread at a glance.
fn mission_row(open_missions: u32) -> String {
    match open_missions {
        0 => "none open".to_string(),
        1 => "1 open".to_string(),
        count => format!("{count} open"),
    }
}

/// The worker that polls `GET /braid` off the render thread.
///
/// One channel carries answers back; the render loop drains it. Dropping the
/// poller stops the worker and waits for the request in flight, whose budget is
/// [`REQUEST_TIMEOUT`].
pub struct BraidPoller {
    answers: Receiver<MissionStatus>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl BraidPoller {
    /// Start polling `url`. The first answer lands within one request budget.
    pub fn spawn(url: impl Into<String>) -> Self {
        let url = url.into();
        let (answers, latest) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker = thread::spawn(move || {
            let client = match build_client() {
                Ok(client) => client,
                Err(reason) => {
                    let _ = answers.send(MissionStatus::Unreachable(reason));
                    return;
                }
            };
            while worker_running.load(Ordering::Relaxed) {
                if answers.send(fetch(&client, &url)).is_err() {
                    return;
                }
                thread::sleep(POLL_INTERVAL);
            }
        });
        Self {
            answers: latest,
            running,
            worker: Some(worker),
        }
    }

    /// Take the newest answer, if one arrived since the last call.
    pub fn try_take(&self) -> Option<MissionStatus> {
        let mut newest = None;
        while let Ok(status) = self.answers.try_recv() {
            newest = Some(status);
        }
        newest
    }
}

impl Drop for BraidPoller {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn build_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        // The agent serves rustls with a certificate it self-signs for
        // `localhost`, `127.0.0.1` and its own address (`runners/agent`), so a
        // client that has not been handed that keypair cannot verify it. The
        // panel is the local operator's window onto the local agent — the
        // position the ops dashboard took — and the address stays
        // `QUALIA_AGENT_URL`; a trusted certificate is what would let this
        // line go.
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|error| classify(&error))
}

fn fetch(client: &reqwest::blocking::Client, url: &str) -> MissionStatus {
    let response = match client.get(format!("{url}/braid")).send() {
        Ok(response) => response,
        Err(error) => return MissionStatus::Unreachable(classify(&error)),
    };
    let response = match response.error_for_status() {
        Ok(response) => response,
        Err(error) => return MissionStatus::Unreachable(classify(&error)),
    };
    match response.json::<BraidState>() {
        Ok(braid) => MissionStatus::State(braid),
        Err(error) => MissionStatus::Unreachable(classify(&error)),
    }
}

/// Short labels an operator can act on, from the ops dashboard's classifier.
fn classify(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "tcp timeout".to_owned()
    } else if error.is_connect() {
        "tcp connect failed".to_owned()
    } else if error.is_decode() {
        format!("response did not decode: {error}")
    } else {
        error.to_string()
    }
}
