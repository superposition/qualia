//! The named states: the committed fixture, plus one fresh region the test
//! creates, plus the default grid arrangement.
//!
//! `docs/frontend-lessons.md` (source 3) is the reason this file exists before
//! the views grew: image snapshots of named degraded states, driven from a
//! committed fixture, so a UI regression fails a test instead of needing eyes,
//! and the empty and error cases are written first because they are the ones
//! that regress.
//!
//! Assertions are accessible labels, not pixels alone. The image snapshots are
//! written beside them through `egui_kittest`'s own snapshot mechanism; each
//! state pins its own panel, and `default_arrangement` pins the grid of all six
//! at once.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};

use egui_kittest::{kittest::Queryable, Harness, OsThreshold, SnapshotOptions};
use qualia_console::views::evidence::EvidenceView;
use qualia_console::views::world::WorldView;
use qualia_console::{fixture, ConsoleState, Sample, View, WindowSet};
use qualia_console::views::brain::BrainView;
use qualia_console::views::brain::matrices::{MATRIX_DIM, STATE_STRIDE};
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

/// How many differing pixels a non-dev-host renderer may produce, on top of the
/// crate's own per-pixel threshold.
///
/// The committed PNGs are rendered by this host's wgpu backend, and
/// `egui_kittest`'s default per-pixel threshold (0.6) already absorbs one-LSB
/// channel noise. The Jetson Orin's wgpu backend still lands a handful of
/// anti-aliased edge pixels on the other side of that threshold — `Diff: 6` was
/// observed for `brain_fresh` and `default_arrangement` against these images —
/// which a byte-exact compare fails. So the rule this file's snapshots use is:
/// every pixel must match within the crate's threshold, and the number that do
/// not is capped — at **zero** on this host (the one that renders the committed
/// images, so the compare stays exact here) and at a small bounded allowance on
/// Linux, the board's platform. The allowance is a floor for rasterizer
/// differences, not a licence to drift: reverting the node-intensity source to
/// the rate vector moves hundreds of pixels (the correctness review measured
/// `Diff: 238`), far above it, and each snapshot's accessible-label assertions
/// pin its structure independently of the image.
const RENDERER_PIXEL_ALLOWANCE: usize = 32;

/// Image comparison policy for this file's snapshots; see
/// [`RENDERER_PIXEL_ALLOWANCE`].
fn snapshot_options() -> SnapshotOptions {
    SnapshotOptions::new()
        .failed_pixel_count_threshold(OsThreshold::new(0).linux(RENDERER_PIXEL_ALLOWANCE))
}

/// Compare a rendered frame against its committed snapshot under
/// [`snapshot_options`].
fn snapshot(harness: &mut Harness<'static, ConsoleState>, name: &str) {
    harness.snapshot_options(name, &snapshot_options());
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

    snapshot(&mut harness, "mission_healthy");
}

#[test]
fn mission_degraded() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut harness = harness(state(
        View::Mission,
        Sample::degraded(&fixture, "tcp connect failed", now),
    ));

    // Unavailable sources share a collapsed drawer, without fixture panels
    // crowding the working views. Details remain available on demand.
    harness.get_by_label("Issues (3)");
    assert!(harness.query_by_label("open missions").is_none());
    assert!(harness.query_by_label("tcp connect failed").is_none());
    snapshot(&mut harness, "mission_degraded");

    harness.get_by_label("Issues (3)").click();
    harness.run();
    harness.get_by_label("tcp connect failed");
    assert_eq!(harness.query_all_by_label("agent unreachable: tcp connect failed").count(), 1);
    harness.get_by_label("Show unavailable panels").click();
    harness.run();
    harness.get_by_label("mission degraded").click();
    harness.run();
    harness.get_by_label("mission degraded: tcp connect failed");
    harness.get_by_label("rendering committed fixture braid-state.json");
    harness.get_by_label("open missions");
    harness.get_by_label("1");
    harness.get_by_label("degraded");

    // Recovery returns the live view without changing the operator's window
    // selection, even if unavailable panels were subsequently hidden.
    harness.get_by_label("Show unavailable panels").click();
    harness.run();
    harness.state_mut().apply(healthy(&fixture, now));
    harness.run();
    harness.get_by_label("connected");
    harness.get_by_label("open missions");
}

#[test]
fn failures_leave_usable_data_visible() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let mut sample = healthy(&fixture, now);
    sample.connection = qualia_console::Connection::Unreachable { reason: "agent offline".into() };
    sample.belief = qualia_console::views::belief::BeliefView::unattached(None, "region unavailable");
    sample.telemetry = qualia_console::views::telemetry::TelemetryView::unattached("region unavailable");
    // A failed directory scan must not hide the independently available ledger.
    sample.evidence.error = Some("directory unavailable".into());
    let mut harness = harness(ConsoleState::from_sample(sample, "http://127.0.0.1:8080"));
    assert!(harness.query_by_label("open missions").is_none());
    assert!(harness.query_by_label("compression").is_none());
    assert!(harness.query_by_label("no telemetry frames").is_none());
    harness.get_by_label("pose confidence");
    harness.get_by_label("evidence root");
    harness.get_by_label("evidence read error");
    assert!(harness.query_by_label("evidence read error: directory unavailable").is_none());
    snapshot(&mut harness, "mixed_availability");
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

    snapshot(&mut harness, "belief_stale");
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

    snapshot(&mut harness, "evidence_empty");
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

    snapshot(&mut harness, "world_fresh");
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
    snapshot(&mut harness, "default_arrangement");
}

#[test]
fn brain_fresh() {
    // A region with one published fly-model state, one written belief layer per
    // layer and one lidar scan: the graph's nodes light from the belief slots,
    // its edges pulse from the fly rate vector, the cloud draws, and the panels
    // below carry the matrices.
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
    // Node intensity reads `BeliefSlot.mean`: layer 0 bright, the rest dimmer,
    // each type reading its own layer's slot.
    for layer in 0..qualia_types::NUM_LAYERS {
        let writer = qualia_shm::LayerWriter::new(region.layer_slot(layer));
        let slot = writer.back_buffer();
        for index in 0..qualia_types::STATE_DIM {
            slot.mean[index] = if layer == 0 {
                0.85
            } else {
                0.20 + 0.06 * layer as f32
            };
        }
        slot.vfe = 0.08;
        slot.layer = layer as u8;
        slot.timestamp_ns = 1_500;
        writer.publish();

        // The weight half of the matrices panel, through the same slot field
        // the panel reads: one element per decimated cell. Publish it here so
        // the region's state is complete; the committed `brain-matrices` figure
        // and `tests/brain_sample.rs` are the visible weight evidence, because
        // the matrices section sits below this window's fold in the pinned
        // frame.
        for row in 0..MATRIX_DIM {
            for column in 0..MATRIX_DIM {
                let cell = row * MATRIX_DIM + column;
                let value = ((cell + layer * 7) % 13) as f32 / 13.0 - 0.5;
                region.write_weight_tile(
                    layer,
                    row * STATE_STRIDE,
                    column * STATE_STRIDE,
                    1,
                    &[value],
                );
            }
        }
    }
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
    // The weights published above must reach the panel's read path rather than
    // sit in an unused buffer; the matrices section is below this window's fold
    // in the pinned frame, so the image itself does not carry them.
    assert!(
        sample
            .brain
            .layers
            .iter()
            .any(|layer| layer.weight.iter().any(|value| value.abs() > 0.1)),
        "the published weight matrix did not reach the panel's read path"
    );
    let mut harness = harness(state(View::Brain, sample));

    harness.get_by_label("belief matrices");
    harness.get_by_label("weight matrix");
    harness.get_by_label("belief vector");
    harness.get_by_label("time axis");
    harness.get_by_label("connectome");
    harness.get_by_label("point cloud");
    harness.get_by_label("fly sim");
    harness.get_by_label("node intensity");
    harness.get_by_label("prior graph");
    harness.get_by_label("braid markers");
    harness.get_by_label("PromotionAccepted g12");
    assert!(
        harness.query_all_by_label("fly sim: not published (QUALIA_FLY_MODE=sim needed)").count() == 0,
        "a published fly state is not the absent arm"
    );

    snapshot(&mut harness, "brain_fresh");
}
