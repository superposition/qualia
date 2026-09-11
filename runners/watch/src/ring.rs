//! Sequence-number change detection and the record types the panels render.
//!
//! The engine publishes into seqlocked rings: a writer appends at
//! `sequence % capacity` and then bumps the shared write sequence. A reader
//! keeps the last sequence it rendered and asks for the half-open window after
//! it, so a redraw is driven by published bytes rather than by a timer — and an
//! unchanged stack costs one atomic load per ring.
//!
//! The ring types are re-exported here so a [`LedgerSource`]/[`ThoughtSource`]
//! implementation (and its tests) can be written without depending on
//! `qualia-types` directly.

use std::ops::Range;

pub use qualia_shm::MAX_LEDGER_ENTRIES;
pub use qualia_types::{LedgerEntry, LedgerEvent, ThoughtEntry, MAX_THOUGHT_LEN, MAX_THOUGHTS, STATE_DIM};

/// The sequence numbers to read this tick, half-open.
///
/// A window never repeats a sequence the caller already read and never spans
/// more than `capacity` rows, so a reader that fell behind skips to the newest
/// data instead of walking the part of the ring that has been overwritten. A
/// `current_seq` below `last_seq` means the region was recreated; the reader
/// rescans from zero rather than waiting forever for a sequence it has passed.
pub fn seq_window(last_seq: u64, current_seq: u64, capacity: u64) -> Range<u64> {
    if capacity == 0 {
        return 0..0;
    }
    let last_seq = if current_seq < last_seq { 0 } else { last_seq };
    if current_seq <= last_seq {
        return 0..0;
    }
    let start = last_seq.max(current_seq.saturating_sub(capacity));
    start..current_seq
}

/// Remembers the last sequence a reader rendered.
#[derive(Clone, Copy, Debug, Default)]
pub struct SeqCursor {
    last: u64,
}

impl SeqCursor {
    pub const fn new() -> Self {
        Self { last: 0 }
    }

    /// The last sequence handed out.
    pub const fn last(&self) -> u64 {
        self.last
    }

    /// Take the window after the last sequence and advance to `current`.
    pub fn take(&mut self, current: u64, capacity: u64) -> Range<u64> {
        let window = seq_window(self.last, current, capacity);
        self.last = current;
        window
    }
}

/// One ledger row, ready to render.
#[derive(Clone, Debug, PartialEq)]
pub struct LedgerRecord {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub layer: u8,
    pub event: LedgerEvent,
    pub detail: String,
}

/// One thought, ready to render.
#[derive(Clone, Debug, PartialEq)]
pub struct ThoughtRecord {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub layer: u8,
    pub kind: u8,
    pub text: String,
}

/// The ledger ring, as the renderer sees it.
pub trait LedgerSource {
    /// Rows appended since the region was created.
    fn ledger_seq(&self) -> u64;

    /// The row at absolute ring slot `index`, or `None` if it is empty.
    fn ledger_entry(&self, index: usize) -> Option<&LedgerEntry>;
}

/// The thought ring, as the renderer sees it.
pub trait ThoughtSource {
    /// Thoughts appended since the region was created.
    fn thought_seq(&self) -> u64;

    /// The entry at absolute ring slot `index`, or `None` if it is empty.
    fn thought(&self, index: usize) -> Option<&ThoughtEntry>;
}

/// Append every newly published ledger row to `out`; returns how many.
///
/// A slot whose stored `seq` is not the one being read is a write in flight and
/// is skipped, leaving the cursor at `current` so the next tick tries again.
pub fn drain_ledger<S: LedgerSource + ?Sized>(
    source: &S,
    cursor: &mut SeqCursor,
    capacity: u64,
    out: &mut Vec<LedgerRecord>,
) -> usize {
    let window = cursor.take(source.ledger_seq(), capacity);
    let mut appended = 0;
    for seq in window {
        let index = (seq % capacity) as usize;
        let Some(entry) = source.ledger_entry(index) else {
            continue;
        };
        if entry.seq != seq {
            continue;
        }
        out.push(LedgerRecord {
            seq,
            timestamp_ns: entry.timestamp_ns,
            layer: entry.layer,
            event: entry.event,
            detail: ledger_detail(entry.event, entry.vfe, entry.residual_norm, entry.compression),
        });
        appended += 1;
    }
    appended
}

/// Append every newly published thought to `out`; returns how many.
pub fn drain_thoughts<S: ThoughtSource + ?Sized>(
    source: &S,
    cursor: &mut SeqCursor,
    capacity: u64,
    out: &mut Vec<ThoughtRecord>,
) -> usize {
    let window = cursor.take(source.thought_seq(), capacity);
    let mut appended = 0;
    for seq in window {
        let index = (seq % capacity) as usize;
        let Some(entry) = source.thought(index) else {
            continue;
        };
        if entry.seq != seq {
            continue;
        }
        out.push(ThoughtRecord {
            seq,
            timestamp_ns: entry.timestamp_ns,
            layer: entry.layer,
            kind: entry.kind,
            text: read_cstr(&entry.text),
        });
        appended += 1;
    }
    appended
}

/// Keep only the newest `max` records.
pub fn trim_front<T>(records: &mut Vec<T>, max: usize) {
    if records.len() > max {
        let excess = records.len() - max;
        records.drain(..excess);
    }
}

/// Read a NUL-terminated UTF-8 field, lossily.
pub fn read_cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&byte| byte == 0).unwrap_or(buf.len());
    if end == 0 {
        return "(empty)".to_string();
    }
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// The one-line detail a ledger row shows beside its event name.
pub fn ledger_detail(
    event: LedgerEvent,
    vfe: f32,
    residual_norm: f32,
    compression: u8,
) -> String {
    match event {
        LedgerEvent::Challenge | LedgerEvent::Escalate => {
            format!("vfe={vfe:.3}  residual={residual_norm:.3}")
        }
        LedgerEvent::Confirm => format!("vfe={vfe:.3}"),
        LedgerEvent::Habit | LedgerEvent::HabitDecay => format!("compression={compression}"),
    }
}
