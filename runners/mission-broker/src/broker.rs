//! One broker loop, bounded by ticks.
//!
//! A tick reads the braid, takes the items that need a decision (the braid's
//! proposals), asks the coach once per item it has not decided this run, and
//! publishes what the repository's own validators accept. Nothing is retried
//! behind the operator's back and nothing is invented: every refusal is a named
//! line, and the run's report carries them all.
//!
//! The run is bounded — `--once` is one tick — because the acceptance is a live,
//! quotable pass, not a daemon whose work cannot be read back.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use qualia_sync_types::{CoachDecisionKind, ProposalEnvelope};

use crate::agent::{self, AgentClient};
use crate::coach::{CoachAnswer, CoachClient, CoachOutcome, DecisionItem};
use crate::config::BrokerConfig;
use crate::envelope::{self, MissionRequest};
use crate::status::{DecisionRow, MissionRow, ModelState, StatusHandle};
use crate::{epoch_seed, now_ms, say, warn};

/// How long one agent request may take before the tick gives up on it.
pub const AGENT_TIMEOUT_MS: u64 = 3_000;

/// What one run is asked to do.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// How many ticks to run; `1` is `--once`.
    pub ticks: u32,
    /// Gap between ticks of a multi-tick run.
    pub interval: Duration,
    /// Files carrying the braid's proposal envelopes.
    pub proposals: Vec<PathBuf>,
    /// Whether to serve the console's status surface.
    pub status: bool,
    pub status_host: String,
    pub status_port: u16,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            ticks: 1,
            interval: Duration::from_millis(crate::config::DEFAULT_TICK_MS),
            proposals: Vec::new(),
            status: true,
            status_host: crate::config::DEFAULT_STATUS_HOST.to_string(),
            status_port: crate::config::DEFAULT_STATUS_PORT,
        }
    }
}

/// What one run did.
#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub ticks: u32,
    pub items: usize,
    pub decided: usize,
    pub posted: usize,
    /// One named line per degradation, in the order they happened.
    pub degradations: Vec<String>,
}

impl RunReport {
    /// The word the run's last line carries.
    pub fn state(&self) -> &'static str {
        if self.decided == 0 && !self.degradations.is_empty() {
            "degraded"
        } else if self.posted > 0 {
            "posted"
        } else if self.decided > 0 {
            "decided"
        } else {
            "idle"
        }
    }
}

/// Run the broker.
pub fn run(config: BrokerConfig, options: RunOptions) -> Result<RunReport, String> {
    let coach = CoachClient::new(config.coach.clone())?;
    let agent = AgentClient::new(
        &config.agent_url,
        config.agent_tls_dir.as_deref(),
        config.broker_token.clone(),
        Duration::from_millis(AGENT_TIMEOUT_MS),
    )?;
    let status = StatusHandle::new(model_state(&config), config.coach.api_key.clone());
    if options.status {
        match status.serve(&options.status_host, options.status_port) {
            Ok(()) => say(&format!(
                "qualia-mission-broker: coach status surface on http://{}:{}/coach (loopback only)",
                options.status_host, options.status_port
            )),
            Err(error) => warn(&format!(
                "qualia-mission-broker: {error}; the console's Coach panel will name the broker as not running"
            )),
        }
    }

    say(&format!(
        "qualia-mission-broker: agent {} token={} coach model={} base_url={} key={} timeout_ms={}",
        agent.base(),
        if agent.has_token() { "configured" } else { "absent (QUALIA_MISSION_BROKER_TOKEN)" },
        config.coach.model,
        config.coach.base_url,
        config
            .coach
            .key_presence()
            .unwrap_or_else(|| "none (llm_priors_ablated=true)".to_string()),
        config.coach.timeout.as_millis()
    ));

    let producer_epoch = epoch_seed();
    let mut report = RunReport::default();
    let mut decided: BTreeSet<String> = BTreeSet::new();
    let mut sequence: u64 = 0;

    if !config.coach.configured() {
        // One named line at the start, so a run without a credential says so
        // even before it has an item to decide about.
        let line = crate::coach::no_key_line(None);
        warn(&line);
        status.push_degradation(line.clone());
        report.degradations.push(line);
    }

    for tick in 1..=options.ticks.max(1) {
        report.ticks = tick;
        let braid = match agent.braid() {
            Ok(braid) => braid,
            Err(error) => {
                let line = format!("qualia-mission-broker: braid unreadable: {error}");
                report.degradations.push(line.clone());
                status.push_degradation(line.clone());
                warn(&line);
                sleep(options.interval);
                continue;
            }
        };
        say(&format!(
            "qualia-mission-broker: tick {tick} braid session={} generation={} open_missions={}",
            if braid.session_id.is_empty() { "(none)" } else { &braid.session_id },
            braid.generation,
            braid.open_missions
        ));

        let items = match load_items(&agent, &options.proposals) {
            Ok(items) => items,
            Err(error) => {
                let line = format!("qualia-mission-broker: {error}");
                report.degradations.push(line.clone());
                status.push_degradation(line.clone());
                warn(&line);
                sleep(options.interval);
                continue;
            }
        };
        report.items = report.items.max(items.len());

        for item in items {
            if decided.contains(&item.item_id) {
                continue;
            }
            decided.insert(item.item_id.clone());
            sequence += 1;
            report.decided += 1;
            let decision_id = format!("coach-{producer_epoch}-{sequence}");
            let mission_id = format!("mission-coach-{producer_epoch}-{sequence}");
            let idempotency_key = format!("coach-{producer_epoch}-{sequence}");
            let outcome = coach.ask(&item, &braid, &config.area);
            match outcome {
                CoachOutcome::NoDecision(no_decision) => {
                    report.decided -= 1;
                    report.degradations.push(no_decision.line.clone());
                    status.set_model(|model| {
                        model.status = match no_decision.reason.as_str() {
                            "no_key" => "no_key",
                            "timeout" => "timeout",
                            _ => "error",
                        }
                        .to_string();
                        model.reason = Some(no_decision.reason.clone());
                        model.last_latency_ms = no_decision.latency_ms;
                        model.last_error = Some(no_decision.line.clone());
                    });
                    status.push_degradation(no_decision.line.clone());
                    warn(&no_decision.line);
                }
                CoachOutcome::Decision(answer) => {
                    status.set_model(|model| {
                        model.status = "ok".to_string();
                        model.reason = None;
                        model.last_latency_ms = Some(answer.latency_ms);
                        model.last_error = None;
                    });
                    say(&format!(
                        "qualia-mission-broker: coach decision content={}",
                        answer.content
                    ));
                    let (disposition, mission, lines) = act(
                        &config,
                        &agent,
                        &item,
                        &answer,
                        &decision_id,
                        &mission_id,
                        &idempotency_key,
                        producer_epoch,
                        sequence,
                    );
                    for line in &lines {
                        say(line);
                    }
                    status.push_decision(DecisionRow {
                        decision_id: decision_id.clone(),
                        decision_kind: kind_name(answer.draft.decision_kind),
                        target_proposal_ids: answer.draft.target_proposal_ids.clone(),
                        output_ids: answer.draft.output_ids.clone(),
                        reason: answer.draft.reason.clone(),
                        llm_priors_ablated: false,
                        model_id: Some(answer.model_id.clone()),
                        prompt_digest: Some(answer.prompt_digest.clone()),
                        response_id: answer.response_id.clone(),
                        prompt_tokens: answer.prompt_tokens,
                        completion_tokens: answer.completion_tokens,
                        usage_reported: answer.usage_reported,
                        latency_ms: Some(answer.latency_ms),
                        decided_at_ms: answer.decided_at_ms,
                        source_observed_at_ms: None,
                        disposition: disposition.clone(),
                    });
                    match mission {
                        Some(row) => {
                            report.posted += 1;
                            status.push_mission(row);
                        }
                        None => {
                            if disposition.starts_with("refused") {
                                report.degradations.push(format!(
                                    "qualia-mission-broker: decision {decision_id} {disposition}"
                                ));
                                status.push_degradation(format!(
                                    "qualia-mission-broker: decision {decision_id} {disposition}"
                                ));
                            }
                        }
                    }
                }
            }
        }
        if tick < options.ticks {
            sleep(options.interval);
        }
    }

    status.observe();
    say(&format!(
        "qualia-mission-broker: run complete ticks={} items={} decided={} posted={} degraded={} state={}",
        report.ticks,
        report.items,
        report.decided,
        report.posted,
        report.degradations.len(),
        report.state()
    ));
    for line in &report.degradations {
        say(&format!("qualia-mission-broker: degraded: {line}"));
    }
    Ok(report)
}

/// The items one tick decides about: the proposals the operator named, else the
/// braid's own record when the agent carries it.
fn load_items(
    agent: &AgentClient,
    paths: &[PathBuf],
) -> Result<Vec<DecisionItem>, String> {
    if paths.is_empty() {
        let proposals = agent.proposals().map_err(|error| {
            format!(
                "the braid's proposals are unavailable ({error}); pass --proposal <path> or QUALIA_MISSION_BROKER_PROPOSALS"
            )
        })?;
        return Ok(proposals
            .into_iter()
            .map(|proposal| DecisionItem {
                item_id: proposal.proposal_id.clone(),
                proposal,
            })
            .collect());
    }
    let mut items = Vec::new();
    for path in paths {
        for proposal in read_proposals(path)? {
            items.push(DecisionItem {
                item_id: proposal.proposal_id.clone(),
                proposal,
            });
        }
    }
    Ok(items)
}

/// Read one proposal file: `{"proposals": [...]}`, a bare array, or one object.
pub fn read_proposals(path: &Path) -> Result<Vec<ProposalEnvelope>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read the proposal file {}: {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("decode the proposal file {}: {error}", path.display()))?;
    let proposals = match value {
        serde_json::Value::Array(entries) => entries,
        serde_json::Value::Object(mut object) => match object.remove("proposals") {
            Some(serde_json::Value::Array(entries)) => entries,
            Some(other) => vec![other],
            None => vec![serde_json::Value::Object(object)],
        },
        other => vec![other],
    };
    proposals
        .into_iter()
        .map(|entry| {
            let proposal: ProposalEnvelope = serde_json::from_value(entry)
                .map_err(|error| format!("decode a proposal in {}: {error}", path.display()))?;
            proposal
                .validate_shape()
                .map_err(|error| format!("proposal {} in {} is invalid: {error}", proposal.proposal_id, path.display()))?;
            Ok(proposal)
        })
        .collect()
}

/// Turn one model answer into the repository's envelopes and publish what the
/// validators accept.
///
/// Returns the disposition, the mission row when one was posted, and the lines
/// the run should print.
#[allow(clippy::too_many_arguments)]
fn act(
    config: &BrokerConfig,
    agent: &AgentClient,
    item: &DecisionItem,
    answer: &CoachAnswer,
    decision_id: &str,
    mission_id: &str,
    idempotency_key: &str,
    producer_epoch: u64,
    sequence: u64,
) -> (String, Option<MissionRow>, Vec<String>) {
    let now = now_ms();
    let mut lines = Vec::new();
    let held = vec![item.proposal.proposal_id.clone()];
    let decision = match envelope::decision_from_draft(
        &answer.draft,
        &config.broker_id,
        &held,
        decision_id,
        now,
    ) {
        Ok(decision) => decision,
        Err(refusal) => {
            let disposition = format!("refused: {refusal}");
            lines.push(format!("qualia-mission-broker: decision {decision_id} {disposition}"));
            return (disposition, None, lines);
        }
    };
    lines.push(format!(
        "qualia-mission-broker: coach decision decision_id={} kind={} targets={:?} outputs={:?}",
        decision.decision_id,
        kind_name(decision.decision_kind),
        decision.target_proposal_ids,
        decision.output_ids
    ));
    if let Some(reason) = &decision.reason {
        lines.push(format!("qualia-mission-broker: coach rationale \"{reason}\""));
    }

    if decision.decision_kind != CoachDecisionKind::Promote {
        let disposition = format!(
            "recorded ({} opens no mission)",
            kind_name(decision.decision_kind)
        );
        lines.push(format!(
            "qualia-mission-broker: decision {decision_id} {disposition}"
        ));
        return (disposition, None, lines);
    }

    // The repository's own promote rule: exactly this proposal, never a
    // planner advisory.
    match envelope::materialize(&item.proposal, &decision) {
        Ok(canonical) => lines.push(format!(
            "qualia-mission-broker: canonical materialized canonical_id={} source_proposal={}",
            canonical.canonical_id, item.proposal.proposal_id
        )),
        Err(refusal) => {
            let disposition = format!("refused: promote rejected by the canonical rule: {refusal}");
            lines.push(format!("qualia-mission-broker: decision {decision_id} {disposition}"));
            return (disposition, None, lines);
        }
    }

    let request = MissionRequest {
        broker_id: &config.broker_id,
        producer_epoch,
        sequence,
        mission_id,
        idempotency_key,
        fly_governed: config.fly_governed,
        area: &config.area,
        now_ms: now,
    };
    let mission = match envelope::mission_from_decision(&decision, &answer.draft, &item.proposal, &request)
    {
        Ok(mission) => mission,
        Err(refusal) => {
            let disposition = format!("refused: {refusal}");
            lines.push(format!("qualia-mission-broker: decision {decision_id} {disposition}"));
            return (disposition, None, lines);
        }
    };
    lines.push(format!(
        "qualia-mission-broker: mission envelope built mission_id={} command=start evidence_refs={} deadline_in_ms={} objective={}",
        mission.mission_id,
        mission.evidence_refs.len(),
        mission.deadline_ms - mission.issued_at_ms,
        mission.objective.summary
    ));

    let post = match agent.post_envelope(&mission) {
        Ok(post) => post,
        Err(error) => {
            let disposition = format!("post failed: {error}");
            lines.push(format!("qualia-mission-broker: decision {decision_id} {disposition}"));
            return (disposition, None, lines);
        }
    };
    lines.push(format!(
        "qualia-mission-broker: posted POST {}/mission-control/envelopes -> HTTP {} accepted={} idempotent_replay={}",
        agent.base(),
        post.http_status,
        post.accepted,
        post.idempotent_replay
    ));
    if !post.accepted {
        let disposition = format!("agent refused: {}", post.refusal());
        lines.push(format!("qualia-mission-broker: decision {decision_id} {disposition}"));
        return (disposition, None, lines);
    }

    let mut detail = format!("HTTP {} accepted", post.http_status);
    match agent.missions() {
        Ok(body) => match agent::mission_summary(&body, mission_id) {
            Some(summary) => {
                lines.push(format!(
                    "qualia-mission-broker: read back GET /mission-control/missions -> mission {} status={} stage={} code={} detail={}",
                    summary.mission_id, summary.status, summary.stage, summary.code, summary.detail
                ));
                detail = format!("HTTP {} accepted; {} {}", post.http_status, summary.status, summary.stage);
            }
            None => lines.push(format!(
                "qualia-mission-broker: read back GET /mission-control/missions did not list {mission_id}"
            )),
        },
        Err(error) => lines.push(format!(
            "qualia-mission-broker: read back GET /mission-control/missions failed: {error}"
        )),
    }
    match agent.events(0) {
        Ok(body) => {
            for event in agent::event_summaries(&body)
                .into_iter()
                .filter(|event| event.mission_id == mission_id)
            {
                lines.push(format!(
                    "qualia-mission-broker: read back GET /mission-control/events -> seq={} kind={} status={} code={} detail={}",
                    event.sequence, event.event_kind, event.status, event.code, event.detail
                ));
            }
        }
        Err(error) => lines.push(format!(
            "qualia-mission-broker: read back GET /mission-control/events failed: {error}"
        )),
    }

    (
        "posted".to_string(),
        Some(MissionRow {
            mission_id: mission_id.to_string(),
            idempotency_key: idempotency_key.to_string(),
            command: "start".to_string(),
            accepted: true,
            ack_http_status: Some(post.http_status),
            detail,
        }),
        lines,
    )
}

/// The model's state before the first request.
fn model_state(config: &BrokerConfig) -> ModelState {
    let configured = config.coach.configured();
    ModelState {
        configured,
        model_id: config.coach.model.clone(),
        base_url: config.coach.base_url.clone(),
        key_presence: config.coach.key_presence(),
        status: if configured { "ok" } else { "no_key" }.to_string(),
        reason: if configured {
            None
        } else {
            Some("no_key".to_string())
        },
        last_latency_ms: None,
        last_error: None,
    }
}

/// The wire name of a decision kind, as the status surface reports it.
fn kind_name(kind: CoachDecisionKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{kind:?}"))
}

fn sleep(interval: Duration) {
    if !interval.is_zero() {
        std::thread::sleep(interval);
    }
}
