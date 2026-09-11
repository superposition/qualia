//! ABI pin between the device structs and the Rust `repr(C)` types.
//!
//! NVRTC cannot evaluate a field offset in a constant expression, so
//! `kernels/belief_update.cu` and `kernels/evidence_layout.cu` assert only the
//! total size and the alignment of each mirrored struct. The offsets the
//! device code reads are asserted here, on the host, against the same
//! constants. A layout change in `qualia_types` that is not mirrored in the
//! kernels fails this test before it can corrupt shared memory on a Jetson.
//!
//! Fields that are private to `qualia_types` (the interior of
//! `AppliedActionSlot`, `AppliedActionHistory` and the two JEPA slots) are
//! covered by their size and alignment only; their order is not observable
//! from here.

use std::mem::{align_of, offset_of, size_of};

use qualia_types::{
    AppliedActionHistory, AppliedActionSlot, BeliefSlot, JepaEvidencePayload, JepaEvidenceSlot,
    JepaTelemetryPayload, JepaTelemetrySlot,
};

#[test]
fn belief_slot_layout_matches_the_kernel() {
    assert_eq!(size_of::<BeliefSlot>(), 16_448);
    assert_eq!(align_of::<BeliefSlot>(), 64);
    assert_eq!(offset_of!(BeliefSlot, mean), 0);
    assert_eq!(offset_of!(BeliefSlot, precision), 4_096);
    assert_eq!(offset_of!(BeliefSlot, vfe), 8_192);
    assert_eq!(offset_of!(BeliefSlot, prediction), 8_196);
    assert_eq!(offset_of!(BeliefSlot, residual), 12_292);
    assert_eq!(offset_of!(BeliefSlot, challenge_vfe), 16_388);
    assert_eq!(offset_of!(BeliefSlot, confirm_streak), 16_392);
    assert_eq!(offset_of!(BeliefSlot, compression), 16_396);
    assert_eq!(offset_of!(BeliefSlot, layer), 16_397);
    assert_eq!(offset_of!(BeliefSlot, timestamp_ns), 16_400);
    assert_eq!(offset_of!(BeliefSlot, cycle_us), 16_408);
}

#[test]
fn applied_action_layout_matches_the_kernel() {
    assert_eq!(size_of::<AppliedActionSlot>(), 128);
    assert_eq!(align_of::<AppliedActionSlot>(), 64);
    assert_eq!(offset_of!(AppliedActionSlot, seq), 0);
    assert_eq!(size_of::<AppliedActionHistory>(), 557_120);
    assert_eq!(align_of::<AppliedActionHistory>(), 64);
}

#[test]
fn jepa_evidence_layout_matches_the_kernel() {
    assert_eq!(size_of::<JepaEvidencePayload>(), 23_808);
    assert_eq!(align_of::<JepaEvidencePayload>(), 64);
    assert_eq!(offset_of!(JepaEvidencePayload, producer_epoch), 16);
    assert_eq!(offset_of!(JepaEvidencePayload, source_skew_ns), 96);
    assert_eq!(offset_of!(JepaEvidencePayload, model_id), 128);
    assert_eq!(offset_of!(JepaEvidencePayload, latent), 256);
    assert_eq!(offset_of!(JepaEvidencePayload, predicted_mean), 1_280);
    assert_eq!(
        offset_of!(JepaEvidencePayload, predicted_log_variance),
        2_304
    );
    assert_eq!(offset_of!(JepaEvidencePayload, evidence), 3_328);
    assert_eq!(offset_of!(JepaEvidencePayload, occupancy_logits), 7_424);
    assert_eq!(size_of::<JepaEvidenceSlot>(), 23_872);
    assert_eq!(align_of::<JepaEvidenceSlot>(), 64);
    assert_eq!(offset_of!(JepaEvidenceSlot, seq), 0);
}

#[test]
fn jepa_telemetry_layout_matches_the_kernel() {
    assert_eq!(size_of::<JepaTelemetryPayload>(), 512);
    assert_eq!(align_of::<JepaTelemetryPayload>(), 64);
    assert_eq!(offset_of!(JepaTelemetryPayload, source_skew_ns), 112);
    assert_eq!(offset_of!(JepaTelemetryPayload, model_id), 120);
    assert_eq!(offset_of!(JepaTelemetryPayload, last_error_code), 248);
    assert_eq!(offset_of!(JepaTelemetryPayload, last_error), 256);
    assert_eq!(size_of::<JepaTelemetrySlot>(), 576);
    assert_eq!(align_of::<JepaTelemetrySlot>(), 64);
    assert_eq!(offset_of!(JepaTelemetrySlot, seq), 0);
}
