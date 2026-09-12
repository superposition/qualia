//! Binary artifact formats for the Male CNS connectome.
//!
//! Three files, all little-endian, all self-describing, all struct-of-arrays so
//! a device upload is one `memcpy` per section (see the layouts in the crate
//! README):
//!
//! * `weights.bin` — magic `QLCN`: the neuron-level CSR of pre→post edges with a
//!   sign per synapse and the released synapse count as the weight.
//! * `positions.bin` — magic `QLCB`: one soma position and one interned cell
//!   type per CSR node, in node order.
//! * `spikes.bin` — magic `QLSP`: the recorded firing stream, one frame per
//!   tick, `tick`, `t_ns`, `count`, then that tick's sorted firing node ids.
//!
//! `manifest.json` carries the counts, the per-section byte ranges and SHA-256
//! digests, and the source download's own size and digest. Loading an artifact
//! verifies every section digest and refuses a file that does not match, which
//! is what makes the artifact self-describing rather than merely readable.

// The device path is where a kernel argument list lives, so `unsafe` is
// contained in `gpu` and nowhere else.
#![cfg_attr(not(feature = "cuda"), forbid(unsafe_code))]

pub mod import;
pub mod lif;
pub mod spikes;
#[cfg(feature = "cuda")]
pub mod gpu;

pub use import::{import, ImportReport, Source};
pub use lif::{run_cpu, step_cpu, LifParams, LifState};
pub use spikes::{decode_spikes, read_spikes, write_spikes, SpikeFrame, SpikeWriter};

use std::error::Error;
use std::fmt::{Display, Formatter, Write as _};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Schema literal carried by every emitted `manifest.json`.
pub const CNS_SCHEMA: &str = "qualia.connectome-cns.v1";
/// Positions schema literal, emitted beside the neuron positions.
pub const POSITIONS_SCHEMA: &str = "qualia.connectome-cns.positions.v1";

/// Magic of `weights.bin`.
pub const WEIGHTS_MAGIC: [u8; 4] = *b"QLCN";
/// Magic of `positions.bin`.
pub const POSITIONS_MAGIC: [u8; 4] = *b"QLCB";
/// Magic of `spikes.bin`.
pub const SPIKES_MAGIC: [u8; 4] = *b"QLSP";
/// Version written into the `mAGIC`-headed files.
pub const FORMAT_VERSION: u16 = 1;

/// Manifest file name inside an artifact directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Attribution file name inside an artifact directory.
pub const ATTRIBUTION_FILE: &str = "attribution.json";
/// Weights file name inside an artifact directory.
pub const WEIGHTS_FILE: &str = "weights.bin";
/// Positions file name inside an artifact directory.
pub const POSITIONS_FILE: &str = "positions.bin";
/// Interned cell-type labels, one per line, index = interned id.
pub const TYPES_FILE: &str = "types.txt";
/// Per-neuron metadata side table, one line per CSR node.
///
/// Text, because it is metadata rather than data: `body_id`, the interned type
/// label, the superclass, the side and the instance name, tab separated. The
/// encoders select their input and output populations from it, so a mapping
/// names released cell types rather than hard-coded indices.
pub const NODES_FILE: &str = "nodes.txt";

/// One neuron's released annotation, as carried by `nodes.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMeta {
    /// Released body id.
    pub body_id: u64,
    /// Cell type, as the release names it.
    pub type_name: String,
    /// Coarse superclass, e.g. `descending_neuron`.
    pub superclass: String,
    /// `L`, `R`, `M`, or empty.
    pub side: String,
    /// Instance name, e.g. `DNp01(GF)_R`.
    pub instance: String,
}

/// A weighed edge whose presynaptic neurotransmitter is not known.
pub const SIGN_UNKNOWN: i8 = 0;
/// An excitatory edge.
pub const SIGN_EXCITATORY: i8 = 1;
/// An inhibitory edge.
pub const SIGN_INHIBITORY: i8 = -1;

/// A failed import, artifact write, or artifact load.
#[derive(Debug, PartialEq, Eq)]
pub enum CnsError {
    /// An input table was missing, unreadable, not Arrow IPC, or lacked a
    /// required column.
    Read(String),
    /// The artifact directory does not hold a readable artifact.
    Artifact(String),
    /// A section's bytes do not hash to the digest the manifest records.
    DigestMismatch {
        /// Section name, e.g. `cols`.
        section: String,
        /// Digest the manifest records.
        expected: String,
        /// Digest of the bytes actually present.
        found: String,
    },
    /// A CSR invariant failed while reading an artifact back.
    Malformed(String),
}

impl Display for CnsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(detail) => write!(formatter, "read: {detail}"),
            Self::Artifact(detail) => write!(formatter, "artifact: {detail}"),
            Self::DigestMismatch {
                section,
                expected,
                found,
            } => write!(
                formatter,
                "digest mismatch in {section}: manifest {expected}, file {found}"
            ),
            Self::Malformed(detail) => write!(formatter, "malformed: {detail}"),
        }
    }
}

impl Error for CnsError {}

/// The dataset attribution the artifact must carry, fixed by the CC-BY licence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    /// Dataset identifier.
    pub dataset: String,
    /// SPDX licence identifier.
    pub licence: String,
    /// Canonical dataset URL.
    pub url: String,
    /// Citation to reproduce.
    pub citation: String,
}

impl Attribution {
    /// The Male CNS v1.0 attribution, as `NOTICE` states it.
    pub fn male_cns() -> Self {
        Self {
            dataset: "male-cns:v1.0".to_string(),
            licence: "CC-BY-4.0".to_string(),
            url: "https://male-cns.janelia.org".to_string(),
            citation: "Berg et al. 2026, Cell; doi:10.1016/j.cell.2026.08.015".to_string(),
        }
    }
}

/// The sign a presynaptic neurotransmitter contributes.
///
/// The release's `predicted_nt` / `consensus_nt` vocabulary is closed, so the
/// mapping is a total function of the label and is stated once here:
/// acetylcholine and glutamate are excitatory, GABA and histamine are
/// inhibitory, and the three monoamines the release reports (dopamine,
/// octopamine, serotonin) are modulatory — they get [`SIGN_UNKNOWN`] rather
/// than an invented polarity. `unclear` and a missing label are unknown too.
pub fn sign_of(label: &str) -> i8 {
    match label {
        "acetylcholine" | "glutamate" => SIGN_EXCITATORY,
        "gaba" | "histamine" => SIGN_INHIBITORY,
        _ => SIGN_UNKNOWN,
    }
}

/// One neuron of the artifact: a CSR node.
#[derive(Debug, Clone, PartialEq)]
pub struct Neuron {
    /// Released body id, the id every other Male CNS table keys on.
    pub body_id: u64,
    /// Index into [`Artifact::types`].
    pub type_index: u32,
    /// Soma position in the release's 8 nm voxel space, when the release
    /// carries one.
    pub soma: Option<[f32; 3]>,
}

/// The neuron-level graph in compressed sparse row form.
///
/// `rowptr` has one more entry than `neuron_ids`; row `i` spans
/// `rowptr[i]..rowptr[i + 1]` of `cols`, `sign` and `weight`, and `cols[j]` is
/// the post-synaptic node index of edge `j`.
#[derive(Debug, Clone, PartialEq)]
pub struct CnsGraph {
    /// Neuron body ids, ascending, in CSR row order.
    pub neuron_ids: Vec<u64>,
    /// Row offsets, length `neuron_ids.len() + 1`.
    pub rowptr: Vec<u64>,
    /// Post-synaptic node index per edge.
    pub cols: Vec<u32>,
    /// Sign per edge, from the presynaptic neuron's neurotransmitter.
    pub sign: Vec<i8>,
    /// Synapses on that edge, as the release counted them.
    pub weight: Vec<u16>,
}

impl CnsGraph {
    /// Number of CSR nodes.
    pub fn neuron_count(&self) -> u64 {
        self.neuron_ids.len() as u64
    }

    /// Number of CSR nonzeros.
    pub fn edge_count(&self) -> u64 {
        self.cols.len() as u64
    }

    /// Total synapses, the release's own count.
    pub fn synapse_count(&self) -> u64 {
        self.weight.iter().map(|weight| u64::from(*weight)).sum()
    }

    /// Synapses per sign class: `(excitatory, inhibitory, unknown)`.
    pub fn sign_totals(&self) -> (u64, u64, u64) {
        let mut totals = (0u64, 0u64, 0u64);
        for (sign, weight) in self.sign.iter().zip(&self.weight) {
            let slot = match *sign {
                SIGN_EXCITATORY => &mut totals.0,
                SIGN_INHIBITORY => &mut totals.1,
                _ => &mut totals.2,
            };
            *slot += u64::from(*weight);
        }
        totals
    }

    /// The incoming-edge CSR, the shape the spiking step consumes.
    ///
    /// A [`CnsGraph`] is pre→post; the LIF step needs, for every neuron, its
    /// incoming edges, so this is the transpose. It is a counting sort over the
    /// edge list, `O(E + N)`, and it is what the runner loads.
    pub fn incoming(&self) -> IncomingCsr {
        let neurons = self.neuron_ids.len();
        let edges = self.cols.len();
        let mut rowptr = vec![0u64; neurons + 1];
        for post in &self.cols {
            rowptr[*post as usize + 1] += 1;
        }
        for index in 0..neurons {
            rowptr[index + 1] += rowptr[index];
        }
        let mut cursor = rowptr.clone();
        let mut cols = vec![0u32; edges];
        let mut sign = vec![0i8; edges];
        let mut weight = vec![0u16; edges];
        for pre in 0..neurons {
            for edge in self.rowptr[pre] as usize..self.rowptr[pre + 1] as usize {
                let post = self.cols[edge];
                let slot = &mut cursor[post as usize];
                cols[*slot as usize] = pre as u32;
                sign[*slot as usize] = self.sign[edge];
                weight[*slot as usize] = self.weight[edge];
                *slot += 1;
            }
        }
        IncomingCsr {
            rowptr,
            cols,
            sign,
            weight,
        }
    }

    /// Whether the CSR invariants hold.
    pub fn is_well_formed(&self) -> bool {
        self.rowptr.len() == self.neuron_ids.len() + 1
            && self.cols.len() == self.sign.len()
            && self.cols.len() == self.weight.len()
            && self.rowptr.first().copied() == Some(0)
            && self.rowptr.last().copied() == Some(self.cols.len() as u64)
            && self.rowptr.windows(2).all(|pair| pair[0] <= pair[1])
            && self
                .cols
                .iter()
                .all(|column| (*column as usize) < self.neuron_ids.len())
    }
}

/// The transposed edge list: for each neuron, its incoming edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingCsr {
    /// Row offsets, length `neuron_count + 1`.
    pub rowptr: Vec<u64>,
    /// Presynaptic node index per incoming edge.
    pub cols: Vec<u32>,
    /// Sign per incoming edge.
    pub sign: Vec<i8>,
    /// Synapses per incoming edge.
    pub weight: Vec<u16>,
}

impl IncomingCsr {
    /// Number of CSR nodes.
    pub fn neuron_count(&self) -> usize {
        self.rowptr.len().saturating_sub(1)
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.cols.len()
    }
}

/// One per-arc section of a binary file: where it is and what it hashes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// Byte offset of the section inside its file.
    pub offset: u64,
    /// Byte length of the section.
    pub len: u64,
    /// SHA-256 of exactly those bytes.
    pub sha256: String,
}

/// One source table, as it was found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    /// File name.
    pub file: String,
    /// Byte size of the download.
    pub bytes: u64,
    /// SHA-256 of the download.
    pub sha256: String,
    /// Rows the importer read from it, where the table is row-shaped.
    pub rows: u64,
}

/// The emitted manifest, as written to `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema literal; always [`CNS_SCHEMA`].
    pub schema: String,
    /// CSR node count (the released neuron set the importer mapped).
    pub neuron_count: u64,
    /// CSR nonzeros.
    pub edge_count: u64,
    /// Total synapses over the emitted edges.
    pub synapse_count: u64,
    /// Synapses on excitatory edges.
    pub excitatory_synapses: u64,
    /// Synapses on inhibitory edges.
    pub inhibitory_synapses: u64,
    /// Synapses on edges whose presynaptic neurotransmitter is unknown.
    pub unknown_synapses: u64,
    /// Rows of the released weights table the importer consumed.
    pub source_rows: u64,
    /// Synapses the released weights table reports over all its rows.
    pub source_synapses: u64,
    /// Weights rows whose pre or post end is not a released neuron.
    pub dropped_rows: u64,
    /// Duplicate `(pre, post)` rows folded into one edge.
    pub folded_rows: u64,
    /// Neurons that appear as the pre end of at least one emitted edge.
    pub presynaptic_neurons: u64,
    /// Neurons with a released soma position.
    pub positioned_count: u64,
    /// Interned cell-type count; `types.txt` has this many lines.
    pub type_count: u64,
    /// Sections of `weights.bin`.
    pub weights_sections: std::collections::BTreeMap<String, Section>,
    /// Sections of `positions.bin`.
    pub positions_sections: std::collections::BTreeMap<String, Section>,
    /// `types.txt` digest.
    pub types_sha256: String,
    /// `nodes.txt` digest.
    pub nodes_sha256: String,
    /// The source tables.
    pub sources: std::collections::BTreeMap<String, SourceFile>,
    /// Build time, milliseconds since the Unix epoch.
    pub created_at_ms: u128,
    /// The dataset attribution.
    pub attribution: Attribution,
}

/// A loaded, verified artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct Artifact {
    /// The graph.
    pub graph: CnsGraph,
    /// The neurons, in CSR node order.
    pub neurons: Vec<Neuron>,
    /// Interned cell-type labels.
    pub types: Vec<String>,
    /// Released annotations, in CSR node order.
    pub nodes: Vec<NodeMeta>,
    /// The manifest the artifact was loaded with.
    pub manifest: Manifest,
}

impl Artifact {
    /// Load and digest-verify an artifact directory.
    pub fn load(dir: &Path) -> Result<Self, CnsError> {
        let manifest: Manifest = read_json(&dir.join(MANIFEST_FILE))?;
        if manifest.schema != CNS_SCHEMA {
            return Err(CnsError::Artifact(format!(
                "{}: schema {:?}, expected {:?}",
                dir.join(MANIFEST_FILE).display(),
                manifest.schema,
                CNS_SCHEMA
            )));
        }
        let weights = fs::read(dir.join(WEIGHTS_FILE))
            .map_err(|error| read_error(&dir.join(WEIGHTS_FILE), error))?;
        let positions = fs::read(dir.join(POSITIONS_FILE))
            .map_err(|error| read_error(&dir.join(POSITIONS_FILE), error))?;
        let types_bytes = fs::read(dir.join(TYPES_FILE))
            .map_err(|error| read_error(&dir.join(TYPES_FILE), error))?;
        let nodes_bytes = fs::read(dir.join(NODES_FILE))
            .map_err(|error| read_error(&dir.join(NODES_FILE), error))?;

        verify_section(&weights, "rowptr", manifest.weights_sections.get("rowptr"))?;
        verify_section(&weights, "cols", manifest.weights_sections.get("cols"))?;
        verify_section(&weights, "sign", manifest.weights_sections.get("sign"))?;
        verify_section(&weights, "weight", manifest.weights_sections.get("weight"))?;
        verify_section(&positions, "id", manifest.positions_sections.get("id"))?;
        verify_section(&positions, "x", manifest.positions_sections.get("x"))?;
        verify_section(&positions, "y", manifest.positions_sections.get("y"))?;
        verify_section(&positions, "z", manifest.positions_sections.get("z"))?;
        verify_section(&positions, "cell_type", manifest.positions_sections.get("cell_type"))?;
        let found = hex_digest(&types_bytes);
        if found != manifest.types_sha256 {
            return Err(CnsError::DigestMismatch {
                section: TYPES_FILE.to_string(),
                expected: manifest.types_sha256.clone(),
                found,
            });
        }
        let found = hex_digest(&nodes_bytes);
        if found != manifest.nodes_sha256 {
            return Err(CnsError::DigestMismatch {
                section: NODES_FILE.to_string(),
                expected: manifest.nodes_sha256.clone(),
                found,
            });
        }

        let mut graph = decode_weights(&weights, &manifest)?;
        let neurons = decode_positions(&positions, &types_bytes, &manifest)?;
        graph.neuron_ids = neurons.iter().map(|neuron| neuron.body_id).collect();
        if !graph.is_well_formed() {
            return Err(CnsError::Malformed("loaded CSR is not well formed".into()));
        }
        let nodes = parse_nodes(&nodes_bytes);
        if nodes.len() != neurons.len() {
            return Err(CnsError::Malformed(format!(
                "{} holds {} rows, the artifact has {} neurons",
                NODES_FILE,
                nodes.len(),
                neurons.len()
            )));
        }

        Ok(Self {
            graph,
            neurons,
            types: split_types(&types_bytes),
            nodes,
            manifest,
        })
    }

    /// The incoming-edge CSR the runner steps.
    pub fn incoming(&self) -> IncomingCsr {
        self.graph.incoming()
    }
}

fn split_types(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out: Vec<String> = text.lines().map(str::to_string).collect();
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// Parse `nodes.txt` back into the per-neuron annotations.
///
/// A blank input is legal: an artifact whose importer carried no metadata is
/// still readable, and the loader then reports zero rows here.
pub fn parse_nodes(bytes: &[u8]) -> Vec<NodeMeta> {
    let text = String::from_utf8_lossy(bytes);
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            NodeMeta {
                body_id: fields.next().unwrap_or_default().parse().unwrap_or(0),
                type_name: fields.next().unwrap_or_default().to_string(),
                superclass: fields.next().unwrap_or_default().to_string(),
                side: fields.next().unwrap_or_default().to_string(),
                instance: fields.next().unwrap_or_default().to_string(),
            }
        })
        .collect()
}

/// The literal `nodes.txt` bytes for a set of neurons.
pub fn encode_nodes(nodes: &[NodeMeta]) -> String {
    let mut text = String::with_capacity(nodes.len() * 32);
    for node in nodes {
        text.push_str(&node.body_id.to_string());
        text.push('\t');
        text.push_str(&node.type_name);
        text.push('\t');
        text.push_str(&node.superclass);
        text.push('\t');
        text.push_str(&node.side);
        text.push('\t');
        text.push_str(&node.instance);
        text.push('\n');
    }
    text
}

fn verify_section(bytes: &[u8], name: &str, section: Option<&Section>) -> Result<(), CnsError> {
    let section = section.ok_or_else(|| {
        CnsError::Artifact(format!("manifest carries no {name} section"))
    })?;
    let start = section.offset as usize;
    let end = start
        .checked_add(section.len as usize)
        .ok_or_else(|| CnsError::Malformed(format!("{name} section overflows")))?;
    let slice = bytes.get(start..end).ok_or_else(|| {
        CnsError::Artifact(format!(
            "{name} section [{start}, {end}) is outside the {} byte file",
            bytes.len()
        ))
    })?;
    let found = hex_digest(slice);
    if found != section.sha256 {
        return Err(CnsError::DigestMismatch {
            section: name.to_string(),
            expected: section.sha256.clone(),
            found,
        });
    }
    Ok(())
}

fn decode_weights(bytes: &[u8], manifest: &Manifest) -> Result<CnsGraph, CnsError> {
    let header = Header::parse(bytes, WEIGHTS_MAGIC, "weights")?;
    let node_count = header.count as usize;
    if node_count as u64 != manifest.neuron_count {
        return Err(CnsError::Malformed(format!(
            "weights header holds {node_count} nodes, manifest {}",
            manifest.neuron_count
        )));
    }
    let edge_count = header.edges as usize;
    if edge_count as u64 != manifest.edge_count {
        return Err(CnsError::Malformed(format!(
            "weights header holds {edge_count} edges, manifest {}",
            manifest.edge_count
        )));
    }
    let mut cursor = WEIGHTS_HEADER_LEN;
    let rowptr = take_u64(bytes, &mut cursor, node_count + 1)?;
    let cols = take_u32(bytes, &mut cursor, edge_count)?;
    let sign = take_i8(bytes, &mut cursor, edge_count)?;
    let weight = take_u16(bytes, &mut cursor, edge_count)?;
    let graph = CnsGraph {
        neuron_ids: Vec::new(),
        rowptr,
        cols,
        sign,
        weight,
    };
    if graph.rowptr.len() != node_count + 1 {
        return Err(CnsError::Malformed(format!(
            "rowptr holds {} entries, expected {}",
            graph.rowptr.len(),
            node_count + 1
        )));
    }
    if graph.rowptr.last().copied() != Some(edge_count as u64) {
        return Err(CnsError::Malformed(format!(
            "rowptr ends at {:?}, expected {edge_count}",
            graph.rowptr.last()
        )));
    }
    Ok(graph)
}

fn decode_positions(
    bytes: &[u8],
    types_bytes: &[u8],
    manifest: &Manifest,
) -> Result<Vec<Neuron>, CnsError> {
    let header = Header::parse(bytes, POSITIONS_MAGIC, "positions")?;
    let count = header.count as usize;
    if count as u64 != manifest.neuron_count {
        return Err(CnsError::Malformed(format!(
            "positions header holds {count} rows, manifest {}",
            manifest.neuron_count
        )));
    }
    let _types = types_bytes;
    let mut cursor = BASE_HEADER_LEN;
    let ids = take_u32(bytes, &mut cursor, count)?;
    let xs = take_f32(bytes, &mut cursor, count)?;
    let ys = take_f32(bytes, &mut cursor, count)?;
    let zs = take_f32(bytes, &mut cursor, count)?;
    let cell_type = take_u32(bytes, &mut cursor, count)?;
    let mut neurons = Vec::with_capacity(count);
    for index in 0..count {
        let soma = if xs[index].is_finite() {
            Some([xs[index], ys[index], zs[index]])
        } else {
            None
        };
        neurons.push(Neuron {
            body_id: u64::from(ids[index]),
            type_index: cell_type[index],
            soma,
        });
    }
    Ok(neurons)
}

/// `magic(4) version u16 reserved u16 count u32` = 12 bytes.
const BASE_HEADER_LEN: usize = 12;
/// The weights header adds a reserved u32 before the u64 edge count, so the
/// `rowptr` section starts 8-aligned.
const WEIGHTS_HEADER_LEN: usize = 24;

struct Header {
    count: u32,
    edges: u64,
}

impl Header {
    fn parse(bytes: &[u8], magic: [u8; 4], what: &str) -> Result<Self, CnsError> {
        if bytes.len() < BASE_HEADER_LEN {
            return Err(CnsError::Artifact(format!(
                "{what}: {} bytes is shorter than the {BASE_HEADER_LEN} byte header",
                bytes.len()
            )));
        }
        if bytes[0..4] != magic {
            return Err(CnsError::Artifact(format!(
                "{what}: bad magic {:02x?}, expected {:02x?}",
                &bytes[0..4],
                magic
            )));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != FORMAT_VERSION {
            return Err(CnsError::Artifact(format!(
                "{what}: format version {version}, this build writes {FORMAT_VERSION}"
            )));
        }
        let count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let edges = if bytes.len() >= WEIGHTS_HEADER_LEN {
            u64::from_le_bytes([
                bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22],
                bytes[23],
            ])
        } else {
            0
        };
        Ok(Self { count, edges })
    }
}

fn take_u64(bytes: &[u8], cursor: &mut usize, count: usize) -> Result<Vec<u64>, CnsError> {
    let span = count * 8;
    let slice = slice_at(bytes, cursor, span, "u64")?;
    Ok(slice
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("8 bytes")))
        .collect())
}

fn take_u32(bytes: &[u8], cursor: &mut usize, count: usize) -> Result<Vec<u32>, CnsError> {
    let span = count * 4;
    let slice = slice_at(bytes, cursor, span, "u32")?;
    Ok(slice
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4 bytes")))
        .collect())
}

fn take_i8(bytes: &[u8], cursor: &mut usize, count: usize) -> Result<Vec<i8>, CnsError> {
    let slice = slice_at(bytes, cursor, count, "i8")?;
    Ok(slice.iter().map(|byte| *byte as i8).collect())
}

fn take_u16(bytes: &[u8], cursor: &mut usize, count: usize) -> Result<Vec<u16>, CnsError> {
    let span = count * 2;
    let slice = slice_at(bytes, cursor, span, "u16")?;
    Ok(slice
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes(chunk.try_into().expect("2 bytes")))
        .collect())
}

fn take_f32(bytes: &[u8], cursor: &mut usize, count: usize) -> Result<Vec<f32>, CnsError> {
    let span = count * 4;
    let slice = slice_at(bytes, cursor, span, "f32")?;
    Ok(slice
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("4 bytes")))
        .collect())
}

fn slice_at<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    span: usize,
    what: &str,
) -> Result<&'a [u8], CnsError> {
    let end = cursor
        .checked_add(span)
        .ok_or_else(|| CnsError::Malformed(format!("{what} section overflows")))?;
    let slice = bytes.get(*cursor..end).ok_or_else(|| {
        CnsError::Malformed(format!(
            "{what} section [{}, {end}) runs past the {}-byte file",
            *cursor,
            bytes.len()
        ))
    })?;
    *cursor = end;
    Ok(slice)
}

/// Encode the weights file: header, then the four sections concatenated.
pub fn encode_weights(graph: &CnsGraph) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        WEIGHTS_HEADER_LEN + 8 * (graph.rowptr.len() + 4 * graph.cols.len() + 3 * graph.sign.len()),
    );
    out.extend_from_slice(&WEIGHTS_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(graph.neuron_ids.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(graph.cols.len() as u64).to_le_bytes());
    for value in &graph.rowptr {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in &graph.cols {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend(graph.sign.iter().map(|value| *value as u8));
    for value in &graph.weight {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Encode the positions file: header, then the five SoA sections.
pub fn encode_positions(neurons: &[Neuron]) -> Vec<u8> {
    let count = neurons.len();
    let mut out = Vec::with_capacity(BASE_HEADER_LEN + count * 20);
    out.extend_from_slice(&POSITIONS_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for neuron in neurons {
        out.extend_from_slice(&(neuron.body_id as u32).to_le_bytes());
    }
    for axis in 0..3 {
        for neuron in neurons {
            let value = neuron.soma.map_or(f32::NAN, |soma| soma[axis]);
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    for neuron in neurons {
        out.extend_from_slice(&neuron.type_index.to_le_bytes());
    }
    out
}

/// The byte range of one weights section, for the manifest.
pub struct WeightsLayout {
    /// `(rowptr, cols, sign, weight)` offsets and lengths.
    pub sections: Vec<(&'static str, usize, usize)>,
}

/// Where each weights section starts and how long it is.
pub fn weights_layout(neuron_count: usize, edge_count: usize) -> WeightsLayout {
    let rowptr = WEIGHTS_HEADER_LEN;
    let rowptr_len = 8 * (neuron_count + 1);
    let cols = rowptr + rowptr_len;
    let cols_len = 4 * edge_count;
    let sign = cols + cols_len;
    let sign_len = edge_count;
    let weight = sign + sign_len;
    let weight_len = 2 * edge_count;
    WeightsLayout {
        sections: vec![
            ("rowptr", rowptr, rowptr_len),
            ("cols", cols, cols_len),
            ("sign", sign, sign_len),
            ("weight", weight, weight_len),
        ],
    }
}

/// Where each positions section starts and how long it is.
pub fn positions_layout(neuron_count: usize) -> Vec<(&'static str, usize, usize)> {
    let mut offset = BASE_HEADER_LEN;
    let mut out = Vec::new();
    for (name, width) in [
        ("id", 4usize),
        ("x", 4),
        ("y", 4),
        ("z", 4),
        ("cell_type", 4),
    ] {
        let len = width * neuron_count;
        out.push((name, offset, len));
        offset += len;
    }
    out
}

fn section_of(bytes: &[u8], offset: usize, len: usize) -> Section {
    Section {
        offset: offset as u64,
        len: len as u64,
        sha256: hex_digest(&bytes[offset..offset + len]),
    }
}

/// Write `weights.bin`, `positions.bin`, `types.txt`, `manifest.json` and
/// `attribution.json` into `dir`.
#[allow(clippy::too_many_arguments)]
pub fn write_artifact(
    dir: &Path,
    graph: &CnsGraph,
    neurons: &[Neuron],
    nodes: &[NodeMeta],
    types: &[String],
    sources: std::collections::BTreeMap<String, SourceFile>,
    source_rows: u64,
    source_synapses: u64,
    dropped_rows: u64,
    folded_rows: u64,
    presynaptic_neurons: u64,
) -> Result<Manifest, CnsError> {
    fs::create_dir_all(dir).map_err(|error| read_error(dir, error))?;

    let weights = encode_weights(graph);
    let positions = encode_positions(neurons);
    let nodes_text = encode_nodes(nodes);
    let types_text = {
        let mut text = String::new();
        for name in types {
            text.push_str(name);
            text.push('\n');
        }
        text
    };

    let mut weights_sections = std::collections::BTreeMap::new();
    for (name, offset, len) in weights_layout(graph.neuron_ids.len(), graph.cols.len()).sections {
        weights_sections.insert(name.to_string(), section_of(&weights, offset, len));
    }
    let mut positions_sections = std::collections::BTreeMap::new();
    for (name, offset, len) in positions_layout(neurons.len()) {
        positions_sections.insert(name.to_string(), section_of(&positions, offset, len));
    }

    let (excitatory, inhibitory, unknown) = graph.sign_totals();
    let manifest = Manifest {
        schema: CNS_SCHEMA.to_string(),
        neuron_count: graph.neuron_count(),
        edge_count: graph.edge_count(),
        synapse_count: graph.synapse_count(),
        excitatory_synapses: excitatory,
        inhibitory_synapses: inhibitory,
        unknown_synapses: unknown,
        source_rows,
        source_synapses,
        dropped_rows,
        folded_rows,
        presynaptic_neurons,
        positioned_count: neurons.iter().filter(|neuron| neuron.soma.is_some()).count() as u64,
        type_count: types.len() as u64,
        weights_sections,
        positions_sections,
        types_sha256: hex_digest(types_text.as_bytes()),
        nodes_sha256: hex_digest(nodes_text.as_bytes()),
        sources,
        created_at_ms: now_ms(),
        attribution: Attribution::male_cns(),
    };

    fs::write(dir.join(WEIGHTS_FILE), &weights).map_err(|error| read_error(dir, error))?;
    fs::write(dir.join(POSITIONS_FILE), &positions).map_err(|error| read_error(dir, error))?;
    fs::write(dir.join(TYPES_FILE), types_text.as_bytes()).map_err(|error| read_error(dir, error))?;
    fs::write(dir.join(NODES_FILE), nodes_text.as_bytes()).map_err(|error| read_error(dir, error))?;
    write_json(dir.join(ATTRIBUTION_FILE), &manifest.attribution)?;
    write_json(dir.join(MANIFEST_FILE), &manifest)?;
    Ok(manifest)
}

fn write_json<T: Serialize>(path: PathBuf, value: &T) -> Result<(), CnsError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| CnsError::Artifact(format!("{}: {error}", path.display())))?;
    fs::write(&path, format!("{text}\n")).map_err(|error| read_error(&path, error))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CnsError> {
    let bytes = fs::read(path).map_err(|error| read_error(path, error))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| CnsError::Artifact(format!("{}: {error}", path.display())))
}

/// Build a [`CnsError::Read`] from a path and an I/O error.
pub fn read_error(path: &Path, error: std::io::Error) -> CnsError {
    CnsError::Read(format!("{}: {error}", path.display()))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hex SHA-256 of a whole file, read in bounded chunks.
pub fn file_digest(path: &Path) -> Result<(u64, String), CnsError> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).map_err(|error| read_error(path, error))?;
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| read_error(path, error))?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    Ok((bytes, out))
}
