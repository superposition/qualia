//! The Brain view's frame cost, measured on a graph far larger than the
//! committed reference prior.
//!
//! `docs/frontend-lessons.md` keeps a wall-clock frame budget separate from the
//! deterministic snapshot set, because a timing assertion is
//! environment-dependent. That is what this file is: one bounded measurement (30
//! frames, never a benchmark loop) over a synthetic 2048-type / 8192-edge prior,
//! which is where the scene's caps actually bite. It reports the average so the
//! handoff can quote a number, and the bound is deliberately loose — it fails
//! only if the scene stops being usable at all.
//!
//! The renderer reads the region and never writes it, the poller runs on its own
//! thread, and every layer is capped, so this cost cannot pace the runners; the
//! measurement is here to show the panel stays interactive.

use std::sync::Arc;
use std::time::Instant;

use egui_kittest::Harness;
use qualia_console::views::brain::layout::{Layout, LayoutPrior};
use qualia_console::views::brain::matrices::{MatrixReading, MATRIX_CELLS};
use qualia_console::views::brain::prior::{PriorGraph, PriorManifest, PriorSource, PRIOR_SCHEMA};
use qualia_console::views::brain::{BrainAssets, BrainView, CloudReading, FiringReading};
use qualia_console::{fixture, ConsoleState, Sample, View, WindowSet};

const TYPE_COUNT: usize = 2048;
const FANOUT: usize = 4;
const FRAMES: usize = 30;
/// A frame this slow would mean the scene stopped being usable.
const BUDGET_MS: f64 = 120.0;

fn synthetic_prior() -> (PriorGraph, Layout) {
    let mut rowptr = vec![0_u64];
    let mut cols = Vec::with_capacity(TYPE_COUNT * FANOUT);
    let mut weights = Vec::with_capacity(TYPE_COUNT * FANOUT);
    for source in 0..TYPE_COUNT {
        for edge in 0..FANOUT {
            cols.push(((source * 7 + edge * 131 + 3) % TYPE_COUNT) as u32);
            weights.push((edge as u32) + 1);
        }
        rowptr.push(cols.len() as u64);
    }
    let edge_count = cols.len();
    let manifest = PriorManifest {
        schema: PRIOR_SCHEMA.to_owned(),
        type_count: TYPE_COUNT as u64,
        edge_count: edge_count as u64,
        source_sha256: "0".repeat(64),
        rowptr_sha256: String::new(),
        cols_sha256: String::new(),
        weights_sha256: String::new(),
    };
    let prior = PriorGraph {
        manifest,
        rowptr,
        cols,
        weights,
        source: PriorSource::Committed,
    };
    let positions = (0..TYPE_COUNT)
        .map(|index| {
            let angle = index as f32 * 0.37;
            [
                angle.cos() * (0.2 + 0.8 * (index % 17) as f32 / 17.0),
                (index % 13) as f32 / 13.0 - 0.5,
                angle.sin() * (0.2 + 0.8 * (index % 19) as f32 / 19.0),
            ]
        })
        .collect();
    let layout = Layout {
        algorithm: "synthetic".to_owned(),
        seed: 0,
        prior: LayoutPrior {
            source_sha256: "0".repeat(64),
            graph_sha256: String::new(),
            type_count: TYPE_COUNT as u64,
            edge_count: edge_count as u64,
        },
        positions,
    };
    (prior, layout)
}

fn brain_view() -> BrainView {
    BrainView {
        region: Some("synthetic".to_owned()),
        firing: Some(FiringReading {
            sim_id: "qualia.fly-circuit.rate.v1".to_owned(),
            type_count: TYPE_COUNT,
            sim_step: 1_000,
            producer_epoch: 1,
            timestamp_ns: 1,
            flags: 0,
            rates: (0..TYPE_COUNT)
                .map(|index| ((index % 23) as f32 / 23.0).sin().abs())
                .collect(),
        }),
        layers: (0..qualia_types::NUM_LAYERS)
            .map(|layer| MatrixReading {
                layer: layer as u8,
                weight: vec![0.5; MATRIX_CELLS],
                belief: vec![0.25; MATRIX_CELLS],
                vfe: 0.1,
                residual_norm: 0.2,
                timestamp_ns: 1,
            })
            .collect(),
        cloud: CloudReading {
            points: (0..720)
                .map(|index| {
                    let angle = index as f32 * 0.01;
                    [angle.cos(), 0.0, angle.sin()]
                })
                .collect(),
            voxels: (0..3000)
                .map(|index| [(index % 32) as f32 * 0.1 - 1.6, 0.0, 0.2])
                .collect(),
            voxel_total: 12_000,
            ..CloudReading::default()
        },
        ..BrainView::default()
    }
}

#[test]
fn brain_frame_stays_usable_on_a_large_prior() {
    let (prior, layout) = synthetic_prior();
    let fixture = fixture();
    let mut sample = Sample::degraded(&fixture, "frame cost only", fixture.braid.last_promotion_ns);
    sample.brain = brain_view();
    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.brain_assets = Arc::new(BrainAssets {
        prior: Some(prior),
        layout: Some(layout),
        error: None,
    });
    state.windows = WindowSet::only(View::Brain);

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();

    let start = Instant::now();
    for _ in 0..FRAMES {
        harness.run();
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0 / FRAMES as f64;
    println!(
        "brain frame cost: {elapsed_ms:.2} ms/frame at {TYPE_COUNT} types and {} edges (drawn {} nodes / {} edges)",
        TYPE_COUNT * FANOUT,
        harness.state().brain.counts.nodes_drawn,
        harness.state().brain.counts.edges_drawn,
    );
    assert!(
        elapsed_ms < BUDGET_MS,
        "the scene averaged {elapsed_ms:.2} ms/frame, over the {BUDGET_MS} ms budget"
    );
}
