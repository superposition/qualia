//! Rate model over the connectome type graph. See README.md.
//!
//! This crate is **not** a connectome simulation. The dataset carries no
//! dynamics, so the model here is invented: one scalar rate per type, one
//! `dt`-scaled integration step, edges taken from the prior's CSR with every
//! weight used as positive. It exists so the belief layers can observe what a
//! graph-shaped rate model does, and nothing here is reachable from the motor
//! path: the whole surface is behind the off-by-default `sim` feature, and with
//! that feature off the crate is empty and pulls no dependency.

#![forbid(unsafe_code)]

#[cfg(feature = "sim")]
mod rate;

#[cfg(feature = "sim")]
pub use rate::{CircuitSim, SIM_ID};
