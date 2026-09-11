//! The semantic-layer runner, `qualia-l6-semantic`.
//!
//! Layer 6 runs inside whichever compute backend this build selected:
//! `qualia-metal` on an Apple host, `qualia-cuda` on the Orin. This binary is
//! the process the stack manifest names, and all it does is hand the layer's
//! ordinal and its operator-facing name to that backend, whose loop takes the
//! process from there.
//!
//! Exactly one backend has to be on. Enabling both is a build error instead of
//! a runtime choice, and enabling neither leaves no loop to enter.

/// Ordinal of the semantic layer, as the compute backends number the stack.
const LAYER_ID: u8 = 6;

/// Name the layer reports itself under, in the backend's operator log lines.
const LAYER_NAME: &str = "l6-semantic";

fn main() {
    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);

    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER_ID, LAYER_NAME);

    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");

    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}
