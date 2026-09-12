//! The coach client: one structured decision per item, or an honest "no
//! decision".
//!
//! The surface is OpenAI-compatible chat completions — `POST
//! {base}/chat/completions` — with `response_format: {"type": "json_object"}`
//! so the model's answer is a JSON object the broker can validate against the
//! repository's own envelope types. Nothing here parses prose into a decision.
//!
//! Every outcome carries its provenance: a model-produced answer records the
//! model id, the prompt digest, the response id, the token counts (or that the
//! provider returned none) and a wall-clock stamp; a degraded outcome records
//! why and produces no decision at all.

use std::time::Instant;

use qualia_sync_types::{CoachDecisionKind, ProposalEnvelope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::BraidView;
use crate::config::{Area, CoachConfig};
use crate::redact;
use crate::{now_ms, warn};

/// The instruction the coach answers, fixed so the decision is auditable.
pub const SYSTEM_PROMPT: &str = "\
You are the coach of a bounded mobile-robot mission broker. You are given one \
world-model proposal and the braid's state as JSON, and you answer with one \
JSON object and nothing else. Answer with a single JSON object with exactly \
these members:
  \"decision_kind\": one of \"promote\", \"reject\", \"deprecate\", \"merge\", \"split\", \"link\";
  \"target_proposal_ids\": an array of proposal ids; use exactly the proposal id you were given;
  \"output_ids\": an array; for promote at most one id (the canonical id you propose), otherwise empty;
  \"reason\": one sentence, at most 400 characters, saying why;
  \"mission\": present only for promote: {\"objective_kind\": \"explore_frontier\" or \"observe_point\" or \"navigate_to\", \"summary\": at most 200 characters, \"target_x_m\": number or null, \"target_y_m\": number or null, \"tolerance_m\": number or null, \"max_distance_m\": number, \"max_runtime_ms\": integer, \"max_replans\": integer}.
Promote means the proposal becomes canonical state and justifies one bounded \
mission; reject means it does not. Use only the proposal id you were given, \
keep every number inside the stated bounds, and never invent evidence. If the \
proposal is a planner advisory it can never be promoted: reject it.";

/// One thing the coach is asked to decide about.
#[derive(Debug, Clone)]
pub struct DecisionItem {
    /// The proposal's own id, used to dedupe decisions across a run.
    pub item_id: String,
    pub proposal: ProposalEnvelope,
}

/// The mission draft a promote decision carries, as the model wrote it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissionDraft {
    #[serde(default)]
    pub objective_kind: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub target_x_m: Option<f32>,
    #[serde(default)]
    pub target_y_m: Option<f32>,
    #[serde(default)]
    pub tolerance_m: Option<f32>,
    #[serde(default)]
    pub speed_ceiling_mps: Option<f32>,
    #[serde(default)]
    pub max_distance_m: Option<f32>,
    #[serde(default)]
    pub max_runtime_ms: Option<u64>,
    #[serde(default)]
    pub max_replans: Option<u8>,
    #[serde(default)]
    pub evidence_max_age_ms: Option<u64>,
}

/// The decision the model produced, before the broker turns it into envelopes.
#[derive(Debug, Clone)]
pub struct DecisionDraft {
    pub decision_kind: CoachDecisionKind,
    pub target_proposal_ids: Vec<String>,
    pub output_ids: Vec<String>,
    pub reason: Option<String>,
    pub mission: Option<MissionDraft>,
}

/// A decision a model actually produced, with everything needed to check it.
#[derive(Debug, Clone)]
pub struct CoachAnswer {
    pub draft: DecisionDraft,
    pub model_id: String,
    pub response_id: Option<String>,
    pub prompt_digest: String,
    pub request_bytes: usize,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub usage_reported: bool,
    pub latency_ms: u64,
    pub decided_at_ms: u64,
    /// The model's own content, for the run's evidence line.
    pub content: String,
}

/// Why no model produced a decision.
#[derive(Debug, Clone)]
pub struct NoDecision {
    /// A short machine word: `no_key`, `timeout`, `transport`, `http_status`,
    /// `unparseable`, `truncated`.
    pub reason: String,
    /// The one named line a run prints.
    pub line: String,
    pub latency_ms: Option<u64>,
}

impl NoDecision {
    /// Whether the provider answered at all.
    pub fn reached_provider(&self) -> bool {
        !matches!(self.reason.as_str(), "no_key" | "transport")
    }
}

/// What one `ask` produced.
pub enum CoachOutcome {
    Decision(Box<CoachAnswer>),
    NoDecision(Box<NoDecision>),
}

#[derive(Deserialize)]
struct RawDraft {
    #[serde(default)]
    decision_kind: Option<String>,
    #[serde(default)]
    target_proposal_ids: Vec<String>,
    #[serde(default)]
    output_ids: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    mission: Option<MissionDraft>,
}

/// The one named line a run with no credential prints.
pub fn no_key_line(item: Option<&str>) -> String {
    let subject = match item {
        Some(item) => format!(" for {item}"),
        None => String::new(),
    };
    format!(
        "qualia-mission-broker: no coach credential in the environment ({} or {}); llm_priors_ablated=true, no decision{subject}",
        crate::config::COACH_KEY_KEYS[0],
        crate::config::COACH_KEY_KEYS[1]
    )
}

/// The blocking coach client.
pub struct CoachClient {
    http: reqwest::blocking::Client,
    config: CoachConfig,
}

impl CoachClient {
    /// Build the client from resolved configuration.
    pub fn new(config: CoachConfig) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(config.timeout)
            .timeout(config.timeout)
            .build()
            .map_err(|error| format!("build the coach client: {error}"))?;
        Ok(Self { http, config })
    }

    /// The configuration this client resolved, for the status surface.
    pub fn config(&self) -> &CoachConfig {
        &self.config
    }

    /// Ask once about one item. Never panics, never blocks past the timeout,
    /// never returns a decision a model did not produce.
    pub fn ask(&self, item: &DecisionItem, braid: &BraidView, area: &Area) -> CoachOutcome {
        let Some(api_key) = self.config.api_key.as_deref() else {
            return CoachOutcome::NoDecision(Box::new(NoDecision {
                reason: "no_key".to_string(),
                line: no_key_line(Some(&item.item_id)),
                latency_ms: None,
            }));
        };

        let request_body = self.request_body(item, braid, area);
        let request_bytes = match serde_json::to_vec(&request_body) {
            Ok(bytes) => bytes,
            Err(error) => {
                return CoachOutcome::NoDecision(Box::new(NoDecision {
                    reason: "transport".to_string(),
                    line: format!(
                        "qualia-mission-broker: the coach request could not be encoded: {error}; llm_priors_ablated=true, no decision"
                    ),
                    latency_ms: None,
                }))
            }
        };
        let prompt_digest = format!("sha256:{}", hex(&request_bytes));
        let request_len = request_bytes.len();

        warn(&format!(
            "qualia-mission-broker: coach request model={} base_url={} key={} prompt_digest={} prompt_bytes={} timeout_ms={}",
            self.config.model,
            self.config.base_url,
            redact::key_prefix(api_key),
            prompt_digest,
            request_bytes.len(),
            self.config.timeout.as_millis()
        ));

        let url = format!("{}/chat/completions", self.config.base_url);
        let started = Instant::now();
        let response = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_bytes)
            .send();
        let latency_ms = started.elapsed().as_millis() as u64;
        let known = [api_key];

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let reason = if error.is_timeout() {
                    "timeout"
                } else {
                    "transport"
                };
                let line = if reason == "timeout" {
                    format!(
                        "qualia-mission-broker: the coach did not answer within {} ms (model {}, item {}); llm_priors_ablated=true, no decision",
                        self.config.timeout.as_millis(),
                        self.config.model,
                        item.item_id
                    )
                } else {
                    format!(
                        "qualia-mission-broker: the coach request failed: {}; llm_priors_ablated=true, no decision for {}",
                        redact::scrub(&error.to_string(), &known),
                        item.item_id
                    )
                };
                return CoachOutcome::NoDecision(Box::new(NoDecision {
                    reason: reason.to_string(),
                    line,
                    latency_ms: Some(latency_ms),
                }));
            }
        };

        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return CoachOutcome::NoDecision(Box::new(NoDecision {
                reason: "http_status".to_string(),
                line: format!(
                    "qualia-mission-broker: the coach answered HTTP {} for {}: {}; llm_priors_ablated=true, no decision",
                    status.as_u16(),
                    item.item_id,
                    redact::scrub(&first_chars(&body, 240), &known)
                ),
                latency_ms: Some(latency_ms),
            }));
        }

        let parsed: serde_json::Value = match serde_json::from_str(&body) {
            Ok(parsed) => parsed,
            Err(error) => {
                return CoachOutcome::NoDecision(Box::new(NoDecision {
                    reason: "unparseable".to_string(),
                    line: format!(
                        "qualia-mission-broker: the coach's response body did not decode as JSON: {error}; llm_priors_ablated=true, no decision"
                    ),
                    latency_ms: Some(latency_ms),
                }))
            }
        };

        let response_id = parsed
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let model_id = parsed
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&self.config.model)
            .to_string();
        let usage = parsed.get("usage");
        let usage_reported = usage
            .and_then(|usage| usage.get("prompt_tokens").or_else(|| usage.get("completion_tokens")))
            .is_some();
        let prompt_tokens = usage
            .and_then(|usage| usage.get("prompt_tokens"))
            .and_then(serde_json::Value::as_u64);
        let completion_tokens = usage
            .and_then(|usage| usage.get("completion_tokens"))
            .and_then(serde_json::Value::as_u64);

        let choice = parsed
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first());
        let finish_reason = choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let content = choice
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        if content.trim().is_empty() {
            return CoachOutcome::NoDecision(Box::new(NoDecision {
                reason: "unparseable".to_string(),
                line: format!(
                    "qualia-mission-broker: the coach's response carried no content (finish_reason={}); llm_priors_ablated=true, no decision",
                    if finish_reason.is_empty() { "absent" } else { &finish_reason }
                ),
                latency_ms: Some(latency_ms),
            }));
        }

        let raw: RawDraft = match serde_json::from_str(&content) {
            Ok(raw) => raw,
            Err(error) => {
                let reason = if finish_reason == "length" { "truncated" } else { "unparseable" };
                return CoachOutcome::NoDecision(Box::new(NoDecision {
                    reason: reason.to_string(),
                    line: format!(
                        "qualia-mission-broker: the coach's content was not a decision object ({error}); llm_priors_ablated=true, no decision"
                    ),
                    latency_ms: Some(latency_ms),
                }));
            }
        };

        let Some(kind) = raw
            .decision_kind
            .as_deref()
            .and_then(|kind| serde_json::from_value::<CoachDecisionKind>(serde_json::Value::String(kind.to_string())).ok())
        else {
            return CoachOutcome::NoDecision(Box::new(NoDecision {
                reason: "unparseable".to_string(),
                line: format!(
                    "qualia-mission-broker: the coach's decision_kind was not one of the six kinds; llm_priors_ablated=true, no decision"
                ),
                latency_ms: Some(latency_ms),
            }));
        };

        if raw.target_proposal_ids.is_empty() {
            return CoachOutcome::NoDecision(Box::new(NoDecision {
                reason: "unparseable".to_string(),
                line: format!(
                    "qualia-mission-broker: the coach's decision named no target proposal; llm_priors_ablated=true, no decision"
                ),
                latency_ms: Some(latency_ms),
            }));
        }

        warn(&format!(
            "qualia-mission-broker: coach response id={} model={} latency_ms={} usage={}{}",
            response_id.as_deref().unwrap_or("none"),
            model_id,
            latency_ms,
            match (prompt_tokens, completion_tokens) {
                (Some(prompt), Some(completion)) => format!("prompt_tokens={prompt} completion_tokens={completion}"),
                _ => "the provider returned no usage".to_string(),
            },
            if finish_reason.is_empty() {
                String::new()
            } else {
                format!(" finish_reason={finish_reason}")
            }
        ));

        CoachOutcome::Decision(Box::new(CoachAnswer {
            draft: DecisionDraft {
                decision_kind: kind,
                target_proposal_ids: raw.target_proposal_ids,
                output_ids: raw.output_ids,
                reason: raw.reason,
                mission: raw.mission,
            },
            model_id,
            response_id,
            prompt_digest,
            request_bytes: request_len,
            prompt_tokens,
            completion_tokens,
            usage_reported,
            latency_ms,
            decided_at_ms: now_ms() as u64,
            content: redact::scrub(&content, &known),
        }))
    }

    /// The request the coach answers: the fixed instruction plus the item, the
    /// braid's state and the envelope's bounds as one JSON document.
    fn request_body(
        &self,
        item: &DecisionItem,
        braid: &BraidView,
        area: &Area,
    ) -> serde_json::Value {
        serde_json::json!({
            "model": self.config.model,
            "temperature": 0,
            "max_tokens": 700,
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": serde_json::to_string(&serde_json::json!({
                    "task": "decide what the broker does with this proposal",
                    "braid": {
                        "session_id": braid.session_id,
                        "generation": braid.generation,
                        "open_missions": braid.open_missions,
                    },
                    "operating_area_odom_m": {
                        "min_x_m": area.min_x_m,
                        "min_y_m": area.min_y_m,
                        "max_x_m": area.max_x_m,
                        "max_y_m": area.max_y_m,
                    },
                    "envelope_bounds": {
                        "speed_ceiling_mps": { "min": 0.01, "max": 0.25, "default": crate::config::DEFAULT_SPEED_CEILING_MPS },
                        "max_distance_m": { "min": 0.05, "max": 5.0, "default": crate::config::DEFAULT_MAX_DISTANCE_M },
                        "max_runtime_ms": { "min": 100, "max": 120000, "default": crate::config::DEFAULT_MAX_RUNTIME_MS },
                        "max_replans": { "min": 0, "max": 8, "default": crate::config::DEFAULT_MAX_REPLANS },
                        "evidence_max_age_ms": { "min": 100, "max": 5000, "default": crate::config::DEFAULT_EVIDENCE_MAX_AGE_MS },
                        "tolerance_m": { "min": 0.05, "max": 1.0 },
                    },
                    "proposal": item.proposal,
                })).unwrap_or_else(|_| "{}".to_string()) },
            ],
        })
    }
}

/// A bounded prefix of a body, for a one-line error.
fn first_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Lowercase SHA-256 hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write;
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}
