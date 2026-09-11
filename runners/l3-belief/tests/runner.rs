//! Behavioural tests for the layer-3 belief runner.
//!
//! The belief loop is the backend's, so what this crate can be held to from
//! outside is its identity and its failure path: the shared-memory slot it
//! drives, the process name the supervisor spawns, and what the process does on
//! a host where the layer cannot run at all. The last test drives the real
//! binary, so a runner wired to the wrong slot or the wrong name fails here
//! instead of writing one layer's beliefs into another layer's slot.

use std::path::Path;
use std::process::{Command, Stdio};

use qualia_l3_belief::{LAYER_ID, LAYER_NAME};

#[test]
fn the_runner_drives_the_l3_slot_under_the_l3_name() {
    // `qualia.json` gives `belief_visual` slot 3, and the slot a runner writes
    // is the only thing it publishes; a consumer reading layer 3 cannot tell a
    // mis-numbered runner from a stalled one.
    assert_eq!(LAYER_ID, 3, "belief_visual is the third layer slot");
    assert_eq!(LAYER_NAME, "l3-belief");
    assert_eq!(format!("qualia-{LAYER_NAME}"), "qualia-l3-belief");
}

#[test]
fn the_binary_fails_and_still_names_the_layer_it_was_asked_for() {
    // On this host there is no backend that can run the layer, and the runner
    // must not paper over that: no third implementation of the belief loop
    // lives here. It has to fail, and in failing it has to name the layer and
    // the slot it was handed, so the failure is attributable to this runner.
    let binary = env!("CARGO_BIN_EXE_qualia-l3-belief");
    assert_eq!(
        Path::new(binary).file_stem().and_then(|stem| stem.to_str()),
        Some("qualia-l3-belief"),
        "the name the default stack manifest spawns"
    );

    let output = Command::new(binary)
        .env("QUALIA_SHM_NAME", "/qualia_l3_belief_absent")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn the l3-belief binary");

    assert!(
        !output.status.success(),
        "a layer that cannot run must not exit successfully"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(LAYER_NAME),
        "the failure names the layer, got: {stderr}"
    );
    assert!(
        stderr.contains(&format!("layer {LAYER_ID}")),
        "the failure names the slot, got: {stderr}"
    );
}
