//! The two checks the ticket names: the artifact's counts against the release,
//! and the digest gate refusing a tampered artifact.
//!
//! The counts check reads the released tables from `QUALIA_MALECNS_DIR` when it
//! is set (the directory holding `connectome-weights.feather`,
//! `body-annotations.feather` and `body-neurotransmitters.feather`); the
//! download is 1.1 GB and is not vendored, so without it the check says so and
//! returns rather than passing on nothing. Run it as:
//!
//! ```console
//! $ QUALIA_MALECNS_DIR=/path/to/flat-connectome cargo test -p qualia-connectome-cns \
//!       --test cns the_artifact_round_trips_the_released_synapse_counts -- --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use qualia_connectome_cns::{
    import, write_artifact, Artifact, CnsError, CnsGraph, NodeMeta, Neuron, Source,
};

const EXPECTED_NEURONS: u64 = 166_700;
const EXPECTED_EDGES: u64 = 25_582_938;
const EXPECTED_SYNAPSES: u64 = 124_177_617;
const EXPECTED_SOURCE_ROWS: u64 = 151_856_684;
const EXPECTED_SOURCE_SYNAPSES: u64 = 311_833_243;
const EXPECTED_EXCITATORY: u64 = 94_542_746;
const EXPECTED_INHIBITORY: u64 = 26_402_637;
const EXPECTED_UNKNOWN: u64 = 3_232_234;

/// The release's own totals must come back out of the artifact.
///
/// Two things are asserted, because they are the two ways the count can be
/// wrong: the counts the importer read from the source (`source_rows`,
/// `source_synapses` — the released table's row count and the sum of its
/// `weight` column, which equals the sum of `post` in `body-stats`), and the
/// counts the artifact carries (nodes, edges, synapses).
#[test]
fn the_artifact_round_trips_the_released_synapse_counts() {
    let Some(dir) = std::env::var_os("QUALIA_MALECNS_DIR").map(PathBuf::from) else {
        eprintln!(
            "the_artifact_round_trips_the_released_synapse_counts: skipped — \
             QUALIA_MALECNS_DIR is not set (the released flat tables are 1.1 GB and not vendored)"
        );
        return;
    };
    let out = std::env::temp_dir().join("qualia-cns-counts");
    let _ = std::fs::remove_dir_all(&out);
    let report = import(
        &Source {
            weights: dir.join("connectome-weights.feather"),
            annotations: dir.join("body-annotations.feather"),
            neurotransmitters: dir.join("body-neurotransmitters.feather"),
        },
        &out,
    )
    .expect("import");

    assert_eq!(
        report.source_rows, EXPECTED_SOURCE_ROWS,
        "weights table row count"
    );
    assert_eq!(
        report.source_synapses, EXPECTED_SOURCE_SYNAPSES,
        "weights table synapse total"
    );
    assert_eq!(report.neurons, EXPECTED_NEURONS, "released neuron count");

    // ... and the same counts after a round trip through the files.
    let loaded = Artifact::load(&out).expect("load");
    assert_eq!(loaded.manifest.neuron_count, EXPECTED_NEURONS);
    assert_eq!(loaded.manifest.edge_count, EXPECTED_EDGES);
    assert_eq!(loaded.manifest.synapse_count, EXPECTED_SYNAPSES);
    assert_eq!(loaded.graph.neuron_count(), EXPECTED_NEURONS);
    assert_eq!(loaded.graph.edge_count(), EXPECTED_EDGES);
    assert_eq!(loaded.graph.synapse_count(), EXPECTED_SYNAPSES);
    assert_eq!(
        loaded.graph.sign_totals(),
        (EXPECTED_EXCITATORY, EXPECTED_INHIBITORY, EXPECTED_UNKNOWN),
        "sign classes"
    );
    assert_eq!(
        loaded.manifest.edge_count + loaded.manifest.dropped_rows + loaded.manifest.folded_rows,
        loaded.manifest.source_rows,
        "every source row is either an edge, a fold, or a counted drop"
    );
    eprintln!(
        "counts: {} neurons, {} edges, {} synapses from {} rows / {} source synapses",
        loaded.manifest.neuron_count,
        loaded.manifest.edge_count,
        loaded.manifest.synapse_count,
        loaded.manifest.source_rows,
        loaded.manifest.source_synapses,
    );
}

/// A mutated sign must not load: the digest gate is the artifact's integrity.
#[test]
fn a_mutated_sign_fails_the_digest_check() {
    let dir = std::env::temp_dir().join("qualia-cns-mutated-sign");
    let _ = std::fs::remove_dir_all(&dir);
    let graph = CnsGraph {
        neuron_ids: vec![10, 20, 30],
        rowptr: vec![0, 1, 3, 3],
        cols: vec![1, 0, 2],
        // One edge of each polarity, so the mutation below is unambiguous.
        sign: vec![1, -1, 1],
        weight: vec![7, 5, 3],
    };
    let neurons: Vec<Neuron> = graph
        .neuron_ids
        .iter()
        .enumerate()
        .map(|(index, body_id)| Neuron {
            body_id: *body_id,
            type_index: index as u32,
            soma: Some([index as f32, 0.0, 0.0]),
        })
        .collect();
    let nodes: Vec<NodeMeta> = graph
        .neuron_ids
        .iter()
        .map(|body_id| NodeMeta {
            body_id: *body_id,
            type_name: "test".to_string(),
            superclass: "cb_intrinsic".to_string(),
            side: "L".to_string(),
            instance: "test_L".to_string(),
        })
        .collect();
    write_artifact(
        &dir,
        &graph,
        &neurons,
        &nodes,
        &["test".to_string()],
        BTreeMap::new(),
        3,
        15,
        0,
        0,
        2,
    )
    .expect("write");

    let loaded = Artifact::load(&dir).expect("a freshly written artifact loads");
    assert_eq!(loaded.graph.sign, vec![1, -1, 1]);

    // Flip one sign in place and nothing else.
    let path = dir.join("weights.bin");
    let mut bytes = std::fs::read(&path).expect("read weights");
    let sign_offset = 24 + 8 * 4 + 4 * 3;
    assert_eq!(bytes[sign_offset], 1, "first sign byte");
    bytes[sign_offset] = 255; // -1
    std::fs::write(&path, &bytes).expect("write weights");

    match Artifact::load(&dir) {
        Err(CnsError::DigestMismatch { section, .. }) => {
            assert_eq!(section, "sign", "the refused section is the mutated one");
        }
        other => panic!("a mutated sign must fail the digest check, got {other:?}"),
    }
}
