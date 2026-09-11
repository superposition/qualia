//! The braid state machine. See `README.md` for the pattern it instantiates.
//!
//! Every strand reports here, and nothing else keeps its own copy of the braid's
//! view: [`observe`] is the sole mutator of [`BraidState`], so the state is the
//! fold of the event stream and can always be rebuilt by replaying it. The braid
//! owns no storage. A strand that has a durable record to make asks for it
//! through the event, and the crate that owns that record writes it: MCAP for
//! partial evidence, the generation registry for a rollback.
//!
//! The MCAP edge is live; the registry edge is not yet wired. This workspace's
//! `crates/jepa-registry` ships no library target, so cargo ignores the
//! dependency this crate declares on it and no registry symbol is callable — and
//! the registry's rollback wants a registry handle, the path of the generation
//! pointer and an async context, none of which the signatures fixed here carry.
//! So a [`BraidEvent::PromotionRolledBack`] only moves the pointer: the view
//! takes the event's generation, `last_promotion_ns` stays where the last
//! acceptance put it, and the `reason` is left on the wire for the strand that
//! owns the registry to dispatch with. The call lands with the registry library
//! in C10 (#76) and the routing with T22 (#37); the dated resolution on issue
//! #34 records both.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The wire contract for the JSON form of a [`BraidEvent`].
pub const BRAID_EVENT_SCHEMA: &str = "qualia.braid-event.v1";

/// The braid's view of a running stack.
///
/// The fields are what every strand can agree on without reading another
/// crate's storage: which generation is current, which session this braid
/// belongs to, how many missions are open, and when the last promotion and the
/// last quarantine happened.
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
///
/// The variants are the five strands' vocabulary: the mission broker opens and
/// closes missions, the evidence writer seals a segment, the promotion gate
/// accepts or rolls back a generation, and recovery quarantines a partial. The
/// enum is internally tagged by `event`, so a strand's envelope carries one
/// `"event"` key naming the variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BraidEvent {
    MissionOpened { mission_id: String },
    MissionClosed { mission_id: String, outcome: String },
    EvidenceSealed { sha256: String },
    PromotionAccepted { generation: u64 },
    PromotionRolledBack { generation: u64, reason: String },
    Quarantined { path: String, reason: String },
    /// A variant this build does not know.
    ///
    /// A strand that ships ahead of the braid must not break the stream, so an
    /// unrecognised `event` tag decodes to this and [`observe`] leaves the state
    /// it already holds untouched.
    #[serde(other)]
    Unknown,
}

/// A dispatch a [`BraidEvent`] asked for could not be carried out.
#[derive(Debug)]
pub enum BraidError {
    /// `qualia-mcap` could not move a partial out of the live namespace. The
    /// bytes are still where they were, and the caller should retry rather than
    /// treat the evidence as gone.
    Quarantine(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for BraidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BraidError::Quarantine(source) => {
                write!(f, "quarantining partial evidence failed: {source}")
            }
        }
    }
}

impl Error for BraidError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BraidError::Quarantine(source) => Some(&**source),
        }
    }
}

/// Fold one event into the braid's view.
///
/// This is the single mutation point for [`BraidState`]. Events that carry a
/// durable action dispatch it to the crate that owns the record and add no
/// storage here:
///
/// - [`BraidEvent::Quarantined`] moves the partials under `path` aside through
///   [`qualia_mcap::quarantine_partials`], then stamps `last_quarantine_ns`. The
///   dispatch runs first, so a recovery whose dispatch fails returns an error
///   with the view untouched. A root with no partials — including one that does
///   not exist — is an empty recovery, not a failure: nothing moved, and the
///   stamp is still taken, because a stack that never logged has nothing to
///   recover.
/// - [`BraidEvent::PromotionAccepted`] and [`BraidEvent::PromotionRolledBack`]
///   move the generation pointer. A rollback is not a promotion, so it leaves
///   `last_promotion_ns` where the last acceptance put it. The rollback's
///   durable record, which is what carries its `reason`, is the generation
///   registry's; the call is deferred, so the pointer moves and the reason stays
///   on the wire (see the module docs).
/// - [`BraidEvent::EvidenceSealed`] carries the digest of a sealed segment; the
///   sealed segment is the record, so the braid keeps only the knowledge that it
///   happened (nothing here changes).
///
/// Delivery is not deduplicated. `open_missions` counts the mission events
/// folded, and the wire carries no set of open missions to check a re-delivery
/// against, so a strand that publishes the same [`BraidEvent::MissionOpened`]
/// twice is counted twice; reconciling that is the re-delivering strand's job,
/// not a guess for the braid to make.
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
        // The pointer moves with the event. `last_promotion_ns` is deliberately
        // not touched, and the rollback's `reason` is deliberately not stored:
        // its durable record belongs to the generation registry, whose dispatch
        // is deferred (see the dated resolution on issue #34). The dropped field
        // is the resolution, not an oversight.
        BraidEvent::PromotionRolledBack { generation, .. } => {
            state.generation = *generation;
        }
        BraidEvent::Quarantined { path, .. } => {
            qualia_mcap::quarantine_partials(Path::new(path))
                .map_err(BraidError::Quarantine)?;
            state.last_quarantine_ns = Some(now_ns());
        }
        BraidEvent::Unknown => {}
    }
    Ok(())
}

/// Nanoseconds since the Unix epoch, the unit every other clock in the stack
/// reports in.
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}
