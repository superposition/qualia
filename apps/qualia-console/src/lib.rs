//! `qualia-console` — the braid's operator console.
//!
//! Six views over one state: the braid the agent reports on `GET /braid`, the
//! belief layers in the shared region, the world the runners have mapped, the
//! evidence MCAP has sealed, the newest frame each sensing runner published, and
//! the fly brain — the connectome prior's firing model, the lidar cloud and the
//! belief matrices. One native binary, no webview, no JavaScript runtime.
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
//! - the named snapshot states in the crate's `tests/snapshots.rs` are driven
//!   by a committed fixture, plus one fresh region and the opening arrangement
//!   — source 3;
//! - the telemetry rows are the ABI's sensing slots, or the runners a named
//!   stack declares — never a runner list invented beside the stack — source 1's
//!   avoid;
//! - assertions are accessible labels, not pixels alone — source 3;
//! - `GET /braid` is polled off the UI thread through a command/message channel
//!   — source 2;
//! - every dataset is a floating panel on one page, arranged in a 3×2 grid so
//!   the whole console is visible at once, with the Mission panel naming the agent,
//!   session and generation behind every other panel's numbers — sources 1, 2
//!   and 4;
//! - presentation lives in [`theme`], one palette, one family and one 8 px
//!   grid, so the six views cannot drift apart;
//! - no `unsafe` in the console: the ABI pointer arithmetic stays in
//!   `qualia-shm` — source 1.

pub mod client;
pub mod hud;
mod issues;
pub mod poller;
pub mod sample;
/// The region read path, public so an evidence run (and the console's own
/// example) samples the live arena through the same code the window does.
pub mod shm_sample;
pub mod stack;
pub mod theme;
pub mod views;

pub use client::{agent_url, fixture, BraidSnapshot, BraidState, DriftReport};
pub use sample::Sample;

use std::time::{SystemTime, UNIX_EPOCH};

use poller::Poller;

/// The six operator screens, in grid order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Mission,
    Belief,
    World,
    Evidence,
    Telemetry,
    Brain,
}

/// One label table drives the window titles, the `Console` menu's checkboxes
/// and the grid order (source 1's lesson: never write the binding twice).
pub const VIEWS: [View; 6] = [
    View::Mission,
    View::Belief,
    View::World,
    View::Evidence,
    View::Telemetry,
    View::Brain,
];

impl View {
    pub const fn label(self) -> &'static str {
        match self {
            View::Mission => "Mission",
            View::Belief => "Belief",
            View::World => "World",
            View::Evidence => "Evidence",
            View::Telemetry => "Telemetry",
            View::Brain => "Brain",
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
}

/// Which floating panels the operator selected. Unavailable panels are grouped
/// in Issues without changing this selection, so recovery restores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSet {
    pub mission: bool,
    pub belief: bool,
    pub world: bool,
    pub evidence: bool,
    pub telemetry: bool,
    pub brain: bool,
}

impl Default for WindowSet {
    fn default() -> Self {
        Self {
            mission: true,
            belief: true,
            world: true,
            evidence: true,
            telemetry: true,
            brain: true,
        }
    }
}

impl WindowSet {
    /// Only `view` open, for the per-panel snapshot states.
    pub fn only(view: View) -> Self {
        let mut set = Self {
            mission: false,
            belief: false,
            world: false,
            evidence: false,
            telemetry: false,
            brain: false,
        };
        set.set(view, true);
        set
    }

    pub fn is_open(self, view: View) -> bool {
        match view {
            View::Mission => self.mission,
            View::Belief => self.belief,
            View::World => self.world,
            View::Evidence => self.evidence,
            View::Telemetry => self.telemetry,
            View::Brain => self.brain,
        }
    }

    pub fn set(&mut self, view: View, open: bool) {
        match view {
            View::Mission => self.mission = open,
            View::Belief => self.belief = open,
            View::World => self.world = open,
            View::Evidence => self.evidence = open,
            View::Telemetry => self.telemetry = open,
            View::Brain => self.brain = open,
        }
    }
}

/// The whole console state: one sample, plus which panels are showing.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleState {
    pub agent_url: String,
    pub observed_at_ns: u64,
    /// Set by the strip, drained by the window, which asks the poller for a
    /// poll now instead of at the next interval.
    pub refresh_requested: bool,
    pub connection: Connection,
    pub braid: BraidState,
    pub drift: Option<DriftReport>,
    pub belief: views::belief::BeliefView,
    pub world: views::world::WorldView,
    pub evidence: views::evidence::EvidenceView,
    pub telemetry: views::telemetry::TelemetryView,
    pub brain: views::brain::BrainView,
    /// The runner telemetry frames the HUD panels draw this poll.
    pub hud: views::stats::StatsView,
    /// The mission broker's decision stream (T63), the console's second
    /// source: the Coach panel's reading, or the named reason it has none.
    pub coach: views::coach::CoachView,
    /// The rate history those panels' sparklines draw; it survives a poll the
    /// way the Brain view keeps its camera.
    pub hud_history: views::stats::StatsHistory,
    /// Which HUD panels are showing and where they sit, remembered between
    /// runs by the binary (the library default is the opening cascade).
    pub hud_layout: hud::HudLayout,
    /// The prior and its layout, read once at start-up: a poll never re-reads a
    /// graph file, and every frame shares the same owned copy.
    pub brain_assets: std::sync::Arc<views::brain::BrainAssets>,
    pub windows: WindowSet,
}

impl ConsoleState {
    /// A whole state from one poll, with every panel showing.
    pub fn from_sample(sample: Sample, agent_url: impl Into<String>) -> Self {
        let mut state = Self {
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
            brain: sample.brain,
            hud: sample.stats,
            coach: sample.coach,
            hud_history: views::stats::StatsHistory::default(),
            hud_layout: hud::HudLayout::default(),
            brain_assets: std::sync::Arc::new(views::brain::BrainAssets::load()),
            windows: WindowSet::default(),
        };
        state.hud_history.record(&state.hud);
        state
    }

    /// Replace everything a poll produces, keeping the open panels and URL.
    pub fn apply(&mut self, sample: Sample) {
        self.observed_at_ns = sample.observed_at_ns;
        self.connection = sample.connection;
        self.braid = sample.braid;
        self.drift = sample.drift;
        self.belief = sample.belief;
        self.world = sample.world;
        self.evidence = sample.evidence;
        self.telemetry = sample.telemetry;
        self.hud = sample.stats;
        self.coach = sample.coach;
        self.hud_history.record(&self.hud);
        // The brain view keeps the operator's camera, its short history and the
        // markers earlier polls produced; only the readings are replaced.
        self.brain.absorb(sample.brain);
    }
}

/// Wall clock in nanoseconds since the Unix epoch.
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}

/// Draw the console: the strip, collapsed issues and available floating panels.
///
/// This is the whole surface, so the snapshot tests drive the same code the
/// window does; [`app_ui`] only wraps it in a panel.
pub fn render_view(ui: &mut egui::Ui, state: &mut ConsoleState) {
    theme::apply(ui.ctx());
    ui.painter().rect_filled(ui.max_rect(), 0.0, theme::BG);
    theme::menu_strip(ui, state);
    issues::render(ui, state);

    let mut windows = state.windows;
    let ctx = ui.ctx().clone();
    let visible: Vec<_> = VIEWS.into_iter().filter(|view| {
        windows.is_open(*view)
            && (!issues::unavailable(state, *view) || issues::show_unavailable_panels(&ctx))
    }).collect();
    let compact = !issues::show_unavailable_panels(&ctx)
        && VIEWS.into_iter().any(|view| windows.is_open(view) && issues::unavailable(state, view));
    let arrangement = (compact, visible.clone());
    let rearrange = ctx.data_mut(|data| {
        let id = egui::Id::new("qualia_visible_panels");
        let previous = data.get_temp::<(bool, Vec<View>)>(id);
        let changed = previous.as_ref() != Some(&arrangement);
        data.insert_temp(id, arrangement);
        changed
    });
    let bounds = ui.available_rect_before_wrap();
    for view in VIEWS {
        if issues::unavailable(state, view) && !issues::show_unavailable_panels(&ctx) {
            continue;
        }
        let mut open = windows.is_open(view);
        let placement = if compact {
            visible.iter().position(|candidate| *candidate == view).map(|index| {
                let columns = visible.len().min(3);
                let rows = visible.len().div_ceil(columns);
                let cell = egui::vec2(bounds.width() / columns as f32, bounds.height() / rows as f32);
                let pos = bounds.min + egui::vec2(cell.x * (index % columns) as f32, cell.y * (index / columns) as f32);
                [pos.x + 16.0, pos.y + 16.0, cell.x - 48.0, cell.y - 72.0]
            })
        } else {
            None
        };
        theme::arranged_panel(&ctx, view, &mut open, placement, rearrange, bounds, |ui| match view {
            View::Mission => views::mission::render(ui, state),
            View::Belief => views::belief::render(ui, state),
            View::World => views::world::render(ui, state),
            View::Evidence => views::evidence::render(ui, state),
            View::Telemetry => views::telemetry::render(ui, state),
            View::Brain => views::brain::render(ui, state),
        });
        windows.set(view, open);
    }
    state.windows = windows;

    // The operator's HUD, additive to the views: `Ctrl+H` shows or hides the
    // whole set from anywhere in the window, and the `HUD` menu does the same
    // per runner.
    if ui.input(|input| input.key_pressed(egui::Key::H) && input.modifiers.command) {
        let on = state.hud_layout.on;
        state.hud_layout.set_on(!on);
    }
    hud::render(ui.ctx(), state);

    // The coach's decisions, beside the HUD: the broker's own surface (T63),
    // not a seventh view over the agent's state.
    if issues::coach_visible(ui.ctx(), state) {
        views::coach::render_panel(ui.ctx(), state);
    }
}

/// One frame of the console: panels around [`render_view`].
pub fn app_ui(ctx: &egui::Context, state: &mut ConsoleState) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(theme::BG))
        .show(ctx, |ui| render_view(ui, state));
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
        let mut app = Self { state, poller };
        // The binary, not the library, owns the remembered layout: tests build
        // a console state without touching the operator's file.
        app.state.hud_layout = hud::HudLayout::load();
        Ok(app)
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
        self.state.hud_layout.save_if_due(now_ns());
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
