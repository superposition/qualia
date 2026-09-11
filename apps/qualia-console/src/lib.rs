//! `qualia-console` — the braid's operator console.
//!
//! Five views over one state: the braid the agent reports on `GET /braid`, the
//! belief layers in the shared region, the world the runners have mapped, the
//! evidence MCAP has sealed, and the newest frame each sensing runner published.
//! One native binary, no webview, no JavaScript runtime.
//!
//! Every design choice here traces to `docs/frontend-lessons.md`, which records
//! what five existing front ends taught and what they got wrong. The short
//! version, one line per decision:
//!
//! - `egui` + `eframe`, one binary — the stack source 2 uses and the one the
//!   operator screens already assume;
//! - one `View` enum, one label table, one dispatch, one module per view under
//!   [`views`] — sources 1, 2, 3 and 4 arrived at this independently;
//! - [`client::agent_url`] is the only address in the source, default
//!   `http://127.0.0.1:8080` — source 2's household-subnet default and source
//!   4's twelve hard-coded hosts are the counter-example;
//! - no subnet autodiscovery, no TLS-insecure default — source 2;
//! - the four snapshot states in the crate's `tests/snapshots.rs` are named
//!   degraded states driven by a committed fixture — source 3;
//! - assertions are accessible labels, not pixels alone — source 3;
//! - `GET /braid` is polled off the UI thread through a command/message channel
//!   — source 2;
//! - the status line names the URL and the connection it is showing — sources
//!   1, 2 and 4;
//! - no `unsafe` in the console: the ABI pointer arithmetic stays in
//!   `qualia-shm` — source 1.

pub mod client;
pub mod poller;
pub mod sample;
mod shm_sample;
pub mod views;

pub use client::{agent_url, fixture, BraidSnapshot, BraidState, DriftReport};
pub use sample::Sample;

use std::time::{SystemTime, UNIX_EPOCH};

use poller::Poller;

/// The five operator screens, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Mission,
    Belief,
    World,
    Evidence,
    Telemetry,
}

/// One label table drives both the tab text and the key that selects it
/// (source 1's lesson: never write the binding twice).
pub const VIEWS: [View; 5] = [
    View::Mission,
    View::Belief,
    View::World,
    View::Evidence,
    View::Telemetry,
];

impl View {
    pub const fn label(self) -> &'static str {
        match self {
            View::Mission => "Mission",
            View::Belief => "Belief",
            View::World => "World",
            View::Evidence => "Evidence",
            View::Telemetry => "Telemetry",
        }
    }
}

/// Whether the console is showing the agent or the committed fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Live,
    Unreachable { reason: String },
}

impl Connection {
    pub fn is_live(&self) -> bool {
        matches!(self, Connection::Live)
    }

    fn status(&self) -> String {
        match self {
            Connection::Live => "live".to_owned(),
            Connection::Unreachable { reason } => format!("unreachable: {reason}"),
        }
    }
}

/// The whole console state: one sample, plus which tab is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleState {
    pub view: View,
    pub agent_url: String,
    pub observed_at_ns: u64,
    /// Set by the tab bar's `Refresh` button, drained by the window, which asks
    /// the poller for a poll now instead of at the next interval.
    pub refresh_requested: bool,
    pub connection: Connection,
    pub braid: BraidState,
    pub drift: Option<DriftReport>,
    pub belief: views::belief::BeliefView,
    pub world: views::world::WorldView,
    pub evidence: views::evidence::EvidenceView,
    pub telemetry: views::telemetry::TelemetryView,
}

impl ConsoleState {
    /// A whole state from one poll, with the mission tab selected.
    pub fn from_sample(sample: Sample, agent_url: impl Into<String>) -> Self {
        Self {
            view: View::Mission,
            agent_url: agent_url.into(),
            observed_at_ns: sample.observed_at_ns,
            refresh_requested: false,
            connection: sample.connection,
            braid: sample.braid,
            drift: sample.drift,
            belief: sample.belief,
            world: sample.world,
            evidence: sample.evidence,
            telemetry: sample.telemetry,
        }
    }

    /// Replace everything a poll produces, keeping the selected tab and URL.
    pub fn apply(&mut self, sample: Sample) {
        self.observed_at_ns = sample.observed_at_ns;
        self.connection = sample.connection;
        self.braid = sample.braid;
        self.drift = sample.drift;
        self.belief = sample.belief;
        self.world = sample.world;
        self.evidence = sample.evidence;
        self.telemetry = sample.telemetry;
    }

    /// The chrome line: which agent, whether it answered, and what is showing.
    pub fn status_line(&self) -> String {
        format!(
            "status: {} | {} | session {} | generation {}",
            self.agent_url,
            self.connection.status(),
            self.braid.session_id,
            self.braid.generation
        )
    }
}

/// Milliseconds between `timestamp_ns` and now, saturating at zero.
pub fn format_age_ms(observed_at_ns: u64, timestamp_ns: u64) -> u64 {
    observed_at_ns.saturating_sub(timestamp_ns) / 1_000_000
}

/// Wall clock in nanoseconds since the Unix epoch.
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

/// Draw the tab bar, the selected view and the status line into `ui`.
///
/// This is the whole surface, so the snapshot tests drive the same code the
/// window does; [`app_ui`] only wraps it in a panel.
pub fn render_view(ui: &mut egui::Ui, state: &mut ConsoleState) {
    ui.horizontal_wrapped(|ui| {
        for view in VIEWS {
            if ui
                .selectable_label(state.view == view, view.label())
                .clicked()
            {
                state.view = view;
            }
        }
        if ui.button("Refresh").clicked() {
            state.refresh_requested = true;
        }
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        match state.view {
            View::Mission => views::mission::render(ui, state),
            View::Belief => views::belief::render(ui, state),
            View::World => views::world::render(ui, state),
            View::Evidence => views::evidence::render(ui, state),
            View::Telemetry => views::telemetry::render(ui, state),
        }
    });

    ui.separator();
    ui.monospace(state.status_line());
}

/// One frame of the console: panels around [`render_view`].
pub fn app_ui(ctx: &egui::Context, state: &mut ConsoleState) {
    egui::CentralPanel::default().show(ctx, |ui| render_view(ui, state));
}

/// The window application: polls, then draws.
pub struct ConsoleApp {
    state: ConsoleState,
    poller: Poller,
}

impl ConsoleApp {
    /// Build the app against `QUALIA_AGENT_URL`, degrading to the fixture.
    pub fn new(agent_url: String) -> Result<Self, String> {
        let fixture = client::fixture();
        let initial = Sample::degraded(
            &fixture,
            &format!("waiting for agent at {agent_url}"),
            now_ns(),
        );
        let state = ConsoleState::from_sample(initial, agent_url.clone());
        let poller = Poller::spawn(
            Box::new(client::HttpSource::new(agent_url)?),
            Box::new(client::FixtureSource::default()),
        );
        Ok(Self { state, poller })
    }
}

impl eframe::App for ConsoleApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.state.refresh_requested {
            self.state.refresh_requested = false;
            self.poller.refresh_now();
        }
        if let Some(sample) = self.poller.try_recv() {
            self.state.apply(sample);
        }
        app_ui(ctx, &mut self.state);
        ctx.request_repaint_after(poller::POLL_INTERVAL);
    }
}

/// Run the native window.
pub fn run() -> eframe::Result<()> {
    let agent_url = agent_url();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Qualia Console"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "qualia-console",
        options,
        Box::new(move |_cc| {
            let app = ConsoleApp::new(agent_url).map_err(
                |error| -> Box<dyn std::error::Error + Send + Sync> { error.into() },
            )?;
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
}
