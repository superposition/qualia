//! The `qualia-l3-belief` process: hand belief layer 3 to the compute backend.
//!
//! Layer 3 is `belief_visual` in `qualia.json` — shared-memory slot 3, 100 Hz,
//! predicting `visual_features` — but the belief loop is not in this crate.
//! `qualia-metal` owns it on macOS and `qualia-cuda` owns it on a CUDA host;
//! each owns the contents of the layer slot it writes and the operator lines
//! the layer emits, and both are budgeted as the same layer, so exactly one of
//! them is compiled in.
//!
//! What the binary owns is the identity it hands over: the slot the backend
//! writes and the name it reports under, which the backend's operator log
//! renders as `qualia-l3-belief` and the default stack manifest spawns. Both
//! backends on, or neither, is refused at compile time rather than resolved by
//! a preference the operator never sees.

/// The shared-memory layer slot this runner drives: `belief_visual`, id 3.
const LAYER_ID: u8 = 3;

/// The layer's name, as the stack manifest spawns it and the backend logs it.
const LAYER_NAME: &str = "l3-belief";

fn main() {
    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);

    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER_ID, LAYER_NAME);
}

#[cfg(all(feature = "cuda", feature = "metal"))]
compile_error!("cuda and metal features are mutually exclusive");

#[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
compile_error!("enable either cuda or metal feature");
