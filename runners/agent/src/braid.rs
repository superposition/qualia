//! The braid view: one state machine every strand reports through.
//!
//! This module is the agent's edge of `qualia-braid`. The state it folds is
//! deliberately the whole of it — a generation, a session, a count of open
//! missions and the two timestamps that say when the ladder last moved — so the
//! view can be rebuilt from the event stream alone and no strand needs a second
//! copy.
//!
//! The type and the event vocabulary here mirror `qualia_braid` exactly. When
//! that crate lands, this module's state, event and fold are replaced by
//! `qualia_braid::{BraidState, BraidEvent, observe}` and nothing else in the
//! agent changes: the mission broker already reports through [`BraidRuntime`],
//! and the wire form is already the console's contract.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::{now_ns, AppState};

/// The schema literal `GET /braid` carries; the console and the operator page
/// both name it.
pub const BRAID_STATE_SCHEMA: &str = "qualia.braid-state.v1";

/// The braid's view of a running stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BraidState {
    pub generation: u64,
    pub session_id: String,
    pub open_missions: u32,
    pub last_promotion_ns: u64,
    pub last_quarantine_ns: Option<u64>,
}

impl Default for BraidState {
    fn default() -> Self {
        Self {
            generation: 0,
            session_id: String::new(),
            open_missions: 0,
            last_promotion_ns: 0,
            last_quarantine_ns: None,
        }
    }
}

/// Something a strand reports to the braid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BraidEvent {
    MissionOpened { mission_id: String },
    MissionClosed { mission_id: String, outcome: String },
    EvidenceSealed { sha256: String },
    PromotionAccepted { generation: u64 },
    PromotionRolledBack { generation: u64, reason: String },
    Quarantined { path: String, reason: String },
    /// A variant this build does not know; the state it holds is untouched.
    #[serde(other)]
    Unknown,
}

/// The error the fold can return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BraidError {
    /// A strand reported an event whose dispatch could not be carried out.
    Dispatch(String),
}

impl std::fmt::Display for BraidError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BraidError::Dispatch(reason) => write!(formatter, "braid dispatch failed: {reason}"),
        }
    }
}

impl std::error::Error for BraidError {}

/// Fold one event into the braid's view.
///
/// This is the single mutation point for [`BraidState`]. A strand that owns a
/// durable record makes it itself; the fold only stamps the view.
///
/// `Quarantined` is the one event with a dispatch attached upstream — moving a
/// partial out of the live namespace is `qualia-mcap`'s work, and it arrives
/// with the evidence strand. Until then the event stamps the timestamp, which
/// is what the braid's view is for.
pub fn observe(state: &mut BraidState, event: &BraidEvent) -> Result<(), BraidError> {
    match event {
        BraidEvent::MissionOpened { .. } => {
            state.open_missions = state.open_missions.saturating_add(1);
        }
        BraidEvent::MissionClosed { .. } => {
            state.open_missions = state.open_missions.saturating_sub(1);
        }
        BraidEvent::EvidenceSealed { .. } => {}
        BraidEvent::PromotionAccepted { generation } => {
            state.generation = *generation;
            state.last_promotion_ns = now_ns();
        }
        BraidEvent::PromotionRolledBack { generation, .. } => {
            state.generation = *generation;
        }
        BraidEvent::Quarantined { .. } => {
            state.last_quarantine_ns = Some(now_ns());
        }
        BraidEvent::Unknown => {}
    }
    Ok(())
}

/// The braid view as the console, the web page and the fixture all read it.
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

    /// Report one event and fold it in. A dispatch failure is logged, never
    /// swallowed silently, and leaves the view as it was.
    pub fn observe(&self, event: BraidEvent) {
        let mut state = self.state.lock().expect("braid state lock");
        if let Err(error) = observe(&mut state, &event) {
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
