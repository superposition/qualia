//! The one client module: `GET /braid` and the committed fixture behind the
//! same interface.
//!
//! Traceability (`docs/frontend-lessons.md`):
//!
//! - sources 2 and 4 — `QUALIA_AGENT_URL` is the only address in the source
//!   (default `http://127.0.0.1:8080`), there is no subnet autodiscovery and no
//!   TLS-insecure default; the client is a blocking `reqwest` client with the
//!   700 ms connect / 1200 ms total budget source 2 used, and transport
//!   failures become short human labels;
//! - source 3 — the live agent and the committed fixture are two
//!   [`BraidSource`]s, so the view never knows which one it has;
//! - source 4 — the wire types belong with the client, not the widget.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The schema literal the agent's `GET /braid` response carries.
pub const BRAID_STATE_SCHEMA: &str = "qualia.braid-state.v1";

/// The only default address in the source; every other address is config.
pub const DEFAULT_AGENT_URL: &str = "http://127.0.0.1:8080";

/// Environment key naming the directory that holds the agent's own `cert.pem`.
pub const AGENT_TLS_DIR_ENV: &str = "QUALIA_AGENT_TLS_DIR";

/// The agent's certificate directory when the deployment names none.
pub const DEFAULT_AGENT_TLS_DIR: &str = ".qualia_tls";

/// Establish budget, from the ops dashboard's 700 ms connect / 1200 ms total.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(1200);

/// The committed fixture the snapshot tests and the offline console both read.
pub const FIXTURE_JSON: &str = include_str!("../tests/fixtures/braid-state.json");

fn default_schema_version() -> String {
    BRAID_STATE_SCHEMA.to_owned()
}

/// The braid state the agent publishes, field for field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BraidState {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub generation: u64,
    pub session_id: String,
    pub open_missions: u32,
    pub last_promotion_ns: u64,
    #[serde(default)]
    pub last_quarantine_ns: Option<u64>,
}

/// How far the observed latent sits from what the model predicted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    pub mahalanobis: f32,
    pub sample_count: u64,
}

/// The braid plus its drift, which is what every console panel draws from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BraidSnapshot {
    pub braid: BraidState,
    #[serde(default)]
    pub drift: Option<DriftReport>,
}

/// The committed fixture, decoded. Panics only if the fixture is malformed,
/// which a test would catch before any binary ships.
pub fn fixture() -> BraidSnapshot {
    serde_json::from_str(FIXTURE_JSON).expect("committed braid-state.json decodes")
}

/// The agent base URL, from the environment only.
pub fn agent_url() -> String {
    std::env::var("QUALIA_AGENT_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT_URL.to_owned())
}

/// A source of braid state: one live, one committed.
pub trait BraidSource: Send {
    fn fetch(&mut self) -> Result<BraidSnapshot, String>;
}

/// Replay the committed fixture; never touches the network.
pub struct FixtureSource {
    snapshot: BraidSnapshot,
}

impl FixtureSource {
    pub fn new(snapshot: BraidSnapshot) -> Self {
        Self { snapshot }
    }
}

impl Default for FixtureSource {
    fn default() -> Self {
        Self::new(fixture())
    }
}

impl BraidSource for FixtureSource {
    fn fetch(&mut self) -> Result<BraidSnapshot, String> {
        Ok(self.snapshot.clone())
    }
}

/// Poll the local agent's `GET /braid`.
pub struct HttpSource {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpSource {
    pub fn new(base_url: impl Into<String>) -> Result<Self, String> {
        let base_url = base_url.into();
        let mut builder = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        // The agent serves TLS with the certificate it generates on first start
        // (`QUALIA_AGENT_TLS_DIR/cert.pem`, default `$HOME/.qualia_tls`). An
        // operator can hand the console that certificate as a root; trusting a
        // named deployment certificate is not the same as disabling
        // verification, and there is still no insecure default.
        if base_url.starts_with("https://") {
            if let Some(certificate) = agent_certificate() {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let client = builder.build().map_err(|error| classify(&error))?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// The agent's own certificate PEM, when the deployment has one on disk.
///
/// The directory is `QUALIA_AGENT_TLS_DIR`, or `$HOME/.qualia_tls` — the
/// default the agent generates into. A missing or unreadable file is not an
/// error: the request then fails verification and the Mission banner names it,
/// which is the honest state for an agent whose certificate this host has not
/// been given.
pub(crate) fn agent_certificate() -> Option<reqwest::Certificate> {
    let directory = std::env::var_os(AGENT_TLS_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(DEFAULT_AGENT_TLS_DIR))
        })?;
    let pem = std::fs::read(directory.join("cert.pem")).ok()?;
    reqwest::Certificate::from_pem(&pem).ok()
}

impl BraidSource for HttpSource {
    fn fetch(&mut self) -> Result<BraidSnapshot, String> {
        let response = self
            .client
            .get(format!("{}/braid", self.base_url))
            .send()
            .map_err(|error| classify(&error))?;
        let response = response.error_for_status().map_err(|error| classify(&error))?;
        let body: BraidBody = response.json().map_err(|error| classify(&error))?;
        Ok(body.into_snapshot())
    }
}

/// The agent answers with a bare [`BraidState`]; a snapshot envelope with a
/// `drift` member is also accepted so a richer agent needs no console change.
#[derive(Deserialize)]
#[serde(untagged)]
enum BraidBody {
    Snapshot(BraidSnapshot),
    State(BraidState),
}

impl BraidBody {
    fn into_snapshot(self) -> BraidSnapshot {
        match self {
            BraidBody::Snapshot(snapshot) => snapshot,
            BraidBody::State(braid) => BraidSnapshot { braid, drift: None },
        }
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
