//! Behavioural tests for the braid state machine.
//!
//! A strand talks to the braid the way a runner will: it decodes a `BraidEvent`
//! and hands it to `observe`, then a later reader looks at the `BraidState` that
//! came out. These tests therefore assert on state a consumer can see and on the
//! one dispatch a `BraidEvent` can trigger, never on how `observe` is written.

use qualia_braid::{observe, BraidEvent, BraidState};
use std::fs;

#[test]
fn mission_lifecycle_updates_open_count() {
    let mut state = BraidState::default();

    observe(
        &mut state,
        &BraidEvent::MissionOpened {
            mission_id: "mission-1".to_string(),
        },
    )
    .unwrap();
    observe(
        &mut state,
        &BraidEvent::MissionOpened {
            mission_id: "mission-2".to_string(),
        },
    )
    .unwrap();
    assert_eq!(state.open_missions, 2, "two opened missions are both open");

    observe(
        &mut state,
        &BraidEvent::MissionClosed {
            mission_id: "mission-1".to_string(),
            outcome: "reached".to_string(),
        },
    )
    .unwrap();
    assert_eq!(state.open_missions, 1, "closing one leaves the other open");

    observe(
        &mut state,
        &BraidEvent::MissionClosed {
            mission_id: "mission-2".to_string(),
            outcome: "aborted".to_string(),
        },
    )
    .unwrap();
    assert_eq!(state.open_missions, 0, "the count returns to zero");

    // A close for a mission this braid never saw opened is still a fact the
    // strand reported; it must not wrap the count around.
    observe(
        &mut state,
        &BraidEvent::MissionClosed {
            mission_id: "mission-3".to_string(),
            outcome: "aborted".to_string(),
        },
    )
    .unwrap();
    assert_eq!(state.open_missions, 0, "a close with nothing open is a no-op");
}

#[test]
fn rollback_event_is_recorded_and_state_survives() {
    let mut state = BraidState::default();

    observe(
        &mut state,
        &BraidEvent::PromotionAccepted { generation: 7 },
    )
    .unwrap();
    let promoted_at = state.last_promotion_ns;
    assert_eq!(state.generation, 7);
    assert!(promoted_at > 0, "an acceptance is stamped with a time");

    observe(
        &mut state,
        &BraidEvent::PromotionRolledBack {
            generation: 6,
            reason: "health gate failed".to_string(),
        },
    )
    .unwrap();

    assert_eq!(state.generation, 6, "the pointer follows the rollback");
    assert_eq!(
        state.last_promotion_ns, promoted_at,
        "a rollback must not look like a promotion"
    );
}

#[test]
fn unknown_events_are_ignored() {
    let mut state = BraidState::default();
    observe(
        &mut state,
        &BraidEvent::MissionOpened {
            mission_id: "mission-1".to_string(),
        },
    )
    .unwrap();
    let before = state.clone();

    // A later strand may publish a variant this build has never heard of. The
    // decoder must accept it and the braid must leave the state it holds alone.
    let future: BraidEvent =
        serde_json::from_str(r#"{"event":"lane_changed","lane":"left"}"#).unwrap();
    assert_eq!(future, BraidEvent::Unknown);
    observe(&mut state, &future).unwrap();

    assert_eq!(state, before);
}

#[test]
fn quarantined_event_dispatches_to_mcap() {
    let temp = tempfile::tempdir().unwrap();
    let partial = temp.path().join("arena-1.mcap.partial");
    fs::write(&partial, b"a segment that was never sealed").unwrap();

    let mut state = BraidState::default();
    observe(
        &mut state,
        &BraidEvent::Quarantined {
            path: temp.path().to_string_lossy().into_owned(),
            reason: "writer stopped early".to_string(),
        },
    )
    .unwrap();

    assert!(
        !partial.exists(),
        "the partial leaves the live namespace through the MCAP dispatch"
    );
    assert!(
        temp.path().join("arena-1.mcap.quarantine").is_file(),
        "the bytes are renamed, not deleted"
    );
    assert!(
        state.last_quarantine_ns.is_some(),
        "the braid stamps the quarantine it dispatched"
    );
}
