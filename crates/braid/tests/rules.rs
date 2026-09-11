//! What the rule layer asks for when a braid event arrives.
//!
//! A rule is data — an `EventPattern` and the `BraidAction` it fires — so these
//! tests drive `evaluate` the way a strand would: hand it one decoded event and
//! a rule set, then assert on the actions a consumer sees. They never assert on
//! how the matcher is written.

use qualia_braid::rules::{default_rules, evaluate, BraidAction, EventPattern, Rule};
use qualia_braid::BraidEvent;

/// One event per variant the braid's vocabulary carries, for the tests that
/// ask what a rule set does across the whole event surface.
fn one_of_each_event() -> Vec<BraidEvent> {
    vec![
        BraidEvent::MissionOpened { mission_id: "m".to_string() },
        BraidEvent::MissionClosed { mission_id: "m".to_string(), outcome: "failed".to_string() },
        BraidEvent::EvidenceSealed { sha256: "0".repeat(64) },
        BraidEvent::PromotionAccepted { generation: 3 },
        BraidEvent::PromotionRolledBack { generation: 2, reason: "gate".to_string() },
        BraidEvent::Quarantined { path: "/partials".to_string(), reason: "torn".to_string() },
        BraidEvent::Unknown,
    ]
}

#[test]
fn default_rules_are_ordered_and_pure() {
    let rules = default_rules();

    assert_eq!(
        rules,
        vec![
            Rule {
                when: EventPattern::MissionClosed { outcome: "failed".to_string() },
                then: BraidAction::LowerCoupling { factor: 0.90 },
            },
            Rule {
                when: EventPattern::PromotionRolledBack,
                then: BraidAction::EnterObserveOnly,
            },
            Rule {
                when: EventPattern::Quarantined,
                then: BraidAction::RequestTraining,
            },
            Rule {
                when: EventPattern::DriftAbove { threshold: 3.0 },
                then: BraidAction::EnterObserveOnly,
            },
        ],
        "the default set is the four rules, in precedence order"
    );

    // Pure: a second call returns an equal value, and it is a fresh vector, so
    // a caller that edits its rule set cannot edit the next caller's.
    let mut second = default_rules();
    assert_eq!(second, rules, "two calls agree");
    second.clear();
    assert_eq!(default_rules(), rules, "the set is rebuilt, not shared");
}

#[test]
fn a_failed_mission_lowers_the_coupling() {
    let event = BraidEvent::MissionClosed {
        mission_id: "mission-7".to_string(),
        outcome: "failed".to_string(),
    };

    assert_eq!(
        evaluate(&event, &default_rules()),
        vec![BraidAction::LowerCoupling { factor: 0.90 }]
    );
}

#[test]
fn a_mission_that_did_not_fail_fires_nothing() {
    // The pattern names an outcome string, so anything else — including a
    // success — must not read as a failure.
    for outcome in ["reached", "aborted", "Failed", ""] {
        let event = BraidEvent::MissionClosed {
            mission_id: "mission-7".to_string(),
            outcome: outcome.to_string(),
        };
        assert!(
            evaluate(&event, &default_rules()).is_empty(),
            "outcome {outcome:?} is not the pattern's failure"
        );
    }
}

#[test]
fn rollback_asks_for_observe_only_and_quarantine_for_training() {
    let rules = default_rules();

    assert_eq!(
        evaluate(
            &BraidEvent::PromotionRolledBack { generation: 2, reason: "gate".to_string() },
            &rules
        ),
        vec![BraidAction::EnterObserveOnly]
    );
    assert_eq!(
        evaluate(
            &BraidEvent::Quarantined { path: "/partials".to_string(), reason: "torn".to_string() },
            &rules
        ),
        vec![BraidAction::RequestTraining]
    );
}

#[test]
fn every_matching_rule_fires_in_rule_order() {
    // Two rules name the same event; the rule set's order is the precedence,
    // and every match fires — later rules are not shadowed by earlier ones.
    let rules = vec![
        Rule { when: EventPattern::Quarantined, then: BraidAction::RequestTraining },
        Rule { when: EventPattern::Quarantined, then: BraidAction::RequestSafeStop },
        Rule { when: EventPattern::PromotionRolledBack, then: BraidAction::RollBackGeneration },
    ];
    let event = BraidEvent::Quarantined { path: "/partials".to_string(), reason: "torn".to_string() };

    assert_eq!(
        evaluate(&event, &rules),
        vec![BraidAction::RequestTraining, BraidAction::RequestSafeStop]
    );
}

#[test]
fn an_event_the_braid_does_not_know_fires_nothing() {
    // A strand that ships ahead of the braid decodes to `Unknown`; the state
    // machine holds its state and the rule layer must not act on a strand it
    // cannot read. No opinion fires nothing.
    assert!(evaluate(&BraidEvent::Unknown, &default_rules()).is_empty());
}

#[test]
fn the_drift_rule_fires_on_no_event_the_braid_carries_yet() {
    // `DriftAbove` is part of the contract, but the frozen `BraidEvent`
    // vocabulary carries no drift measurement: T34's `DriftReport` is consumed
    // by the healing ladder directly, and a drift-carrying event does not exist
    // on main. The pattern therefore matches none of the events `evaluate` can
    // be handed. This pins the seam, and the "no opinion" convention with it:
    // when the wire grows the event, a reading with `sample_count == 0` is a
    // malformed sample and must still fire nothing.
    let rules = vec![Rule {
        when: EventPattern::DriftAbove { threshold: 3.0 },
        then: BraidAction::EnterObserveOnly,
    }];

    for event in one_of_each_event() {
        assert!(
            evaluate(&event, &rules).is_empty(),
            "{event:?} carries no drift, so the drift rule fires nothing"
        );
    }
}
