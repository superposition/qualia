//! Contract tests for loading the connectome prior as coupling.
//!
//! The artifact these tests read is written here in the exact shape the
//! builder emits: three little-endian sections inside `graph.bin` and a
//! `manifest.json` that carries one digest per section. That makes an edit to
//! a single byte observable: the artifact still parses, and only the digest
//! recorded over the changed section can reveal it.

use std::fs;
use std::path::Path;

use qualia_jepa::prior::{CouplingPrior, PriorError, GRAPH_FILE, MANIFEST_FILE};
use sha2::{Digest, Sha256};

/// Three types, four edges, in-strength `4`, `8` and `2`.
fn graph() -> (u32, Vec<u64>, Vec<u32>, Vec<u32>) {
    (3, vec![0, 1, 3, 4], vec![1, 1, 2, 0], vec![3, 5, 2, 4])
}

fn section_u64(values: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 8);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn section_u32(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Writes a faithful artifact and returns the `graph.bin` bytes.
fn write_artifact(dir: &Path) -> Vec<u8> {
    let (type_count, rowptr, cols, weights) = graph();
    let rowptr_bytes = section_u64(&rowptr);
    let cols_bytes = section_u32(&cols);
    let weights_bytes = section_u32(&weights);

    let mut graph_bin = Vec::new();
    graph_bin.extend_from_slice(&rowptr_bytes);
    graph_bin.extend_from_slice(&cols_bytes);
    graph_bin.extend_from_slice(&weights_bytes);

    let manifest = serde_json::json!({
        "schema": "qualia.connectome-prior.v1",
        "type_count": type_count,
        "edge_count": cols.len(),
        "source_sha256": sha256_hex(&graph_bin),
        "rowptr_sha256": sha256_hex(&rowptr_bytes),
        "cols_sha256": sha256_hex(&cols_bytes),
        "weights_sha256": sha256_hex(&weights_bytes),
        "created_at_ms": 1_700_000_000_000u64,
        "attribution": {
            "dataset": "male-cns:v1.0",
            "licence": "CC-BY-4.0",
            "url": "https://male-cns.janelia.org",
            "citation": "Berg et al. 2026, Cell",
        },
    });

    fs::write(dir.join(GRAPH_FILE), &graph_bin).expect("graph.bin is writable");
    fs::write(
        dir.join(MANIFEST_FILE),
        format!("{manifest}\n").into_bytes(),
    )
    .expect("manifest.json is writable");
    graph_bin
}

#[test]
fn rejects_tampered_artifact() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let graph_bin = write_artifact(dir.path());
    CouplingPrior::load(dir.path()).expect("an untouched artifact loads");

    let rowptr_len = 4 * 8;
    let cols_len = 4 * 4;
    let sections = [
        ("rowptr", 0),
        ("cols", rowptr_len),
        ("weights", rowptr_len + cols_len),
    ];

    for (section, offset) in sections {
        let mut edited = graph_bin.clone();
        edited[offset] ^= 0x01;
        fs::write(dir.path().join(GRAPH_FILE), &edited).expect("graph.bin is writable");

        match CouplingPrior::load(dir.path()) {
            Err(PriorError::Digest { section: named }) => assert_eq!(named, section),
            other => panic!("an edited {section} section loaded: {other:?}"),
        }
    }
}

#[test]
fn rejects_truncated_artifact() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let graph_bin = write_artifact(dir.path());
    fs::write(dir.path().join(GRAPH_FILE), &graph_bin[..graph_bin.len() - 1])
        .expect("graph.bin is writable");

    match CouplingPrior::load(dir.path()) {
        Err(PriorError::Length { expected, actual }) => {
            assert_eq!(expected, graph_bin.len() as u64);
            assert_eq!(actual, graph_bin.len() as u64 - 1);
        }
        other => panic!("a truncated artifact loaded: {other:?}"),
    }
}

#[test]
fn loads_a_verified_artifact() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_artifact(dir.path());

    let prior = CouplingPrior::load(dir.path()).expect("a faithful artifact loads");
    assert_eq!(prior.type_count, 3);
    assert_eq!(prior.rowptr, vec![0, 1, 3, 4]);
    assert_eq!(prior.cols, vec![1, 1, 2, 0]);
    assert_eq!(prior.weights, vec![3, 5, 2, 4]);
}

#[test]
fn couple_scales_slots_by_normalised_in_strength() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_artifact(dir.path());
    let prior = CouplingPrior::load(dir.path()).expect("a faithful artifact loads");

    let mut belief = vec![10.0f32, 20.0, 40.0];
    let applied = prior.couple(&mut belief, &[(0, 0), (1, 1), (2, 2)]);

    // In-strength 4 / 8 / 2 against a peak of 8: factors 0.5, 1.0, 0.25.
    assert_eq!(belief, vec![5.0, 20.0, 10.0]);
    assert_eq!(applied, 1.75);
}

#[test]
fn couple_is_a_no_op_without_mapped_slots() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_artifact(dir.path());
    let prior = CouplingPrior::load(dir.path()).expect("a faithful artifact loads");

    let mut belief = vec![1.0f32, 2.0, 3.0];
    assert_eq!(prior.couple(&mut belief, &[]), 0.0);
    assert_eq!(belief, vec![1.0, 2.0, 3.0]);

    // A slot outside the belief or a type outside the graph is ignored, never
    // a panic.
    assert_eq!(prior.couple(&mut belief, &[(7, 0), (1, 9)]), 0.0);
    assert_eq!(belief, vec![1.0, 2.0, 3.0]);
}
