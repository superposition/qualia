//! The four named states, driven by the committed fixture.
//!
//! `docs/frontend-lessons.md` (source 3) is the reason this file exists before
//! the views grew: image snapshots of named degraded states, driven from a
//! committed fixture, so a UI regression fails a test instead of needing eyes,
//! and the empty and error cases are written first because they are the ones
//! that regress.
//!
//! Assertions are accessible labels, not pixels alone. The image snapshots are
//! written beside them through `egui_kittest`'s own snapshot mechanism.

mod support;

use egui_kittest::{kittest::Queryable, Harness};
use qualia_console::views::evidence::EvidenceView;
use qualia_console::{fixture, ConsoleState, Sample, View};
use support::{healthy, ms_after_promotion};

const VIEWPORT: [f32; 2] = [1280.0, 820.0];

fn state(view: View, sample: Sample) -> ConsoleState {
    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.view = view;
    state
}

fn harness(state: ConsoleState) -> Harness<'static, ConsoleState> {
    Harness::builder()
        .with_size(egui::Vec2::new(VIEWPORT[0], VIEWPORT[1]))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state)
}

#[test]
fn mission_healthy() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let expected_status = state(View::Mission, healthy(&fixture, now)).status_line();
    let mut harness = harness(state(View::Mission, healthy(&fixture, now)));
    harness.run();

    harness.get_by_label("open missions: 1");
    harness.get_by_label("generation: 12");
    harness.get_by_label("drift mahalanobis: 4.200");
    harness.get_by_label("drift samples: 512");
    harness
        .query_by_label(&expected_status)
        .expect("the status line names the configured agent and that it is live");

    harness.snapshot("mission_healthy");
}

#[test]
fn mission_degraded() {
    let fixture = fixture();
    let now = ms_after_promotion(&fixture, 20);
    let expected_status =
        state(View::Mission, Sample::degraded(&fixture, "tcp connect failed", now)).status_line();
    let mut harness = harness(state(
        View::Mission,
        Sample::degraded(&fixture, "tcp connect failed", now),
    ));
    harness.run();

    // The degraded state still renders the committed braid: no empty window.
    harness.get_by_label("mission degraded: tcp connect failed");
    harness.get_by_label("rendering committed fixture braid-state.json");
    harness.get_by_label("open missions: 1");
    harness
        .query_by_label(&expected_status)
        .expect("the status line says why the agent is not live");

    harness.snapshot("mission_degraded");
}

#[test]
fn belief_stale() {
    let fixture = fixture();
    // Thirty seconds after the fixture's promotion: every layer slot is older
    // than the 2 s staleness line, so every row is the named "stale" state.
    let now = ms_after_promotion(&fixture, 30_000);
    let mut harness = harness(state(View::Belief, healthy(&fixture, now)));
    harness.run();

    harness.get_by_label("shm region: qualia");
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
    harness.run();

    harness.get_by_label("no sealed segments");
    harness.get_by_label("ledger empty");

    harness.snapshot("evidence_empty");
}
