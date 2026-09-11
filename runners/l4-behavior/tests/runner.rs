//! What a supervisor observes when it starts `qualia-l4-behavior`.
//!
//! The runner takes no arguments and produces no stdout of its own: it hands
//! the process to the compute backend, so the signal the stack reads back is
//! the exit status and the reason the backend leaves on stderr. On a host with
//! no Metal device the backend refuses to start the layer — the loop itself is
//! covered by the backend's own tests.
//!
//! The refusal is only the reachable path where the compiled-in backend cannot
//! run the layer at all. On macOS the metal build blocks; with the `cuda`
//! feature on a CUDA host the layer runs for real (`running at ... Hz`), so the
//! checks below would wait on a live behaviour loop rather than read a refusal.
//! They therefore cover the metal-on-a-non-macOS-host configuration, where the
//! refusal is structural.
//!
//! The expectations below are spelled out rather than imported from the crate,
//! so they state what the stack manifest asks for instead of echoing whatever
//! the runner happens to hold.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use std::process::Command;

/// Start the runner the way the supervisor does: from the built binary, with no
/// arguments, capturing both streams.
fn start() -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_qualia-l4-behavior"))
        .output()
        .expect("the runner binary should be spawnable")
}

#[test]
fn without_a_metal_device_the_runner_refuses_instead_of_reporting_success() {
    let output = start();
    assert!(
        !output.status.success(),
        "the runner exited successfully on a host with no Metal device; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn the_refusal_names_the_layer_the_runner_was_started_for() {
    let output = start();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layer 4 (l4-behavior)"),
        "the refusal does not name the layer the runner was started for: {stderr}"
    );
}
