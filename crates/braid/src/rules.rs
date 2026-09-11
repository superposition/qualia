//! The symbolic layer: rules over braid events.
//!
//! A rule is data — an [`EventPattern`] and the [`BraidAction`] it asks for —
//! and [`evaluate`] is the join between them. Given one [`BraidEvent`] and a
//! rule set it returns the action of every rule whose pattern the event
//! matches, in the rule set's own order. That order is the precedence: the
//! first rule in the vector comes first, and a later rule is never shadowed by
//! an earlier one, so an operator can reorder or replace a rule set by handing
//! [`evaluate`] a different vector rather than changing code.
//!
//! Nothing here has I/O, a clock or state. The same event and the same rules
//! always produce the same actions, which is what lets the rule set be a value
//! an operator can hold, diff and review.
//!
//! [`BraidAction`] is a *request*. The braid decides nothing; a caller — the
//! agent, in front of the motion authority — owns what happens next.

use crate::BraidEvent;

/// The bounded dial the mission-outcome rule drives (T30, #46).
///
/// The arithmetic is the coupling's own contract, so it lives beside
/// `CouplingPrior::couple`, which applies the scale a dial reading names. The
/// rule layer re-exports it because the dial is what
/// [`BraidAction::LowerCoupling`] asks for: the failed-mission rule fires the
/// same [`COUPLING_STEP_DOWN`] factor [`next_coupling_scale`] steps by, and the
/// agent persists the bounded reading for the belief runners.
pub use qualia_jepa::prior::{
    clamp_coupling_scale, next_coupling_scale, COUPLING_SCALE_CEILING, COUPLING_SCALE_DEFAULT,
    COUPLING_SCALE_FLOOR, COUPLING_STEP_DOWN, COUPLING_STEP_UP,
};

/// The braid event a rule fires on.
#[derive(Debug, Clone, PartialEq)]
pub enum EventPattern {
    /// A mission closed with `outcome`, compared to the event's own outcome
    /// string. `"failed"` fires only on a failure, not on an abort or a success.
    MissionClosed { outcome: String },
    /// The promotion gate rolled a generation back.
    PromotionRolledBack,
    /// Partial evidence was quarantined.
    Quarantined,
    /// The drift measurement rose above `threshold`.
    ///
    /// No [`BraidEvent`] in this crate's vocabulary carries a drift measurement
    /// yet: T34's [`DriftReport`](crate::drift::DriftReport) is consumed by the
    /// healing ladder directly, so this pattern matches none of the events
    /// [`evaluate`] can be handed today and fires nothing. It is part of the
    /// rule set so the rule is already data for the day the wire carries drift.
    /// The convention that governs it is the drift module's: only a reading
    /// with `sample_count > 0` is an opinion, so a malformed sample must never
    /// fire this rule.
    DriftAbove { threshold: f32 },
}

/// What a fired rule asks the agent to do.
#[derive(Debug, Clone, PartialEq)]
pub enum BraidAction {
    /// Lower the prior's coupling by `factor`, bounded by the caller (T30,
    /// #46: `0.90` carries a failed mission down one step). The caller is the
    /// agent, and [`next_coupling_scale`] is the step it takes: the product is
    /// clamped to [`COUPLING_SCALE_FLOOR`]..[`COUPLING_SCALE_CEILING`] so no
    /// failure can drive the coupling to zero.
    LowerCoupling { factor: f32 },
    /// Feed what survived a quarantine back into training.
    RequestTraining,
    /// Ask for the previous generation to be made current again.
    RollBackGeneration,
    /// Hold the current generation and change nothing.
    EnterObserveOnly,
    /// Ask the motion authority to stop safely. A request, never a command.
    RequestSafeStop,
}

/// A rule: the event pattern that fires it and the action it asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub when: EventPattern,
    pub then: BraidAction,
}

/// The default rule set, in precedence order.
///
/// A failed mission carries the prior's coupling down one step; a promotion
/// rollback and a drift above `3.0` both hold the generation observe-only,
/// because neither is evidence the current generation is trustworthy; a
/// quarantine asks for training on what survived.
pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule {
            when: EventPattern::MissionClosed { outcome: "failed".to_string() },
            then: BraidAction::LowerCoupling { factor: COUPLING_STEP_DOWN },
        },
        Rule { when: EventPattern::PromotionRolledBack, then: BraidAction::EnterObserveOnly },
        Rule { when: EventPattern::Quarantined, then: BraidAction::RequestTraining },
        Rule { when: EventPattern::DriftAbove { threshold: 3.0 }, then: BraidAction::EnterObserveOnly },
    ]
}

/// Every action the rules ask for on `event`, in rule order.
///
/// A rule that does not match contributes nothing, so an event no pattern names
/// — including [`BraidEvent::Unknown`], a strand this build cannot read — fires
/// no action at all.
pub fn evaluate(event: &BraidEvent, rules: &[Rule]) -> Vec<BraidAction> {
    rules
        .iter()
        .filter(|rule| fires(&rule.when, event))
        .map(|rule| rule.then.clone())
        .collect()
}

/// Whether `event` satisfies `pattern`.
fn fires(pattern: &EventPattern, event: &BraidEvent) -> bool {
    match pattern {
        EventPattern::MissionClosed { outcome } => {
            matches!(
                event,
                BraidEvent::MissionClosed { outcome: event_outcome, .. } if event_outcome == outcome
            )
        }
        EventPattern::PromotionRolledBack => {
            matches!(event, BraidEvent::PromotionRolledBack { .. })
        }
        EventPattern::Quarantined => matches!(event, BraidEvent::Quarantined { .. }),
        // No drift-carrying event exists in the vocabulary yet; see the variant
        // docs. This arm keeps the pattern total without inventing a wire field.
        EventPattern::DriftAbove { .. } => false,
    }
}
