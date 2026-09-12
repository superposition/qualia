//! The agent's operator surface, read and written the way the agent defines it.
//!
//! One blocking client: the braid's view (`GET /braid`), the braid's proposal
//! record (`GET /world-model/proposals`), and the mission wire
//! (`POST /mission-control/envelopes`, read back through
//! `/mission-control/missions` and `/mission-control/events`).
//!
//! `AuthScope::MissionBroker` is excluded from the loopback bypass in
//! `runners/agent/src/auth.rs:76-80` — only `Read`, `Peer` and `Compute` get it
//! — so this client always presents the bearer when a token is configured and
//! reports the refusal by name when one is not.
//!
//! The transport follows the console's and `qualia-watch`'s posture (D-023):
//! TLS with the agent's certificate added as a root, never `danger_accept_invalid_certs`.

use std::path::Path;
use std::time::Duration;

use qualia_sync_types::{MissionEnvelopeV1, ProposalEnvelope};
use serde::{Deserialize, Serialize};

use crate::redact;

/// The braid view `GET /braid` returns, `schema_version` and all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BraidView {
    #[serde(default)]
    pub schema_version: String,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub open_missions: u32,
    #[serde(default)]
    pub last_promotion_ns: u64,
    #[serde(default)]
    pub last_quarantine_ns: Option<u64>,
}

/// What the agent answered to one envelope delivery.
#[derive(Debug, Clone)]
pub struct EnvelopePost {
    pub http_status: u16,
    pub accepted: bool,
    pub idempotent_replay: bool,
    pub body: serde_json::Value,
}

/// One accepted mission, as the broker reads it back.
#[derive(Debug, Clone, Serialize)]
pub struct MissionSummary {
    pub mission_id: String,
    pub status: String,
    pub stage: String,
    pub code: String,
    pub detail: String,
}

/// One mission event, as the broker reads it back.
#[derive(Debug, Clone, Serialize)]
pub struct EventSummary {
    pub sequence: u64,
    pub mission_id: String,
    pub event_kind: String,
    pub status: String,
    pub code: String,
    pub detail: String,
}

#[derive(Deserialize)]
struct ProposalsResponse {
    proposals: Vec<ProposalEnvelope>,
}

/// A blocking client for the agent's operator surface.
pub struct AgentClient {
    http: reqwest::blocking::Client,
    base: String,
    token: Option<String>,
}

impl AgentClient {
    /// Build the client, trusting the agent's certificate when the base URL is
    /// TLS and the certificate is readable.
    pub fn new(
        base_url: &str,
        tls_dir: Option<&Path>,
        token: Option<String>,
        timeout: Duration,
    ) -> Result<Self, String> {
        let mut builder = reqwest::blocking::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout);
        if base_url.starts_with("https") {
            if let Some(dir) = tls_dir {
                let certificate = dir.join("cert.pem");
                let pem = std::fs::read(&certificate).map_err(|error| {
                    format!(
                        "the agent's certificate {} could not be read: {error} (set QUALIA_AGENT_TLS_DIR; D-023: no insecure bypass)",
                        certificate.display()
                    )
                })?;
                let pem = reqwest::Certificate::from_pem(&pem)
                    .map_err(|error| format!("{} is not a usable certificate: {error}", certificate.display()))?;
                builder = builder.add_root_certificate(pem);
            }
        }
        let http = builder
            .build()
            .map_err(|error| format!("build the agent client: {error}"))?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            token,
        })
    }

    /// The base URL this client speaks to.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Whether a bearer token was configured for the mission-broker scope.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn authorized(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match &self.token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }

    /// `GET /braid` — the braid's view.
    pub fn braid(&self) -> Result<BraidView, String> {
        let response = self
            .authorized(self.http.get(self.url("/braid")))
            .send()
            .map_err(|error| format!("GET /braid: {}", classify(&error)))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "GET /braid answered HTTP {}: {}",
                status.as_u16(),
                first_line(&body)
            ));
        }
        serde_json::from_str(&body)
            .map_err(|error| format!("GET /braid answered HTTP {} but its view did not decode: {error}", status.as_u16()))
    }

    /// `GET /world-model/proposals` — the braid's proposal record, when this
    /// build's agent carries the spatial world model; a `503` names the missing
    /// subsystem rather than inventing an empty list.
    pub fn proposals(&self) -> Result<Vec<ProposalEnvelope>, String> {
        let response = self
            .authorized(self.http.get(self.url("/world-model/proposals")))
            .send()
            .map_err(|error| format!("GET /world-model/proposals: {}", classify(&error)))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "GET /world-model/proposals answered HTTP {}: {}",
                status.as_u16(),
                unavailable_reason(&body)
            ));
        }
        serde_json::from_str::<ProposalsResponse>(&body)
            .map(|decoded| decoded.proposals)
            .map_err(|error| {
                format!(
                    "GET /world-model/proposals answered HTTP {} but its body did not decode: {error}",
                    status.as_u16()
                )
            })
    }

    /// `POST /mission-control/envelopes` — one delivery, acknowledged or
    /// refused with the agent's own words.
    pub fn post_envelope(&self, envelope: &MissionEnvelopeV1) -> Result<EnvelopePost, String> {
        let response = self
            .authorized(self.http.post(self.url("/mission-control/envelopes")))
            .json(envelope)
            .send()
            .map_err(|error| format!("POST /mission-control/envelopes: {}", classify(&error)))?;
        let http_status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|_| {
            serde_json::json!({ "raw": first_line(&body) })
        });
        let accepted = parsed
            .get("accepted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let idempotent_replay = parsed
            .get("idempotent_replay")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Ok(EnvelopePost {
            http_status,
            accepted,
            idempotent_replay,
            body: parsed,
        })
    }

    /// `GET /mission-control/missions` — every mission the agent holds.
    pub fn missions(&self) -> Result<serde_json::Value, String> {
        self.get_json("/mission-control/missions")
    }

    /// `GET /mission-control/events` — the audit trail newer than a sequence.
    pub fn events(&self, after_sequence: u64) -> Result<serde_json::Value, String> {
        self.get_json(&format!("/mission-control/events?after_sequence={after_sequence}"))
    }

    fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let response = self
            .authorized(self.http.get(self.url(path)))
            .send()
            .map_err(|error| format!("GET {path}: {}", classify(&error)))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "GET {path} answered HTTP {}: {}",
                status.as_u16(),
                unavailable_reason(&body)
            ));
        }
        serde_json::from_str(&body)
            .map_err(|error| format!("GET {path} answered HTTP {} but its body did not decode: {error}", status.as_u16()))
    }
}

impl EnvelopePost {
    /// The agent's own refusal words, when it refused.
    pub fn refusal(&self) -> String {
        let error = self
            .body
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no error field");
        redact::secrets(&format!(
            "HTTP {}: {error}",
            self.http_status
        ))
    }
}

/// The mission this broker delivered, as the agent now reports it.
pub fn mission_summary(
    body: &serde_json::Value,
    mission_id: &str,
) -> Option<MissionSummary> {
    let missions = body.get("missions")?.as_array()?;
    let row = missions.iter().find(|row| {
        row.get("envelope")
            .and_then(|envelope| envelope.get("mission_id"))
            .and_then(serde_json::Value::as_str)
            == Some(mission_id)
    })?;
    let field = |name: &str| {
        row.get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some(MissionSummary {
        mission_id: mission_id.to_string(),
        status: field("status"),
        stage: field("stage"),
        code: field("last_code"),
        detail: field("last_detail"),
    })
}

/// The events newer than `after_sequence`, oldest first.
pub fn event_summaries(body: &serde_json::Value) -> Vec<EventSummary> {
    let Some(events) = body.get("events").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    events
        .iter()
        .map(|event| EventSummary {
            sequence: event
                .get("sequence")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            mission_id: event
                .get("mission_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            event_kind: event
                .get("event_kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: event
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            code: event
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            detail: event
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// Short transport labels an operator can act on, from the console's classifier.
fn classify(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "tcp timeout".to_string()
    } else if error.is_connect() {
        "connection refused".to_string()
    } else if error.is_decode() {
        "malformed body".to_string()
    } else {
        redact::secrets(&error.to_string())
    }
}

/// The first line of a body, bounded, for an error line that stays one line.
fn first_line(body: &str) -> String {
    let line = body.lines().next().unwrap_or_default().trim();
    redact::secrets(&line.chars().take(240).collect::<String>())
}

/// A `503` from an unlanded subsystem names it; anything else is reported as
/// the body's first line.
fn unavailable_reason(body: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return first_line(body);
    };
    let error = parsed
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("no error field");
    match parsed.get("subsystem").and_then(serde_json::Value::as_str) {
        Some(subsystem) => redact::secrets(&format!("{error} (subsystem: {subsystem})")),
        None => redact::secrets(error),
    }
}
