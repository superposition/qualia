//! `qualia-health` — the stack's 10 Hz health tap.
//!
//! Once per tick the runner samples every layer slot in the shared-memory
//! arena, flattens each belief into the `#[repr(C)]` [`HealthReport`] the
//! supervisor parses, and writes the reports to stdout as raw bytes in slot
//! order. The bytes are the contract: nothing here is serialized, formatted or
//! buffered beyond one frame at a time, and the frame is flushed as a unit.
//!
//! Configuration is one environment key. `QUALIA_SHM_NAME` names the region to
//! attach to and defaults to [`DEFAULT_SHM_NAME`]. The runner only ever opens a
//! region, never creates one — `qualia-init` owns creation — so a missing
//! region is a startup failure rather than something to paper over.

use std::io::Write;
use std::time::{Duration, Instant};

use qualia_shm::{BeliefSlot, HealthReport, LayerReader, ShmRegion, NUM_LAYERS};

/// The environment key naming the region to attach to.
pub const SHM_NAME_ENV: &str = "QUALIA_SHM_NAME";

/// The region `qualia-init` creates when the supervisor names no other.
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";

/// Wall time between two frames: 10 Hz.
pub const TICK: Duration = Duration::from_millis(100);

/// Bytes one [`HealthReport`] occupies on stdout.
pub const REPORT_BYTES: usize = std::mem::size_of::<HealthReport>();

/// The region name from the environment, falling back to [`DEFAULT_SHM_NAME`].
pub fn shm_name() -> String {
    std::env::var(SHM_NAME_ENV).unwrap_or_else(|_| DEFAULT_SHM_NAME.to_string())
}

/// Flatten one published belief into the supervisor's compact report.
///
/// The report is labelled with the slot index `layer`; a belief's own `layer`
/// field is the writer's stamp and is deliberately not what the consumer reads.
/// Every other field is carried through unchanged.
pub fn health_report(layer: usize, belief: &BeliefSlot) -> HealthReport {
    HealthReport {
        layer: layer as u8,
        compression: belief.compression,
        vfe: belief.vfe,
        challenge_vfe: belief.challenge_vfe,
        confirm_streak: belief.confirm_streak,
        cycle_us: belief.cycle_us,
        timestamp_ns: belief.timestamp_ns,
        _pad: [0; 2],
    }
}

/// Write one frame — every layer slot, in slot order — and flush it.
///
/// Returns the bytes written, always `NUM_LAYERS * REPORT_BYTES`.
pub fn write_frame(shm: &ShmRegion, out: &mut impl Write) -> std::io::Result<usize> {
    let mut written = 0;
    for layer in 0..NUM_LAYERS {
        let reader = LayerReader::new(shm.layer_slot(layer));
        let belief = reader.read();
        out.write_all(&encode_report(&health_report(layer, belief)))?;
        written += REPORT_BYTES;
    }
    out.flush()?;
    Ok(written)
}

/// Publish frames at [`TICK`] until the process ends.
///
/// A failed write is dropped rather than fatal: the consumer may be restarting,
/// and it must not be able to take the health runner down with it. A frame that
/// overruns its tick starts the next one immediately.
pub fn stream(shm: &ShmRegion, out: &mut impl Write) {
    loop {
        let start = Instant::now();
        let _ = write_frame(shm, out);
        let elapsed = start.elapsed();
        if elapsed < TICK {
            std::thread::sleep(TICK - elapsed);
        }
    }
}

/// Encode one report into its stdout bytes.
///
/// The offsets are the `#[repr(C)]` layout of [`HealthReport`], little-endian.
/// They are written out field by field rather than viewed as a raw struct slice
/// so the struct's alignment padding is deterministically zero instead of
/// whatever happened to be on the stack; the layout test pins each offset
/// against `offset_of!`, so the two cannot drift apart.
fn encode_report(report: &HealthReport) -> [u8; REPORT_BYTES] {
    let mut bytes = [0u8; REPORT_BYTES];
    bytes[0] = report.layer;
    bytes[1] = report.compression;
    bytes[2..4].copy_from_slice(&report._pad);
    bytes[4..8].copy_from_slice(&report.vfe.to_le_bytes());
    bytes[8..12].copy_from_slice(&report.challenge_vfe.to_le_bytes());
    bytes[12..16].copy_from_slice(&report.confirm_streak.to_le_bytes());
    bytes[16..20].copy_from_slice(&report.cycle_us.to_le_bytes());
    // Bytes 20..24 are the alignment padding before the u64; they stay zero.
    bytes[24..32].copy_from_slice(&report.timestamp_ns.to_le_bytes());
    bytes
}
