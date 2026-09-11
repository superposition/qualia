//! The `qualia-l0-superposition` binary: the process front door for layer 0.
//!
//! L0 is the bottom of the belief stack, so there is nothing here to compute —
//! the layer's loop and kernels live in the compute backend this build selects
//! (`qualia-metal`, or `qualia-cuda` for the device path). This binary exists to
//! fix *which* layer that backend runs and what it is called in the operator
//! log, so the process the fleet starts is the same shape as the L1-L6 runners.

/// L0 sits at the bottom of the belief stack.
const LAYER_ID: u8 = 0;

/// The layer name the backend prints in its operator log lines.
const LAYER_NAME: &str = "l0-superposition";

fn main() {
    // The two backends are alternatives, not layers of one stack: each has its
    // own kernels and its own device context, and a binary that linked both
    // would have to choose at run time what it can only choose at build time.
    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);
    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER_ID, LAYER_NAME);
    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");
    // With neither backend there is no loop to hand the layer to.
    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}
