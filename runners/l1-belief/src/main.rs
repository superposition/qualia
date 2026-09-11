//! `qualia-l1-belief`: the process a stack spawns to run belief layer 1.
//!
//! This crate is deliberately only the process boundary. The belief loop, the
//! shared-memory publication and every operator line it prints belong to the
//! compute backend — `qualia-cuda` on the Orin Nano, `qualia-metal` on Apple
//! silicon — and the backend is chosen at build time rather than probed at
//! run time, so a deployment cannot silently fall back to the wrong one.
//!
//! What this binary owns is the hand-off: which arena slot it claims and which
//! name the backend logs and panics under. The supervisor's stack manifest
//! spawns it as `qualia-l1-belief`, and the health tap reads the same slot
//! index, so [`LAYER_ID`] and [`LAYER_NAME`] are the whole of its contract.

/// Arena slot this layer owns.
///
/// `qualia-shm` orders slots from the bottom of the stack upward, and the
/// health tap flattens them in that order; the supervisor's manifest lists the
/// runners in the same order, so this value is the layer's identity, not a
/// tuning knob.
const LAYER_ID: u8 = 1;

/// The layer's operator-facing name.
///
/// The backend prefixes its log lines with `qualia-<name>`, which makes this
/// the string an operator greps for in the child log, and it is also the last
/// word of the refusal printed on a host with no compute device.
const LAYER_NAME: &str = "l1-belief";

fn main() {
    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);

    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER_ID, LAYER_NAME);

    // Both backends compiled in would make the choice of device implicit in
    // `cfg` order, and neither leaves the process with nothing to drive.
    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");

    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}
