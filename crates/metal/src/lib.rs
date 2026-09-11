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

// The active coupling is off by default, so the entry point that carries a
// verified prior exists only in a build that asked for it.
#[cfg(all(target_os = "macos", feature = "fly-prior"))]
pub use macos::run_layer_with_prior;

#[cfg(target_os = "macos")]
pub use cognition_macos::MetalCognitionStack;

#[cfg(not(target_os = "macos"))]
pub use cognition_stub::MetalCognitionStack;

use qualia_jepa::prior::CouplingPrior;

// ── The connectome prior, behind `fly-prior` ─────────────────────────────────

/// A mapping the loaded prior cannot be coupled through.
///
/// Same contract as the CUDA stack's error: the coupling is data (D-001), the
/// verified graph supplies the normalised in-strength that scales a belief
/// slot, and a mapping that names a type the graph does not carry, or a slot
/// the stack's belief cannot hold, is refused rather than skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetalError {
    /// The coupling mapped a type the loaded graph does not carry.
    UnknownType(u32),
    /// The coupling mapped a slot outside the layer's belief.
    SlotOutOfRange(usize),
}

impl std::fmt::Display for MetalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownType(type_index) => write!(
                formatter,
                "coupling slot names type {type_index}, outside the loaded prior graph"
            ),
            Self::SlotOutOfRange(slot) => {
                write!(formatter, "coupling slot {slot} lies outside the layer belief")
            }
        }
    }
}

impl std::error::Error for MetalError {}

/// Couples the loaded connectome prior and returns the total weight it applies.
///
/// `slots` pairs a connectome type with the belief slot it feeds; the type's
/// in-strength, normalised by the graph's strongest type, is the scale that
/// slot receives, so coupling attenuates a belief but never amplifies it. The
/// total is the value [`CouplingPrior::couple`] applies to a belief spanning
/// the mapped slots, read back from a scratch belief so the arithmetic has a
/// single home. `slots` is empty when no prior is loaded, which returns zero.
///
/// The coupling is host-side data (D-001) and the same computation the CUDA
/// stack runs, so it takes no device handle: the Metal belief loop holds the
/// layer's `MetalContext` for its kernels, and building a cognition stack only
/// to pass one here would allocate the whole layer set for a host-side sum.
#[cfg(feature = "fly-prior")]
pub fn couple_prior(
    prior: &CouplingPrior,
    slots: &[(u32, usize)],
) -> Result<f32, MetalError> {
    for &(_, slot) in slots {
        if slot >= STATE_DIM {
            return Err(MetalError::SlotOutOfRange(slot));
        }
    }
    for &(type_index, _) in slots {
        if type_index >= prior.type_count {
            return Err(MetalError::UnknownType(type_index));
        }
    }
    let span = slots
        .iter()
        .map(|&(_, slot)| slot)
        .max()
        .map_or(0, |slot| slot + 1);
    let mut scratch = vec![0.0f32; span];
    Ok(prior.couple(&mut scratch, slots))
}

/// Without the `fly-prior` feature the coupling is not compiled in: the belief
/// layers run exactly as they do with no prior loaded.
#[cfg(not(feature = "fly-prior"))]
pub fn couple_prior(
    _prior: &CouplingPrior,
    _slots: &[(u32, usize)],
) -> Result<f32, MetalError> {
    Ok(0.0)
}

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

/// Refuses a coupled belief layer where there is no Metal device.
///
/// The prior cannot change that there is no layer to couple it into, so this
/// is the same refusal [`run_layer`] gives, reached through the entry point a
/// runner built with the coupling calls.
#[cfg(all(not(target_os = "macos"), feature = "fly-prior"))]
pub fn run_layer_with_prior(
    layer_id: u8,
    name: &str,
    _prior: Option<qualia_jepa::prior::CouplingPrior>,
) -> ! {
    run_layer(layer_id, name)
}
