//! `qualia-l5-behavior` — the process boundary for the L5 behavior layer.
//!
//! The supervisor spawns one process per layer, and this crate is the one that
//! stands up layer 5. Its whole job is to say *which* layer is running and hand
//! the work to the compute backend chosen at build time: the backend's
//! `run_layer` owns the belief loop, the shared-memory slots and the operator
//! log lines, so the Metal and CUDA builds drive byte-for-byte the same layer
//! and differ only in device.
//!
//! Exactly one backend must be selected. `metal` is the default; on a host
//! without a Metal device build with `--no-default-features --features cuda`.
//! Asking for both, or for neither, is a build error rather than a runtime
//! surprise.

/// Layer 5: the deep behavior layer, `behavior_deep` in the stack plan.
const LAYER_ID: u8 = 5;

/// The label the backend prints in its log lines and stamps on thought records.
const LAYER_NAME: &str = "l5-behavior";

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
