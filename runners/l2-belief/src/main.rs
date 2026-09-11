//! `qualia-l2-belief`: the executive belief layer of the body stack.
//!
//! Layer 2 reads the layer beneath it out of the shared arena and is the sole
//! writer of its own slot; the update itself is a GPU kernel, so the compute
//! loop lives in whichever backend the build selected — `qualia-metal` on
//! Apple silicon, `qualia-cuda` on Jetson. This binary carries the part the
//! backend cannot know: which layer it is and what to call it in the log.
//!
//! Exactly one backend is compiled in. Choosing neither would leave a belief
//! layer with no update rule, and choosing both would run two of them over the
//! same slot, so both misconfigurations are build errors rather than runtime
//! surprises.

/// Layer index this runner owns, counted from the sensor plane up.
const LAYER: u8 = 2;

/// Name the backend uses in its operator lines and in the arena narration.
const NAME: &str = "l2-belief";

fn main() {
    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER, NAME);

    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER, NAME);

    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");

    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}
