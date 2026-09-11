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
    run_layer, ActionScoreContext, BeliefCoupleContext, CostmapStats, CostmapStatsContext,
    CudaCognitionStack, CudaContext, PerceptionVoxelContext, SmokeContext,
};
