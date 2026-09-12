//! The spiking step: a sparse leaky integrate-and-fire neuron model.
//!
//! One tick is one frame. Per neuron, in order: gather the current its spiking
//! presynaptic partners inject, leak the membrane toward rest, threshold,
//! spike or not, and hold a refractory count. The model is deliberately plain —
//! no learning, no neuromodulation, no plasticity — because the point of the
//! slice is the plumbing, and the plumbing is the arithmetic below.
//!
//! This module is the CPU reference. [`crate::gpu`] runs the identical
//! recurrence on the device and the two are compared tick for tick. The spike
//! vector is double-buffered here exactly as it is there: every neuron in a
//! tick reads the *previous* tick's spikes, so the update is synchronous and
//! independent of neuron order.

use crate::IncomingCsr;

/// The parameters of one run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifParams {
    /// Membrane leak per tick; `v *= decay` before the current is added.
    pub decay: f32,
    /// Firing threshold on the membrane potential.
    pub threshold: f32,
    /// Potential a neuron resets to after it fires.
    pub reset: f32,
    /// Ticks a neuron is held at [`Self::reset`] after firing.
    pub refractory_ticks: u32,
}

impl Default for LifParams {
    fn default() -> Self {
        Self {
            decay: 0.9,
            threshold: 1.0,
            reset: 0.0,
            refractory_ticks: 2,
        }
    }
}

/// The mutable state of a running network.
#[derive(Debug, Clone, PartialEq)]
pub struct LifState {
    /// Membrane potential per neuron.
    pub v: Vec<f32>,
    /// Refractory ticks left per neuron.
    pub refractory: Vec<u8>,
    /// Spikes of the tick that just ran; `spike[i]` is 0 or 1.
    pub spike: Vec<u8>,
    /// The other spike buffer, holding the tick before last after the swap.
    /// [`step_cpu`] writes the tick it is computing here so that no neuron
    /// reads this tick's output in place of last tick's input.
    pub spike_out: Vec<u8>,
    /// Firing node ids of the last tick, ascending.
    pub fired: Vec<u32>,
    /// Ticks stepped so far.
    pub tick: u64,
    /// Spikes emitted over the whole run.
    pub total_spikes: u64,
}

impl LifState {
    /// A quiet network of `neuron_count` neurons.
    pub fn new(neuron_count: usize) -> Self {
        Self {
            v: vec![0.0; neuron_count],
            refractory: vec![0; neuron_count],
            spike: vec![0; neuron_count],
            spike_out: vec![0; neuron_count],
            fired: Vec::new(),
            tick: 0,
            total_spikes: 0,
        }
    }
}

/// Advance the network one tick on the CPU.
///
/// `external[i]` is the injected current for neuron `i` this tick — the sensory
/// drive; pass an empty slice for a free-running network.
pub fn step_cpu(
    graph: &IncomingCsr,
    params: &LifParams,
    state: &mut LifState,
    external: &[f32],
) {
    let neurons = graph.neuron_count();
    debug_assert_eq!(state.v.len(), neurons);
    debug_assert_eq!(state.spike_out.len(), neurons);
    debug_assert!(external.is_empty() || external.len() == neurons);
    state.fired.clear();
    for neuron in 0..neurons {
        if state.refractory[neuron] > 0 {
            state.refractory[neuron] -= 1;
            state.v[neuron] = params.reset;
            state.spike_out[neuron] = 0;
            continue;
        }
        let start = graph.rowptr[neuron] as usize;
        let end = graph.rowptr[neuron + 1] as usize;
        let mut current = 0.0f32;
        for edge in start..end {
            if state.spike[graph.cols[edge] as usize] != 0 {
                current += f32::from(graph.sign[edge]) * f32::from(graph.weight[edge]);
            }
        }
        if !external.is_empty() {
            current += external[neuron];
        }
        let v = state.v[neuron] * params.decay + current;
        if v >= params.threshold {
            state.spike_out[neuron] = 1;
            state.v[neuron] = params.reset;
            state.refractory[neuron] = params.refractory_ticks.min(u32::from(u8::MAX)) as u8;
            state.fired.push(neuron as u32);
            state.total_spikes += 1;
        } else {
            state.spike_out[neuron] = 0;
            state.v[neuron] = v;
        }
    }
    std::mem::swap(&mut state.spike, &mut state.spike_out);
    state.tick += 1;
}

/// A free-running `ticks`-tick CPU run returning the firing sets per tick.
pub fn run_cpu(
    graph: &IncomingCsr,
    params: &LifParams,
    ticks: u64,
    external: &[f32],
) -> (LifState, Vec<Vec<u32>>) {
    let mut state = LifState::new(graph.neuron_count());
    let mut frames = Vec::with_capacity(ticks as usize);
    for _ in 0..ticks {
        step_cpu(graph, params, &mut state, external);
        frames.push(state.fired.clone());
    }
    (state, frames)
}
