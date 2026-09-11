//! The committed 3D layout of the prior, and whether it still fits it.
//!
//! The layout is computed offline by `assets/brain/make_layout.py` (a seeded
//! spectral embedding of the CSR — see that script and `assets/brain/README.md`
//! for the algorithm and the regeneration command) and committed beside the
//! prior. It is a function of the graph, so it carries the prior's digests: the
//! console refuses to place nodes with a layout derived from a different prior
//! rather than draw a graph in positions that mean nothing.

use serde::Deserialize;

use super::prior::PriorGraph;

/// The layout schema this reader understands.
pub const LAYOUT_SCHEMA: &str = "qualia.brain-layout.v1";

/// The committed layout, embedded so the offline console needs no file.
const COMMITTED_LAYOUT: &str = include_str!("../../../../../assets/brain/layout.json");

/// The prior a layout was derived from.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LayoutPrior {
    pub source_sha256: String,
    #[serde(default)]
    pub graph_sha256: String,
    pub type_count: u64,
    pub edge_count: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct LayoutFile {
    schema: String,
    algorithm: String,
    seed: u64,
    prior: LayoutPrior,
    nodes: Vec<[f32; 3]>,
}

/// A committed 3D layout: one position per prior type, in type order.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub algorithm: String,
    pub seed: u64,
    pub prior: LayoutPrior,
    pub positions: Vec<[f32; 3]>,
}

impl Layout {
    pub fn parse(json: &str) -> Result<Self, String> {
        let file: LayoutFile =
            serde_json::from_str(json).map_err(|error| format!("layout.json: {error}"))?;
        if file.schema != LAYOUT_SCHEMA {
            return Err(format!(
                "layout.json: schema {} is not {LAYOUT_SCHEMA}",
                file.schema
            ));
        }
        if file.nodes.len() != file.prior.type_count as usize {
            return Err(format!(
                "layout.json: {} positions for {} types",
                file.nodes.len(),
                file.prior.type_count
            ));
        }
        Ok(Self {
            algorithm: file.algorithm,
            seed: file.seed,
            prior: file.prior,
            positions: file.nodes,
        })
    }

    /// The layout committed under `assets/brain/layout.json`.
    pub fn committed() -> Result<Self, String> {
        Self::parse(COMMITTED_LAYOUT)
    }

    /// Whether this layout was derived from `prior`.
    ///
    /// The builder sets the manifest's `source_sha256` to the digest of the
    /// graph it wrote, so it is the graph identity the layout records.
    pub fn matches(&self, prior: &PriorGraph) -> bool {
        !prior.manifest.source_sha256.is_empty()
            && self.prior.source_sha256 == prior.manifest.source_sha256
            && self.prior.type_count as usize == prior.type_count()
            && self.prior.edge_count as usize == prior.edge_count()
    }

    pub fn node_count(&self) -> usize {
        self.positions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_layout_matches_the_committed_prior() {
        let prior = PriorGraph::committed().expect("committed prior loads");
        let layout = Layout::committed().expect("committed layout parses");
        assert!(layout.matches(&prior), "the committed pair is consistent");
        assert_eq!(layout.node_count(), prior.type_count());
        assert!(!layout.algorithm.is_empty());
    }

    #[test]
    fn a_layout_for_another_prior_does_not_match() {
        let mut prior = PriorGraph::committed().expect("committed prior loads");
        prior.manifest.source_sha256 = "0".repeat(64);
        let layout = Layout::committed().expect("committed layout parses");
        assert!(!layout.matches(&prior));
    }

    #[test]
    fn a_layout_with_the_wrong_number_of_positions_is_refused() {
        let json = r#"{
            "schema": "qualia.brain-layout.v1",
            "algorithm": "spectral-laplacian-v1",
            "seed": 1,
            "prior": {"source_sha256": "x", "type_count": 2, "edge_count": 1},
            "nodes": [[0.0, 0.0, 0.0]]
        }"#;
        let error = Layout::parse(json).expect_err("a short layout is refused");
        assert!(error.contains("positions"), "{error}");
    }
}
