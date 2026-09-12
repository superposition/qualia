//! Coach view: the mission broker's decision stream, read from its status
//! surface (`GET /coach`, `qualia.coach-state.v1`).
//!
//! This is a *second* source beside the agent, and deliberately so: the agent
//! owns missions and has no coach stream today, so the broker (T63,
//! `runners/mission-broker`) serves the model's state and its newest decisions
//! on a read-only loopback endpoint for this panel. When the agent grows a
//! decision stream of its own — `/world-model/decisions` is the shape it would
//! take, and it is a `503` stub in this build — this panel should read that and
//! the broker's endpoint should be retired. The crate README records the same
//! direction from the other side.
//!
//! The panel's honesty rule is the console's: no reading, no number. A broker
//! that is not running degrades one named line — `coach broker not running at
//! <url>` — and shows no decision it did not receive.

use std::time::Duration;

use egui::Ui;
use serde::{Deserialize, Serialize};

use crate::theme;

/// The address the broker's status surface is read from; environment only, the
/// way `QUALIA_AGENT_URL` is.
pub const COACH_URL_ENV: &str = "QUALIA_COACH_URL";
/// The broker's own default (`QUALIA_COACH_STATUS_PORT`, loopback only).
pub const DEFAULT_COACH_URL: &str = "http://127.0.0.1:8091";
/// The braid client's budget, kept here so both sources fail at the same speed.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(1200);

/// The broker's status URL, from the environment only.
pub fn coach_url() -> String {
    std::env::var(COACH_URL_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_COACH_URL.to_string())
}

/// The model's state, as the broker reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelState {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub key_presence: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub last_latency_ms: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// One decision the broker holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRow {
    #[serde(default)]
    pub decision_id: String,
    #[serde(default)]
    pub decision_kind: String,
    #[serde(default)]
    pub target_proposal_ids: Vec<String>,
    #[serde(default)]
    pub output_ids: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub llm_priors_ablated: bool,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub prompt_digest: Option<String>,
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub usage_reported: bool,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub decided_at_ms: u64,
    #[serde(default)]
    pub disposition: String,
}

/// One mission the broker delivered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionRow {
    #[serde(default)]
    pub mission_id: String,
    #[serde(default)]
    pub idempotency_key: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub ack_http_status: Option<u16>,
    #[serde(default)]
    pub detail: String,
}

/// The broker's whole payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachSnapshot {
    #[serde(default)]
    pub schema_version: String,
    #[serde(default)]
    pub observed_at_ms: u64,
    #[serde(default)]
    pub model: Option<ModelState>,
    #[serde(default)]
    pub decisions: Vec<DecisionRow>,
    #[serde(default)]
    pub missions: Vec<MissionRow>,
    #[serde(default)]
    pub degradations: Vec<String>,
}

/// What the panel is showing: the broker's answer, or why it has none.
#[derive(Debug, Clone, PartialEq)]
pub enum CoachView {
    Live(Box<CoachSnapshot>),
    Unreachable { url: String, reason: String },
}

impl Default for CoachView {
    fn default() -> Self {
        Self::Unreachable {
            url: coach_url(),
            reason: "not polled".to_string(),
        }
    }
}

impl CoachView {
    /// The named line a degraded panel shows, in the operator's words.
    pub fn degraded_line(&self) -> Option<String> {
        match self {
            Self::Live(_) => None,
            Self::Unreachable { url, reason } => {
                Some(format!("coach broker not running at {url}: {reason}"))
            }
        }
    }
}

/// Read the broker's status surface once.
pub fn fetch(url: &str) -> Result<CoachSnapshot, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("client: {error}"))?;
    let response = client
        .get(format!("{}/coach", url.trim_end_matches('/')))
        .send()
        .map_err(|error| classify(&error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    response
        .json::<CoachSnapshot>()
        .map_err(|_| "malformed body".to_string())
}

/// The brokered state for one poll: the broker's answer, or the named reason.
pub fn sample(url: &str) -> CoachView {
    match fetch(url) {
        Ok(snapshot) => CoachView::Live(Box::new(snapshot)),
        Err(reason) => CoachView::Unreachable {
            url: url.to_string(),
            reason,
        },
    }
}

/// Short transport labels an operator can act on, from the braid client's
/// classifier.
fn classify(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "tcp timeout".to_owned()
    } else if error.is_connect() {
        "connection refused".to_owned()
    } else {
        "transport failure".to_owned()
    }
}

/// Draw the Coach panel as a floating window.
///
/// The default place is the right margin, where a wide window has room beside
/// the six-view grid; the window itself is the same movable, resizable,
/// collapsible one the operator drags (and the HUD panels use the same
/// convention).
pub fn render_panel(ctx: &egui::Context, state: &crate::ConsoleState) {
    let screen = ctx.content_rect();
    let pos = egui::pos2((screen.right() - 416.0).max(24.0), 44.0);
    egui::Window::new("Coach")
        .id(egui::Id::new("qualia_coach_panel"))
        .default_pos(pos)
        .default_size(egui::vec2(392.0, 296.0))
        .resizable(true)
        .collapsible(true)
        .movable(true)
        .show(ctx, |ui| render(ui, state));
}

/// The panel's body.
pub fn render(ui: &mut Ui, state: &crate::ConsoleState) {
    match &state.coach {
        CoachView::Unreachable { .. } => {
            let line = state.coach.degraded_line().unwrap_or_default();
            theme::error_banner(ui, &line);
            theme::state_line(
                ui,
                "no coach decisions to show; start the broker with --ticks or --once",
                theme::TEXT_SECOND,
            );
            return;
        }
        CoachView::Live(snapshot) => {
            let pill = match snapshot.model.as_ref().map(|model| model.status.as_str()) {
                Some("ok") => ("model configured", theme::ACCENT),
                Some("no_key") => ("no key", theme::WARN),
                Some("timeout") => ("timeout", theme::FAIL),
                Some("error") => ("error", theme::FAIL),
                _ => ("unknown", theme::MUTED),
            };
            theme::status_pill(ui, pill.0, pill.1);
            ui.add_space(theme::GAP_S);

            if let Some(model) = &snapshot.model {
                theme::field_text(ui, "model", &model.model_id);
                theme::field_path(ui, "base url", &model.base_url);
                theme::field_text(
                    ui,
                    "key",
                    &model
                        .key_presence
                        .clone()
                        .unwrap_or_else(|| "absent".to_string()),
                );
                theme::field(
                    ui,
                    "last latency",
                    &model
                        .last_latency_ms
                        .map(|millis| millis.to_string())
                        .unwrap_or_else(|| theme::DASH.to_string()),
                    Some("ms"),
                );
                if let Some(error) = &model.last_error {
                    theme::state_line(ui, error, theme::WARN);
                }
            } else {
                theme::state_line(ui, "the broker reported no model state", theme::WARN);
            }
            ui.add_space(theme::GAP_S);
            theme::hairline(ui);
            ui.add_space(theme::GAP_S);

            match snapshot.decisions.first() {
                Some(decision) => {
                    theme::field_text(
                        ui,
                        "decision",
                        &format!(
                            "{} {}",
                            decision.decision_kind,
                            decision.target_proposal_ids.join(", ")
                        ),
                    );
                    if let Some(reason) = &decision.reason {
                        theme::field_text(ui, "reason", reason);
                    }
                    theme::field_text(
                        ui,
                        "provenance",
                        &format!(
                            "{} · {}{}",
                            decision.model_id.clone().unwrap_or_else(|| "no model".to_string()),
                            match (decision.prompt_tokens, decision.completion_tokens) {
                                (Some(prompt), Some(completion)) =>
                                    format!("tokens {prompt}/{completion}"),
                                _ => "no usage".to_string(),
                            },
                            if decision.llm_priors_ablated {
                                " · llm_priors_ablated=true"
                            } else {
                                ""
                            }
                        ),
                    );
                    theme::field_text(
                        ui,
                        "response",
                        decision.response_id.as_deref().unwrap_or(theme::DASH),
                    );
                    theme::field_text(ui, "broker", &decision.disposition);
                    if snapshot.decisions.len() > 1 {
                        theme::state_line(
                            ui,
                            &format!("{} earlier decision(s) held", snapshot.decisions.len() - 1),
                            theme::TEXT_SECOND,
                        );
                    }
                }
                None => theme::state_line(ui, "the broker holds no decision yet", theme::TEXT_SECOND),
            }

            if let Some(mission) = snapshot.missions.first() {
                ui.add_space(theme::GAP_S);
                theme::hairline(ui);
                ui.add_space(theme::GAP_S);
                theme::field_text(ui, "mission", &mission.mission_id);
                theme::field_text(
                    ui,
                    "accepted",
                    &format!(
                        "{} {}",
                        mission.command,
                        mission
                            .ack_http_status
                            .map(|status| format!("HTTP {status}"))
                            .unwrap_or_else(|| theme::DASH.to_string())
                    ),
                );
            }
        }
    }
}
