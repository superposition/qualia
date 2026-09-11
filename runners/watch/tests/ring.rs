//! Change-detection tests for the ledger and thought rings.
//!
//! The readers are driven against an in-memory ring, so the tests observe the
//! bytes a source publishes and the sequence bookkeeping, not the terminal.

use qualia_watch::ring::{
    drain_ledger, drain_thoughts, ledger_detail, read_cstr, seq_window, trim_front, LedgerEntry,
    LedgerEvent, LedgerRecord, LedgerSource, SeqCursor, ThoughtEntry, ThoughtRecord, ThoughtSource,
    MAX_THOUGHT_LEN, STATE_DIM,
};

fn ledger_row(seq: u64, vfe: f32, event: LedgerEvent, timestamp_ns: u64) -> LedgerEntry {
    LedgerEntry {
        seq,
        layer: 3,
        event,
        compression: 200,
        _pad: 0,
        vfe,
        residual_norm: 0.25,
        belief_mean: [0.0; STATE_DIM],
        timestamp_ns,
    }
}

fn thought_row(seq: u64, text: &str, layer: u8, kind: u8) -> ThoughtEntry {
    let mut entry = ThoughtEntry {
        text: [0; MAX_THOUGHT_LEN],
        layer,
        kind,
        _pad: [0; 2],
        vfe: 0.1,
        timestamp_ns: 1_000 + seq,
        seq,
    };
    let bytes = text.as_bytes();
    let n = bytes.len().min(MAX_THOUGHT_LEN - 1);
    entry.text[..n].copy_from_slice(&bytes[..n]);
    entry
}

struct FakeLedger {
    seq: u64,
    rows: Vec<Option<LedgerEntry>>,
}

impl FakeLedger {
    fn new(capacity: usize) -> Self {
        Self {
            seq: 0,
            rows: vec![None; capacity],
        }
    }

    /// Mirrors `ShmRegion::append_ledger`: the write sequence is bumped before
    /// the slot is written, so a claimed row is visible before its bytes are.
    fn publish(&mut self, entry: LedgerEntry) {
        let index = (entry.seq as usize) % self.rows.len();
        self.seq = entry.seq + 1;
        self.rows[index] = Some(entry);
    }

    /// Claim the next sequence without writing its slot, as the producer is
    /// between the bump and the write.
    fn claim(&mut self, seq: u64) {
        self.seq = seq + 1;
    }

    fn reset_to(&mut self, seq: u64) {
        self.rows.iter_mut().for_each(|row| *row = None);
        self.seq = seq;
    }
}

impl LedgerSource for FakeLedger {
    fn ledger_seq(&self) -> u64 {
        self.seq
    }

    fn ledger_entry(&self, index: usize) -> Option<&LedgerEntry> {
        self.rows.get(index).and_then(|row| row.as_ref())
    }
}

struct FakeThoughts {
    seq: u64,
    rows: Vec<Option<ThoughtEntry>>,
}

impl FakeThoughts {
    fn new(capacity: usize) -> Self {
        Self {
            seq: 0,
            rows: vec![None; capacity],
        }
    }

    /// Mirrors `ShmRegion::emit_thought`: the write sequence is bumped before
    /// the slot is stamped and filled.
    fn publish(&mut self, entry: ThoughtEntry) {
        let index = (entry.seq as usize) % self.rows.len();
        self.seq = entry.seq + 1;
        self.rows[index] = Some(entry);
    }

    /// Claim the next sequence without writing its slot.
    fn claim(&mut self, seq: u64) {
        self.seq = seq + 1;
    }
}

impl ThoughtSource for FakeThoughts {
    fn thought_seq(&self) -> u64 {
        self.seq
    }

    fn thought(&self, index: usize) -> Option<&ThoughtEntry> {
        self.rows.get(index).and_then(|row| row.as_ref())
    }
}

// ── sequence windows ────────────────────────────────────────────────────

#[test]
fn an_unchanged_sequence_reads_nothing() {
    assert!(seq_window(5, 5, 512).is_empty());
    assert!(seq_window(0, 0, 512).is_empty());
}

#[test]
fn an_incremental_window_starts_after_the_last_read() {
    assert_eq!(seq_window(2, 5, 512), 2..5);
    assert_eq!(seq_window(0, 1, 512), 0..1);
}

#[test]
fn a_large_gap_is_clamped_to_the_newest_capacity() {
    assert_eq!(seq_window(0, 1_000, 512), 488..1_000);
    assert_eq!(seq_window(10, 20, 4), 16..20);
}

#[test]
fn a_backwards_sequence_rescans_the_recreated_region() {
    // A region that has been recreated restarts at zero; the reader must not
    // sit forever waiting for a sequence number it already passed.
    assert_eq!(seq_window(900, 3, 512), 0..3);
    assert_eq!(seq_window(u64::MAX, 1, 4), 0..1);
}

#[test]
fn a_zero_capacity_reads_nothing() {
    assert!(seq_window(0, 100, 0).is_empty());
}

#[test]
fn a_cursor_tracks_the_last_sequence_it_committed() {
    let mut cursor = SeqCursor::new();
    assert_eq!(cursor.window(7, 512), 0..7);
    assert_eq!(cursor.last(), 0, "peeking does not advance the cursor");
    cursor.commit(7);
    assert_eq!(cursor.last(), 7);
    assert!(cursor.window(7, 512).is_empty());
    assert_eq!(cursor.window(9, 512), 7..9);
}

// ── ledger ring ─────────────────────────────────────────────────────────

#[test]
fn published_ledger_rows_are_read_in_order() {
    let mut source = FakeLedger::new(16);
    for seq in 0..3 {
        source.publish(ledger_row(
            seq,
            0.5 + seq as f32,
            LedgerEvent::Challenge,
            100 + seq,
        ));
    }

    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_ledger(&source, &mut cursor, 16, &mut out), 3);
    let seqs: Vec<u64> = out.iter().map(|row| row.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2]);
    assert_eq!(out[1].detail, "vfe=1.500  residual=0.250");
    assert_eq!(out[1].timestamp_ns, 101);
    assert_eq!(out[1].event, LedgerEvent::Challenge);
}

#[test]
fn a_second_drain_without_writes_appends_nothing() {
    let mut source = FakeLedger::new(16);
    source.publish(ledger_row(0, 0.1, LedgerEvent::Confirm, 1));
    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();

    drain_ledger(&source, &mut cursor, 16, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(drain_ledger(&source, &mut cursor, 16, &mut out), 0);
    assert_eq!(out.len(), 1, "corrected rows must not be re-emitted");
}

#[test]
fn a_row_claimed_but_not_yet_written_is_delivered_on_the_next_tick() {
    // `append_ledger` bumps the write sequence before it writes the slot, so a
    // tick can land between the two: sequence 2 is claimed while slot 2 is
    // still empty. One cursor across ticks must retry that row, not skip it.
    let mut source = FakeLedger::new(16);
    source.publish(ledger_row(0, 0.1, LedgerEvent::Confirm, 1));
    source.publish(ledger_row(1, 0.2, LedgerEvent::Confirm, 2));
    source.claim(2);

    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_ledger(&source, &mut cursor, 16, &mut out), 2);
    let seqs: Vec<u64> = out.iter().map(|row| row.seq).collect();
    assert_eq!(seqs, vec![0, 1]);
    assert_eq!(cursor.last(), 2, "the cursor waits on the unfinished row");

    // The writer completes the row and publishes one more.
    source.publish(ledger_row(2, 0.3, LedgerEvent::Confirm, 3));
    source.publish(ledger_row(3, 0.4, LedgerEvent::Confirm, 4));

    out.clear();
    assert_eq!(drain_ledger(&source, &mut cursor, 16, &mut out), 2);
    let seqs: Vec<u64> = out.iter().map(|row| row.seq).collect();
    assert_eq!(
        seqs,
        vec![2, 3],
        "the row written between ticks is delivered exactly once"
    );
    assert_eq!(cursor.last(), 4);
}

#[test]
fn a_torn_row_holds_the_cursor_until_the_write_completes() {
    // Sequence 1 is claimed but its slot still holds the previous row, so the
    // reader shows nothing for it this tick and retries it on the same cursor.
    let mut source = FakeLedger::new(16);
    source.publish(ledger_row(0, 0.1, LedgerEvent::Confirm, 1));
    source.claim(1);
    source.rows[1] = Some(ledger_row(99, 9.9, LedgerEvent::Escalate, 9));

    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_ledger(&source, &mut cursor, 16, &mut out), 1);
    assert_eq!(out[0].seq, 0);
    assert_eq!(cursor.last(), 1, "the torn row stays in the window");

    source.rows[1] = Some(ledger_row(1, 0.2, LedgerEvent::Confirm, 2));
    out.clear();
    assert_eq!(drain_ledger(&source, &mut cursor, 16, &mut out), 1);
    assert_eq!(out[0].seq, 1);
    assert_eq!(out[0].detail, "vfe=0.200");
    assert_eq!(cursor.last(), 2);
}

#[test]
fn a_ring_wrap_reports_the_newest_rows_within_capacity() {
    let mut source = FakeLedger::new(4);
    for seq in 0..10 {
        source.publish(ledger_row(seq, seq as f32, LedgerEvent::Confirm, seq));
    }

    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_ledger(&source, &mut cursor, 4, &mut out), 4);
    let seqs: Vec<u64> = out.iter().map(|row| row.seq).collect();
    assert_eq!(seqs, vec![6, 7, 8, 9]);
}

#[test]
fn a_recreated_ring_is_read_again_from_zero() {
    let mut source = FakeLedger::new(16);
    for seq in 0..5 {
        source.publish(ledger_row(seq, seq as f32, LedgerEvent::Confirm, seq));
    }
    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    drain_ledger(&source, &mut cursor, 16, &mut out);
    assert_eq!(out.len(), 5);

    source.reset_to(0);
    source.publish(ledger_row(0, 7.0, LedgerEvent::Habit, 7));
    drain_ledger(&source, &mut cursor, 16, &mut out);
    assert_eq!(out.len(), 6);
    assert_eq!(out[5].seq, 0);
    assert_eq!(out[5].detail, "compression=200");
}

#[test]
fn a_missing_slot_is_retried_rather_than_panicking() {
    let mut source = FakeLedger::new(4);
    // Sequence 0 is claimed but its slot was never written.
    source.claim(0);
    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_ledger(&source, &mut cursor, 4, &mut out), 0);
    assert_eq!(cursor.last(), 0, "an unwritten slot keeps its sequence");
}

// ── thought ring ────────────────────────────────────────────────────────

#[test]
fn published_thoughts_carry_their_text() {
    let mut source = FakeThoughts::new(8);
    source.publish(thought_row(0, "scene stable", 1, 1));
    source.publish(thought_row(1, "surprise", 255, 2));

    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_thoughts(&source, &mut cursor, 8, &mut out), 2);
    let texts: Vec<&str> = out.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(texts, vec!["scene stable", "surprise"]);
    assert_eq!(out[1].layer, 255);
    assert_eq!(out[1].kind, 2);
}

#[test]
fn an_unchanged_thought_ring_appends_nothing() {
    let mut source = FakeThoughts::new(8);
    source.publish(thought_row(0, "hello", 0, 0));
    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    drain_thoughts(&source, &mut cursor, 8, &mut out);
    assert_eq!(drain_thoughts(&source, &mut cursor, 8, &mut out), 0);
    assert_eq!(out.len(), 1);
}

#[test]
fn a_thought_claimed_but_not_yet_written_is_delivered_on_the_next_tick() {
    let mut source = FakeThoughts::new(8);
    source.publish(thought_row(0, "first", 0, 1));
    source.claim(1);

    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_thoughts(&source, &mut cursor, 8, &mut out), 1);
    assert_eq!(cursor.last(), 1, "the cursor waits on the unfinished thought");

    source.publish(thought_row(1, "second", 2, 4));
    out.clear();
    assert_eq!(drain_thoughts(&source, &mut cursor, 8, &mut out), 1);
    assert_eq!(out[0].seq, 1);
    assert_eq!(out[0].text, "second");
    assert_eq!(out[0].kind, 4);
}

#[test]
fn a_thought_ring_wrap_keeps_the_newest_capacity() {
    let mut source = FakeThoughts::new(3);
    for seq in 0..7 {
        source.publish(thought_row(seq, &format!("t{seq}"), 0, 4));
    }
    let mut cursor = SeqCursor::new();
    let mut out = Vec::new();
    assert_eq!(drain_thoughts(&source, &mut cursor, 3, &mut out), 3);
    let texts: Vec<&str> = out.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(texts, vec!["t4", "t5", "t6"]);
}

// ── record formatting ───────────────────────────────────────────────────

#[test]
fn ledger_details_match_the_event() {
    assert_eq!(
        ledger_detail(LedgerEvent::Challenge, 0.1234, 0.5, 10),
        "vfe=0.123  residual=0.500"
    );
    assert_eq!(ledger_detail(LedgerEvent::Confirm, 0.25, 1.0, 10), "vfe=0.250");
    assert_eq!(ledger_detail(LedgerEvent::Habit, 0.0, 0.0, 77), "compression=77");
    assert_eq!(
        ledger_detail(LedgerEvent::HabitDecay, 0.0, 0.0, 3),
        "compression=3"
    );
    assert_eq!(
        ledger_detail(LedgerEvent::Escalate, 0.9, 0.01, 0),
        "vfe=0.900  residual=0.010"
    );
}

#[test]
fn c_strings_stop_at_the_first_nul() {
    assert_eq!(read_cstr(b"hello\0world"), "hello");
    assert_eq!(read_cstr(b"hello"), "hello");
    assert_eq!(read_cstr(b"\0"), "(empty)");
    assert_eq!(read_cstr(b""), "(empty)");
    assert_eq!(read_cstr(b"\xff\xfe\0"), "\u{fffd}\u{fffd}");
}

#[test]
fn trimming_keeps_the_newest_records() {
    let mut records: Vec<LedgerRecord> = (0..5)
        .map(|seq| LedgerRecord {
            seq,
            timestamp_ns: seq,
            layer: 0,
            event: LedgerEvent::Confirm,
            detail: String::new(),
        })
        .collect();
    trim_front(&mut records, 2);
    let seqs: Vec<u64> = records.iter().map(|row| row.seq).collect();
    assert_eq!(seqs, vec![3, 4]);

    let mut thoughts: Vec<ThoughtRecord> = Vec::new();
    trim_front(&mut thoughts, 4);
    assert!(thoughts.is_empty());
}
