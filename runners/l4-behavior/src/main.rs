//! The `qualia-l4-behavior` process.
//!
//! Layer 4 is the stack's short-horizon behaviour layer: the supervisor starts
//! this binary from the stack manifest by name, with no arguments, and reads
//! its exit status. Everything that happens once it is running — attaching to
//! the arena, claiming the layer's slot, driving the loop and writing the
//! operator log — belongs to the compute backend, `qualia-metal` on the macOS
//! host and `qualia-cuda` on the Jetson. This crate owns the two things that
//! are specific to layer 4: the identity handed to that backend, and the
//! compile-time choice of which backend receives it.

/// The layer index the backend claims its arena slot and start-of-loop state
/// from.
const LAYER_ID: u8 = 4;

/// The layer name the backend stamps into its operator log.
const LAYER_NAME: &str = "l4-behavior";

/// Hand the process over to the backend this build selected.
///
/// Exactly one backend is compiled in. The two `compile_error!` arms reject a
/// build that selects both or neither, so a misconfigured feature set fails at
/// compile time instead of quietly starting the layer on the wrong backend.
fn enter_backend() {
    #[cfg(all(feature = "cuda", not(feature = "metal")))]
    qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);
    #[cfg(all(feature = "metal", not(feature = "cuda")))]
    qualia_metal::run_layer(LAYER_ID, LAYER_NAME);
    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");
    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}

fn main() {
    enter_backend();
}
