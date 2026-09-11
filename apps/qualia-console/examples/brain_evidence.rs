//! Capture the Brain view's inputs through the real chain, for the figures.
//!
//! This example is evidence, not a fixture: it loads the committed prior, steps
//! `crates/fly-circuit`'s rate model over it with a synthetic drive (the runtime
//! drives no type yet, so a live stack publishes an all-zero, flat rate vector),
//! publishes the model state and evolving belief slots into a fresh shared
//! region, samples the region through `BrainView::sample` — the same read path
//! the console uses — and writes the result to
//! `docs/figures/fly-brain/firing-sample.json`. The figure script and the
//! console therefore draw the same numbers.
//!
//! The layer weights are **synthetic too**, and disclosed as such: the runtime
//! evolves no weight matrix yet, so the example publishes a deterministic
//! pattern into the real slot field the panel and the figures read
//! (`write_weight_tile` writes `layer_slot(layer).weights`). The pattern is
//! stated in `decimated_weight` and checked before the sample is written: a
//! recording whose weight matrix is flat is a refused output, not a blank
//! figure. Wiring a real weight update is the same follow-up as the fly drive.
//!
//! Run from the workspace root:
//!
//! ```console
//! $ cargo run -p qualia-console --example brain_evidence
//! ```

use std::path::PathBuf;

use qualia_console::views::brain::matrices::{MATRIX_DIM, STATE_STRIDE};
use qualia_console::views::brain::{BrainView, LayerPoint, PriorGraph};
use qualia_types::{BeliefSlot, FlySimPayload, LidarScanSnapshot, JEPA_FLAG_OUTPUT_FINITE, JEPA_FLAG_VALID, NUM_LAYERS, STATE_DIM};
use qualia_shm::{LayerWriter, ShmRegion};

/// Steps recorded into the sample.
const STEPS: usize = 64;
/// Integration step. The runtime's publisher uses 1 ms; this recording uses
/// 20 ms so a 64-step window reaches a settled firing pattern instead of the
/// first few percent of one, and the figure is a picture of the model's
/// behaviour rather than of its first millisecond.
const STEP_SECONDS: f32 = 0.02;

/// The synthetic generative weight the example publishes for one decimated
/// cell at one step.
///
/// The runtime evolves no weight matrix yet, so this stands in exactly as the
/// drive above does. It has the belief's own shape at that cell — the belief
/// mean is `0.5·sin(cell·0.05 + step·0.2 + layer)`, and this is the same phase
/// scaled by a per-row envelope and a per-frame growth factor — so the two
/// matrices the panels draw are related, deterministic (no clock, no RNG) and
/// sign-changing, and `growth` rises 0.20 → 0.50 across the recording so the
/// weight scale moves on the time axis. Four constants, no hidden state.
fn decimated_weight(layer: usize, step: usize, row: usize, column: usize) -> f32 {
    let cell = (row * MATRIX_DIM + column) as f32;
    let phase = cell * 0.05 + step as f32 * 0.2 + layer as f32;
    let envelope = 0.55 + 0.45 * (row as f32 * 0.41 + layer as f32).cos();
    let growth = 0.20 + 0.30 * (step as f32 / (STEPS - 1) as f32);
    growth * phase.sin() * envelope
}

fn prior_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/brain/prior")
}

fn output_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/figures/fly-brain/firing-sample.json")
}

fn main() -> Result<(), String> {
    let prior = PriorGraph::load_dir(&prior_dir())?;
    let layout = qualia_console::views::brain::Layout::committed()?;
    if !layout.matches(&prior) {
        return Err("the committed layout does not match the committed prior".to_owned());
    }

    let mut sim = qualia_fly_circuit::CircuitSim::load(&prior_dir())
        .map_err(|error| format!("fly model: {error}"))?;
    let type_count = sim.type_count();
    if type_count != prior.type_count() {
        return Err(format!(
            "the model carries {type_count} types, the prior {}",
            prior.type_count()
        ));
    }

    let fixture = qualia_console::fixture();
    let base_ns = fixture.braid.last_promotion_ns.saturating_sub(STEPS as u64 * 1_000_000);
    let region_name = format!("/qualia_brain_evidence_{}", std::process::id());
    let region = ShmRegion::create(&region_name).map_err(|error| error.to_string())?;

    let mut scan = LidarScanSnapshot {
        scan_start_ns: base_ns,
        scan_end_ns: base_ns + STEPS as u64 * 1_000_000,
        point_count: 64,
        ..LidarScanSnapshot::default()
    };
    for (index, point) in scan.points[..64].iter_mut().enumerate() {
        point.angle_rad = index as f32 * 0.098;
        point.distance_m = 0.4 + 0.6 * ((index as f32) * 0.37).sin().abs();
        point.intensity = 128;
    }
    region
        .lidar_scan_mut()
        .publish(&scan)
        .map_err(|error| error.to_string())?;

    let mut history: Vec<LayerPoint> = Vec::new();

    for step in 0..STEPS {
        let timestamp_ns = base_ns + step as u64 * 1_000_000;
        let mut drive = vec![0.0_f32; type_count];
        // A sustained drive into type 0 so the chain reaches its steady state
        // rather than decaying to zero, and a slower alternating drive into type
        // 2 so the settled pattern still moves.
        drive[0] = 1.0;
        if type_count > 2 {
            drive[2] = if (step / 8) % 2 == 0 { 0.6 } else { 0.0 };
        }
        let rates = sim.step(&drive, STEP_SECONDS);

        let mut payload = FlySimPayload {
            type_count: type_count as u32,
            flags: JEPA_FLAG_VALID | JEPA_FLAG_OUTPUT_FINITE,
            producer_epoch: 1,
            sim_step: step as u64 + 1,
            timestamp_ns,
            dt: STEP_SECONDS,
            ..FlySimPayload::default()
        };
        payload.state[..type_count].copy_from_slice(&rates);
        region
            .fly_sim()
            .publish(payload)
            .map_err(|error| error.to_string())?;

        for layer in 0..NUM_LAYERS {
            let writer = LayerWriter::new(region.layer_slot(layer));
            let slot: &mut BeliefSlot = writer.back_buffer();
            for index in 0..STATE_DIM {
                let phase = index as f32 * 0.05 + step as f32 * 0.2 + layer as f32;
                slot.mean[index] = 0.5 * phase.sin();
                slot.residual[index] = 0.05 * (1.0 + (phase * 0.5).sin());
            }
            slot.vfe = 0.10 + 0.05 * (step as f32 * 0.3).sin();
            slot.layer = layer as u8;
            slot.compression = (1 + layer % 4) as u8;
            slot.timestamp_ns = timestamp_ns;
            slot.cycle_us = 900;
            writer.publish();

            // The weight half of #173 step 3, through the same slot the panel
            // reads. One element per decimated cell (stride `STATE_STRIDE`) is
            // what `MatrixReading::sample` copies, so this writes exactly the
            // samples the panel and the figures see; a full 1024×1024 write per
            // frame would be a megabyte per layer, and a real layer runner does
            // that, not this recording.
            for row in 0..MATRIX_DIM {
                for column in 0..MATRIX_DIM {
                    region.write_weight_tile(
                        layer,
                        row * STATE_STRIDE,
                        column * STATE_STRIDE,
                        1,
                        &[decimated_weight(layer, step, row, column)],
                    );
                }
            }
        }

        let sample = BrainView::sample(&region);
        for reading in &sample.layers {
            history.push(LayerPoint::from_reading(reading));
        }
    }

    let mut sample = BrainView::sample(&region);
    sample.record_braid(&fixture.braid);
    sample.observe_coupling_scale(base_ns);

    // #173 step 3 reads the recorded weight matrices as evidence, so a flat one
    // is a refused output rather than a blank figure: count every distinct
    // decimated value the panels will see and stop if the matrix (or the
    // weight scale the history records) has only one.
    let mut distinct: Vec<f32> = sample
        .layers
        .iter()
        .flat_map(|layer| layer.weight.iter().copied())
        .collect();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();
    let weight_peak = sample
        .layers
        .iter()
        .map(|layer| {
            layer
                .weight
                .iter()
                .fold(0.0_f32, |peak, value| peak.max(value.abs()))
        })
        .fold(0.0_f32, f32::max);
    let mut scales: Vec<f32> = history
        .iter()
        .map(|point| point.weight_scale)
        .collect();
    scales.sort_by(f32::total_cmp);
    scales.dedup();
    if weight_peak <= f32::EPSILON || distinct.len() < 2 || scales.len() < 2 {
        return Err(format!(
            "the recorded weight matrix is flat: peak {weight_peak}, \
             {} distinct decimated value(s), {} distinct weight scale(s)",
            distinct.len(),
            scales.len()
        ));
    }

    let firing = sample.firing.as_ref().ok_or("the fly slot is empty")?;
    let markers: Vec<serde_json::Value> = sample
        .markers
        .iter()
        .map(|marker| {
            serde_json::json!({
                "kind": match marker.kind {
                    qualia_console::views::brain::MarkerKind::CouplingScale => "CouplingScale",
                    qualia_console::views::brain::MarkerKind::PromotionAccepted => "PromotionAccepted",
                    qualia_console::views::brain::MarkerKind::PartialsQuarantined => {
                        "PartialsQuarantined"
                    }
                },
                "timestamp_ns": marker.timestamp_ns,
                "label": marker.label,
            })
        })
        .collect();

    let layers: Vec<serde_json::Value> = sample
        .layers
        .iter()
        .map(|layer| {
            serde_json::json!({
                "layer": layer.layer,
                "weight": layer.weight,
                "belief": layer.belief,
                "vfe": layer.vfe,
                "residual_norm": layer.residual_norm,
                "timestamp_ns": layer.timestamp_ns,
            })
        })
        .collect();
    let history_json: Vec<serde_json::Value> = history
        .iter()
        .map(|point| {
            serde_json::json!({
                "layer": point.layer,
                "timestamp_ns": point.timestamp_ns,
                "residual_norm": point.residual_norm,
                "weight_scale": point.weight_scale,
            })
        })
        .collect();

    let prior_type_count = prior.type_count();
    let prior_edge_count = prior.edge_count();
    let document = serde_json::json!({
        "schema": "qualia.brain-firing-sample.v1",
        "prior": {
            "type_count": prior_type_count,
            "edge_count": prior_edge_count,
            "source_sha256": prior.manifest.source_sha256,
        },
        "sim_step": firing.sim_step,
        "producer_epoch": firing.producer_epoch,
        "timestamp_ns": firing.timestamp_ns,
        "rates": firing.rates,
        "layers": layers,
        "history": history_json,
        "markers": markers,
        "counts": {
            "nodes": prior.type_count(),
            "edges": prior.edge_count(),
            "lidar_points": sample.cloud.points.len(),
        },
    });

    let path = output_path();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&document).map_err(|error| error.to_string())? + "\n",
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;

    println!(
        "brain evidence: {} types, {} edges, peak rate {:.4}, weight peak {:.4} ({} decimated values, {} weight scales), {} lidar points, {} history frames, {} markers -> {}",
        prior.type_count(),
        prior.edge_count(),
        firing.peak(),
        weight_peak,
        distinct.len(),
        scales.len(),
        sample.cloud.points.len(),
        history.len(),
        sample.markers.len(),
        path.display()
    );
    Ok(())
}
