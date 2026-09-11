//! The `qualia-l0-superposition` process contract.
//!
//! L0 owns no loop of its own: it is the bottom of the belief stack, and the
//! kernels that push the sensor plane through it belong to the backend the
//! build selects. The observable thing about this binary is therefore the
//! hand-off — which layer it asks the backend to run, under which name — and
//! that a backend which cannot run a layer says so instead of exiting as if the
//! layer had run.
//!
//! A host without the selected accelerator reaches the refusal path: the
//! fallback build of `qualia-metal` refuses every layer, naming it.

#![cfg(all(not(target_os = "macos"), feature = "metal", not(feature = "cuda")))]

use std::process::Command;

#[test]
fn the_runner_asks_for_layer_zero_by_name_and_a_refusal_is_a_failure() {
    let output = Command::new(env!("CARGO_BIN_EXE_qualia-l0-superposition"))
        .output()
        .expect("the runner binary is built alongside its tests");

    assert!(
        !output.status.success(),
        "a layer that never ran is not a successful run: {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layer 0 (l0-superposition)"),
        "the refusal has to name the layer this binary drives, got: {stderr}"
    );
}
