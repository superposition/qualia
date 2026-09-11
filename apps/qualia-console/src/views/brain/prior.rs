//! The connectome prior the firing model is drawn over.
//!
//! `crates/connectome-prior` emits a self-describing artifact: `manifest.json`
//! records how many types and edges the graph carries, and `graph.bin` holds the
//! little-endian `rowptr` (`u64`), `cols` (`u32`) and `weights` (`u32`) sections
//! back to back. The console reads the same artifact the runtime loads
//! (`QUALIA_FLY_PRIOR_PATH`) so the graph it draws is the graph the belief
//! runners couple, and falls back to the small prior committed under
//! `assets/brain/prior/` when no deployment names one.
//!
//! This is the one place in the console that reads a graph file; the CSR
//! invariants are checked the way `crates/fly-circuit` checks them on load, so a
//! truncated or edited artifact is reported instead of drawn.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Manifest file name inside a prior directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Binary graph file name inside a prior directory.
pub const GRAPH_FILE: &str = "graph.bin";
/// The manifest schema this reader understands.
pub const PRIOR_SCHEMA: &str = "qualia.connectome-prior.v1";
/// The environment key a deployment points at its prior artifact.
pub const PRIOR_DIR_ENV: &str = "QUALIA_FLY_PRIOR_PATH";

/// The committed reference prior, embedded so the offline console needs no file.
const COMMITTED_MANIFEST: &str = include_str!("../../../../../assets/brain/prior/manifest.json");
const COMMITTED_GRAPH: &[u8] = include_bytes!("../../../../../assets/brain/prior/graph.bin");

/// The manifest fields the console reads.
///
/// The builder writes more (`created_at_ms`, `attribution`, …); serde ignores
/// what is not named here, so a future manifest stays readable.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PriorManifest {
    pub schema: String,
    pub type_count: u64,
    pub edge_count: u64,
    #[serde(default)]
    pub source_sha256: String,
    #[serde(default)]
    pub rowptr_sha256: String,
    #[serde(default)]
    pub cols_sha256: String,
    #[serde(default)]
    pub weights_sha256: String,
}

/// Where the graph on screen came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriorSource {
    /// The prior committed under `assets/brain/prior/`.
    Committed,
    /// The artifact `QUALIA_FLY_PRIOR_PATH` names.
    Path(PathBuf),
}

impl PriorSource {
    pub fn label(&self) -> String {
        match self {
            PriorSource::Committed => "committed assets/brain/prior".to_owned(),
            PriorSource::Path(path) => path.display().to_string(),
        }
    }
}

/// The type-level graph in compressed sparse row form.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorGraph {
    pub manifest: PriorManifest,
    pub rowptr: Vec<u64>,
    pub cols: Vec<u32>,
    pub weights: Vec<u32>,
    pub source: PriorSource,
}

impl PriorGraph {
    /// Validate `manifest_json` and `graph` and build the graph.
    pub fn from_parts(
        manifest_json: &str,
        graph: &[u8],
        source: PriorSource,
    ) -> Result<Self, String> {
        let manifest: PriorManifest = serde_json::from_str(manifest_json)
            .map_err(|error| format!("{MANIFEST_FILE}: {error}"))?;
        if manifest.schema != PRIOR_SCHEMA {
            return Err(format!(
                "{MANIFEST_FILE}: schema {} is not {PRIOR_SCHEMA}",
                manifest.schema
            ));
        }
        let type_count = manifest.type_count as usize;
        let edge_count = manifest.edge_count as usize;
        let expected = (type_count + 1) * std::mem::size_of::<u64>()
            + edge_count * std::mem::size_of::<u32>() * 2;
        if graph.len() != expected {
            return Err(format!(
                "{GRAPH_FILE}: {} bytes do not match {type_count} types and {edge_count} edges",
                graph.len()
            ));
        }

        let rowptr_bytes = (type_count + 1) * std::mem::size_of::<u64>();
        let cols_bytes = edge_count * std::mem::size_of::<u32>();
        let rowptr = decode_u64(&graph[..rowptr_bytes]);
        let cols = decode_u32(&graph[rowptr_bytes..rowptr_bytes + cols_bytes]);
        let weights = decode_u32(&graph[rowptr_bytes + cols_bytes..]);

        if rowptr.first().copied() != Some(0)
            || rowptr.last().copied() != Some(edge_count as u64)
            || rowptr.windows(2).any(|pair| pair[0] > pair[1])
            || cols.iter().any(|column| *column as usize >= type_count)
        {
            return Err(format!(
                "{GRAPH_FILE}: CSR invariants do not hold for {type_count} types and {edge_count} edges"
            ));
        }

        Ok(Self {
            manifest,
            rowptr,
            cols,
            weights,
            source,
        })
    }

    /// Read the artifact `dir` names.
    pub fn load_dir(dir: &Path) -> Result<Self, String> {
        let manifest_path = dir.join(MANIFEST_FILE);
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        let graph_path = dir.join(GRAPH_FILE);
        let graph = std::fs::read(&graph_path)
            .map_err(|error| format!("{}: {error}", graph_path.display()))?;
        Self::from_parts(&manifest, &graph, PriorSource::Path(dir.to_path_buf()))
    }

    /// The prior committed under `assets/brain/prior/`.
    pub fn committed() -> Result<Self, String> {
        Self::from_parts(COMMITTED_MANIFEST, COMMITTED_GRAPH, PriorSource::Committed)
    }

    pub fn type_count(&self) -> usize {
        self.manifest.type_count as usize
    }

    pub fn edge_count(&self) -> usize {
        self.manifest.edge_count as usize
    }

    /// Every edge as `(pre_type, post_type, weight)`, in CSR order.
    pub fn edges(&self) -> impl Iterator<Item = (usize, usize, u32)> + '_ {
        (0..self.type_count()).flat_map(move |source| {
            let start = self.rowptr[source] as usize;
            let end = self.rowptr[source + 1] as usize;
            (start..end).map(move |edge| {
                (
                    source,
                    self.cols[edge] as usize,
                    self.weights[edge],
                )
            })
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_prior_loads_with_its_manifest_numbers() {
        let prior = PriorGraph::committed().expect("committed prior loads");
        assert_eq!(prior.type_count(), prior.manifest.type_count as usize);
        assert_eq!(prior.edge_count(), prior.cols.len());
        assert_eq!(prior.edges().count(), prior.edge_count());
        assert_eq!(prior.source, PriorSource::Committed);
    }

    #[test]
    fn a_truncated_graph_is_refused() {
        let prior = PriorGraph::committed().expect("committed prior loads");
        let mut graph = prior
            .rowptr
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<u8>>();
        graph.extend(prior.cols.iter().flat_map(|value| value.to_le_bytes()));
        graph.extend(prior.weights.iter().flat_map(|value| value.to_le_bytes()));
        let error = PriorGraph::from_parts(COMMITTED_MANIFEST, &graph[..graph.len() - 4], PriorSource::Committed)
            .expect_err("a short graph is refused");
        assert!(error.contains("do not match"), "{error}");
    }

    #[test]
    fn a_graph_whose_columns_leave_the_type_range_is_refused() {
        let mut graph = Vec::new();
        graph.extend(0u64.to_le_bytes());
        graph.extend(1u64.to_le_bytes());
        graph.extend(1u64.to_le_bytes());
        graph.extend(9u32.to_le_bytes());
        graph.extend(1u32.to_le_bytes());
        let manifest = r#"{"schema":"qualia.connectome-prior.v1","type_count":2,"edge_count":1}"#;
        let error = PriorGraph::from_parts(manifest, &graph, PriorSource::Committed)
            .expect_err("an out-of-range column is refused");
        assert!(error.contains("CSR invariants"), "{error}");
    }
}
