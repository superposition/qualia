//! The layer-3 belief runner, observed from outside its own process.
//!
//! The belief loop belongs to the compute backend, so what this binary owns and
//! can be held to is the identity it hands over and what it does where no
//! backend can run the layer: fail, name layer 3 and this runner, and leave the
//! shared arena exactly as it found it.
//!
//! The refusal is only the reachable path where the compiled-in backend cannot
//! run the layer at all. On macOS the metal build blocks; with the `cuda`
//! feature on a CUDA host the layer runs for real (`running at ... Hz`), so the
//! checks below would wait on a live belief loop rather than read a refusal —
//! and the one that hands the child a live arena would wait forever. They
//! therefore cover the metal-on-a-non-macOS-host configuration, where the
//! refusal is structural.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use qualia_shm::{BeliefSlot, LayerReader, LayerWriter, ShmRegion, STATE_DIM};

/// The binary the default stack manifest spawns.
const BIN: &str = env!("CARGO_BIN_EXE_qualia-l3-belief");

/// The layer this runner drives: `belief_visual`, slot 3.
const LAYER: usize = 3;

/// The name the backend is handed, which it repeats in its operator lines.
const NAME: &str = "l3-belief";

/// A region name no other process on the host uses.
fn scratch_name() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "/qualia_l3_belief_runner_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A published belief whose values a freshly created slot cannot hold, so an
/// overwrite cannot pass unnoticed.
fn sentinel_belief() -> BeliefSlot {
    BeliefSlot {
        mean: [0.5; STATE_DIM],
        precision: [2.0; STATE_DIM],
        vfe: 1234.5,
        prediction: [0.0; STATE_DIM],
        residual: [0.0; STATE_DIM],
        challenge_vfe: 67.5,
        confirm_streak: 89,
        compression: 7,
        layer: LAYER as u8,
        _pad: [0; 2],
        timestamp_ns: 4_200_000_000,
        cycle_us: 9_999,
        _pad2: [0; 4],
    }
}

#[test]
fn a_host_without_a_runnable_backend_fails_naming_its_layer() {
    let output = Command::new(BIN)
        .env("QUALIA_SHM_NAME", "/qualia_l3_belief_process_test")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn the l3-belief runner");

    assert_eq!(
        output.status.code(),
        Some(101),
        "a runner that cannot enter its layer must abort, not exit clean"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("layer {LAYER} ({NAME})")),
        "the refusal names the layer and runner it was handed: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing is published on stdout by a layer that never ran"
    );
}

#[test]
fn a_refused_start_leaves_the_layer_slot_as_it_was() {
    // The runner must not run any part of the belief loop itself. If it seeded
    // the slot or published a belief before handing over, a consumer would read
    // a half-started layer from a process that never entered it.
    let name = scratch_name();
    let arena = ShmRegion::create(&name).expect("create the scratch arena");

    let published = sentinel_belief();
    let writer = LayerWriter::new(arena.layer_slot(LAYER));
    *writer.back_buffer() = published;
    writer.publish();
    const TILE: &[f32] = &[1000.0, 1000.5, 1000.25, 1000.75];
    arena.write_weight_tile(LAYER, 0, 0, 2, TILE);

    let flip_before = arena.layer_slot(LAYER).write_idx.load(Ordering::Acquire);

    let run = Command::new(BIN)
        .env("QUALIA_SHM_NAME", &name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn the l3-belief runner");
    assert!(
        !run.status.success(),
        "the runner cannot enter the layer on this host"
    );

    let slot = arena.layer_slot(LAYER);
    assert_eq!(
        slot.write_idx.load(Ordering::Acquire),
        flip_before,
        "the runner published a belief into a layer it never entered"
    );
    let reader = LayerReader::new(slot);
    let seen = reader.read();
    assert_eq!(seen.vfe, published.vfe);
    assert_eq!(seen.challenge_vfe, published.challenge_vfe);
    assert_eq!(seen.confirm_streak, published.confirm_streak);
    assert_eq!(seen.compression, published.compression);
    assert_eq!(seen.timestamp_ns, published.timestamp_ns);
    assert_eq!(seen.cycle_us, published.cycle_us);
    assert_eq!(seen.mean[0], published.mean[0]);
    assert_eq!(
        arena.read_weight_tile(LAYER, 0, 0, 2),
        TILE,
        "the runner seeded layer-3 weights although the layer never ran"
    );
}
