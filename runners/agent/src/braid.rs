//! The braid view: one state machine every strand reports through.
//!
//! The state, the event vocabulary and the fold are `qualia_braid`'s; this
//! module is the agent's edge of it. It holds the one `BraidState` this process
//! owns, binds it to the run, and serves the two routes that reach it:
//!
//! - `GET /braid` is the read the console and the operator page poll.
//! - `POST /braid` is where a strand that lives in another process reports. The
//!   evidence recorder hands over `BraidEvent::EvidenceSealed` when it seals a
//!   segment, and the JEPA runtime hands over
//!   `BraidEvent::PromotionAccepted` / `PromotionRolledBack` when the generation
//!   pointer moves. The body is the `BraidEvent` JSON the braid crate fixes
//!   (`{"event": …}`), and the reply is the view the report folded into.
//!
//! The mission strand runs in this process, so it calls
//! [`BraidRuntime::observe`] directly instead of going through HTTP; the count
//! the operator page shows is the braid's, not a second copy the broker keeps.
//!
//! A strand this build has never heard of — one that ships ahead of the braid,
//! or a runtime that started before this wiring — does not break the reader: its
//! event decodes to `BraidEvent::Unknown`, the fold leaves the state alone, and
//! `GET /braid` keeps answering with the state the agent does know.
//!
//! The crate's vocabulary carries one more variant than a strand reports:
//! `BraidEvent::Quarantined`, whose fold renames `*.partial` files under the
//! path the caller names. That dispatch is recovery's, over a local root, so
//! this edge refuses the variant before the fold runs; a refusal moves no state
//! and touches no file.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use qualia_braid::{BraidError, BraidEvent, BraidState};
use serde::{Deserialize, Serialize};

use crate::auth::AuthScope;
use crate::AppState;

/// The schema literal `GET /braid` carries; the console and the operator page
/// both name it.
pub const BRAID_STATE_SCHEMA: &str = "qualia.braid-state.v1";

/// The braid view as the console, the web page and the fixture all read it.
///
/// It is the state under the schema tag the readers key on, field for field:
/// the console renders these names, so the view is not the crate's `BraidState`
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BraidView {
    pub schema_version: String,
    pub generation: u64,
    pub session_id: String,
    pub open_missions: u32,
    pub last_promotion_ns: u64,
    pub last_quarantine_ns: Option<u64>,
}

impl BraidView {
    fn of(state: &BraidState) -> Self {
        Self {
            schema_version: BRAID_STATE_SCHEMA.to_string(),
            generation: state.generation,
            session_id: state.session_id.clone(),
            open_missions: state.open_missions,
            last_promotion_ns: state.last_promotion_ns,
            last_quarantine_ns: state.last_quarantine_ns,
        }
    }
}

/// The braid, as this process holds it.
#[derive(Clone)]
pub struct BraidRuntime {
    state: Arc<Mutex<BraidState>>,
}

impl Default for BraidRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl BraidRuntime {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(BraidState::default())),
        }
    }

    /// Fold one event in, answering with the view it produced.
    ///
    /// The fold's one fallible dispatch — moving a partial out of the live
    /// namespace — is returned to the reporter, which owns the retry; the view
    /// is the one before the report, because a dispatch that could not run moves
    /// nothing.
    pub fn report(&self, event: BraidEvent) -> Result<BraidView, BraidError> {
        let mut state = self.state.lock().expect("braid state lock");
        qualia_braid::observe(&mut state, &event)?;
        Ok(BraidView::of(&state))
    }

    /// Report one event from inside this process, where the reporter has no
    /// retry that a log line does not already cover.
    pub fn observe(&self, event: BraidEvent) {
        if let Err(error) = self.report(event) {
            eprintln!("qualia-agent: braid event rejected: {error}");
        }
    }

    /// Bind the session the braid belongs to. The session store's active
    /// session names it; until that lands the id stays empty.
    pub fn bind_session(&self, session_id: impl Into<String>) {
        self.state.lock().expect("braid state lock").session_id = session_id.into();
    }

    /// The current view.
    pub fn view(&self) -> BraidView {
        BraidView::of(&self.state.lock().expect("braid state lock"))
    }
}

/// `GET /braid` — the view the console polls every 250 ms and the operator page
/// every second.
pub async fn view_get(State(state): State<AppState>) -> Json<BraidView> {
    Json(state.braid.view())
}

/// `POST /braid` — one strand reports one event.
///
/// The body is the `BraidEvent` JSON the braid crate fixes, e.g.
/// `{"event":"evidence_sealed","sha256":…}`; the reply is the view the report
/// folded into, so the strand reads back exactly what a later reader will see.
/// The edge accepts the five events a strand reports and answers the crate's
/// recovery-only `Quarantined` with `400` before the fold — nothing moved, no
/// file touched. An `event` tag this build does not know is accepted like any
/// other — the fold leaves the state it holds — because a strand must never
/// have to know the braid's revision to speak.
pub async fn event_post(
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<BraidEvent>,
) -> Response {
    if let Err(response) = state.auth.authorize(AuthScope::Peer, &headers, remote.ip()) {
        return response;
    }
    if !is_strand_report(&event) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": UNSUPPORTED_EVENT })),
        )
            .into_response();
    }
    match state.braid.report(event) {
        Ok(view) => (StatusCode::OK, Json(view)).into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

/// The refusal `POST /braid` answers with when the body names an event no
/// strand reports.
const UNSUPPORTED_EVENT: &str = concat!(
    "the braid edge accepts mission_opened, mission_closed, evidence_sealed, ",
    "promotion_accepted and promotion_rolled_back; quarantined is not a strand report",
);

/// Whether the strand edge accepts one event.
///
/// The ticket fixes five reports and no others. The match is exhaustive with no
/// wildcard on purpose: a variant the braid crate adds later stops this edge
/// compiling until it decides whether that variant is a strand report. An
/// unrecognised `event` tag is the one exception that stays open — it decodes to
/// [`BraidEvent::Unknown`], carries no fold, and must keep working so a strand
/// that ships ahead of the braid can still speak.
fn is_strand_report(event: &BraidEvent) -> bool {
    match event {
        BraidEvent::MissionOpened { .. }
        | BraidEvent::MissionClosed { .. }
        | BraidEvent::EvidenceSealed { .. }
        | BraidEvent::PromotionAccepted { .. }
        | BraidEvent::PromotionRolledBack { .. } => true,
        BraidEvent::Unknown => true,
        // Recovery's dispatch: it renames `*.partial` under a caller-named
        // path, so it is not a report a strand may send to this edge.
        BraidEvent::Quarantined { .. } => false,
    }
}
