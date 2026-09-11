//! `qualia-l3-belief` — the process that drives belief layer 3.
//!
//! Layer 3 is `belief_visual` in `qualia.json`: slot 3 of the shared-memory
//! arena, 100 Hz, the visual-belief layer that predicts `visual_features`. The
//! belief loop itself is not here. `qualia-metal` owns it on macOS and
//! `qualia-cuda` owns it on a CUDA host — each owns the kernels, the layer
//! slot's seeds and the operator log lines the layer emits — and the two are
//! budgeted as the same layer, so exactly one of them is compiled in.
//!
//! What this crate owns is therefore the identity it hands the backend: the
//! slot the backend writes ([`LAYER_ID`]) and the name that appears in the
//! backend's operator output as `qualia-l3-belief` ([`LAYER_NAME`]). A build
//! with both backends, or with neither, is refused here rather than resolved
//! by a preference the operator never sees.

/// The shared-memory layer slot this runner drives: `belief_visual`, id 3.
pub const LAYER_ID: u8 = 3;

/// The layer's short name; the backend renders it as `qualia-l3-belief` in its
/// operator log and the default stack manifest spawns it by that name.
pub const LAYER_NAME: &str = "l3-belief";

/// Drive layer 3 through the backend this build selected.
///
/// `qualia-metal` is the default; `qualia-cuda` replaces it under the `cuda`
/// feature. A backend that cannot run on this host fails loudly in its own
/// `run_layer` — the honest outcome, since a runner that quietly did nothing
/// would be indistinguishable from a layer with no belief to publish.
pub fn run() {
    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER_ID, LAYER_NAME);

    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);
}

#[cfg(all(feature = "cuda", feature = "metal"))]
compile_error!("cuda and metal features are mutually exclusive");

#[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
compile_error!("enable either cuda or metal feature");
