//! Behavioural tests for the braid state machine.
//!
//! A strand talks to the braid the way a runner will: it decodes a `BraidEvent`
//! and hands it to `observe`, then a later reader looks at the `BraidState` that
//! came out. These tests therefore assert on state a consumer can see and on the
//! dispatches a `BraidEvent` can trigger, never on how `observe` is written.
//!
//! The rollback's durable record is the generation registry's, so the test that
//! pins the record drives a real registry through `route` and reads the record
//! back where it is written: `crates/jepa-registry`'s test module.

use qualia_braid::{observe, route, BraidError, BraidEvent, BraidState};
use qualia_jepa_registry::CandidateRegistry;
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

#[tokio::test]
async fn rollback_event_is_recorded_and_state_survives() {
    let mut state = BraidState::default();

    observe(
        &mut state,
        &BraidEvent::PromotionAccepted { generation: 7 },
    )
    .unwrap();
    let promoted_at = state.last_promotion_ns;
    assert_eq!(state.generation, 7);
    assert!(promoted_at > 0, "an acceptance is stamped with a time");

    // A strand publishes the rollback as an envelope, so the reason has to
    // survive the decode: that value is what the registry dispatch records.
    let rollback: BraidEvent = serde_json::from_str(
        r#"{"event":"promotion_rolled_back","generation":6,"reason":"health gate failed"}"#,
    )
    .unwrap();
    match &rollback {
        BraidEvent::PromotionRolledBack { generation, reason } => {
            assert_eq!(*generation, 6, "the envelope carries the generation");
            assert_eq!(
                reason, "health gate failed",
                "the reason stays on the wire for the registry dispatch"
            );
        }
        other => panic!("the envelope decodes to the rollback variant, not {other:?}"),
    }

    observe(&mut state, &rollback).unwrap();

    assert_eq!(state.generation, 6, "the pointer follows the rollback");
    assert_eq!(
        state.last_promotion_ns, promoted_at,
        "a rollback must not look like a promotion"
    );

    // The reason is deliberately not stored here. Its durable record is the
    // generation registry's, whose rollback call this revision cannot make (see
    // the dated resolution on issue #34: the call lands with C10 #76, the
    // routing with T22 #37), so no part of the braid's own view carries it.
    // The routing below is where the record is written.
    let view = serde_json::to_string(&state).unwrap();
    assert!(
        !view.contains("health gate failed"),
        "the braid keeps no reason of its own: {view}"
    );

    // The routing owns the registry edge the fold cannot reach. A registry that
    // does not own the generation pointer cannot publish the rollback, so the
    // dispatch fails — and because the dispatch runs first, the pointer does not
    // move and no stamp is taken: a reader never sees a pointer the record does
    // not have.
    let temp = tempfile::tempdir().unwrap();
    let registry = CandidateRegistry::open(temp.path().join("registry.turso"))
        .await
        .unwrap();
    let generation_file = temp.path().join("active-generation.json");

    let mut routed = BraidState::default();
    observe(
        &mut routed,
        &BraidEvent::PromotionAccepted { generation: 7 },
    )
    .unwrap();
    let routed_at = routed.last_promotion_ns;

    let refused = route(&mut routed, &rollback, &registry, &generation_file).await;
    assert!(
        matches!(refused, Err(BraidError::Rollback(_))),
        "a rollback the registry will not publish is reported, not swallowed: {refused:?}"
    );
    assert_eq!(
        routed.generation, 7,
        "the pointer does not move when the record cannot be written"
    );
    assert_eq!(
        routed.last_promotion_ns, routed_at,
        "and no promotion stamp is taken either"
    );
    assert!(
        !generation_file.exists(),
        "the registry published no generation when it could not roll back"
    );
}

#[tokio::test]
async fn routing_folds_the_events_that_have_no_registry_edge() {
    // Only the rollback has a registry edge. Every other event, including one
    // this build cannot read, is the fold the caller already knows: the routing
    // must not consult the record for them.
    let temp = tempfile::tempdir().unwrap();
    let registry = CandidateRegistry::open(temp.path().join("registry.turso"))
        .await
        .unwrap();
    let generation_file = temp.path().join("active-generation.json");

    let mut state = BraidState::default();
    for event in [
        BraidEvent::MissionOpened {
            mission_id: "mission-1".to_string(),
        },
        BraidEvent::Unknown,
    ] {
        route(&mut state, &event, &registry, &generation_file)
            .await
            .unwrap();
    }
    assert_eq!(state.open_missions, 1, "the mission count is the fold's");
    assert!(
        !generation_file.exists(),
        "no event but a rollback writes the generation pointer"
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

#[test]
fn failed_quarantine_dispatch_leaves_state_untouched() {
    // Pointing the dispatch at something that is not a directory makes
    // `quarantine_partials` fail, so nothing was moved aside. The view must not
    // move either: a reader that saw the stamp would believe partials were
    // quarantined when they are exactly where the strand left them.
    let temp = tempfile::tempdir().unwrap();
    let not_a_directory = temp.path().join("arena-1.mcap.partial");
    fs::write(&not_a_directory, b"a segment that was never sealed").unwrap();

    let mut state = BraidState::default();
    observe(
        &mut state,
        &BraidEvent::MissionOpened {
            mission_id: "mission-1".to_string(),
        },
    )
    .unwrap();
    let before = state.clone();

    let result = observe(
        &mut state,
        &BraidEvent::Quarantined {
            path: not_a_directory.to_string_lossy().into_owned(),
            reason: "writer stopped early".to_string(),
        },
    );

    assert!(
        matches!(result, Err(BraidError::Quarantine(_))),
        "a dispatch that could not run is reported to the caller: {result:?}"
    );
    assert_eq!(
        state, before,
        "no part of the view moves when the dispatch fails"
    );
    assert!(
        not_a_directory.is_file(),
        "the bytes are where the strand left them, for the caller to retry"
    );
}

#[test]
fn quarantined_event_with_nothing_to_move_still_stamps() {
    // A stack that never logged has no partials. `quarantine_partials` reports
    // an empty recovery for a root that is not there, and the braid still
    // records that the recovery ran; it creates nothing on the way.
    let temp = tempfile::tempdir().unwrap();
    let never_logged = temp.path().join("never-logged");

    let mut state = BraidState::default();
    observe(
        &mut state,
        &BraidEvent::Quarantined {
            path: never_logged.to_string_lossy().into_owned(),
            reason: "no partials reached the disk".to_string(),
        },
    )
    .unwrap();

    assert!(
        state.last_quarantine_ns.is_some(),
        "an empty recovery is still a recovery the braid was told about"
    );
    assert!(!never_logged.exists(), "an empty recovery touches nothing");
}
