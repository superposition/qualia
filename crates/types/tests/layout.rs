//! The `FlySimSlot` shared-memory layout.
//!
//! The slot carries the invented fly rate model's output to whoever is
//! watching, and nothing else: one seqlocked payload, published by the JEPA
//! runtime when the fly mode is `sim`, read by observers that never write.
//! Layout is pinned as hard numbers because a mapped consumer sees the bytes,
//! not the fields; a size here that moves is a silent break.

use qualia_types::*;
use std::alloc::{alloc_zeroed, Layout};
use std::mem::{align_of, offset_of, size_of};
use std::sync::atomic::Ordering;

/// Allocates a zeroed, correctly aligned slot, the way a mapped consumer gets
/// one: this type has no constructor on purpose.
fn slot() -> &'static FlySimSlot {
    let layout = Layout::from_size_align(size_of::<FlySimSlot>(), align_of::<FlySimSlot>())
        .expect("valid layout");
    // SAFETY: the layout is non-empty and the zeroed bytes are a valid initial
    // state for the sequence word and the payload.
    let pointer = unsafe { alloc_zeroed(layout) } as *mut FlySimSlot;
    assert!(!pointer.is_null(), "allocation failed");
    unsafe { &*pointer }
}

#[test]
fn fly_sim_slot_is_repr_c() {
    assert_eq!(align_of::<FlySimSlot>(), 64);
    assert_eq!(offset_of!(FlySimSlot, seq), 0);
    // The payload field is private, so its offset is pinned by the two sizes:
    // the sequence word is one `AtomicU64` at the base and the payload is
    // 64-byte aligned, leaving it at 64.
    assert_eq!(size_of::<FlySimSlot>(), 64 + size_of::<FlySimPayload>());
    assert_eq!(size_of::<FlySimSlot>(), 65_984);

    assert_eq!(align_of::<FlySimPayload>(), 64);
    assert_eq!(offset_of!(FlySimPayload, abi_version), 0);
    assert_eq!(offset_of!(FlySimPayload, type_count), 4);
    assert_eq!(offset_of!(FlySimPayload, flags), 8);
    assert_eq!(offset_of!(FlySimPayload, producer_epoch), 16);
    assert_eq!(offset_of!(FlySimPayload, runner_epoch), 24);
    assert_eq!(offset_of!(FlySimPayload, sim_step), 32);
    assert_eq!(offset_of!(FlySimPayload, timestamp_ns), 40);
    assert_eq!(offset_of!(FlySimPayload, last_error_len), 48);
    assert_eq!(offset_of!(FlySimPayload, dt), 52);
    assert_eq!(offset_of!(FlySimPayload, sim_id), 64);
    assert_eq!(offset_of!(FlySimPayload, last_error), 128);
    assert_eq!(offset_of!(FlySimPayload, state), 384);
    assert_eq!(size_of::<FlySimPayload>(), 65_920);

    assert_eq!(FLY_SIM_MAX_TYPES * size_of::<f32>(), 65_536);
}

#[test]
fn fly_sim_slot_round_trips_observe_only() {
    let slot = slot();
    assert_eq!(slot.snapshot(4).expect("fresh slot").type_count, 0);

    let mut published = FlySimPayload {
        producer_epoch: 1,
        runner_epoch: 2,
        sim_step: 3,
        timestamp_ns: 4_000,
        dt: 0.001,
        flags: JEPA_FLAG_VALID | JEPA_FLAG_OUTPUT_FINITE,
        ..FlySimPayload::default()
    };
    published.type_count = 2;
    published.state[0] = 0.25;
    published.state[1] = -1.5;
    published.sim_id[..12].copy_from_slice(b"test.rate.v1");

    assert_eq!(slot.publish(published).expect("publish"), 2);
    let read = slot.snapshot(4).expect("snapshot");
    assert_eq!(read.type_count, 2);
    assert_eq!(read.state[0], 0.25);
    assert_eq!(read.state[1], -1.5);
    assert_eq!(read.dt, 0.001);
    assert_eq!(read.sim_step, 3);
    assert_eq!(&read.sim_id[..12], b"test.rate.v1");
    assert_eq!(read.flags, JEPA_FLAG_VALID | JEPA_FLAG_OUTPUT_FINITE);
    assert_eq!(read.abi_version, FLY_SIM_ABI_VERSION);

    // An observer never sees half a publication: an odd sequence is a read the
    // writer has not finished, so it is refused rather than returned torn.
    slot.seq.store(3, Ordering::Release);
    assert!(matches!(slot.snapshot(2), Err(SnapshotError::TornRead)));
    slot.seq.store(4, Ordering::Release);

    // And a second publisher is refused while the first holds the slot.
    slot.seq.store(5, Ordering::Release);
    assert!(matches!(
        slot.publish(FlySimPayload::default()),
        Err(SnapshotError::WriterBusy)
    ));
}
