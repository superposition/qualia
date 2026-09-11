//! `qualia-cuda`: the CUDA compute backend for the qualia stack.
//!
//! With the `cuda` feature enabled the crate exposes a belief-layer context, a
//! smoke context, a costmap reduction context and the persistent L3-L6
//! cognition stack. Kernels are embedded as `.cu` source and compiled at
//! runtime by NVRTC, so nothing CUDA-shaped is needed at build time and the
//! default build is pure Rust.
//!
//! Without the feature the crate is a thin re-export of the shared ABI types,
//! plus [`cpu`] — a host-side implementation of the same arithmetic that
//! serves as the fallback and as the oracle the device path is checked
//! against.

pub use qualia_types::*;

pub mod cpu;

#[cfg(feature = "cuda")]
mod cuda_impl;

#[cfg(feature = "cuda")]
pub use cuda_impl::{
    run_layer, CostmapStats, CostmapStatsContext, CudaCognitionStack, CudaContext, SmokeContext,
};

// The active coupling is off by default, so the entry point that carries a
// verified prior exists only in a build that asked for it.
#[cfg(feature = "fly-prior")]
pub use cuda_impl::run_layer_with_prior;
#[cfg(feature = "cuda")]
use qualia_jepa::prior::CouplingPrior;

// ── The connectome prior, behind `fly-prior` ─────────────────────────────────

/// A mapping the loaded prior cannot be coupled through.
///
/// The coupling is data (D-001): the verified graph supplies the normalised
/// in-strength that scales a belief slot, and a mapping that names a type the
/// graph does not carry, or a slot the layer's belief cannot hold, is refused
/// rather than skipped, so a stale mapping never reads as a partial success.
///
/// Gated on `cuda` rather than `fly-prior` because the no-coupling build
/// answers with the same error type; `fly-prior` carries `cuda` with it.
#[cfg(feature = "cuda")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CudaError {
    /// The coupling mapped a type the loaded graph does not carry.
    UnknownType(u32),
    /// The coupling mapped a slot outside the layer's belief.
    SlotOutOfRange(usize),
}

#[cfg(feature = "cuda")]
impl std::fmt::Display for CudaError {
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

#[cfg(feature = "cuda")]
impl std::error::Error for CudaError {}

/// Couples the loaded connectome prior into the layer `ctx` drives and returns
/// the total weight it applies.
///
/// `slots` pairs a connectome type with the belief slot it feeds; the type's
/// in-strength, normalised by the graph's strongest type, is the scale that
/// slot receives, so coupling attenuates a belief but never amplifies it. The
/// total is the value [`CouplingPrior::couple`] applies to a belief spanning
/// the mapped slots, read back from a scratch belief so the arithmetic has a
/// single home. `slots` is empty when no prior is loaded, which returns zero.
///
/// The prior is data, not a kernel (D-001): the coupling is host-side, so `ctx`
/// is the layer's identity and nothing is launched for it.
#[cfg(all(feature = "cuda", feature = "fly-prior"))]
pub fn couple_prior(
    ctx: &CudaContext,
    prior: &CouplingPrior,
    slots: &[(u32, usize)],
) -> Result<f32, CudaError> {
    // The total belongs to the layer this context drives; the coupling itself
    // is host-side data, so the context is the layer's identity and no part of
    // the device is asked to do the work.
    let _ = ctx;
    for &(_, slot) in slots {
        if slot >= STATE_DIM {
            return Err(CudaError::SlotOutOfRange(slot));
        }
    }
    for &(type_index, _) in slots {
        if type_index >= prior.type_count {
            return Err(CudaError::UnknownType(type_index));
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
#[cfg(all(feature = "cuda", not(feature = "fly-prior")))]
pub fn couple_prior(
    _ctx: &CudaContext,
    _prior: &CouplingPrior,
    _slots: &[(u32, usize)],
) -> Result<f32, CudaError> {
    Ok(0.0)
}
