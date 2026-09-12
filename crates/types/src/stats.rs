//! Runner telemetry stats frames: the fixed-width record an operator's HUD reads.
//!
//! Every runner that has something to say about its own work publishes one
//! [`RunnerStats`] slot into a small stats region beside the data it describes.
//! The framing is the house one — a magic, a version, then fixed-width fields —
//! with no JSON, no length prefixes and no allocation on the publish path, and a
//! seqlock so a reader never sees a half-written frame.
//!
//! The fields are the ones an operator needs at a glance: the tick or frame rate
//! (`rate_milli_hz`), the byte rate (`bytes_per_sec`), the queue depth
//! (`backlog`), the last values the runner emitted (`values` and their `labels`),
//! the error count (`errors`) and the time of the last update
//! (`published_at_ns`). A runner that has just started publishes
//! `rate_milli_hz == 0` rather than a guessed cadence, and a runner whose work
//! failed counts it in `errors` instead of stopping the panel.
//!
//! The region is addressed the way every other region in this stack is: the
//! data arena's own name ([`crate::SHM_MAGIC`]'s region, `/qualia_body` by
//! default) with [`STATS_REGION_SUFFIX`] appended. The supervisor creates it
//! beside the arena it owns; a producer that starts before the supervisor (or a
//! producer run by hand) creates it itself, and a reader that finds no region
//! says so rather than drawing zeroes.

use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

use crate::SnapshotError;

/// Magic stamped into every runner stats frame: `QLST`.
pub const RUNNER_STATS_MAGIC: u32 = 0x514C_5354;

/// Frame layout version. A reader that does not know this value refuses the
/// frame rather than misreading the fields behind it.
pub const RUNNER_STATS_VERSION: u16 = 1;

/// Runner slots the stats region holds. The supervisor creates the region; the
/// count is ABI, so a reader walks it rather than assuming any runner's name.
pub const RUNNER_STATS_SLOTS: usize = 32;

/// Bytes of a runner's name in the frame, NUL-padded.
pub const RUNNER_NAME_BYTES: usize = 32;

/// Last emitted values a frame carries.
pub const RUNNER_VALUE_COUNT: usize = 4;

/// Bytes of one value's label in the frame, NUL-padded.
///
/// The width is ABI ([`RUNNER_STATS_VERSION`]), so a producer chooses a label
/// that fits rather than the frame growing: a longer one is truncated on a
/// UTF-8 boundary, which is how `l0 compression` reaches a panel as
/// `l0 compressi`. Every producer in this tree now names its values inside the
/// field; keep it that way when adding one.
pub const RUNNER_VALUE_LABEL_BYTES: usize = 12;

/// The producer has published at least once and its last update is within the
/// cadence it advertises; clear once a reader can see it has gone quiet.
pub const RUNNER_STATS_FLAG_PUBLISHING: u16 = 1 << 0;

/// The runner's last operation failed; `errors` carries the count.
pub const RUNNER_STATS_FLAG_ERRORED: u16 = 1 << 1;

/// Magic stamped into the stats region's own header: `QLSTREGN`.
pub const STATS_REGION_MAGIC: u64 = 0x514C_5354_5245_474E;

/// Stats region layout version.
pub const STATS_REGION_VERSION: u32 = 1;

/// Appended to the data arena's name to name its stats region.
pub const STATS_REGION_SUFFIX: &str = "_stats";

/// Bytes the stats region header occupies before the first slot.
pub const STATS_HEADER_SIZE: usize = std::mem::size_of::<StatsHeader>();

/// Bytes one runner slot occupies.
pub const RUNNER_STATS_SIZE: usize = std::mem::size_of::<RunnerStats>();

/// Bytes the stats region maps: the header plus every slot.
pub const STATS_REGION_SIZE: usize = STATS_HEADER_SIZE + RUNNER_STATS_SLOTS * RUNNER_STATS_SIZE;

/// The stats region's header, at offset zero.
///
/// `slots_claimed` is bumped with `fetch_add` when a producer attaches, so two
/// producers that start at the same instant land on different slots without
/// either one reading a name list.
#[repr(C)]
pub struct StatsHeader {
    pub magic: u64,
    pub version: u32,
    pub slot_count: u32,
    pub slots_claimed: AtomicU32,
    pub _pad: u32,
}

impl StatsHeader {
    /// Whether this build can read the region the header describes.
    pub fn is_current(&self) -> bool {
        self.magic == STATS_REGION_MAGIC
            && self.version == STATS_REGION_VERSION
            && self.slot_count == RUNNER_STATS_SLOTS as u32
    }

    /// Slots a producer has claimed, capped at [`RUNNER_STATS_SLOTS`].
    pub fn claimed(&self) -> usize {
        (self.slots_claimed.load(Ordering::Acquire) as usize).min(RUNNER_STATS_SLOTS)
    }
}

/// One runner's fixed-width stats frame.
///
/// Single writer per slot, many readers; the layout is frozen by
/// [`RUNNER_STATS_VERSION`]. Fields are written under the seqlock and read back
/// through [`RunnerStats::snapshot`].
#[repr(C)]
pub struct RunnerStats {
    /// Seqlock: odd while a writer is mid-publish, even when the frame is whole.
    pub seq: AtomicU64,
    /// [`RUNNER_STATS_MAGIC`], so a slot that was never claimed reads as empty.
    pub magic: u32,
    /// [`RUNNER_STATS_VERSION`]; a frame from another build is refused.
    pub version: u16,
    /// [`RUNNER_STATS_FLAG_PUBLISHING`] et al.
    pub flags: u16,
    /// The runner's name, NUL-padded.
    pub runner: [u8; RUNNER_NAME_BYTES],
    /// When the runner attached, for an operator who wants to see a restart.
    pub started_at_ns: u64,
    /// The time of the last update: every frame carries its own clock.
    pub published_at_ns: u64,
    /// Units of work completed since the runner attached.
    pub ticks: u64,
    /// Failed operations since the runner attached.
    pub errors: u64,
    /// Bytes moved since the runner attached.
    pub bytes: u64,
    /// Bytes moved over the writer's last one-second window.
    pub bytes_per_sec: u64,
    /// Ticks per second over the writer's last one-second window, in millihertz
    /// so a sub-1 Hz stream keeps its precision.
    pub rate_milli_hz: u32,
    /// Queue depth or backlog the runner is carrying.
    pub backlog: u32,
    /// The last values the runner emitted, in the runner's own order.
    pub values: [f32; RUNNER_VALUE_COUNT],
    /// What each value is, NUL-padded; a label the console does not know is
    /// still shown as itself.
    pub labels: [[u8; RUNNER_VALUE_LABEL_BYTES]; RUNNER_VALUE_COUNT],
}

/// Owned copy of one coherent [`RunnerStats`] frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerStatsSnapshot {
    pub seq: u64,
    pub flags: u16,
    pub runner: String,
    pub started_at_ns: u64,
    pub published_at_ns: u64,
    pub ticks: u64,
    pub errors: u64,
    pub bytes: u64,
    pub bytes_per_sec: u64,
    pub rate_milli_hz: u32,
    pub backlog: u32,
    pub values: [f32; RUNNER_VALUE_COUNT],
    pub labels: [String; RUNNER_VALUE_COUNT],
}

impl Default for RunnerStatsSnapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            flags: 0,
            runner: String::new(),
            started_at_ns: 0,
            published_at_ns: 0,
            ticks: 0,
            errors: 0,
            bytes: 0,
            bytes_per_sec: 0,
            rate_milli_hz: 0,
            backlog: 0,
            values: [0.0; RUNNER_VALUE_COUNT],
            labels: std::array::from_fn(|_| String::new()),
        }
    }
}

impl RunnerStatsSnapshot {
    /// An empty frame for `runner`, before its first publish.
    pub fn new(runner: &str) -> Self {
        Self {
            runner: runner.to_owned(),
            ..Self::default()
        }
    }

    /// The advertised rate in hertz.
    pub fn rate_hz(&self) -> f64 {
        f64::from(self.rate_milli_hz) / 1000.0
    }

    /// Whether the producer says it is still publishing.
    pub fn is_publishing(&self) -> bool {
        self.flags & RUNNER_STATS_FLAG_PUBLISHING != 0
    }

    /// Whether the producer's last operation failed.
    pub fn is_errored(&self) -> bool {
        self.flags & RUNNER_STATS_FLAG_ERRORED != 0
    }

    /// The label of value `index`, or the empty string when it is unset.
    pub fn label(&self, index: usize) -> &str {
        self.labels.get(index).map(String::as_str).unwrap_or("")
    }
}

impl RunnerStats {
    /// Publish one frame. Returns the new even sequence number.
    ///
    /// The same contract as every other seqlocked slot in this ABI: the
    /// sequence goes odd across the field copies, so a reader that sees an odd
    /// value (or an even value that moved) retries instead of trusting a torn
    /// frame.
    pub fn publish(&mut self, snapshot: &RunnerStatsSnapshot) -> Result<u64, SnapshotError> {
        let current = self.seq.load(Ordering::Acquire);
        if current & 1 != 0 {
            return Err(SnapshotError::WriterBusy);
        }
        let writing = current.wrapping_add(1);
        self.seq
            .compare_exchange(current, writing, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SnapshotError::WriterBusy)?;

        self.magic = RUNNER_STATS_MAGIC;
        self.version = RUNNER_STATS_VERSION;
        self.flags = snapshot.flags;
        write_fixed(&mut self.runner, &snapshot.runner);
        self.started_at_ns = snapshot.started_at_ns;
        self.published_at_ns = snapshot.published_at_ns;
        self.ticks = snapshot.ticks;
        self.errors = snapshot.errors;
        self.bytes = snapshot.bytes;
        self.bytes_per_sec = snapshot.bytes_per_sec;
        self.rate_milli_hz = snapshot.rate_milli_hz;
        self.backlog = snapshot.backlog;
        self.values = snapshot.values;
        for (index, label) in snapshot.labels.iter().enumerate() {
            write_fixed(&mut self.labels[index], label);
        }

        let published = writing.wrapping_add(1);
        self.seq.store(published, Ordering::Release);
        Ok(published)
    }

    /// Take an owned copy, retrying at most `max_attempts` times.
    pub fn snapshot(&self, max_attempts: usize) -> Result<RunnerStatsSnapshot, SnapshotError> {
        for _ in 0..max_attempts.max(1) {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            // SAFETY: the writer keeps the sequence odd across every field copy
            // and issues a release store only after the last one, so a volatile
            // read of plain fields between two equal even reads is a whole frame.
            let magic = unsafe { std::ptr::read_volatile(&self.magic) };
            let version = unsafe { std::ptr::read_volatile(&self.version) };
            let flags = unsafe { std::ptr::read_volatile(&self.flags) };
            let runner = unsafe { std::ptr::read_volatile(&self.runner) };
            let started_at_ns = unsafe { std::ptr::read_volatile(&self.started_at_ns) };
            let published_at_ns = unsafe { std::ptr::read_volatile(&self.published_at_ns) };
            let ticks = unsafe { std::ptr::read_volatile(&self.ticks) };
            let errors = unsafe { std::ptr::read_volatile(&self.errors) };
            let bytes = unsafe { std::ptr::read_volatile(&self.bytes) };
            let bytes_per_sec = unsafe { std::ptr::read_volatile(&self.bytes_per_sec) };
            let rate_milli_hz = unsafe { std::ptr::read_volatile(&self.rate_milli_hz) };
            let backlog = unsafe { std::ptr::read_volatile(&self.backlog) };
            let values = unsafe { std::ptr::read_volatile(&self.values) };
            let raw_labels = unsafe { std::ptr::read_volatile(&self.labels) };
            fence(Ordering::Acquire);
            let after = self.seq.load(Ordering::Acquire);
            if before != after || after & 1 != 0 {
                continue;
            }
            if magic != RUNNER_STATS_MAGIC || version != RUNNER_STATS_VERSION {
                return Err(SnapshotError::TornRead);
            }
            return Ok(RunnerStatsSnapshot {
                seq: after,
                flags,
                runner: read_fixed(&runner),
                started_at_ns,
                published_at_ns,
                ticks,
                errors,
                bytes,
                bytes_per_sec,
                rate_milli_hz,
                backlog,
                values,
                labels: std::array::from_fn(|index| read_fixed(&raw_labels[index])),
            });
        }
        Err(SnapshotError::TornRead)
    }
}

/// Write `text` into a fixed-width NUL-padded field, truncating on a UTF-8
/// boundary so a reader never decodes a partial character.
pub fn write_fixed(bytes: &mut [u8], text: &str) {
    let mut end = text.len().min(bytes.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    bytes[..end].copy_from_slice(&text.as_bytes()[..end]);
    for byte in &mut bytes[end..] {
        *byte = 0;
    }
}

/// Read a fixed-width NUL-padded field back to a string.
pub fn read_fixed(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// The stats region name for a data arena: `QUALIA_STATS_SHM_NAME` when the
/// deployment sets it, otherwise the arena's own name plus the suffix.
pub fn stats_region_name(data_region: &str, configured: Option<&str>) -> String {
    match configured {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => format!("{data_region}{STATS_REGION_SUFFIX}"),
    }
}
