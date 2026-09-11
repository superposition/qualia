//! `qualia-cuda`: the CUDA compute backend for the qualia stack.
//!
//! With the `cuda` feature enabled the crate exposes a belief-layer context, a
//! smoke context, a costmap reduction context and the persistent L3-L6
//! cognition stack. Kernels are embedded as `.cu` source and compiled at
//! runtime by NVRTC, so nothing CUDA-shaped is needed at build time and the
//! default build is pure Rust. Setting `CUDAARCHS` asks the build script for
//! real `sm_` device code instead; the run time then loads that fatbin in
//! preference to NVRTC.
//!
//! Without the feature the crate is a thin re-export of the shared ABI types,
//! plus [`cpu`] — a host-side implementation of the same arithmetic that
//! serves as the fallback and as the oracle the device path is checked
//! against.

use std::mem::size_of;

pub use qualia_types::*;

pub mod cpu;

/// The planar shape of one costmap reduction, in cells.
///
/// The costmap is the one backend input whose size the caller chooses, so the
/// device allocation plan is expressed against its shape. `width` and `depth`
/// are the grid dimensions the compute contract's `PlannerGrid` carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CostmapStatsShape {
    /// Cells along the horizontal axis.
    pub width: u32,
    /// Cells along the vertical axis.
    pub depth: u32,
}

impl CostmapStatsShape {
    pub const fn new(width: u32, depth: u32) -> Self {
        Self { width, depth }
    }

    /// Cells in the grid. Two `u32` dimensions always fit a `u64`.
    pub const fn cells(self) -> u64 {
        self.width as u64 * self.depth as u64
    }
}

/// The device budget of the deployment target: 6 GiB of the 8 GB an Orin Nano
/// carries, leaving 2 GB for the runtime and the operating system.
pub const ORIN_NANO_DEVICE_BUDGET_BYTES: u64 = 6 * 1024 * 1024 * 1024;

/// Cognition layers [`CudaCognitionStack`] keeps resident on the device.
pub const COGNITION_STACK_LAYERS: usize = 4;

/// Bytes one costmap cell costs on the device: the occupancy byte and the cost
/// byte the reduction uploads.
const COSTMAP_BYTES_PER_CELL: u64 = 2 * size_of::<u8>() as u64;

/// The four `u32` counters one costmap reduction accumulates.
const COSTMAP_COUNTER_BYTES: u64 = 4 * size_of::<u32>() as u64;

/// The device allocation plan for one costmap reduction of `shape`.
///
/// [`CostmapStatsContext`] holds exactly these buffers resident between
/// launches: one `u8` per cell for occupancy, one for cost, and the four `u32`
/// counters. The plan is therefore linear in the grid's cell count, and a
/// deployment checks it against [`ORIN_NANO_DEVICE_BUDGET_BYTES`] before it
/// accepts a costmap — the budget is the device's ceiling, so the check is what
/// stops a future per-cell channel from crossing the 2 GB the runtime needs.
pub fn memory_plan(shape: &CostmapStatsShape) -> u64 {
    shape.cells() * COSTMAP_BYTES_PER_CELL + COSTMAP_COUNTER_BYTES
}

#[cfg(feature = "cuda")]
mod cuda_impl;

#[cfg(feature = "cuda")]
pub use cuda_impl::{
    run_layer, ActionScoreContext, BeliefCoupleContext, CostmapStats, CostmapStatsContext,
    CudaCognitionStack, CudaContext, PerceptionVoxelContext, SmokeContext,
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
