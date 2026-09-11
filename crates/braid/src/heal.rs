//! The healing ladder: which rung the drift escalates to.
//!
//! The ladder is the plan's escalation table (epic #11, step 35), and it is the
//! one place in the tree that compares a drift to `3.0`, `5.0` and `8.0`.
//! [`next_step`] is that table: one [`DriftReport`] and the escalations already
//! attempted give the rung, in order. At or below `3.0` nothing is wrong and the
//! ladder asks for nothing; at or below `5.0` the calibration gate is re-run
//! through `crates/jepa-registry`; at or below `8.0` the registry rolls back to
//! the previous generation, which the runtime keeps resident; above `8.0`, or
//! once three escalations have been attempted and the drift has not come back,
//! the generation is held observe-only.
//!
//! The healthy rung is the gate the budget cannot override: a reading at or
//! below `3.0` selects `None` even when three escalations have been attempted,
//! because the drift coming back *is* the healing succeeding and the caller's
//! attempt count is then stale. The budget is spent on the rungs below it, not
//! on the reading that says they worked.
//!
//! # No opinion
//!
//! A report with `sample_count == 0` is not a measurement — it is what
//! [`crate::drift::measure`] returns for input the JEPA math rejects. Neither
//! the table nor [`Ladder`] acts on it: it selects nothing, and it does not
//! restart a hold, because a malformed sample is not evidence that observe-only
//! ended.
//!
//! # The safe stop
//!
//! `SafeStop` is the one rung the table cannot name from a single reading,
//! because it is a statement about *duration*: observe-only has held for ten
//! seconds with the drift still above `8.0`. [`Ladder`] is the table plus that
//! one piece of state. A reading that selects any other rung drops the hold; the
//! first reading above `8.0` begins one.
//!
//! Every outcome is a *request*. A [`HealStep`] is a value the caller — the
//! agent, in front of the motion authority — owns, like the rule layer's
//! [`BraidAction`](crate::rules::BraidAction). Nothing here has I/O, a clock,
//! a motor or a stop: an agent forwards `SafeStop` to leash and leash decides,
//! exactly as the compute API's authority statement requires. No function in
//! this crate calls a motor, and no test may assert that it does.

use crate::drift::DriftReport;

/// Squared Mahalanobis at or below which the prediction is trustworthy and the
/// ladder asks for nothing. The same `3.0` the rule layer's default
/// [`DriftAbove`](crate::rules::EventPattern::DriftAbove) threshold carries.
const HEALTHY: f32 = 3.0;

/// Squared Mahalanobis at or below which the calibration gate is re-run.
const RECALIBRATE_BELOW: f32 = 5.0;

/// Squared Mahalanobis at or below which the previous generation is rolled back
/// to; above it the generation is held observe-only.
const ROLLBACK_BELOW: f32 = 8.0;

/// Escalations the ladder attempts before it stops walking the rungs and holds
/// observe-only.
const ATTEMPT_LIMIT: u8 = 3;

/// How long observe-only must hold with the drift still above [`ROLLBACK_BELOW`]
/// before the ladder requests a safe stop, in the nanoseconds the rest of the
/// stack reports.
const SAFE_STOP_HOLD_NS: u64 = 10 * 1_000_000_000;

/// A rung of the healing ladder, in escalation order.
///
/// The variants are the escalation outcomes, not actions taken: a caller owns
/// what `Recalibrate` and `RollBack` dispatch and forwards `SafeStop` to the
/// motion authority, which decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealStep {
    /// Re-run the calibration gate through `crates/jepa-registry`.
    Recalibrate,
    /// Ask the registry to roll back to the previous generation, which the
    /// runtime keeps resident.
    RollBack,
    /// Hold the generation and change nothing.
    ObserveOnly,
    /// Ask the motion authority to stop safely. A request, never a command.
    SafeStop,
}

/// The rung `drift` selects after `attempts` escalations, or `None` when the
/// ladder asks for nothing.
///
/// The thresholds are the table's and are inclusive at the top of each band, so
/// `3.0` is healthy, `5.0` recalibrates and `8.0` rolls back. `attempts` is the
/// escalations already made: at [`ATTEMPT_LIMIT`] the ladder stops trying the
/// recalibrate and rollback rungs and holds observe-only, which is why the
/// budget is checked before either.
///
/// A report with `sample_count == 0` is no opinion and selects `None` at any
/// budget; see the module docs.
pub fn next_step(drift: &DriftReport, attempts: u8) -> Option<HealStep> {
    if drift.sample_count == 0 {
        return None;
    }
    if drift.mahalanobis <= HEALTHY {
        return None;
    }
    if attempts >= ATTEMPT_LIMIT {
        return Some(HealStep::ObserveOnly);
    }
    if drift.mahalanobis <= RECALIBRATE_BELOW {
        return Some(HealStep::Recalibrate);
    }
    if drift.mahalanobis <= ROLLBACK_BELOW {
        return Some(HealStep::RollBack);
    }
    Some(HealStep::ObserveOnly)
}

/// The ladder as a running state: [`next_step`] plus the observe-only hold that
/// `SafeStop` needs.
///
/// The only state is when the current observe-only hold began. A reading that
/// selects a rung other than [`HealStep::ObserveOnly`] drops the hold; the first
/// reading that selects observe-only begins one, and it is the caller's clock
/// that dates it — this type holds no clock of its own, so a caller replays a
/// run and gets the same steps back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ladder {
    observe_only_since_ns: Option<u64>,
}

impl Ladder {
    /// A ladder that is not holding observe-only.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one reading at `now_ns` and report the rung it selects.
    ///
    /// `now_ns` is nanoseconds on the caller's clock. A report with
    /// `sample_count == 0` selects nothing and leaves a live hold where it was;
    /// otherwise the step is [`next_step`]'s, except that once observe-only has
    /// held for ten seconds with the drift still above [`ROLLBACK_BELOW`] the
    /// step becomes [`HealStep::SafeStop`] and stays there while the condition
    /// holds — a stop already requested is not un-requested by the next second.
    pub fn step(&mut self, drift: &DriftReport, attempts: u8, now_ns: u64) -> Option<HealStep> {
        if drift.sample_count == 0 {
            return None;
        }

        match next_step(drift, attempts) {
            Some(HealStep::ObserveOnly) => {
                let since_ns = *self.observe_only_since_ns.get_or_insert(now_ns);
                if drift.mahalanobis > ROLLBACK_BELOW
                    && now_ns.saturating_sub(since_ns) >= SAFE_STOP_HOLD_NS
                {
                    Some(HealStep::SafeStop)
                } else {
                    Some(HealStep::ObserveOnly)
                }
            }
            step => {
                self.observe_only_since_ns = None;
                step
            }
        }
    }
}
