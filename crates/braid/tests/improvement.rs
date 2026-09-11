//! The self-improvement loop's braid-side contract: the agent's dial on the
//! prior's coupling.
//!
//! The dial is arithmetic a consumer can hold, so these tests drive it the way
//! the agent does — one mission outcome per step — and assert on the scale a
//! belief layer would be handed, never on how the step is written.

use qualia_braid::rules::{
    next_coupling_scale, COUPLING_SCALE_CEILING, COUPLING_SCALE_DEFAULT, COUPLING_SCALE_FLOOR,
};

#[test]
fn coupling_scale_is_bounded() {
    // A run of failures carries the dial down one step at a time; the floor is
    // what stops a failure from ever coupling at zero.
    let mut scale = COUPLING_SCALE_DEFAULT;
    for _ in 0..40 {
        scale = next_coupling_scale(scale, false);
        assert!(
            scale >= COUPLING_SCALE_FLOOR,
            "a failure stepped the coupling below its floor: {scale}"
        );
    }
    assert_eq!(
        scale, COUPLING_SCALE_FLOOR,
        "forty failures settle on the floor, not on zero"
    );

    // And a run of successes climbs toward the ceiling; the clamp is what stops
    // it at infinity.
    let mut scale = COUPLING_SCALE_DEFAULT;
    for _ in 0..40 {
        scale = next_coupling_scale(scale, true);
        assert!(
            scale <= COUPLING_SCALE_CEILING,
            "a success stepped the coupling above its ceiling: {scale}"
        );
    }
    assert_eq!(
        scale, COUPLING_SCALE_CEILING,
        "forty successes settle on the ceiling, not on infinity"
    );

    // A reading that is already outside the range — a hand-edited manifest, or
    // a value that is not a number at all — cannot start a step outside it
    // either, from either direction.
    assert_eq!(next_coupling_scale(0.0, false), COUPLING_SCALE_FLOOR);
    assert_eq!(next_coupling_scale(f32::INFINITY, true), COUPLING_SCALE_CEILING);
    assert_eq!(
        next_coupling_scale(f32::NAN, false),
        COUPLING_SCALE_DEFAULT,
        "an unreadable dial is the default dial, not a NaN coupling"
    );
}
