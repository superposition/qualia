//! The invented rate model, compiled only with the `sim` feature.

use std::fs;
use std::path::Path;

use qualia_connectome_prior::{GRAPH_FILE, MANIFEST_FILE, PRIOR_SCHEMA, PriorError, PriorManifest};

/// Identifier of this model, stamped on anything it publishes.
pub const SIM_ID: &str = "qualia.fly-circuit.rate.v1";

/// Rate leak per unit time.
const LEAK: f32 = 1.0;

/// A rate model over the prior's type graph.
///
/// One `f32` state per type, all zero at load. One [`CircuitSim::step`] is one
/// explicit-Euler step of
///
/// ```text
/// ds_i / dt = -LEAK * s_i + gain * sum over edges p -> i of w[p, i] * s_p + input_i
/// ```
///
/// where the edges are the prior's CSR rows: row `p` lists the types the edge
/// leaves `p` for, and `w` is the prior's weight. The artifact carries no sign —
/// the builder aggregates `dimorphism` away — so every weight is used as
/// written, as positive; every edge here excites (README says so too).
///
/// The prior's weights are synapse counts, so the coupling sum is scaled by
/// `gain = 1 / max(1, max_i sum over edges p -> i of w[p, i])`: the type with the
/// heaviest incoming weight sum sees the plain average of its sources, and the
/// relative strength of every other type is preserved rather than flattened.
pub struct CircuitSim {
    rowptr: Vec<u64>,
    cols: Vec<u32>,
    weights: Vec<u32>,
    state: Vec<f32>,
    drive: Vec<f32>,
    gain: f32,
}

impl CircuitSim {
    /// Load a prior artifact directory written by `qualia-connectome-prior`.
    ///
    /// `manifest.json` is read for its schema, type count and edge count;
    /// `graph.bin` is then read as `rowptr` (`u64`, little-endian), `cols`
    /// (`u32`) and `weights` (`u32`), in that order, with the section lengths
    /// the manifest implies. Failure — a missing or unreadable file, a schema
    /// this model does not know, a byte length that disagrees with the
    /// manifest, or CSR offsets that do not hold — is reported as
    /// [`PriorError::Read`].
    pub fn load(dir: &Path) -> Result<Self, PriorError> {
        let manifest_path = dir.join(MANIFEST_FILE);
        let manifest_bytes = fs::read(&manifest_path).map_err(|error| read_error(&manifest_path, error))?;
        let manifest: PriorManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| PriorError::Read(format!("{}: {error}", manifest_path.display())))?;
        if manifest.schema != PRIOR_SCHEMA {
            return Err(PriorError::Read(format!(
                "{}: schema {} is not {PRIOR_SCHEMA}",
                manifest_path.display(),
                manifest.schema
            )));
        }

        let graph_path = dir.join(GRAPH_FILE);
        let bytes = fs::read(&graph_path).map_err(|error| read_error(&graph_path, error))?;
        let type_count = manifest.type_count as usize;
        let edge_count = manifest.edge_count as usize;
        let rowptr_bytes = (type_count + 1) * std::mem::size_of::<u64>();
        let edge_bytes = edge_count * std::mem::size_of::<u32>();
        if bytes.len() != rowptr_bytes + 2 * edge_bytes {
            return Err(PriorError::Read(format!(
                "{}: {} bytes do not match {type_count} types and {edge_count} edges",
                graph_path.display(),
                bytes.len()
            )));
        }

        let rowptr = decode_u64(&bytes[..rowptr_bytes]);
        let cols = decode_u32(&bytes[rowptr_bytes..rowptr_bytes + edge_bytes]);
        let weights = decode_u32(&bytes[rowptr_bytes + edge_bytes..]);
        if rowptr.first().copied() != Some(0)
            || rowptr.last().copied() != Some(edge_count as u64)
            || rowptr.windows(2).any(|pair| pair[0] > pair[1])
            || cols.iter().any(|column| *column as usize >= type_count)
        {
            return Err(PriorError::Read(format!(
                "{}: CSR invariants do not hold for {type_count} types and {edge_count} edges",
                graph_path.display()
            )));
        }

        let mut incoming = vec![0u64; type_count];
        for (edge, column) in cols.iter().enumerate() {
            incoming[*column as usize] += u64::from(weights[edge]);
        }
        let heaviest = incoming.iter().copied().max().unwrap_or(0).max(1);

        Ok(Self {
            rowptr,
            cols,
            weights,
            state: vec![0.0; type_count],
            drive: vec![0.0; type_count],
            gain: 1.0 / heaviest as f32,
        })
    }

    /// Advance every type by one `dt` step and return the new state.
    ///
    /// The returned vector holds one rate per type, in the prior's type order.
    /// `input` carries one drive per type in the same order and must be that
    /// long: a mismatched length is a caller bug and panics rather than being
    /// silently padded. Coupling reads the state as it was before the step, so
    /// the returned rates are a function of the pre-step state, the input and
    /// `dt` alone — repeating a call on a freshly loaded model reproduces it
    /// bit for bit.
    pub fn step(&mut self, input: &[f32], dt: f32) -> Vec<f32> {
        assert_eq!(
            input.len(),
            self.state.len(),
            "input carries one drive per type"
        );

        self.drive.fill(0.0);
        for source in 0..self.state.len() {
            let rate = self.state[source];
            if rate == 0.0 {
                continue;
            }
            let start = self.rowptr[source] as usize;
            let end = self.rowptr[source + 1] as usize;
            for edge in start..end {
                let column = self.cols[edge] as usize;
                self.drive[column] += self.weights[edge] as f32 * rate;
            }
        }

        for index in 0..self.state.len() {
            self.state[index] +=
                dt * (self.gain * self.drive[index] + input[index] - LEAK * self.state[index]);
        }

        self.state.clone()
    }
}

fn read_error(path: &Path, error: std::io::Error) -> PriorError {
    PriorError::Read(format!("{}: {error}", path.display()))
}

fn decode_u64(bytes: &[u8]) -> Vec<u64> {
    bytes
        .chunks_exact(std::mem::size_of::<u64>())
        .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("chunks are 8 bytes")))
        .collect()
}

fn decode_u32(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(std::mem::size_of::<u32>())
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("chunks are 4 bytes")))
        .collect()
}
