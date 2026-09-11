//! The committed firing recording's weight half.
//!
//! `docs/figures/fly-brain/firing-sample.json` is the recording every fly-brain
//! figure and the turntable read, and #173's step 3 reads its matrices as the
//! evidence that a weight change is *visible*. The example that writes it
//! refuses a flat weight matrix, and this test pins the same observable
//! property on the committed artifact itself, so a reader cannot be handed a
//! recording whose weight panel is one colour.
//!
//! The figure's `heat` normalises each matrix by its own `max |value|` (with a
//! `1e-9` floor), so a recorded matrix with more than one distinct value and a
//! non-zero peak cannot render as a single flat colour. The assertions below are
//! exactly that input contract: they are about the committed sample, not about
//! the example's code.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/figures/fly-brain/firing-sample.json")
}

fn matrix(layer: &serde_json::Value) -> Vec<f64> {
    layer["weight"]
        .as_array()
        .expect("a decimated weight matrix")
        .iter()
        .map(|value| value.as_f64().expect("a float"))
        .collect()
}

fn peak(values: &[f64]) -> f64 {
    values.iter().fold(0.0_f64, |peak, value| peak.max(value.abs()))
}

fn distinct(values: &[f64]) -> BTreeSet<u64> {
    values.iter().map(|value| value.to_bits()).collect()
}

#[test]
fn the_recorded_weight_matrix_is_not_flat() {
    let path = sample_path();
    let document: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    )
    .expect("parse the committed firing sample");

    let layers = document["layers"].as_array().expect("a layers array");
    assert!(!layers.is_empty(), "the sample records no layers");

    // The matrix the figure squares into its weight heatmap is layer 0's, or the
    // layer the figure falls back to when there is none.
    let first = layers
        .iter()
        .find(|layer| layer["layer"] == 0)
        .unwrap_or(&layers[0]);
    let first_weight = matrix(first);
    assert!(
        distinct(&first_weight).len() > 1,
        "the weight heatmap's input is flat: {} distinct value(s)",
        distinct(&first_weight).len()
    );
    assert!(
        peak(&first_weight) > 1e-6,
        "the weight heatmap's input is all zero (peak {})",
        peak(&first_weight)
    );

    // Every recorded layer carries the same property, not only the one on
    // screen.
    for layer in layers {
        let values = matrix(layer);
        assert!(
            distinct(&values).len() > 1,
            "layer {} records a flat weight matrix",
            layer["layer"]
        );
    }

    // The time axis's weight scale moves across the recording, so the recorded
    // change is visible over the history and not only in the final frame.
    let scales: Vec<f64> = document["history"]
        .as_array()
        .expect("a history array")
        .iter()
        .filter(|point| point["layer"] == first["layer"])
        .map(|point| point["weight_scale"].as_f64().expect("a weight scale"))
        .collect();
    assert!(!scales.is_empty(), "the history records no frames");
    assert!(
        distinct(&scales).len() > 1,
        "the recorded weight scale is pinned at one value"
    );
    assert!(
        peak(&scales) > 1e-6,
        "the recorded weight scale sits at the epsilon guard"
    );
}
