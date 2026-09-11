//! The mission broker: bounded missions in, decisions and an audit trail out.
//!
//! A mission arrives as a `qualia-sync-types` envelope from the mission broker
//! (or from an operator tool speaking the same wire contract), is validated
//! against its own bounds, and from then on lives in a small state machine with
//! a journal line per transition. Nothing here moves the robot: the motion
//! authority is Leash's, reached through the endpoint `QUALIA_LEASH_BASE_URL`
//! names, and that forwarding path is the Leash ticket's to land.
//!
//! What this build can reach on its own is the part that needs no evidence: a
//! mission is accepted, parks until referenced evidence exists, can be paused,
//! and can be cancelled or time out with a verified-zero stop because it never
//! entered motion. Each of those publishes a braid mission event, so the
//! operator page's mission count is the broker's count.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use qualia_braid::BraidEvent;
use qualia_sync_types::{
    MissionCommandV1, MissionEnvelopeV1, MissionEventKindV1, MissionEventV1, MissionStatusV1,
    JSON_SAFE_INTEGER_MAX, MISSION_EVENT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::auth::AuthScope;
use crate::config::{AgentConfig, LeashEndpoint, MissionBrokerEndpoint};
use crate::{now_ms, AppState};

/// State envelope schema for `/mission-control/missions`.
pub const MISSION_STATE_SCHEMA_VERSION: &str = "qualia.mission-control-state.v1";
/// Journal record schema.
pub const MISSION_JOURNAL_SCHEMA_VERSION: &str = "qualia.mission-control-journal.v1";
/// Acknowledgement schema for `/mission-control/envelopes`.
pub const MISSION_ACK_SCHEMA_VERSION: &str = "qualia.mission-ack.v1";
/// How many outbound events are retained.
pub const MAX_EVENTS: usize = 1_024;
/// How many missions are retained.
pub const MAX_MISSIONS: usize = 256;
/// An operator authorization lasts at most this long.
pub const MAX_OPERATOR_LEASE_SECS: u64 = 30;
/// How often the supervisor looks at the missions it owns.
const SUPERVISOR_INTERVAL_MS: u64 = 200;

/// Where a mission is in its life.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionStage {
    Planning,
    AwaitingEvidence,
    AwaitingOperatorLease,
    Active,
    PausedByBroker,
    Stopping,
    Terminal,
}

/// The evidence-grounded plan a mission must have before it may be authorized.
///
/// The spatial world model proposes it and Leash executes it; this build holds
/// the type so the record's shape is stable, and no mission reaches the stage
/// where one exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionPlanV1 {
    pub schema_version: String,
    pub plan_id: String,
    pub mission_id: String,
    pub evidence_id: String,
    pub frame_id: String,
    pub target_x_m: f32,
    pub target_y_m: f32,
    pub tolerance_m: f32,
    pub speed_ceiling_mps: f32,
    pub operating_area: qualia_sync_types::MissionOperatingAreaV1,
    pub created_at_ms: u128,
    pub expires_at_ms: u128,
    pub safety_authority: String,
}

/// The accepted mission, as `/mission-control/missions` reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionRecordV1 {
    pub envelope: MissionEnvelopeV1,
    pub status: MissionStatusV1,
    pub stage: MissionStage,
    pub accepted_at_ms: u128,
    pub updated_at_ms: u128,
    pub plan: Option<MissionPlanV1>,
    pub operator_lease_expires_at_ms: Option<u128>,
    pub replans: u8,
    pub cancel_requested: bool,
    pub pause_requested: bool,
    pub last_code: String,
    pub last_detail: String,
}

impl MissionRecordV1 {
    fn new(envelope: MissionEnvelopeV1, now: u128) -> Self {
        Self {
            envelope,
            status: MissionStatusV1::Queued,
            stage: MissionStage::Planning,
            accepted_at_ms: now,
            updated_at_ms: now,
            plan: None,
            operator_lease_expires_at_ms: None,
            replans: 0,
            cancel_requested: false,
            pause_requested: false,
            last_code: "accepted".to_string(),
            last_detail: "bounded mission was durably accepted".to_string(),
        }
    }
}

/// One event waiting to be read by whoever follows the mission stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutboundMissionEvent {
    event: MissionEventV1,
    delivered: bool,
    attempts: u32,
    next_attempt_at_ms: u128,
}

/// The journalled broker state.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MissionControlStateV1 {
    schema_version: String,
    producer_epoch: u64,
    next_event_sequence: u64,
    broker_producer_epoch: Option<u64>,
    broker_sequence: u64,
    deliveries: BTreeMap<String, MissionEnvelopeV1>,
    missions: BTreeMap<String, MissionRecordV1>,
    events: Vec<OutboundMissionEvent>,
}

impl MissionControlStateV1 {
    fn new() -> Self {
        Self {
            schema_version: MISSION_STATE_SCHEMA_VERSION.to_string(),
            producer_epoch: epoch_seed(),
            next_event_sequence: 0,
            broker_producer_epoch: None,
            broker_sequence: 0,
            deliveries: BTreeMap::new(),
            missions: BTreeMap::new(),
            events: Vec::new(),
        }
    }
}

/// One line of the journal.
#[derive(Debug, Serialize, Deserialize)]
struct MissionJournalRecord {
    schema_version: String,
    state_sha256: String,
    state: MissionControlStateV1,
}

/// The broker runtime, shared by every handler and the supervisor.
#[derive(Clone)]
pub struct MissionControlRuntime {
    inner: Arc<Mutex<MissionControlStateV1>>,
    journal_path: Arc<PathBuf>,
    persistence_error: Arc<Mutex<Option<String>>>,
    notify: Arc<Notify>,
    shutting_down: Arc<AtomicBool>,
    /// The braid every mission opening and closing is reported to.
    braid: crate::braid::BraidRuntime,
    /// Configured, but its client belongs to the Leash-forwarding ticket.
    broker: Option<Arc<MissionBrokerEndpoint>>,
    leash: Option<Arc<LeashEndpoint>>,
}

impl MissionControlRuntime {
    /// Open the journal, or start a fresh broker state when there is none.
    pub fn from_config(config: &AgentConfig, braid: crate::braid::BraidRuntime) -> Self {
        let journal_path = PathBuf::from(&config.mission_journal);
        let (mut state, mut error) = match load_journal(&journal_path) {
            Ok(Some(state)) => (state, None),
            Ok(None) => (MissionControlStateV1::new(), None),
            Err(load_error) => (MissionControlStateV1::new(), Some(load_error)),
        };
        if error.is_none() && migrate_unsafe_event_epoch(&mut state) {
            if let Err(migration_error) = append_journal(&journal_path, &state) {
                error = Some(format!(
                    "persist JSON-safe mission event epoch migration: {migration_error}"
                ));
            } else {
                eprintln!("qualia-agent: migrated mission event ordering to a JSON-safe producer epoch");
            }
        }
        Self {
            inner: Arc::new(Mutex::new(state)),
            journal_path: Arc::new(journal_path),
            persistence_error: Arc::new(Mutex::new(error)),
            notify: Arc::new(Notify::new()),
            shutting_down: Arc::new(AtomicBool::new(false)),
            braid,
            broker: config.mission_broker.clone().map(Arc::new),
            leash: config.leash.clone().map(Arc::new),
        }
    }

    /// Whether a mission broker was configured to pull missions from.
    pub fn broker_configured(&self) -> bool {
        self.broker.is_some()
    }

    /// Whether a Leash endpoint was configured for bounded motion.
    pub fn leash_configured(&self) -> bool {
        self.leash.is_some()
    }

    /// The mission list as `/mission-control/missions` reports it.
    pub fn missions_envelope(&self) -> serde_json::Value {
        let runtime = self.inner.lock().expect("mission control state lock");
        let mut missions = runtime.missions.values().cloned().collect::<Vec<_>>();
        missions.sort_by(|left, right| right.accepted_at_ms.cmp(&left.accepted_at_ms));
        json!({
            "schema_version": MISSION_STATE_SCHEMA_VERSION,
            "producer_epoch": runtime.producer_epoch,
            "broker_producer_epoch": runtime.broker_producer_epoch,
            "broker_sequence": runtime.broker_sequence,
            "missions": missions,
        })
    }

    /// The event stream as `/mission-control/events` reports it, from
    /// `after_sequence` inclusive of everything newer.
    pub fn events_envelope(&self, after_sequence: u64) -> serde_json::Value {
        let runtime = self.inner.lock().expect("mission control state lock");
        let events = runtime
            .events
            .iter()
            .filter(|record| record.event.sequence > after_sequence)
            .take(256)
            .map(|record| record.event.clone())
            .collect::<Vec<_>>();
        json!({
            "schema_version": MISSION_EVENT_SCHEMA_VERSION,
            "producer_epoch": runtime.producer_epoch,
            "latest_sequence": runtime.next_event_sequence,
            "events": events,
        })
    }

    fn healthy(&self) -> Result<(), String> {
        self.persistence_error
            .lock()
            .expect("mission persistence error lock")
            .clone()
            .map_or(Ok(()), Err)
    }

    /// Append the current state to the journal, rolling back on failure so the
    /// in-memory view never claims something the journal does not have.
    fn commit(&self, previous: MissionControlStateV1) -> Result<(), String> {
        let state = self.inner.lock().expect("mission control state lock");
        if let Err(error) = append_journal(&self.journal_path, &state) {
            drop(state);
            *self.inner.lock().expect("mission control state lock") = previous;
            *self
                .persistence_error
                .lock()
                .expect("mission persistence error lock") = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    /// Validate and apply one envelope. `true` in the first slot means the
    /// delivery was an idempotent replay.
    fn ingest(&self, envelope: MissionEnvelopeV1) -> Result<(bool, MissionRecordV1), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("mission control is shutting down".to_string());
        }
        self.healthy()?;
        envelope.validate()?;
        let now = now_ms();
        if envelope.deadline_ms <= now {
            return Err("mission envelope is already expired".to_string());
        }
        let mut state = self.inner.lock().expect("mission control state lock");
        if let Some(existing) = state.deliveries.get(&envelope.idempotency_key) {
            if existing != &envelope {
                return Err(
                    "mission idempotency key was reused for a different envelope".to_string(),
                );
            }
            let mission = state
                .missions
                .get(&envelope.mission_id)
                .cloned()
                .ok_or_else(|| "mission delivery index is inconsistent".to_string())?;
            return Ok((true, mission));
        }
        validate_broker_order(&state, &envelope)?;
        let previous = state.clone();
        let mission_id = envelope.mission_id.clone();
        let mut opened = false;
        match envelope.command {
            MissionCommandV1::Start => {
                if state.missions.contains_key(&mission_id) {
                    return Err("mission_id already exists with a different delivery".to_string());
                }
                state
                    .missions
                    .insert(mission_id.clone(), MissionRecordV1::new(envelope.clone(), now));
                emit_event(
                    &mut state,
                    &mission_id,
                    MissionEventKindV1::Accepted,
                    MissionStatusV1::Queued,
                    "accepted",
                    "bounded mission was durably accepted",
                )?;
                opened = true;
            }
            MissionCommandV1::Pause => {
                let mission = state
                    .missions
                    .get_mut(&mission_id)
                    .ok_or_else(|| "cannot pause an unknown mission".to_string())?;
                if mission.status.terminal() {
                    return Err("cannot pause a terminal mission".to_string());
                }
                mission.pause_requested = true;
                mission.updated_at_ms = now;
            }
            MissionCommandV1::Resume => {
                let mission = state
                    .missions
                    .get_mut(&mission_id)
                    .ok_or_else(|| "cannot resume an unknown mission".to_string())?;
                if mission.status != MissionStatusV1::Paused {
                    return Err("only a paused mission can resume".to_string());
                }
                mission.envelope = envelope.clone();
                mission.stage = MissionStage::Planning;
                mission.status = MissionStatusV1::Queued;
                mission.pause_requested = false;
                mission.operator_lease_expires_at_ms = None;
                mission.updated_at_ms = now;
            }
            MissionCommandV1::Cancel => {
                let mission = state
                    .missions
                    .get_mut(&mission_id)
                    .ok_or_else(|| "cannot cancel an unknown mission".to_string())?;
                if !mission.status.terminal() {
                    mission.cancel_requested = true;
                    mission.updated_at_ms = now;
                }
            }
        }
        state.broker_producer_epoch = Some(envelope.producer_epoch);
        state.broker_sequence = envelope.sequence;
        state
            .deliveries
            .insert(envelope.idempotency_key.clone(), envelope);
        prune_state(&mut state);
        let view = state
            .missions
            .get(&mission_id)
            .cloned()
            .expect("mission exists after delivery");
        drop(state);
        self.commit(previous)?;
        if opened {
            // The accepted mission is the run's session: the arena recorder
            // names its MCAP segment and the session store records it under
            // this mission id. Binding it is what makes the console's Mission
            // view and the watch Braid line name the run the operator opened
            // instead of an empty session.
            self.braid.bind_session(mission_id.clone());
            self.braid.observe(BraidEvent::MissionOpened {
                mission_id: mission_id.clone(),
            });
        }
        self.notify.notify_one();
        Ok((false, view))
    }

    /// Apply one mutation under the lint of a journalled transition.
    fn update(
        &self,
        mission_id: &str,
        change: impl FnOnce(&mut MissionControlStateV1) -> Result<(), String>,
    ) -> Result<MissionRecordV1, String> {
        self.healthy()?;
        let mut state = self.inner.lock().expect("mission control state lock");
        if !state.missions.contains_key(mission_id) {
            return Err("mission not found".to_string());
        }
        let previous = state.clone();
        change(&mut state)?;
        let view = state
            .missions
            .get(mission_id)
            .cloned()
            .ok_or_else(|| "mission disappeared during update".to_string())?;
        drop(state);
        self.commit(previous)?;
        self.notify.notify_one();
        Ok(view)
    }
}

/// A request to grant an operator authorization.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorLeaseRequestV1 {
    ttl_secs: u64,
}

#[derive(Debug, Deserialize, Default)]
pub struct MissionEventsQuery {
    after_sequence: Option<u64>,
}

/// `POST /mission-control/envelopes` — one delivery from the broker.
pub async fn envelope_post(
    ConnectInfo(remote): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(envelope): Json<MissionEnvelopeV1>,
) -> Response {
    if let Err(response) = state
        .auth
        .authorize(AuthScope::MissionBroker, &headers, remote.ip())
    {
        return response;
    }
    let mission_id = envelope.mission_id.clone();
    match state.mission_control.ingest(envelope) {
        Ok((replay, mission)) => (
            if replay {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            Json(json!({
                "schema_version": MISSION_ACK_SCHEMA_VERSION,
                "accepted": true,
                "idempotent_replay": replay,
                "mission": mission,
            })),
        )
            .into_response(),
        Err(error) if error.contains("idempotency") || error.contains("already exists") => {
            mission_error(StatusCode::CONFLICT, &mission_id, &error)
        }
        Err(error) => mission_error(StatusCode::BAD_REQUEST, &mission_id, &error),
    }
}

/// `GET /mission-control/missions` — every mission the broker holds.
pub async fn missions_get(
    ConnectInfo(remote): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = state.auth.authorize(AuthScope::Read, &headers, remote.ip()) {
        return response;
    }
    Json(state.mission_control.missions_envelope()).into_response()
}

/// `GET /mission-control/events` — the audit trail, newest last.
pub async fn events_get(
    ConnectInfo(remote): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MissionEventsQuery>,
) -> Response {
    if let Err(response) = state.auth.authorize(AuthScope::Read, &headers, remote.ip()) {
        return response;
    }
    let after = query.after_sequence.unwrap_or(0);
    Json(state.mission_control.events_envelope(after)).into_response()
}

/// `POST /mission-control/missions/{id}/authorize` — grant bounded authority.
pub async fn authorize_post(
    ConnectInfo(remote): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(mission_id): Path<String>,
    Json(request): Json<OperatorLeaseRequestV1>,
) -> Response {
    if let Err(response) = state.auth.authorize(AuthScope::Admin, &headers, remote.ip()) {
        return response;
    }
    if state.mission_control.shutting_down.load(Ordering::Acquire) {
        return mission_error(
            StatusCode::SERVICE_UNAVAILABLE,
            &mission_id,
            "mission control is shutting down",
        );
    }
    match grant_operator_lease(&state, &mission_id, request.ttl_secs) {
        Ok(mission) => (StatusCode::ACCEPTED, Json(mission)).into_response(),
        Err(error) => mission_error(StatusCode::CONFLICT, &mission_id, &error),
    }
}

/// Grant a bounded operator authorization for a mission that has a plan.
///
/// The plan is the evidence-grounded one the spatial world model proposes, and
/// handing it to the motion authority is Leash's. Neither has landed here, so
/// a mission can never be in the authorizable stage and this reports exactly
/// that instead of pretending authority was granted.
fn grant_operator_lease(
    state: &AppState,
    mission_id: &str,
    requested_ttl_secs: u64,
) -> Result<MissionRecordV1, String> {
    if requested_ttl_secs == 0 || requested_ttl_secs > MAX_OPERATOR_LEASE_SECS {
        return Err(format!(
            "operator lease ttl_secs must be 1..={MAX_OPERATOR_LEASE_SECS}"
        ));
    }
    let mission = state
        .mission_control
        .inner
        .lock()
        .expect("mission control state lock")
        .missions
        .get(mission_id)
        .cloned()
        .ok_or_else(|| "mission not found".to_string())?;
    if mission.stage != MissionStage::AwaitingOperatorLease
        || mission.status != MissionStatusV1::Paused
    {
        return Err("mission is not awaiting operator authorization".to_string());
    }
    let _plan = mission
        .plan
        .clone()
        .ok_or_else(|| "mission has no evidence-grounded plan".to_string())?;
    Err("Leash mission transport is not configured".to_string())
}

/// Start the supervisor loop, and the broker adapter when one is configured.
pub fn start(state: AppState) {
    let supervisor = state.clone();
    tokio::spawn(async move {
        loop {
            if supervisor.mission_control.shutting_down.load(Ordering::Acquire) {
                return;
            }
            supervise_once(&supervisor).await;
            tokio::select! {
                _ = supervisor.mission_control.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(SUPERVISOR_INTERVAL_MS)) => {}
            }
        }
    });
}

/// One supervision pass over every mission the broker still owns.
pub async fn supervise_once(state: &AppState) {
    if state.mission_control.shutting_down.load(Ordering::Acquire) {
        return;
    }
    let mission_ids: Vec<String> = {
        let runtime = state
            .mission_control
            .inner
            .lock()
            .expect("mission control state lock");
        runtime
            .missions
            .iter()
            .filter(|(_, mission)| !mission.status.terminal())
            .map(|(mission_id, _)| mission_id.clone())
            .collect()
    };
    for mission_id in mission_ids {
        let _ = supervise_mission(state, &mission_id).await;
    }
}

async fn supervise_mission(state: &AppState, mission_id: &str) -> Result<(), String> {
    let mission = {
        let runtime = state
            .mission_control
            .inner
            .lock()
            .expect("mission control state lock");
        runtime.missions.get(mission_id).cloned()
    };
    let Some(mission) = mission else {
        return Ok(());
    };
    if mission.status.terminal() {
        return Ok(());
    }
    let now = now_ms();
    if mission.cancel_requested {
        return state.mission_control.finish(
            mission_id,
            MissionStatusV1::Cancelled,
            MissionEventKindV1::Cancelled,
            "cancelled",
            "broker cancelled the mission",
        )
        .await;
    }
    if mission.pause_requested {
        return state.mission_control.finish(
            mission_id,
            MissionStatusV1::Paused,
            MissionEventKindV1::Paused,
            "paused",
            "broker paused the mission",
        )
        .await;
    }
    if now >= mission.envelope.deadline_ms
        || now.saturating_sub(mission.accepted_at_ms)
            >= mission.envelope.constraints.max_runtime_ms as u128
    {
        return state.mission_control.finish(
            mission_id,
            MissionStatusV1::Failed,
            MissionEventKindV1::Failed,
            "deadline_exceeded",
            "mission deadline or runtime bound expired",
        )
        .await;
    }
    match mission.stage {
        MissionStage::Planning | MissionStage::AwaitingEvidence => {
            if mission.replans > mission.envelope.constraints.max_replans {
                return state.mission_control.finish(
                    mission_id,
                    MissionStatusV1::Failed,
                    MissionEventKindV1::Failed,
                    "replan_limit_exceeded",
                    "mission exhausted its bounded replan allowance",
                )
                .await;
            }
            // The evidence-grounded plan needs the spatial world model, which
            // this build does not carry. The documented wait is the honest
            // place for the mission to sit until it does.
            if mission.stage != MissionStage::AwaitingEvidence {
                state.mission_control.update(mission_id, |runtime| {
                    let mission = runtime
                        .missions
                        .get_mut(mission_id)
                        .expect("mission exists");
                    mission.stage = MissionStage::AwaitingEvidence;
                    mission.status = MissionStatusV1::Paused;
                    mission.last_code = "awaiting_fresh_evidence".to_string();
                    mission.last_detail =
                        "mission is paused until referenced fresh spatial evidence exists"
                            .to_string();
                    mission.updated_at_ms = now_ms();
                    emit_event(
                        runtime,
                        mission_id,
                        MissionEventKindV1::Paused,
                        MissionStatusV1::Paused,
                        "awaiting_fresh_evidence",
                        "mission is paused until referenced fresh spatial evidence exists",
                    )
                })?;
            }
        }
        MissionStage::AwaitingOperatorLease | MissionStage::PausedByBroker => {}
        MissionStage::Active => {
            if mission
                .operator_lease_expires_at_ms
                .is_none_or(|expires_at| now >= expires_at)
            {
                return state.mission_control.finish(
                    mission_id,
                    MissionStatusV1::Failed,
                    MissionEventKindV1::Failed,
                    "operator_lease_expired",
                    "operator authorization lease expired",
                )
                .await;
            }
        }
        MissionStage::Stopping | MissionStage::Terminal => {}
    }
    Ok(())
}

/// End a mission and publish the events the ending earned.
///
/// A mission that never entered motion needs no transport to stop: the broker
/// can assert zero output itself, which is what the operator sees as
/// `stop_verified`. A mission that *did* enter motion needs Leash to confirm it,
/// and without a Leash endpoint the ending is reported as a failure instead of
/// being claimed.
impl MissionControlRuntime {
    async fn finish(
        &self,
        mission_id: &str,
        requested_status: MissionStatusV1,
        requested_kind: MissionEventKindV1,
        code: &str,
        detail: &str,
    ) -> Result<(), String> {
        let was_active = {
            self.inner
                .lock()
                .expect("mission control state lock")
                .missions
                .get(mission_id)
                .is_some_and(|mission| mission.stage == MissionStage::Active)
        };
        if was_active {
            let _ = self.update(mission_id, |runtime| {
                let mission = runtime
                    .missions
                    .get_mut(mission_id)
                    .expect("mission exists");
                mission.stage = MissionStage::Stopping;
                mission.updated_at_ms = now_ms();
                Ok(())
            });
            let closed = self.update(mission_id, |runtime| {
                let detail =
                    format!("{detail}; verified stop failed: Leash stop transport is not configured");
                let mission = runtime
                    .missions
                    .get_mut(mission_id)
                    .expect("mission exists");
                mission.status = MissionStatusV1::Failed;
                mission.stage = MissionStage::Terminal;
                mission.last_code = "stop_transport_failure".to_string();
                mission.last_detail = detail.clone();
                mission.updated_at_ms = now_ms();
                emit_event(
                    runtime,
                    mission_id,
                    MissionEventKindV1::Failed,
                    MissionStatusV1::Failed,
                    "stop_transport_failure",
                    &detail,
                )
            })?;
            let _ = closed;
            self.report_closed(mission_id, MissionStatusV1::Failed);
            return Ok(());
        }
        let view = self.update(mission_id, |runtime| {
            let mission = runtime
                .missions
                .get_mut(mission_id)
                .expect("mission exists");
            mission.status = requested_status;
            mission.stage = if requested_status.terminal() {
                MissionStage::Terminal
            } else {
                MissionStage::PausedByBroker
            };
            mission.operator_lease_expires_at_ms = None;
            mission.cancel_requested = false;
            mission.pause_requested = false;
            if requested_status == MissionStatusV1::Paused {
                mission.replans = mission.replans.saturating_add(1);
                mission.plan = None;
            }
            mission.last_code = code.to_string();
            mission.last_detail = detail.to_string();
            mission.updated_at_ms = now_ms();
            emit_event(
                runtime,
                mission_id,
                MissionEventKindV1::StopVerified,
                requested_status,
                "stop_verified",
                "mission never entered active motion",
            )?;
            emit_event(
                runtime,
                mission_id,
                requested_kind,
                requested_status,
                code,
                detail,
            )
        })?;
        if view.status.terminal() {
            self.report_closed(mission_id, view.status);
        }
        Ok(())
    }

    /// Report a mission closing to the braid once its record is terminal.
    fn report_closed(&self, mission_id: &str, status: MissionStatusV1) {
        self.braid.observe(BraidEvent::MissionClosed {
            mission_id: mission_id.to_string(),
            outcome: braid_outcome(status),
        });
    }
}

/// Stop every mission the broker owns, for a clean shutdown.
pub async fn shutdown(state: &AppState) -> Result<(), String> {
    state
        .mission_control
        .shutting_down
        .store(true, Ordering::Release);
    state.mission_control.notify.notify_waiters();
    let mission_ids: Vec<String> = {
        let runtime = state
            .mission_control
            .inner
            .lock()
            .expect("mission control state lock");
        runtime
            .missions
            .iter()
            .filter(|(_, mission)| !mission.status.terminal())
            .map(|(mission_id, _)| mission_id.clone())
            .collect()
    };
    let mut failures = Vec::new();
    for mission_id in mission_ids {
        if let Err(error) = state
            .mission_control
            .finish(
                &mission_id,
                MissionStatusV1::Cancelled,
                MissionEventKindV1::Cancelled,
                "cancelled",
                "agent shutting down",
            )
            .await
        {
            failures.push(format!("{mission_id}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Record one outbound event, numbering it in this producer's epoch.
fn emit_event(
    state: &mut MissionControlStateV1,
    mission_id: &str,
    event_kind: MissionEventKindV1,
    status: MissionStatusV1,
    code: &str,
    detail: &str,
) -> Result<(), String> {
    let mission = state
        .missions
        .get(mission_id)
        .ok_or_else(|| "cannot emit an event for an unknown mission".to_string())?;
    state.next_event_sequence = state.next_event_sequence.saturating_add(1);
    let sequence = state.next_event_sequence;
    let occurred_at_ms = now_ms();
    let event = MissionEventV1 {
        schema_version: MISSION_EVENT_SCHEMA_VERSION.to_string(),
        producer_epoch: state.producer_epoch,
        sequence,
        event_id: format!("{}-{sequence}", state.producer_epoch),
        mission_id: mission_id.to_string(),
        mission_idempotency_key: mission.envelope.idempotency_key.clone(),
        event_kind,
        status,
        occurred_at_ms,
        code: code.to_string(),
        detail: detail.to_string(),
        evidence_refs: mission.envelope.evidence_refs.clone(),
        plan_id: None,
    };
    event.validate()?;
    state.events.push(OutboundMissionEvent {
        event,
        delivered: false,
        attempts: 0,
        next_attempt_at_ms: occurred_at_ms,
    });
    Ok(())
}

/// A broker delivers strictly in order: sequence 1 opens an epoch, and every
/// later delivery in the same epoch is exactly one ahead.
fn validate_broker_order(
    state: &MissionControlStateV1,
    envelope: &MissionEnvelopeV1,
) -> Result<(), String> {
    match state.broker_producer_epoch {
        None => {
            if envelope.sequence != 1 {
                return Err("first broker delivery must start at sequence 1".to_string());
            }
        }
        Some(epoch) if epoch == envelope.producer_epoch => {
            if envelope.sequence != state.broker_sequence.saturating_add(1) {
                return Err(format!(
                    "broker delivery sequence gap: expected {}, received {}",
                    state.broker_sequence.saturating_add(1),
                    envelope.sequence
                ));
            }
        }
        Some(_) => {
            if envelope.sequence != 1 {
                return Err("new broker epoch must restart at sequence 1".to_string());
            }
        }
    }
    Ok(())
}

/// Keep the retained state bounded: delivered events go first, then the oldest
/// terminal missions.
fn prune_state(state: &mut MissionControlStateV1) {
    if state.events.len() > MAX_EVENTS {
        let excess = state.events.len() - MAX_EVENTS;
        let mut removed = 0;
        state.events.retain(|record| {
            if removed < excess && record.delivered {
                removed += 1;
                false
            } else {
                true
            }
        });
        if state.events.len() > MAX_EVENTS {
            let excess = state.events.len() - MAX_EVENTS;
            state.events.drain(0..excess);
        }
    }
    if state.missions.len() > MAX_MISSIONS {
        let mut terminal: Vec<String> = state
            .missions
            .iter()
            .filter(|(_, mission)| mission.status.terminal())
            .map(|(mission_id, _)| mission_id.clone())
            .collect();
        terminal.sort_by_key(|mission_id| {
            state
                .missions
                .get(mission_id)
                .map(|mission| mission.accepted_at_ms)
                .unwrap_or(0)
        });
        let excess = state.missions.len() - MAX_MISSIONS;
        for mission_id in terminal.into_iter().take(excess) {
            state.missions.remove(&mission_id);
        }
    }
}

fn append_journal(path: &PathBuf, state: &MissionControlStateV1) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create mission journal dir: {error}"))?;
        }
    }
    let record = MissionJournalRecord {
        schema_version: MISSION_JOURNAL_SCHEMA_VERSION.to_string(),
        state_sha256: mission_state_digest(state)?,
        state: state.clone(),
    };
    let line = serde_json::to_string(&record)
        .map_err(|error| format!("encode mission journal record: {error}"))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open mission journal {}: {error}", path.display()))?;
    writeln!(file, "{line}").map_err(|error| format!("append mission journal: {error}"))
}

fn load_journal(path: &PathBuf) -> Result<Option<MissionControlStateV1>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read mission journal {}: {error}", path.display())),
    };
    let Some(line) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
        return Ok(None);
    };
    let record: MissionJournalRecord = serde_json::from_str(line)
        .map_err(|error| format!("decode mission journal {}: {error}", path.display()))?;
    if record.schema_version != MISSION_JOURNAL_SCHEMA_VERSION {
        return Err(format!(
            "mission journal schema {} is not supported",
            record.schema_version
        ));
    }
    if record.state_sha256 != mission_state_digest(&record.state)? {
        return Err("mission journal digest does not match its state".to_string());
    }
    Ok(Some(record.state))
}

fn mission_state_digest(state: &MissionControlStateV1) -> Result<String, String> {
    let canonical = serde_json::to_value(state)
        .map_err(|error| format!("canonicalize mission state: {error}"))?;
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| format!("encode canonical mission state: {error}"))?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write;
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

/// A producer epoch that fits a JSON number exactly and still orders restarts.
fn epoch_seed() -> u64 {
    let now = now_ms().min(JSON_SAFE_INTEGER_MAX as u128) as u64;
    let time_bits = now & (JSON_SAFE_INTEGER_MAX >> 12);
    ((time_bits << 12) ^ std::process::id() as u64).max(1)
}

/// Bring a journal from a build that numbered events with a raw clock back into
/// the JSON-safe ordering, so a browser cannot round two sequences together.
fn migrate_unsafe_event_epoch(state: &mut MissionControlStateV1) -> bool {
    let unsafe_ordering = state.producer_epoch == 0
        || state.producer_epoch > JSON_SAFE_INTEGER_MAX
        || state.next_event_sequence > JSON_SAFE_INTEGER_MAX
        || state.events.iter().any(|record| {
            record.event.producer_epoch == 0
                || record.event.producer_epoch > JSON_SAFE_INTEGER_MAX
                || record.event.sequence == 0
                || record.event.sequence > JSON_SAFE_INTEGER_MAX
        });
    if !unsafe_ordering {
        return false;
    }
    let producer_epoch = epoch_seed();
    let now = now_ms();
    state.events.retain(|record| !record.delivered);
    for (index, record) in state.events.iter_mut().enumerate() {
        let sequence = index as u64 + 1;
        record.event.producer_epoch = producer_epoch;
        record.event.sequence = sequence;
        record.event.event_id = format!("{producer_epoch}-{sequence}");
        record.attempts = 0;
        record.next_attempt_at_ms = now;
    }
    state.producer_epoch = producer_epoch;
    state.next_event_sequence = state.events.len() as u64;
    true
}

fn mission_error(status: StatusCode, mission_id: &str, error: &str) -> Response {
    (
        status,
        Json(json!({ "error": format!("mission {mission_id}: {error}") })),
    )
        .into_response()
}

/// The outcome string the braid records when a mission closes.
pub fn braid_outcome(status: MissionStatusV1) -> String {
    match status {
        MissionStatusV1::Queued => "queued",
        MissionStatusV1::Running => "running",
        MissionStatusV1::Paused => "paused",
        MissionStatusV1::Completed => "completed",
        MissionStatusV1::Failed => "failed",
        MissionStatusV1::Cancelled => "cancelled",
    }
    .to_string()
}
