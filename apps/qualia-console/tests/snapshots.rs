//! The named states: the committed fixture, plus one fresh region the test
//! creates, plus the default staggered arrangement.
//!
//! `docs/frontend-lessons.md` (source 3) is the reason this file exists before
//! the views grew: image snapshots of named degraded states, driven from a
//! committed fixture, so a UI regression fails a test instead of needing eyes,
//! and the empty and error cases are written first because they are the ones
//! that regress.
//!
//! Assertions are accessible labels, not pixels alone. The image snapshots are
//! written beside them through `egui_kittest`'s own snapshot mechanism; each
//! state pins its own panel, and `default_arrangement` pins the cascade of all
//! five at once.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};

use egui_kittest::{kittest::Queryable, Harness};
use qualia_console::views::evidence::EvidenceView;
use qualia_console::views::world::WorldView;
use qualia_console::{fixture, ConsoleState, Sample, View, WindowSet};
use qualia_console::views::brain::BrainView;
use qualia_shm::ShmRegion;
use qualia_types::{FlySimPayload, LidarScanSnapshot};
use support::{healthy, ms_after_promotion};

const VIEWPORT: [f32; 2] = [1280.0, 820.0];

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

/// Unique per run, so a region left behind by a killed run is never attached.
fn region_name(tag: &str) -> String {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    format!("/qualia_console_snap_{}_{}_{}", std::process::id(), tag, index)
}

fn state(view: View, sample: Sample) -> ConsoleState {
    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(view);
    state
}

fn harness(state: ConsoleState) -> Harness<'static, ConsoleState> {
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(VIEWPORT[0], VIEWPORT[1]))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    // Two frames: a floating window's size settles on the frame after it first
    // appears, and the snapshot must be the settled layout.
    harness.run();
    harness.run();
    harness
}

#[test]
fn mission_healthy() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut harness = harness(state(View::Mission, healthy(&fixture, now)));

    harness.get_by_label("open missions");
    harness.get_by_label("1");
    harness.get_by_label("generation");
    harness.get_by_label("12");
    harness.get_by_label("session");
    harness.get_by_label("sess-2026-09-11-explore-frontier");
    harness.get_by_label("drift mahalanobis");
    harness.get_by_label("4.200");
    harness.get_by_label("drift samples");
    harness.get_by_label("512");
    // The panel names the agent it is showing and the connection it has.
    harness.get_by_label("agent");
    harness.get_by_label("http://127.0.0.1:8080");
    harness.get_by_label("connected");

    harness.snapshot("mission_healthy");
}

#[test]
fn mission_degraded() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut harness = harness(state(
        View::Mission,
        Sample::degraded(&fixture, "tcp connect failed", now),
    ));

    // The degraded state still renders the committed braid: no empty window,
    // and the reason sits in the banner at the top of the panel.
    harness.get_by_label("mission degraded: tcp connect failed");
    harness.get_by_label("rendering committed fixture braid-state.json");
    harness.get_by_label("open missions");
    harness.get_by_label("1");
    harness.get_by_label("degraded");

    harness.snapshot("mission_degraded");
}

#[test]
fn belief_stale() {
    let fixture = fixture();
    // Thirty seconds after the fixture's promotion: every layer slot is older
    // than the 2 s staleness line, so every row is the named "stale" state.
    let now = ms_after_promotion(&fixture, 30_000);
    let mut harness = harness(state(View::Belief, healthy(&fixture, now)));

    harness.get_by_label("shm region");
    harness.get_by_label("qualia");
    assert_eq!(
        harness.query_all_by_label("stale").count(),
        qualia_types::NUM_LAYERS,
        "every layer should report the named stale state"
    );
    assert_eq!(
        harness.query_all_by_label("live").count(),
        0,
        "no layer should still report live"
    );

    harness.snapshot("belief_stale");
}

#[test]
fn evidence_empty() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut sample = healthy(&fixture, now);
    sample.evidence = EvidenceView {
        root: sample.evidence.root.clone(),
        ..EvidenceView::default()
    };
    let mut harness = harness(state(View::Evidence, sample));

    harness.get_by_label("no sealed segments");
    harness.get_by_label("ledger empty");
    harness.get_by_label("no quarantined partials");

    harness.snapshot("evidence_empty");
}

#[test]
fn world_fresh() {
    // A region the console attaches to but nobody has written still renders
    // its three absent arms: no fix, not published, not published.
    let name = region_name("world");
    let region = ShmRegion::create(&name).expect("create region");
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut sample = healthy(&fixture, now);
    sample.world = WorldView {
        region: Some(name),
        ..WorldView::sample(&region)
    };

    let mut harness = harness(state(View::World, sample));

    harness.get_by_label("pose: no fix");
    harness.get_by_label("map: not published");
    harness.get_by_label("voxels: not published");
    assert_eq!(
        harness.query_all_by_label("no world data").count(),
        0,
        "an attached region is not an absent source"
    );

    harness.snapshot("world_fresh");
}

#[test]
fn default_arrangement() {
    // The console's opening picture: every panel open at its own grid place,
    // the whole stack readable at once.
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let state = ConsoleState::from_sample(healthy(&fixture, now), "http://127.0.0.1:8080");
    assert_eq!(state.windows, WindowSet::default(), "all six show by default");
    let mut harness = harness(state);

    // One accessible label from each of the six panels.
    harness.get_by_label("open missions");
    harness.get_by_label("compression");
    harness.get_by_label("pose confidence");
    harness.get_by_label("evidence root");
    harness.get_by_label("newest frame");
    harness.get_by_label("belief matrices");
    harness.snapshot("default_arrangement");
}

#[test]
fn brain_fresh() {
    // A region with one published fly-model state and one lidar scan: the graph
    // fires, the cloud draws, and the panels below carry the matrices.
    let name = region_name("brain");
    let region = ShmRegion::create(&name).expect("create region");
    let mut payload = FlySimPayload {
        type_count: 5,
        sim_step: 7,
        producer_epoch: 2,
        timestamp_ns: 1_000,
        ..FlySimPayload::default()
    };
    payload.state[..5].copy_from_slice(&[0.90, 0.40, 0.72, 0.20, 0.55]);
    region.fly_sim().publish(payload).expect("publish fly sim");
    let scan = LidarScanSnapshot {
        scan_start_ns: 1_000,
        scan_end_ns: 2_000,
        point_count: 3,
        ..LidarScanSnapshot::default()
    };
    region.lidar_scan_mut().publish(&scan).expect("publish scan");

    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut sample = healthy(&fixture, now);
    sample.brain = BrainView {
        region: Some(name),
        ..BrainView::sample(&region)
    };
    sample.brain.record_braid(&fixture.braid);
    let mut harness = harness(state(View::Brain, sample));

    harness.get_by_label("belief matrices");
    harness.get_by_label("weight matrix");
    harness.get_by_label("belief vector");
    harness.get_by_label("time axis");
    harness.get_by_label("connectome");
    harness.get_by_label("point cloud");
    harness.get_by_label("fly sim");
    harness.get_by_label("prior graph");
    harness.get_by_label("braid markers");
    harness.get_by_label("PromotionAccepted g12");
    assert!(
        harness.query_all_by_label("fly sim: not published (QUALIA_FLY_MODE=sim needed)").count() == 0,
        "a published fly state is not the absent arm"
    );

    harness.snapshot("brain_fresh");
}
