//! The agent's Leash client: the only place this process reaches the motion
//! authority.
//!
//! Qualia proposes and Leash decides. Nothing in this module writes a motor or a
//! shared-memory slot — every path is one HTTP request to the base URL
//! `QUALIA_LEASH_BASE_URL` names, and every answer is one of three outcomes:
//! **accepted**, **refused** or **timeout**. The outcome is logged as an
//! operator line (D-008) and returned to the caller, so a refusal or a silence
//! is never rendered as a success.
//!
//! The commands are the ones the entity contract (`qualia_types::EntityAuthority`)
//! marks `leash:http`: `motion.navigate` forwards an evidence-grounded plan as a
//! proposal, and `safety.estop` reaches Leash's acknowledged zero-output stop.
//! Both fail closed before any request leaves the process when the declaration is
//! wrong: a command that is not marked `leash:http` or is owned by someone other
//! than Leash, an e-stop the declaration does not require acknowledgement for,
//! and a navigation proposal with no operator token are all refused locally.
//!
//! The wire fields are the reference's, so a Leash that answers the reference
//! answers this: the goal body is `leash.navigation-goal.v1` with
//! `schema_version`, `mission_id`, `idempotency_key`, `token`, `approval`,
//! `frame_id`, `x_m`, `y_m`, `tolerance_m`, `speed_mode` and `deadline_ms`, and
//! the stop body is `{"reason": "operator-request"}`. Leash's replies are the
//! `{ok, active, status, message}` navigation status and the
//! `{acknowledged, statement}` verified-zero evidence.

use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use qualia_types::{EntityAuthority, EntityProfile};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::AuthScope;
use crate::config::LeashEndpoint;
use crate::mission_control::MissionPlanV1;
use crate::AppState;

/// The transport an authority must name for this client to carry it.
pub const LEASH_HTTP_TRANSPORT: &str = "leash:http";
/// The owner an authority must name for this client to carry it.
pub const LEASH_OWNER: &str = "leash";
/// The entity command that forwards an evidence-grounded plan as a proposal.
pub const NAVIGATE_COMMAND: &str = "motion.navigate";
/// The entity command that reaches Leash's acknowledged stop.
pub const ESTOP_COMMAND: &str = "safety.estop";
/// The schema Leash fixes for a navigation goal.
pub const LEASH_NAVIGATION_GOAL_SCHEMA_VERSION: &str = "leash.navigation-goal.v1";
/// The only speed mode Qualia asks Leash for.
pub const LEASH_SPEED_MODE: &str = "low";
/// The schema literal every Leash outcome reply carries.
pub const LEASH_OUTCOME_SCHEMA_VERSION: &str = "qualia.leash-outcome.v1";
/// How long a Leash request may take before it is a timeout, matching the
/// reference's bounded transport.
pub const LEASH_REQUEST_TIMEOUT_MS: u64 = 6_000;
/// The environment key that carries the operator's Leash label.
pub const LEASH_OPERATOR_TOKEN_FILE_ENV: &str = "QUALIA_LEASH_OPERATOR_TOKEN_FILE";

/// Which of the three outcomes an exchange had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeashOutcomeKind {
    /// Leash took the command.
    Accepted,
    /// Leash declined it, the transport never reached Leash, or the local
    /// declaration refused it before a request was sent.
    Refused,
    /// Leash did not answer inside the bounded transport window. Nothing is
    /// known about the command, so nothing is claimed.
    Timeout,
}

impl LeashOutcomeKind {
    /// The word the log line and the reply carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Refused => "refused",
            Self::Timeout => "timeout",
        }
    }
}

/// One exchange with Leash, kept distinct all the way to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeashOutcome {
    /// The schema literal of this reply.
    pub schema_version: String,
    /// The entity command that was carried, e.g. `motion.navigate`.
    pub command: String,
    pub outcome: LeashOutcomeKind,
    /// A stable reason code: `accepted`, `leash_refused`, `stop_unverified`,
    /// `leash_unreachable`, `leash_timeout`, `authority_denied`,
    /// `acknowledgement_required`, or `operator_token_unavailable`.
    pub code: String,
    /// The human-readable detail, including Leash's own status when it answered.
    pub detail: String,
}

impl LeashOutcome {
    fn accepted(command: &str, detail: impl Into<String>) -> Self {
        Self {
            schema_version: LEASH_OUTCOME_SCHEMA_VERSION.to_string(),
            command: command.to_string(),
            outcome: LeashOutcomeKind::Accepted,
            code: "accepted".to_string(),
            detail: detail.into(),
        }
    }

    fn refused(command: &str, code: &str, detail: impl Into<String>) -> Self {
        Self {
            schema_version: LEASH_OUTCOME_SCHEMA_VERSION.to_string(),
            command: command.to_string(),
            outcome: LeashOutcomeKind::Refused,
            code: code.to_string(),
            detail: detail.into(),
        }
    }

    fn timeout(command: &str, detail: impl Into<String>) -> Self {
        Self {
            schema_version: LEASH_OUTCOME_SCHEMA_VERSION.to_string(),
            command: command.to_string(),
            outcome: LeashOutcomeKind::Timeout,
            code: "leash_timeout".to_string(),
            detail: detail.into(),
        }
    }

    /// The status this outcome answers with.
    pub fn status(&self) -> StatusCode {
        match self.outcome {
            LeashOutcomeKind::Accepted => StatusCode::OK,
            LeashOutcomeKind::Refused => StatusCode::CONFLICT,
            LeashOutcomeKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }
}

/// Leash's navigation status, as `/navigation/goals` returns it.
#[derive(Debug, Deserialize)]
struct LeashNavigationStatus {
    ok: bool,
    active: bool,
    status: String,
    message: String,
}

/// Leash's verified-zero evidence, as `/motors/stop/verified` returns it.
#[derive(Debug, Deserialize)]
struct LeashVerifiedZero {
    acknowledged: bool,
    statement: String,
}

/// The client, built once per request from the resolved configuration.
#[derive(Clone)]
pub struct LeashClient {
    endpoint: LeashEndpoint,
    http: reqwest::Client,
    timeout: Duration,
}

impl LeashClient {
    /// The configured client, with the bounded transport window.
    pub fn from_endpoint(endpoint: LeashEndpoint) -> Self {
        Self::with_timeout(endpoint, Duration::from_millis(LEASH_REQUEST_TIMEOUT_MS))
    }

    /// The same client with an explicit window, for the tests that must see a
    /// silence inside a run.
    pub fn with_timeout(endpoint: LeashEndpoint, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .expect("valid Leash HTTP client");
        Self {
            endpoint,
            http,
            timeout,
        }
    }

    /// Forward an evidence-grounded plan to Leash as a navigation proposal.
    ///
    /// The plan is a *proposal*: this call moves no motor, and Leash's reply is
    /// the only thing recorded.
    pub async fn navigate(&self, authority: &EntityAuthority, plan: &MissionPlanV1) -> LeashOutcome {
        if let Err(outcome) = self.gate(authority, NAVIGATE_COMMAND) {
            return self.record(outcome);
        }
        let token = match self.operator_token(NAVIGATE_COMMAND) {
            Ok(token) => token,
            Err(outcome) => return self.record(outcome),
        };
        let body = json!({
            "schema_version": LEASH_NAVIGATION_GOAL_SCHEMA_VERSION,
            "mission_id": plan.mission_id,
            "idempotency_key": plan.plan_id,
            "token": token,
            "approval": true,
            "frame_id": plan.frame_id,
            "x_m": plan.target_x_m,
            "y_m": plan.target_y_m,
            "tolerance_m": plan.tolerance_m,
            "speed_mode": LEASH_SPEED_MODE,
            "deadline_ms": plan.expires_at_ms,
        });
        let response = match self.send("/navigation/goals", &body, NAVIGATE_COMMAND).await {
            Ok(response) => response,
            Err(outcome) => return self.record(outcome),
        };
        let status: LeashNavigationStatus = match decode(response, NAVIGATE_COMMAND).await {
            Ok(status) => status,
            Err(outcome) => return self.record(outcome),
        };
        let outcome = if status.ok {
            LeashOutcome::accepted(
                NAVIGATE_COMMAND,
                format!(
                    "Leash status {} (active={}): {}",
                    status.status, status.active, status.message
                ),
            )
        } else {
            LeashOutcome::refused(
                NAVIGATE_COMMAND,
                "leash_refused",
                format!("Leash status {}: {}", status.status, status.message),
            )
        };
        self.record(outcome)
    }

    /// Reach Leash's acknowledged zero-output stop for `safety.estop`.
    ///
    /// The declaration must require acknowledgement and Leash must confirm zero
    /// output; either missing is a refusal, never a claimed stop.
    pub async fn estop(&self, authority: &EntityAuthority) -> LeashOutcome {
        if let Err(outcome) = self.gate(authority, ESTOP_COMMAND) {
            return self.record(outcome);
        }
        let body = json!({ "reason": "operator-request" });
        let response = match self
            .send("/motors/stop/verified", &body, ESTOP_COMMAND)
            .await
        {
            Ok(response) => response,
            Err(outcome) => return self.record(outcome),
        };
        let evidence: LeashVerifiedZero = match decode(response, ESTOP_COMMAND).await {
            Ok(evidence) => evidence,
            Err(outcome) => return self.record(outcome),
        };
        let outcome = if evidence.acknowledged {
            LeashOutcome::accepted(ESTOP_COMMAND, evidence.statement)
        } else {
            LeashOutcome::refused(ESTOP_COMMAND, "stop_unverified", evidence.statement)
        };
        self.record(outcome)
    }

    /// The local declaration gate, before any request is sent.
    fn gate(&self, authority: &EntityAuthority, command: &str) -> Result<(), LeashOutcome> {
        if authority.command != command
            || authority.owner != LEASH_OWNER
            || authority.transport != LEASH_HTTP_TRANSPORT
        {
            return Err(LeashOutcome::refused(
                command,
                "authority_denied",
                format!(
                    "the entity declaration carries '{}' over '{}' owned by '{}', not {command} over {LEASH_HTTP_TRANSPORT} owned by {LEASH_OWNER}",
                    authority.command, authority.transport, authority.owner
                ),
            ));
        }
        if command == ESTOP_COMMAND && !authority.acknowledgement_required {
            return Err(LeashOutcome::refused(
                command,
                "acknowledgement_required",
                "the entity declaration does not require an acknowledged e-stop, so none was sent",
            ));
        }
        Ok(())
    }

    /// The operator label Leash's goal route carries.
    fn operator_token(&self, command: &str) -> Result<String, LeashOutcome> {
        let Some(path) = self.endpoint.operator_token_file.as_ref() else {
            return Err(LeashOutcome::refused(
                command,
                "operator_token_unavailable",
                format!("{LEASH_OPERATOR_TOKEN_FILE_ENV} is not set, so no proposal is forwarded"),
            ));
        };
        let token = std::fs::read_to_string(path).map_err(|error| {
            LeashOutcome::refused(
                command,
                "operator_token_unavailable",
                format!("operator token {} is unreadable: {error}", path.display()),
            )
        })?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(LeashOutcome::refused(
                command,
                "operator_token_unavailable",
                format!("operator token {} is empty", path.display()),
            ));
        }
        Ok(token)
    }

    /// One bounded POST. A non-success status is Leash's own refusal.
    async fn send(
        &self,
        path: &str,
        body: &serde_json::Value,
        command: &str,
    ) -> Result<reqwest::Response, LeashOutcome> {
        let url = format!("{}{path}", self.endpoint.base_url);
        let response = self.http.post(&url).json(body).send().await.map_err(|error| {
            if error.is_timeout() {
                LeashOutcome::timeout(
                    command,
                    format!(
                        "Leash did not answer {url} within {} ms",
                        self.timeout.as_millis()
                    ),
                )
            } else {
                LeashOutcome::refused(
                    command,
                    "leash_unreachable",
                    format!("Leash was not reachable at {url}: {error}"),
                )
            }
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        Err(LeashOutcome::refused(
            command,
            "leash_refused",
            format!("Leash returned {status}: {body}"),
        ))
    }

    /// Log the outcome as the operator line and hand it back.
    ///
    /// This is the record step 2 asks for that the current braid vocabulary can
    /// carry: `qualia-braid`'s events are the five strands' and hold no leash
    /// outcome, so the operator line and the HTTP reply are where accepted,
    /// refused and timeout stay distinct. A dedicated braid event for leash
    /// outcomes is a vocabulary decision for its own ticket.
    fn record(&self, outcome: LeashOutcome) -> LeashOutcome {
        eprintln!(
            "qualia-agent: leash {} {}: {}",
            outcome.command,
            outcome.outcome.as_str(),
            outcome.detail
        );
        outcome
    }
}

/// Read a Leash reply, or report the refusal as Leash's own invalid reply.
async fn decode<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    command: &str,
) -> Result<T, LeashOutcome> {
    let url = response.url().to_string();
    response.json().await.map_err(|error| {
        LeashOutcome::refused(
            command,
            "leash_refused",
            format!("Leash answered {url} with invalid JSON: {error}"),
        )
    })
}

/// The declaration this client carries for `command`.
///
/// The loaded entity profile wins when it declares the command; otherwise the
/// built-in declaration stands for the entity contract, which is where the two
/// `leash:http` commands and the e-stop's acknowledgement requirement come
/// from. Either way the gate re-checks transport, owner and acknowledgement, so
/// a profile that declares something else is refused rather than trusted.
pub fn authority_for(profiles: &[EntityProfile], command: &str) -> EntityAuthority {
    profiles
        .iter()
        .filter_map(|profile| profile.authority(command))
        .find(|authority| authority.transport == LEASH_HTTP_TRANSPORT)
        .cloned()
        .unwrap_or_else(|| EntityAuthority {
            command: command.to_string(),
            owner: LEASH_OWNER.to_string(),
            transport: LEASH_HTTP_TRANSPORT.to_string(),
            acknowledgement_required: command == ESTOP_COMMAND,
        })
}

/// The configured client, or the response that says why there is none.
fn client_of(state: &AppState) -> Result<LeashClient, Response> {
    state
        .config
        .leash
        .clone()
        .map(LeashClient::from_endpoint)
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "schema_version": crate::pending::UNAVAILABLE_SCHEMA_VERSION,
                    "error": "QUALIA_LEASH_BASE_URL is not configured",
                    "subsystem": "the Leash endpoint",
                })),
            )
                .into_response()
        })
}

fn outcome_response(outcome: LeashOutcome) -> Response {
    (outcome.status(), Json(outcome)).into_response()
}

/// `POST /leash/navigate` — forward an evidence-grounded plan as a proposal.
pub async fn navigate_post(
    ConnectInfo(remote): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(plan): Json<MissionPlanV1>,
) -> Response {
    if let Err(response) = state.auth.authorize(AuthScope::Admin, &headers, remote.ip()) {
        return response;
    }
    let client = match client_of(&state) {
        Ok(client) => client,
        Err(response) => return response,
    };
    let authority = authority_for(state.entity_profiles.as_slice(), NAVIGATE_COMMAND);
    outcome_response(client.navigate(&authority, &plan).await)
}

/// `POST /leash/estop` — reach Leash's acknowledged zero-output stop.
pub async fn estop_post(
    ConnectInfo(remote): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = state.auth.authorize(AuthScope::Admin, &headers, remote.ip()) {
        return response;
    }
    let client = match client_of(&state) {
        Ok(client) => client,
        Err(response) => return response,
    };
    let authority = authority_for(state.entity_profiles.as_slice(), ESTOP_COMMAND);
    outcome_response(client.estop(&authority).await)
}

/// The Leash surface, merged into the agent's router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/leash/navigate", post(navigate_post))
        .route("/leash/estop", post(estop_post))
}
