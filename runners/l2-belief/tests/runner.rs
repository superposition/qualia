//! The process contract `qualia-l2-belief` keeps where no compute backend can
//! run: it must still say which layer it is, and it must fail rather than idle.
//!
//! The belief loop itself belongs to the backend crate named by the feature
//! flags, so a host with neither an Apple GPU nor a CUDA driver has nothing to
//! drive. What is observable from outside the process is the exit status and
//! the line it writes to stderr; that line carries this runner's identity, and
//! it is what an operator reads when the supervisor keeps restarting a layer.
//!
//! That refusal is only the reachable path where the compiled-in backend cannot
//! run the layer at all. On macOS the metal build blocks; with the `cuda`
//! feature on a CUDA host the layer runs for real (`running at ... Hz`), so the
//! check below would wait on a live belief loop rather than read a refusal, and
//! `Command::output()` has no timeout. It therefore covers the metal-on-a-
//! non-macOS-host configuration, where the refusal is structural.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use std::process::Command;

/// The layer this binary drives, counted up from the sensor plane.
const LAYER: u8 = 2;

/// The name the backend is handed, which it repeats in its own lines.
const NAME: &str = "l2-belief";

#[test]
fn a_host_without_a_backend_fails_naming_its_layer() {
    let output = Command::new(env!("CARGO_BIN_EXE_qualia-l2-belief"))
        .env("QUALIA_SHM_NAME", "/qualia_l2_belief_process_test")
        .output()
        .expect("qualia-l2-belief should be runnable");

    assert!(
        !output.status.success(),
        "a runner with no backend must not report success: {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("layer {LAYER} ({NAME})")),
        "the refusal must name this layer and runner, got: {stderr}"
    );
}
