//! The type-level connectome prior, loaded as belief coupling.
//!
//! `crates/connectome-prior` emits a self-describing artifact: `graph.bin`
//! holds the little-endian `rowptr`, `cols` and `weights` sections back to
//! back, and `manifest.json` records how many types and edges those sections
//! carry together with a SHA-256 for each one. This module reads that
//! artifact, and refuses any file whose bytes do not reproduce the recorded
//! digests, so a truncated or edited graph is rejected instead of being
//! applied to a belief.
//!
//! Only [`CouplingPrior::load`] touches the filesystem; [`CouplingPrior::couple`]
//! is a pure function of the loaded graph.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Manifest file name inside a prior directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Binary graph file name inside a prior directory.
pub const GRAPH_FILE: &str = "graph.bin";

/// The manifest schema this loader understands.
const PRIOR_SCHEMA: &str = "qualia.connectome-prior.v1";

/// A prior artifact that was rejected while loading.
#[derive(Debug, PartialEq, Eq)]
pub enum PriorError {
    /// A file was missing, unreadable, or not the documented shape.
    Read(String),
    /// The manifest names a schema this loader does not implement.
    Schema { found: String },
    /// `graph.bin` was not the size the manifest's counts describe.
    Length { expected: u64, actual: u64 },
    /// A section did not reproduce the digest the manifest records for it.
    Digest { section: &'static str },
    /// The decoded arrays did not form the CSR the manifest describes.
    Csr(&'static str),
}

impl Display for PriorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(detail) => write!(formatter, "read: {detail}"),
            Self::Schema { found } => {
                write!(formatter, "schema: unsupported prior schema {found:?}")
            }
            Self::Length { expected, actual } => {
                write!(formatter, "graph: {actual} bytes; expected {expected}")
            }
            Self::Digest { section } => {
                write!(formatter, "digest: {section} section does not match its manifest digest")
            }
            Self::Csr(detail) => write!(formatter, "csr: {detail}"),
        }
    }
}

impl Error for PriorError {}

/// The manifest fields the loader verifies.
///
/// The builder writes more than these (`created_at_ms`, `attribution`, ...);
/// serde ignores what the loader does not name, so the extra fields a future
/// manifest may carry do not make an artifact unreadable.
#[derive(Debug, Deserialize)]
struct Manifest {
    /// Schema literal; the loader accepts only [`PRIOR_SCHEMA`].
    schema: String,
    /// Number of type-level nodes.
    type_count: u32,
    /// Number of type-level edges, i.e. CSR nonzeros.
    edge_count: u64,
    /// SHA-256 of the `rowptr` section of `graph.bin`.
    rowptr_sha256: String,
    /// SHA-256 of the `cols` section of `graph.bin`.
    cols_sha256: String,
    /// SHA-256 of the `weights` section of `graph.bin`.
    weights_sha256: String,
}

/// The type-level connectome graph, verified against its manifest.
///
/// `rowptr` has one more entry than there are types; row `i` spans
/// `rowptr[i]..rowptr[i + 1]` of `cols` and `weights`, and `cols[j]` is the
/// post-synaptic type of edge `j`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CouplingPrior {
    /// Number of type-level nodes.
    pub type_count: u32,
    /// CSR row offsets.
    pub rowptr: Vec<u64>,
    /// Post-synaptic type index per edge.
    pub cols: Vec<u32>,
    /// Aggregated weight per edge.
    pub weights: Vec<u32>,
}

impl CouplingPrior {
    /// Loads a prior directory and verifies its artifact.
    ///
    /// Each of the three section digests in `manifest.json` is recomputed
    /// over the `graph.bin` bytes it names before any array is decoded, and
    /// the file must hold exactly the sections the manifest's counts imply,
    /// so an edited or truncated artifact is rejected here rather than
    /// coupled into a belief.
    pub fn load(dir: &Path) -> Result<Self, PriorError> {
        let manifest_bytes =
            fs::read(dir.join(MANIFEST_FILE)).map_err(|error| read_error(MANIFEST_FILE, error))?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| PriorError::Read(format!("{MANIFEST_FILE}: {error}")))?;
        if manifest.schema != PRIOR_SCHEMA {
            return Err(PriorError::Schema {
                found: manifest.schema,
            });
        }

        let bytes = fs::read(dir.join(GRAPH_FILE)).map_err(|error| read_error(GRAPH_FILE, error))?;
        let rowptr_len = (u64::from(manifest.type_count) + 1)
            .checked_mul(8)
            .ok_or(PriorError::Csr("rowptr section length overflows"))?;
        let edges_len = manifest
            .edge_count
            .checked_mul(8)
            .ok_or(PriorError::Csr("edge section length overflows"))?;
        let expected = rowptr_len
            .checked_add(edges_len)
            .ok_or(PriorError::Csr("graph length overflows"))?;
        let actual = bytes.len() as u64;
        if actual != expected {
            return Err(PriorError::Length { expected, actual });
        }

        let type_count = manifest.type_count;
        let edge_count = manifest.edge_count as usize;
        let split = rowptr_len as usize;
        let sections = [
            ("rowptr", manifest.rowptr_sha256.as_str(), &bytes[..split]),
            (
                "cols",
                manifest.cols_sha256.as_str(),
                &bytes[split..split + edge_count * 4],
            ),
            ("weights", manifest.weights_sha256.as_str(), &bytes[split + edge_count * 4..]),
        ];
        for (section, recorded, section_bytes) in sections {
            if sha256_hex(section_bytes) != recorded {
                return Err(PriorError::Digest { section });
            }
        }

        let prior = Self {
            type_count,
            rowptr: decode_u64(&bytes[..split]),
            cols: decode_u32(&bytes[split..split + edge_count * 4]),
            weights: decode_u32(&bytes[split + edge_count * 4..]),
        };
        prior.validate()?;
        Ok(prior)
    }

    /// Scales each mapped belief slot by its type's normalised in-strength.
    ///
    /// A type's in-strength is the summed weight of the edges that end on it,
    /// normalised by the largest in-strength of any type: the most strongly
    /// innervated type couples at unit weight and every other type scales
    /// down in proportion, so coupling can attenuate a belief but never
    /// amplify it. `slots` pairs a type index with the belief slot it feeds;
    /// a pair naming a type outside the graph or a slot outside `belief` is
    /// ignored. Returns the weight actually applied, which is zero when
    /// `slots` is empty — the state when no prior is loaded.
    pub fn couple(&self, belief: &mut [f32], slots: &[(u32, usize)]) -> f32 {
        let mut in_strength = vec![0u64; self.type_count as usize];
        let mut peak = 0u64;
        for (column, weight) in self.cols.iter().zip(self.weights.iter()) {
            let strength = &mut in_strength[*column as usize];
            *strength += u64::from(*weight);
            peak = peak.max(*strength);
        }
        if peak == 0 {
            return 0.0;
        }

        let mut applied = 0.0f32;
        for &(type_index, slot) in slots {
            let Some(strength) = in_strength.get(type_index as usize) else {
                continue;
            };
            let Some(value) = belief.get_mut(slot) else {
                continue;
            };
            let factor = *strength as f32 / peak as f32;
            *value *= factor;
            applied += factor;
        }
        applied
    }

    fn validate(&self) -> Result<(), PriorError> {
        if self.rowptr.len() != self.type_count as usize + 1 {
            return Err(PriorError::Csr("rowptr length does not match the type count"));
        }
        if self.cols.len() != self.weights.len() {
            return Err(PriorError::Csr("cols and weights differ in length"));
        }
        if self.rowptr.first().copied() != Some(0)
            || self.rowptr.last().copied() != Some(self.cols.len() as u64)
            || self.rowptr.windows(2).any(|pair| pair[0] > pair[1])
        {
            return Err(PriorError::Csr("rowptr is not a monotone span of the edges"));
        }
        if self.cols.iter().any(|column| *column >= self.type_count) {
            return Err(PriorError::Csr("cols names a type outside the graph"));
        }
        Ok(())
    }
}

fn read_error(path: &str, error: std::io::Error) -> PriorError {
    PriorError::Read(format!("{path}: {error}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decode_u64(bytes: &[u8]) -> Vec<u64> {
    bytes
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight-byte chunk")))
        .collect()
}

fn decode_u32(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("four-byte chunk")))
        .collect()
}
