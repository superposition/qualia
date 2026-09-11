//! The process contract of `qualia-l1-belief`.
//!
//! Everything the belief loop does happens in the backend crate, so the only
//! thing this package can be held to from outside is the hand-off: the binary
//! the stack manifest names exists, it enters the backend selected at build
//! time, and it enters it as layer 1 under the name `l1-belief`. On a host
//! with no Metal device that hand-off ends in the backend's refusal, and the
//! refusal repeats both values — which is exactly what the test reads back.
//!
//! The refusal is only the reachable path where the compiled-in backend cannot
//! run the layer at all. On macOS the metal build blocks; with the `cuda`
//! feature on a CUDA host the layer runs for real (`running at ... Hz`), so the
//! check below would wait on a live belief loop rather than read a refusal, and
//! `Command::output()` has no timeout. It therefore covers the metal-on-a-
//! non-macOS-host configuration, where the refusal is structural.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use std::process::Command;

/// Exit status of the runner pointed at a host with no compute device.
///
/// The runner itself sets no exit code: it neither recovers nor translates a
/// backend failure, so the process ends the way the backend ends it, and a
/// panic must surface as a failure to the supervisor rather than as a clean
/// exit it would read as "layer finished".
#[test]
fn hands_layer_one_to_the_backend_and_fails_where_no_device_exists() {
    let output = Command::new(env!("CARGO_BIN_EXE_qualia-l1-belief"))
        .output()
        .expect("the manifest names this binary, so it must exist");

    assert!(
        !output.status.success(),
        "a layer with no compute device is not a success (status {:?})",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layer 1 (l1-belief)"),
        "the backend should be asked for layer 1 under this layer's name, got: {stderr}"
    );
}
