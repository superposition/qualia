//! Behaviour of the invented rate model, over a prior artifact the builder wrote.
//!
//! The whole surface is behind the `sim` feature, so is this target: the
//! manifest declares `required-features = ["sim"]`, and a build that does not ask
//! for the feature compiles neither the model nor these tests.

use std::path::Path;

use qualia_connectome_prior::{Attribution, TypeGraph, write_prior};
use qualia_fly_circuit::CircuitSim;
use tempfile::TempDir;

/// A three-type cycle: `0 -> 1` weight 2, `1 -> 2` weight 1, `2 -> 0` weight 3.
fn fixture() -> (TempDir, TypeGraph) {
    let graph = TypeGraph {
        types: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        rowptr: vec![0, 1, 2, 3],
        cols: vec![1, 2, 0],
        weights: vec![2, 1, 3],
    };
    let directory = TempDir::new().expect("temporary directory");
    write_prior(directory.path(), &graph, &Attribution::male_cns()).expect("prior writes");
    (directory, graph)
}

fn load(directory: &Path) -> CircuitSim {
    CircuitSim::load(directory).expect("prior loads")
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

#[test]
fn step_is_deterministic() {
    let (directory, graph) = fixture();
    let mut first = load(directory.path());
    let mut second = load(directory.path());
    let drive = [0.75f32, 0.0, -0.25];

    for _ in 0..3 {
        let one = first.step(&drive, 0.1);
        let other = second.step(&drive, 0.1);
        assert_eq!(one.len(), graph.types.len());
        assert_eq!(bits(&one), bits(&other));
    }
}

#[test]
fn an_unforced_graph_stays_quiet() {
    let (directory, graph) = fixture();
    let mut sim = load(directory.path());
    let quiet = vec![0.0f32; graph.types.len()];

    for _ in 0..4 {
        assert_eq!(sim.step(&quiet, 0.25), quiet);
    }
}

#[test]
fn a_step_couples_along_the_prior_edges() {
    let (directory, _) = fixture();
    let mut sim = load(directory.path());

    let first = sim.step(&[1.0, 0.0, 0.0], 0.05);
    assert!(first[0] > 0.0, "the drive reaches its own type");
    assert_eq!(first[1], 0.0, "coupling reads the state before the step");

    let second = sim.step(&[0.0, 0.0, 0.0], 0.05);
    assert!(second[1] > 0.0, "the 0 -> 1 edge carries state forward");
    assert!(second[0] < first[0], "the undriven type leaks toward zero");
    assert_eq!(second[2], 0.0, "the 1 -> 2 edge has had no source state yet");
}

#[test]
fn free_response_stays_bounded() {
    let (directory, _) = fixture();
    let mut sim = load(directory.path());

    sim.step(&[1.0, 0.0, 0.0], 0.05);
    let mut last = Vec::new();
    for _ in 0..200 {
        last = sim.step(&[0.0, 0.0, 0.0], 0.05);
    }
    assert!(last.iter().all(|value| value.is_finite()));
    assert!(last.iter().all(|value| value.abs() < 1.0));
}

#[test]
#[should_panic]
fn step_rejects_an_input_of_the_wrong_length() {
    let (directory, _) = fixture();
    let mut sim = load(directory.path());
    sim.step(&[1.0, 0.0], 0.05);
}

#[test]
fn load_reports_a_directory_without_a_manifest() {
    let directory = TempDir::new().expect("temporary directory");
    assert!(CircuitSim::load(directory.path()).is_err());
}

#[test]
fn load_reports_a_graph_that_disagrees_with_the_manifest() {
    let (directory, _) = fixture();
    let graph = directory.path().join("graph.bin");
    let bytes = std::fs::read(&graph).expect("graph reads");
    std::fs::write(&graph, &bytes[..bytes.len() - 4]).expect("truncated graph writes");

    assert!(CircuitSim::load(directory.path()).is_err());
}
