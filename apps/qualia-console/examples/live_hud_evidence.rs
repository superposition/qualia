//! Render the live console state to a PNG, with no window on any desktop.
//!
//! This is the evidence path the operator directive asks for: the same
//! `render_view` the window draws, driven headlessly through `egui_kittest`'s
//! wgpu backend, over **live** readings — the shared region through
//! [`qualia_console::shm_sample`] and the braid through the console's own
//! `GET /braid` client (trusting the agent's `cert.pem` the way the window
//! does). Nothing here is a fixture: if the agent or a producer is down, the
//! frame shows the degraded reading it would show in the window.
//!
//! The figure is written where `egui_kittest` keeps this crate's snapshots;
//! copy it into `docs/evidence/` after the run.
//!
//! Run from the workspace root, with the stack up:
//!
//! ```console
//! $ QUALIA_AGENT_URL=https://127.0.0.1:8080 QUALIA_AGENT_TLS_DIR=$HOME/.qualia_tls \
//!     UPDATE_SNAPSHOTS=1 cargo run -p qualia-console --example live_hud_evidence
//! ```

use egui_kittest::{Harness, SnapshotOptions};
use qualia_console::client::{agent_url, HttpSource};
use qualia_console::views::evidence::EvidenceView;
use qualia_console::{shm_sample, stack, theme, Connection, ConsoleState, Sample};

/// The figure's viewport: wide enough for the six-view grid plus a column of
/// HUD panels on the right, and tall enough for three panels whose rows all
/// fit — the numbers, the bar and sparkline, and the four labelled values.
const VIEWPORT: [f32; 2] = [1600.0, 1200.0];

/// Where the HUD panels are placed for the figure: a column in the right
/// margin, so nothing covers the six views. In the window these are the rects
/// the operator drags; here they are seeded through the same `record_rect` the
/// window uses, so the layout that is drawn is a layout the console can keep.
const PANEL_COLUMN: [f32; 4] = [1300.0, 44.0, 290.0, 360.0];

fn main() {
    let region = shm_sample::region_name();
    let sensing = stack::load_sensing_set();
    let observed_at_ns = qualia_console::now_ns();
    let shm = shm_sample::sample(&region, &sensing);

    let url = agent_url();
    let (connection, braid, drift) = match HttpSource::new(url.clone()).and_then(|mut source| {
        qualia_console::client::BraidSource::fetch(&mut source)
    }) {
        Ok(snapshot) => (Connection::Live, snapshot.braid, snapshot.drift),
        Err(reason) => {
            eprintln!("live_hud_evidence: agent did not answer: {reason}");
            let fixture = qualia_console::fixture();
            (
                Connection::Unreachable { reason },
                fixture.braid,
                fixture.drift,
            )
        }
    };

    let mut brain = shm.brain;
    brain.record_braid(&braid);
    brain.observe_coupling_scale(observed_at_ns);
    let sample = Sample {
        observed_at_ns,
        connection,
        braid,
        drift,
        belief: shm.belief,
        world: shm.world,
        telemetry: shm.telemetry,
        evidence: EvidenceView::default(),
        brain,
        stats: shm.stats,
        // The broker's own surface, live: the same read the window makes.
        coach: qualia_console::views::coach::sample(
            &qualia_console::views::coach::coach_url(),
        ),
    };

    let mut state = ConsoleState::from_sample(sample, url);
    // Place a panel per live runner down the right margin, through the same
    // recorded-rect path the window's drag uses.
    for (index, row) in state.hud.rows.iter().enumerate() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                PANEL_COLUMN[0],
                PANEL_COLUMN[1] + index as f32 * (PANEL_COLUMN[3] + 24.0),
            ),
            egui::vec2(PANEL_COLUMN[2], PANEL_COLUMN[3]),
        );
        state.hud_layout.record_rect(&row.runner, rect);
    }
    println!(
        "live_hud_evidence: region {region}, {} live runner panel(s)",
        state.hud.rows.len()
    );
    for row in &state.hud.rows {
        let values = row
            .values
            .iter()
            .map(|(label, value)| format!("{label} {value:.3}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  {} | {:.2} Hz | {} | updated {} | uptime {} | {}",
            row.runner,
            row.rate_hz,
            if row.publishing {
                "publishing"
            } else {
                "stopped"
            },
            theme::age(observed_at_ns, row.published_at_ns),
            theme::uptime(observed_at_ns, row.started_at_ns),
            values
        );
    }
    if !state.hud.gaps.is_empty() {
        println!("live_hud_evidence: no producer: {}", state.hud.gaps.join(", "));
    }

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(VIEWPORT[0], VIEWPORT[1]))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();
    harness.run();
    harness.snapshot_options("live_hud_evidence", &SnapshotOptions::new());
    println!("live_hud_evidence: frame written");
}
