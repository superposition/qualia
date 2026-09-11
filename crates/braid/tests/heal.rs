//! What the healing ladder escalates to when the drift says the generation is
//! no longer trustworthy.
//!
//! The ladder is the plan's escalation table (epic #11, step 35) and its rungs
//! are exact: at or below `3.0` nothing, at or below `5.0` recalibrate, at or
//! below `8.0` roll back, above `8.0` or once three escalations have been
//! attempted observe-only, and a safe stop only once observe-only has held ten
//! seconds with the drift still above `8.0`.
//!
//! Every outcome is a value — a `HealStep` a caller decides what to do with.
//! The ladder holds no motor and touches no file, and no test here asserts that
//! it does.

use qualia_braid::drift::{measure, DriftReport};
use qualia_braid::heal::{next_step, HealStep, Ladder};

/// One well-formed opinion at `mahalanobis`: the report a measurement of a
/// real latent would carry.
fn drift(mahalanobis: f32) -> DriftReport {
    DriftReport { mahalanobis, sample_count: 1 }
}

/// A ragged sample, which `measure` rejects: the "no opinion" report.
fn no_opinion() -> DriftReport {
    measure(&[1.0, 2.0], &[1.0], &[0.0, 0.0])
}

#[test]
fn ladder_escalates_with_drift() {
    // The ticket's four synthetic drifts, one per rung of the table.
    assert_eq!(next_step(&drift(2.0), 0), None);
    assert_eq!(next_step(&drift(4.0), 0), Some(HealStep::Recalibrate));
    assert_eq!(next_step(&drift(7.0), 0), Some(HealStep::RollBack));
    assert_eq!(next_step(&drift(12.0), 0), Some(HealStep::ObserveOnly));
}

#[test]
fn the_band_edges_belong_to_the_lower_rung() {
    // The table is written `<=`, so each threshold lands on its own rung and
    // the next one starts a hair above it.
    assert_eq!(next_step(&drift(3.0), 0), None);
    assert_eq!(next_step(&drift(5.0), 0), Some(HealStep::Recalibrate));
    assert_eq!(next_step(&drift(8.0), 0), Some(HealStep::RollBack));

    assert_eq!(next_step(&drift(3.000_001), 0), Some(HealStep::Recalibrate));
    assert_eq!(next_step(&drift(5.000_001), 0), Some(HealStep::RollBack));
    assert_eq!(next_step(&drift(8.000_001), 0), Some(HealStep::ObserveOnly));
}

#[test]
fn exhausted_attempts_hold_observe_only() {
    // Three escalations were made and the drift has not come back: the ladder
    // stops trying the lower rungs and holds, however far the drift has risen.
    assert_eq!(next_step(&drift(4.0), 3), Some(HealStep::ObserveOnly));
    assert_eq!(next_step(&drift(7.0), 3), Some(HealStep::ObserveOnly));

    // Short of the budget the rungs still walk, so the clause is a budget and
    // not a lower band.
    assert_eq!(next_step(&drift(4.0), 2), Some(HealStep::Recalibrate));
    assert_eq!(next_step(&drift(7.0), 2), Some(HealStep::RollBack));

    // A healthy reading is the gate: the drift coming back *is* the healing
    // succeeding, so a spent budget selects nothing rather than a hold.
    assert_eq!(next_step(&drift(2.0), 3), None);
}

#[test]
fn no_opinion_selects_nothing() {
    let report = no_opinion();
    assert_eq!(report.sample_count, 0, "a ragged sample is no measurement");

    assert_eq!(next_step(&report, 0), None);
    assert_eq!(next_step(&report, 3), None, "and a spent budget does not change that");
}

#[test]
fn safe_stop_follows_a_ten_second_hold_above_eight() {
    let mut ladder = Ladder::new();
    let started = 1_000_000_000_u64;
    let above = drift(12.0);

    // Observe-only begins with the reading that first selects it.
    assert_eq!(ladder.step(&above, 0, started), Some(HealStep::ObserveOnly));

    // Held, but not yet ten seconds.
    assert_eq!(
        ladder.step(&above, 0, started + 9_999_999_999),
        Some(HealStep::ObserveOnly)
    );

    // Ten seconds with the drift still above 8.0: the ladder asks for a stop,
    // and keeps asking while the condition holds.
    assert_eq!(
        ladder.step(&above, 0, started + 10_000_000_000),
        Some(HealStep::SafeStop)
    );
    assert_eq!(
        ladder.step(&above, 0, started + 11_000_000_000),
        Some(HealStep::SafeStop)
    );
}

#[test]
fn the_hold_needs_a_drift_still_above_eight() {
    let mut ladder = Ladder::new();

    // Observe-only selected by a spent budget, not by the drift: it can hold
    // forever and never request a stop, because the stop needs drift above 8.0.
    assert_eq!(ladder.step(&drift(7.0), 3, 0), Some(HealStep::ObserveOnly));
    assert_eq!(
        ladder.step(&drift(7.0), 3, 60_000_000_000),
        Some(HealStep::ObserveOnly)
    );
}

#[test]
fn a_reading_that_leaves_observe_only_restarts_the_hold() {
    let mut ladder = Ladder::new();

    // A hold, then the drift falls to the recalibrate band, then rises again.
    assert_eq!(ladder.step(&drift(12.0), 0, 0), Some(HealStep::ObserveOnly));
    assert_eq!(ladder.step(&drift(4.0), 0, 9_000_000_000), Some(HealStep::Recalibrate));

    // The new hold starts now; the nine seconds before it do not count.
    assert_eq!(
        ladder.step(&drift(12.0), 0, 10_000_000_000),
        Some(HealStep::ObserveOnly)
    );
    assert_eq!(
        ladder.step(&drift(12.0), 0, 19_000_000_000),
        Some(HealStep::ObserveOnly)
    );
    assert_eq!(
        ladder.step(&drift(12.0), 0, 20_000_000_000),
        Some(HealStep::SafeStop)
    );
}

#[test]
fn no_opinion_leaves_a_live_hold_where_it_was() {
    let mut ladder = Ladder::new();
    assert_eq!(ladder.step(&drift(12.0), 0, 0), Some(HealStep::ObserveOnly));

    // Nothing was measured: no step, and no evidence the hold ended either.
    assert_eq!(ladder.step(&no_opinion(), 0, 5_000_000_000), None);
    assert_eq!(
        ladder.step(&drift(12.0), 0, 10_000_000_000),
        Some(HealStep::SafeStop)
    );
}
