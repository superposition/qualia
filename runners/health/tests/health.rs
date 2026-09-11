//! Behavioural tests for the health runner.
//!
//! Everything asserted here is observable from outside the crate: the byte
//! layout a consumer parses off stdout, the layer order of a published frame,
//! the environment key the process reads, and what the process does when the
//! region is missing. The tests drive a real [`ShmRegion`] and, for the stream
//! contract, the real binary, so they fail if the published contract moves.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use qualia_health::{
    health_report, shm_name, write_frame, DEFAULT_SHM_NAME, REPORT_BYTES, SHM_NAME_ENV, TICK,
};
use qualia_shm::{BeliefSlot, LayerWriter, ShmRegion, NUM_LAYERS, STATE_DIM};

/// Unique region name per test *and* per run, so the suite never reuses a
/// mapping another test or a killed earlier process left behind — the same
/// discipline the shm crate's own tests use.
static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region_name(tag: &str) -> String {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    format!("/qualia_health_test_{}_{}_{}", std::process::id(), tag, index)
}

/// A belief with every health-relevant field carrying a distinct value, so a
/// swap between two of them cannot pass unnoticed.
fn belief(layer: u8, compression: u8, vfe: f32, challenge_vfe: f32, streak: u32, cycle_us: u32, timestamp_ns: u64) -> BeliefSlot {
    BeliefSlot {
        mean: [0.0; STATE_DIM],
        precision: [0.0; STATE_DIM],
        vfe,
        prediction: [0.0; STATE_DIM],
        residual: [0.0; STATE_DIM],
        challenge_vfe,
        confirm_streak: streak,
        compression,
        layer,
        _pad: [0; 2],
        timestamp_ns,
        cycle_us,
        _pad2: [0; 4],
    }
}

/// Publish `value` as the newest belief of `region`'s `layer` slot.
fn publish(region: &ShmRegion, layer: usize, value: BeliefSlot) {
    let writer = LayerWriter::new(region.layer_slot(layer));
    *writer.back_buffer() = value;
    writer.publish();
}

/// A frame's worth of `HealthReport`s decoded at their `#[repr(C)]` offsets.
#[derive(Debug, PartialEq)]
struct Decoded {
    layer: u8,
    compression: u8,
    vfe: f32,
    challenge_vfe: f32,
    confirm_streak: u32,
    cycle_us: u32,
    timestamp_ns: u64,
}

fn decode(bytes: &[u8]) -> Vec<Decoded> {
    assert_eq!(bytes.len() % REPORT_BYTES, 0, "frame is a whole number of reports");
    bytes
        .chunks_exact(REPORT_BYTES)
        .map(|row| Decoded {
            layer: row[0],
            compression: row[1],
            vfe: f32::from_le_bytes(row[4..8].try_into().unwrap()),
            challenge_vfe: f32::from_le_bytes(row[8..12].try_into().unwrap()),
            confirm_streak: u32::from_le_bytes(row[12..16].try_into().unwrap()),
            cycle_us: u32::from_le_bytes(row[16..20].try_into().unwrap()),
            timestamp_ns: u64::from_le_bytes(row[24..32].try_into().unwrap()),
        })
        .collect()
}

/// Kills the health process on drop so a failing assertion cannot leak it.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_wire_layout_is_the_declared_repr_c_layout() {
    // Pins the offsets `encode_report` writes against the struct definition, so
    // a change in qualia-types fails here instead of silently shifting bytes.
    use std::mem::{offset_of, size_of};
    assert_eq!(REPORT_BYTES, size_of::<qualia_shm::HealthReport>());
    assert_eq!(offset_of!(qualia_shm::HealthReport, layer), 0);
    assert_eq!(offset_of!(qualia_shm::HealthReport, compression), 1);
    assert_eq!(offset_of!(qualia_shm::HealthReport, vfe), 4);
    assert_eq!(offset_of!(qualia_shm::HealthReport, challenge_vfe), 8);
    assert_eq!(offset_of!(qualia_shm::HealthReport, confirm_streak), 12);
    assert_eq!(offset_of!(qualia_shm::HealthReport, cycle_us), 16);
    assert_eq!(offset_of!(qualia_shm::HealthReport, timestamp_ns), 24);
}

#[test]
fn defaults_and_environment_key_are_the_documented_ones() {
    assert_eq!(SHM_NAME_ENV, "QUALIA_SHM_NAME");
    assert_eq!(DEFAULT_SHM_NAME, "/qualia_body");
    // These two are what a supervisor reads out of the process table, so they
    // are part of the contract rather than an implementation detail.
    assert_eq!(REPORT_BYTES, 32);
    assert_eq!(TICK, std::time::Duration::from_millis(100));

    std::env::remove_var(SHM_NAME_ENV);
    assert_eq!(shm_name(), DEFAULT_SHM_NAME, "no override falls back to the default");

    std::env::set_var(SHM_NAME_ENV, "/qualia_health_override");
    assert_eq!(shm_name(), "/qualia_health_override", "the override is read verbatim");
    std::env::remove_var(SHM_NAME_ENV);
}

#[test]
fn report_carries_the_belief_and_the_slot_layer() {
    // The slot index is the layer the supervisor reads, even if the belief
    // itself was stamped with something stale.
    let value = belief(7, 3, 1.5, 2.5, 41, 900, 1_700_000_000);
    let report = health_report(2, &value);

    assert_eq!(report.layer, 2, "the report labels the slot, not the belief");
    assert_eq!(report.compression, 3);
    assert_eq!(report._pad, [0; 2]);
    assert_eq!(report.vfe, 1.5);
    assert_eq!(report.challenge_vfe, 2.5);
    assert_eq!(report.confirm_streak, 41);
    assert_eq!(report.cycle_us, 900);
    assert_eq!(report.timestamp_ns, 1_700_000_000);
}

#[test]
fn frame_is_one_report_per_layer_in_slot_order() {
    let name = region_name("order");
    let region = ShmRegion::create(&name).expect("create region");
    for layer in 0..NUM_LAYERS {
        publish(
            &region,
            layer,
            belief(
                (layer + 1) as u8,
                layer as u8,
                layer as f32,
                10.0 + layer as f32,
                100 + layer as u32,
                1_000 + layer as u32,
                5_000 + layer as u64,
            ),
        );
    }

    let mut sink = Vec::new();
    let written = write_frame(&region, &mut sink).expect("write frame");
    assert_eq!(written, NUM_LAYERS * REPORT_BYTES);

    let rows = decode(&sink);
    assert_eq!(rows.len(), NUM_LAYERS);
    for (layer, row) in rows.iter().enumerate() {
        assert_eq!(row.layer, layer as u8, "reports are ordered by slot");
        assert_eq!(row.compression, layer as u8);
        assert_eq!(row.vfe, layer as f32);
        assert_eq!(row.challenge_vfe, 10.0 + layer as f32);
        assert_eq!(row.confirm_streak, 100 + layer as u32);
        assert_eq!(row.cycle_us, 1_000 + layer as u32);
        assert_eq!(row.timestamp_ns, 5_000 + layer as u64);
    }
}

#[test]
fn a_region_with_no_publisher_still_yields_a_labelled_zero_frame() {
    // The degraded case: a fresh region reads as zeros, and the consumer must
    // still be able to tell which layer each report belongs to.
    let name = region_name("empty");
    let region = ShmRegion::create(&name).expect("create region");

    let mut sink = Vec::new();
    write_frame(&region, &mut sink).expect("write frame");

    let rows = decode(&sink);
    assert_eq!(rows.len(), NUM_LAYERS);
    for (layer, row) in rows.iter().enumerate() {
        assert_eq!(row.layer, layer as u8);
        assert_eq!(row.vfe, 0.0);
        assert_eq!(row.confirm_streak, 0);
        assert_eq!(row.timestamp_ns, 0);
    }
}

#[test]
fn the_binary_streams_a_frame_of_the_live_region() {
    // End to end: the real process attaches to a region this test created,
    // reads its published beliefs and emits the frame on stdout.
    let name = region_name("stream");
    let region = ShmRegion::create(&name).expect("create region");
    publish(&region, 0, belief(0, 9, 0.25, 0.5, 7, 250, 42));
    publish(&region, NUM_LAYERS - 1, belief(7, 4, 8.0, 9.0, 11, 800, 77));

    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_qualia-health"))
            .env(SHM_NAME_ENV, &name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the health binary"),
    );

    let mut stdout = child.0.stdout.take().expect("piped stdout");
    let mut frame = vec![0u8; NUM_LAYERS * REPORT_BYTES];
    stdout.read_exact(&mut frame).expect("read exactly one frame");

    let rows = decode(&frame);
    assert_eq!(rows[0].compression, 9);
    assert_eq!(rows[0].vfe, 0.25);
    assert_eq!(rows[0].cycle_us, 250);
    assert_eq!(rows[0].timestamp_ns, 42);
    assert_eq!(rows[NUM_LAYERS - 1].layer, (NUM_LAYERS - 1) as u8);
    assert_eq!(rows[NUM_LAYERS - 1].challenge_vfe, 9.0);
    assert_eq!(rows[NUM_LAYERS - 1].timestamp_ns, 77);
}

#[test]
fn the_binary_names_the_region_and_fails_when_it_is_absent() {
    let name = region_name("missing");
    let output = Command::new(env!("CARGO_BIN_EXE_qualia-health"))
        .env(SHM_NAME_ENV, &name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run the health binary");

    assert!(!output.status.success(), "a missing region must not look healthy");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("qualia-health: opening shm '{name}'")),
        "the configured region name is logged before the attempt, got: {stderr}"
    );
    assert!(
        stderr.contains("Failed to open shm"),
        "the failure names the open, got: {stderr}"
    );
}

#[test]
fn the_binary_keeps_ticking_after_its_consumer_disconnects() {
    // A consumer restart must not take the health runner with it: once stdout
    // is closed the process keeps reading the region and discarding frames.
    let name = region_name("closed");
    // Held for the child's lifetime: the mapping name exists while this handle does.
    let _region = ShmRegion::create(&name).expect("create region");

    let mut child = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_qualia-health"))
            .env(SHM_NAME_ENV, &name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the health binary"),
    );

    {
        let mut stdout = child.0.stdout.take().expect("piped stdout");
        let mut frame = vec![0u8; NUM_LAYERS * REPORT_BYTES];
        stdout.read_exact(&mut frame).expect("read one frame");
        // Dropping the read end closes the pipe on both platforms.
    }

    std::thread::sleep(TICK * 4);
    assert!(
        child.0.try_wait().expect("poll the child").is_none(),
        "a disconnected consumer must not stop the health loop"
    );
}
