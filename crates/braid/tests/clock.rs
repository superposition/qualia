//! What the braid's belief clock reports to a consumer.
//!
//! The clock reads what the belief layers and the ledger already write, so each
//! test builds a region the way the runners do — a belief runner fills the back
//! buffer and publishes it, a ledger writer appends a row — and then asserts only
//! the number a caller of [`BeliefClock::decision_latency_ns`] gets back.

use std::sync::atomic::{AtomicU64, Ordering};

use qualia_braid::BeliefClock;
use qualia_shm::{LayerWriter, ShmRegion, MAX_LEDGER_ENTRIES};
use qualia_types::{LedgerEntry, LedgerEvent, NUM_LAYERS, STATE_DIM};

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

/// A region named for this test and this run: two threads of the suite never
/// share one, and a region left behind by a killed process is not reopened.
fn test_region(tag: &str) -> ShmRegion {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    let name = format!("/qualia-braid-clock-{tag}-{}-{index}", std::process::id());
    ShmRegion::create(&name).expect("create test region")
}

/// Commit a belief the way a belief runner does: fill the back buffer, then flip
/// the index so readers see it.
fn commit_belief(region: &ShmRegion, layer: usize, timestamp_ns: u64) {
    let writer = LayerWriter::new(region.layer_slot(layer));
    writer.back_buffer().timestamp_ns = timestamp_ns;
    writer.publish();
}

/// Append one finished ledger row stamped `timestamp_ns`.
fn append_ledger(region: &ShmRegion, timestamp_ns: u64) {
    let entry = LedgerEntry {
        seq: region.ledger_seq(),
        layer: 0,
        event: LedgerEvent::Confirm,
        compression: 0,
        _pad: 0,
        vfe: 0.0,
        residual_norm: 0.0,
        belief_mean: [0.0; STATE_DIM],
        timestamp_ns,
    };
    region.append_ledger(&entry);
}

#[test]
fn latency_is_the_newest_belief_commit_minus_the_newest_ledger_row() {
    let region = test_region("newest");
    commit_belief(&region, 2, 1_000_000_000);
    append_ledger(&region, 400_000_000);
    assert_eq!(BeliefClock::new(&region).decision_latency_ns(), 600_000_000);

    // Neither side stops at the first stamp: the newest stamp on each side is
    // what the latency is measured between.
    commit_belief(&region, NUM_LAYERS - 1, 3_000_000_000);
    append_ledger(&region, 2_750_000_000);
    assert_eq!(BeliefClock::new(&region).decision_latency_ns(), 250_000_000);
}

#[test]
fn latency_is_zero_until_both_a_belief_and_a_ledger_row_exist() {
    let region = test_region("empty");
    assert_eq!(BeliefClock::new(&region).decision_latency_ns(), 0);

    commit_belief(&region, 0, 5_000_000_000);
    assert_eq!(BeliefClock::new(&region).decision_latency_ns(), 0);

    let region = test_region("ledger-only");
    append_ledger(&region, 5_000_000_000);
    assert_eq!(BeliefClock::new(&region).decision_latency_ns(), 0);
}

#[test]
fn a_ledger_row_newer_than_the_belief_commit_reports_zero() {
    let region = test_region("order");
    commit_belief(&region, 4, 2_000_000_000);
    append_ledger(&region, 2_500_000_000);
    assert_eq!(BeliefClock::new(&region).decision_latency_ns(), 0);
}

#[test]
fn the_newest_row_is_read_from_its_wrapped_ring_slot() {
    let region = test_region("wrapped");
    let belief_ns = 10_000_000_000;
    commit_belief(&region, 1, belief_ns);

    // More rows than the ring holds, so the newest one lands back on slot zero
    // with a sequence only the last append could have written.
    for row in 0..=MAX_LEDGER_ENTRIES as u64 {
        append_ledger(&region, 1_000_000 + row);
    }
    assert!(region.ledger_seq() > MAX_LEDGER_ENTRIES as u64);
    assert_eq!(
        BeliefClock::new(&region).decision_latency_ns(),
        belief_ns - (1_000_000 + MAX_LEDGER_ENTRIES as u64)
    );
}
