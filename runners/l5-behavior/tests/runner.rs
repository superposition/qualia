//! The L5 runner as an operator meets it.
//!
//! Two things are externally visible about this process and nothing else: it
//! identifies itself as layer 5, and it refuses to run when there is no stack
//! for it to attach to. The test below points it at a region that nothing
//! created and reads the process, not any internal state.
//!
//! That refusal is only the reachable path where the compiled-in backend cannot
//! run the layer at all. On macOS the metal build blocks; with the `cuda`
//! feature on a CUDA host the runner only stops because the region named below
//! is absent — against a live arena it would enter the real behaviour loop and
//! `Command::output()` has no timeout. The test therefore covers the
//! metal-on-a-non-macOS-host configuration, where the refusal is structural.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use std::process::{Command, Output};

/// A region name no supervisor would have created. The runner only ever
/// attaches to an arena — `qualia-init` is the one that makes it — so this must
/// end in a refusal rather than a fresh arena.
const ABSENT_REGION: &str = "/qualia-l5-behavior-absent-probe";

fn spawn_against_absent_region() -> Output {
    Command::new(env!("CARGO_BIN_EXE_qualia-l5-behavior"))
        .env("QUALIA_SHM_NAME", ABSENT_REGION)
        .output()
        .expect("the l5-behavior binary must be spawnable")
}

#[test]
fn refuses_to_run_without_a_stack_and_names_layer_five() {
    let output = spawn_against_absent_region();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "the runner reported success with no arena to attach to; stderr: {stderr}"
    );
    assert!(
        stderr.contains("layer 5 (l5-behavior)"),
        "the refusal did not name layer 5 and the runner; stderr: {stderr}"
    );
}
