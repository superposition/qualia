//! `qualia-metal`: the Metal compute path for the belief and cognition layers.
//!
//! On macOS the kernels in `kernels/` run on the GPU and the layer runners
//! enter through [`run_layer`]. Every other platform still compiles this crate
//! — the module tree is `cfg`-gated rather than the crate being absent — and
//! exports [`MetalCognitionStack`] backed by host memory so a consumer builds
//! and runs unchanged. That fallback is honest about what it is: its
//! `backend_name` is `cpu-fallback`, and [`run_layer`] refuses outright, since
//! a belief layer has no Metal device to drive there.

pub use qualia_types::*;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
mod cognition_macos;

#[cfg(target_os = "macos")]
mod gpu;

#[cfg(not(target_os = "macos"))]
mod cognition_stub;

#[cfg(target_os = "macos")]
pub use macos::{run_layer, MetalContext};

#[cfg(target_os = "macos")]
pub use cognition_macos::MetalCognitionStack;

#[cfg(not(target_os = "macos"))]
pub use cognition_stub::MetalCognitionStack;

/// Refuses to run a belief layer where there is no Metal device.
///
/// The layer runners compile on every host so the workspace stays uniform; on a
/// host without Metal the honest answer is a panic naming the platform, not a
/// silent second implementation of the belief loop.
#[cfg(not(target_os = "macos"))]
pub fn run_layer(layer_id: u8, name: &str) -> ! {
    panic!(
        "qualia-metal is only supported on macOS; cannot run layer {} ({}) on this platform",
        layer_id, name
    );
}
