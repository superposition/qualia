//! The neuro-symbolic seam: drift measured, ladder escalated, stop requested.
//!
//! These are the seam tests of epic #11, step 36. Each one joins layers by
//! their public APIs rather than re-testing either half alone: [`measure`]
//! produces the number, [`next_step`] and [`Ladder`] escalate on it, and
//! [`evaluate`] turns a braid event into a request. The last test pins the one
//! boundary this crate does not own — the only leash call site is the agent's,
//! behind `QUALIA_LEASH_BASE_URL`, and the braid only ever produces a request
//! value.
//!
//! Nothing here has a motor, a socket or a clock of its own: the ladder's clock
//! is the caller's `now_ns`, and a stop is a value the caller forwards.

use qualia_braid::drift::{measure, DriftReport};
use qualia_braid::heal::{next_step, HealStep, Ladder};
use qualia_braid::rules::{default_rules, evaluate, BraidAction, EventPattern, Rule};
use qualia_braid::BraidEvent;

/// The braid's own sources: the layers this file is the seam between.
const DRIFT_SRC: &str = include_str!("../src/drift.rs");
const HEAL_SRC: &str = include_str!("../src/heal.rs");
const RULES_SRC: &str = include_str!("../src/rules.rs");
const LIB_SRC: &str = include_str!("../src/lib.rs");

/// The agent's leash path: the client that reaches the motion authority, and
/// the config that builds its endpoint.
const LEASH_SRC: &str = include_str!("../../../runners/agent/src/leash.rs");
const AGENT_CONFIG_SRC: &str = include_str!("../../../runners/agent/src/config.rs");

/// The environment key the agent's leash endpoint is configured from.
const LEASH_BASE_URL_ENV: &str = "QUALIA_LEASH_BASE_URL";

/// One well-formed opinion at about `target`, measured from a real residual:
/// a latent of `sqrt(target)` from a mean of zero under unit precision, so the
/// squared residual — and the measurement — is `target`.
fn measured(target: f32) -> DriftReport {
    measure(&[target.sqrt()], &[0.0], &[0.0])
}

/// One event per variant the braid's vocabulary carries, so the seam across
/// the whole event surface can be driven.
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
fn drift_is_zero_for_identity_prediction() {
    // The predictor's mean sits exactly on the latent, under unit variance
    // (log-variance zero is precision one): there is no residual to weight.
    let latent = [1.5_f32, -0.25, 3.0, 0.75];
    let predicted_mean = latent;
    let predicted_log_variance = [0.0_f32; 4];

    let report = measure(&latent, &predicted_mean, &predicted_log_variance);

    assert_eq!(report.mahalanobis, 0.0, "an identity prediction has no drift");
    assert_eq!(report.sample_count, 1, "a well-formed sample is one opinion");
}

#[test]
fn ladder_escalates_with_drift() {
    // The four synthetic readings enter through `measure` and leave through
    // the ladder, so this drives the join and not either half alone. The
    // thresholds are `heal.rs`'s: `HEALTHY = 3.0`, `RECALIBRATE_BELOW = 5.0`,
    // `ROLLBACK_BELOW = 8.0`, each inclusive at the top of its band.
    let (two, four, seven, twelve) = (measured(2.0), measured(4.0), measured(7.0), measured(12.0));

    assert_eq!(two.sample_count, 1, "each reading is a measurement");
    assert_eq!(next_step(&two, 0), None, "at or below 3.0 the prediction is trusted");
    assert_eq!(next_step(&four, 0), Some(HealStep::Recalibrate), "above 3.0, at or below 5.0");
    assert_eq!(next_step(&seven, 0), Some(HealStep::RollBack), "above 5.0, at or below 8.0");
    assert_eq!(next_step(&twelve, 0), Some(HealStep::ObserveOnly), "above 8.0");
}

#[test]
fn mismatched_sample_is_no_opinion() {
    // Ragged slices: the JEPA math has no residual to pair up, so the report
    // is "nothing was measured" rather than a zero-distance measurement.
    let report = measure(&[1.0, 2.0], &[1.0], &[0.0, 0.0]);
    assert_eq!(report, DriftReport { mahalanobis: 0.0, sample_count: 0 });

    // No ladder rung is selected at any budget...
    assert_eq!(next_step(&report, 0), None);
    assert_eq!(next_step(&report, 3), None, "a spent budget does not invent an opinion");

    // ...and a running ladder neither advances nor ends a hold.
    let mut ladder = Ladder::new();
    assert_eq!(ladder.step(&report, 0, 0), None);

    // And no rule fires. The only rule that could act on drift is `DriftAbove`,
    // and this wire carries no drift measurement in a braid event, so a report
    // that is no opinion cannot reach the rule layer — and `DriftAbove` fires
    // nothing on any event the braid can be handed (see `rules.rs`).
    let drift_rule = vec![Rule {
        when: EventPattern::DriftAbove { threshold: 3.0 },
        then: BraidAction::EnterObserveOnly,
    }];
    for event in one_of_each_event() {
        assert!(
            evaluate(&event, &drift_rule).is_empty(),
            "{event:?} carries no drift, so no rule fires"
        );
    }
}

#[test]
fn safe_stop_is_requested_never_asserted() {
    // A drift above 8.0 held observe-only for ten seconds: the ladder produces
    // the request `SafeStop`, and keeps producing it while the condition
    // holds. It is a value; the ladder has no motor and no socket, so it
    // cannot assert the stop itself.
    let mut ladder = Ladder::new();
    let above = measured(12.0);
    assert_eq!(ladder.step(&above, 0, 0), Some(HealStep::ObserveOnly));
    assert_eq!(
        ladder.step(&above, 0, 10_000_000_000),
        Some(HealStep::SafeStop),
        "ten seconds above 8.0 is a request, never a command"
    );

    // The symbolic layer's counterpart is a request too: a rule set that names
    // `RequestSafeStop` returns that value and decides nothing.
    let stop_rule =
        vec![Rule { when: EventPattern::Quarantined, then: BraidAction::RequestSafeStop }];
    let event =
        BraidEvent::Quarantined { path: "/partials".to_string(), reason: "torn".to_string() };
    assert_eq!(evaluate(&event, &stop_rule), vec![BraidAction::RequestSafeStop]);
    assert!(
        !default_rules().iter().any(|rule| rule.then == BraidAction::RequestSafeStop),
        "the standing rule set asks for no stop on its own"
    );

    // The braid never reaches the motion authority: no braid source names the
    // leash endpoint...
    for source in [DRIFT_SRC, HEAL_SRC, RULES_SRC, LIB_SRC] {
        assert!(
            !source.contains(LEASH_BASE_URL_ENV),
            "the braid requests; it must not carry a leash call site"
        );
    }

    // ...because that call site is the agent's, and the agent's leash client is
    // configured from the existing `QUALIA_LEASH_BASE_URL` path. A stop reaches
    // a motor only through the agent forwarding it to leash, which decides.
    assert!(
        AGENT_CONFIG_SRC.contains(LEASH_BASE_URL_ENV),
        "the leash endpoint is configured from the env path"
    );
    assert!(
        LEASH_SRC.contains(LEASH_BASE_URL_ENV),
        "the leash client reports the env path as its configured endpoint"
    );
}
