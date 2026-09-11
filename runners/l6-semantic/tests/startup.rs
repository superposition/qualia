//! Startup contract of the `qualia-l6-semantic` binary.
//!
//! The layer loop itself belongs to the compute backend; what this crate owns
//! is the process the stack manifest starts. On a host with no backend device
//! that process cannot enter the loop, and what an operator and the rest of the
//! stack then observe is fixed here: the exit status, the layer named on the
//! log, and a shared arena with nothing new written into it. Leaving the arena
//! alone matters as much as the refusal — a consumer must never read a layer
//! slot that a runner half-filled before giving up.
//!
//! On macOS the same binary enters the real layer loop and blocks, and with the
//! `cuda` feature on a CUDA host it enters the loop wherever the arena exists
//! (`running at ... Hz`), so `Command::output()` — which has no timeout — would
//! wait forever. These checks therefore cover the hosts where the refusal is
//! the structural path: metal on a non-macOS host.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use qualia_shm::ShmRegion;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_qualia-l6-semantic");

/// Semantic layer's ordinal in the stack.
const LAYER: usize = 6;

/// An arena name no other process on the host uses.
fn scratch_name() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "/qualia_l6_semantic_startup_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A square block the layer-6 loop would own once it started.
fn sentinel_tile() -> Vec<f32> {
    vec![1000.0, 1000.5, 1000.25, 1000.75]
}

#[test]
fn refuses_to_enter_the_layer_and_names_it_on_the_log() {
    let run = Command::new(BIN)
        .output()
        .expect("spawn the l6-semantic runner");

    assert_eq!(
        run.status.code(),
        Some(101),
        "a runner that cannot enter its layer must abort, not exit clean"
    );

    let log = String::from_utf8_lossy(&run.stderr);
    assert!(
        log.contains("layer 6 (l6-semantic)"),
        "the refusal names the layer and runner: {log}"
    );
    assert!(
        run.stdout.is_empty(),
        "the runner publishes nothing on stdout before the layer runs"
    );
}

#[test]
fn a_refused_start_writes_nothing_into_the_arena() {
    let name = scratch_name();
    let arena = ShmRegion::create(&name).expect("create the scratch arena");
    let sentinel = sentinel_tile();
    arena.write_weight_tile(LAYER, 0, 0, 2, &sentinel);
    let ledger_before = arena.ledger_seq();

    let run = Command::new(BIN)
        .env("QUALIA_SHM_NAME", &name)
        .output()
        .expect("spawn the l6-semantic runner");

    assert!(
        !run.status.success(),
        "the runner cannot enter the layer on this host"
    );
    assert_eq!(
        arena.read_weight_tile(LAYER, 0, 0, 2),
        sentinel,
        "the layer-6 weight tile moved although the layer never ran"
    );
    assert_eq!(
        arena.ledger_seq(),
        ledger_before,
        "the ledger advanced although the layer never ran"
    );
}
