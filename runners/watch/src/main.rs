//! `qualia-watch` — the engine's supervisor and panel.
//!
//! One binary is both the program that starts the stack and the program that
//! shows it, so bring-up is one command and the operator cannot be looking at a
//! different stack from the one running. The child set comes from
//! `QUALIA_STACK_MANIFEST`; there is no second copy of it here.
//!
//! The layout is three rows: a tab bar, the content panel, and a status bar.
//! The tab bar, the digit keys and the layout dispatch all read
//! [`qualia_watch::view::VIEW_LABELS`], so a panel is one enum variant, one
//! label, one match arm and one render function.
//!
//! `--attach` renders an existing region without taking ownership of it: no
//! children are spawned, no region is created and no control socket is bound.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use qualia_ipc::ControlListener;
use qualia_shm::{LayerReader, ShmRegion, MAX_LEDGER_ENTRIES};
use qualia_types::{
    parse_stack_manifest, BeliefSlot, LedgerEvent, RunnerStdout, StackManifest, MAX_OBJECTS,
    MAX_THOUGHTS, NUM_LAYERS, STATE_DIM,
};
use qualia_watch::ring::{
    self, LedgerRecord, LedgerSource, SeqCursor, ThoughtRecord, ThoughtSource,
};
use qualia_watch::view::{self, ViewMode, ViewState, LAYER_COUNT, VIEW_LABELS};
use ratatui::prelude::*;
use ratatui::widgets::*;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Cleared by the signal handler to leave the render loop.
static RUNNING: AtomicBool = AtomicBool::new(true);

const LAYER_NAMES: [&str; NUM_LAYERS] = [
    "superposition",
    "belief_motor",
    "belief_local",
    "belief_visual",
    "behavior_short",
    "behavior_deep",
    "semantic",
    "senses",
];

const LAYER_FREQ: [&str; NUM_LAYERS] = ["1000", "100", "100", "100", "1", "0.1", "0.05", "30"];

const MAX_DISPLAY_EVENTS: usize = 100;
const MAX_DISPLAY_THOUGHTS: usize = 50;
const VFE_HISTORY_LEN: usize = 120;
const POLL_INTERVAL: Duration = Duration::from_millis(50);

const DEFAULT_SHM_NAME: &str = "/qualia_body";
const DEFAULT_SOCKET: &str = "/tmp/qualia_body.sock";
const DEFAULT_MANIFEST: &str = "config/stack-manifest.default.json";

/// The only key names a child inherits from the supervisor's environment,
/// besides the region name and socket path it is always given. A panel that
/// supervises must not leak its own environment into the runners; a runner that
/// needs another key names it in the manifest's `env_passthrough`.
const CHILD_ENV_ALLOWLIST: &[&str] = &[
    "RUST_LOG",
    "QUALIA_FLY_MODE",
    "QUALIA_FLY_PRIOR_PATH",
    "QUALIA_FLY_COUPLING_SCALE",
    "QUALIA_CUDA_SM",
    "QUALIA_COMPUTE_SOCKET",
    "QUALIA_LEASH_BASE_URL",
    "QUALIA_STACK_MANIFEST",
    "QUALIA_WEB_PORT",
];

// ── Program entry ─────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let attach = std::env::args().any(|arg| arg == "--attach");
    let manifest_path =
        std::env::var("QUALIA_STACK_MANIFEST").unwrap_or_else(|_| DEFAULT_MANIFEST.to_string());
    let manifest = load_manifest(&manifest_path);
    if let Err(reason) = &manifest {
        eprintln!("qualia-watch: stack manifest unavailable ({reason}); starting with no children");
    }

    let shm_name = std::env::var("QUALIA_SHM_NAME")
        .ok()
        .or_else(|| manifest.as_ref().ok().map(|m| m.shared_memory.name.clone()))
        .unwrap_or_else(|| DEFAULT_SHM_NAME.to_string());
    let socket_path = std::env::var("QUALIA_SOCK_PATH")
        .ok()
        .or_else(|| manifest.as_ref().ok().map(|m| m.control.socket.clone()))
        .unwrap_or_else(|| DEFAULT_SOCKET.to_string());

    let shm = if attach {
        ShmRegion::open(&shm_name)
            .map_err(|error| format!("failed to open shm '{shm_name}': {error}"))?
    } else {
        cleanup_stale_shm(&shm_name);
        ShmRegion::create(&shm_name)
            .map_err(|error| format!("failed to create shm '{shm_name}': {error}"))?
    };

    let _control = if attach {
        None
    } else {
        match ControlListener::bind(&socket_path) {
            Ok(listener) => Some(listener),
            Err(error) => {
                eprintln!("qualia-watch: control socket '{socket_path}' unavailable: {error}");
                None
            }
        }
    };

    install_signal_handlers();

    let mut supervisor = Supervisor::new(
        executable_dir(),
        shm_name.clone(),
        socket_path.clone(),
        manifest.map(|m| m.runners.clone()).unwrap_or_default(),
    );
    if !attach {
        supervisor.spawn_children();
    }

    let belief: [BeliefSlot; NUM_LAYERS] =
        std::array::from_fn(|layer| *LayerReader::new(shm.layer_slot(layer)).read());
    let mut app = App {
        shm,
        shm_name,
        socket_path,
        attach,
        start: Instant::now(),
        runner_count: supervisor.children.len(),
        view: ViewState::new(),
        ledger_cursor: SeqCursor::new(),
        events: Vec::with_capacity(MAX_DISPLAY_EVENTS),
        thought_cursor: SeqCursor::new(),
        thoughts: Vec::with_capacity(MAX_DISPLAY_THOUGHTS),
        belief,
        vfe_history: [[0.0; VFE_HISTORY_LEN]; NUM_LAYERS],
        vfe_head: 0,
        hex_bytes: Vec::new(),
        hex_layer: usize::MAX,
        hex_stamp: u64::MAX,
    };

    let guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &mut app, &mut supervisor);

    drop(terminal);
    drop(guard);

    if !app.attach {
        supervisor.shutdown();
        let _ = std::fs::remove_file(&app.socket_path);
    }

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    supervisor: &mut Supervisor,
) -> Result<(), Box<dyn std::error::Error>> {
    terminal.draw(|frame| ui(frame, app))?;
    while RUNNING.load(Ordering::Relaxed) {
        let changed = refresh(app);
        if changed {
            terminal.draw(|frame| ui(frame, app))?;
        }

        if event::poll(POLL_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match handle_key(app, key) {
                    KeyOutcome::Quit => break,
                    KeyOutcome::Restart => {
                        supervisor.shutdown();
                        supervisor.spawn_children();
                        app.runner_count = supervisor.children.len();
                    }
                    KeyOutcome::Continue => {}
                }
                terminal.draw(|frame| ui(frame, app))?;
            }
        }
    }
    Ok(())
}

enum KeyOutcome {
    Continue,
    Quit,
    Restart,
}

/// Map one key press to a state change. The digit keys come from the label
/// table, never from a second `match` beside it.
fn handle_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => KeyOutcome::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => KeyOutcome::Quit,
        KeyCode::Char('r') => KeyOutcome::Restart,
        KeyCode::Char('j') | KeyCode::Down => {
            app.view.move_layer(1);
            KeyOutcome::Continue
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.view.move_layer(-1);
            KeyOutcome::Continue
        }
        KeyCode::Tab => {
            app.view.next_view();
            KeyOutcome::Continue
        }
        KeyCode::BackTab => {
            app.view.prev_view();
            KeyOutcome::Continue
        }
        KeyCode::PageDown => {
            app.view.scroll(10);
            KeyOutcome::Continue
        }
        KeyCode::PageUp => {
            app.view.scroll(-10);
            KeyOutcome::Continue
        }
        KeyCode::Home => {
            app.view.reset_scroll();
            KeyOutcome::Continue
        }
        KeyCode::Char(c) => {
            if let Some(selected) = view::view_for_digit(c) {
                app.view.set_view(selected);
            }
            KeyOutcome::Continue
        }
        _ => KeyOutcome::Continue,
    }
}

/// Restore the terminal even when the render loop returns early.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        std::io::stdout().execute(EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = std::io::stdout().execute(LeaveAlternateScreen);
    }
}

// ── Supervisor ──────────────────────────────────────────────────────────

/// One child process and the name it was launched as.
struct ChildProcess {
    name: String,
    child: Child,
}

/// Starts the manifest's runners next to this executable and reaps them.
struct Supervisor {
    bin_dir: Option<PathBuf>,
    shm_name: String,
    socket_path: String,
    runners: Vec<qualia_types::RunnerConfig>,
    children: Vec<ChildProcess>,
}

impl Supervisor {
    fn new(
        bin_dir: Option<PathBuf>,
        shm_name: String,
        socket_path: String,
        runners: Vec<qualia_types::RunnerConfig>,
    ) -> Self {
        Self {
            bin_dir,
            shm_name,
            socket_path,
            runners,
            children: Vec::new(),
        }
    }

    fn spawn_children(&mut self) {
        let Some(bin_dir) = self.bin_dir.clone() else {
            eprintln!("qualia-watch: cannot resolve the executable directory; not spawning runners");
            return;
        };
        let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        for spec in &self.runners {
            let mut command = Command::new(bin_dir.join(&spec.name));
            command
                .env("QUALIA_SHM_NAME", &self.shm_name)
                .env("QUALIA_SOCK_PATH", &self.socket_path)
                .env("RUST_LOG", &rust_log);
            for key in CHILD_ENV_ALLOWLIST {
                if let Ok(value) = std::env::var(key) {
                    command.env(key, value);
                }
            }
            for key in &spec.env_passthrough {
                if let Ok(value) = std::env::var(key) {
                    command.env(key, value);
                }
            }
            if matches!(spec.stdout, RunnerStdout::Null) {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            match command.spawn() {
                Ok(child) => self.children.push(ChildProcess {
                    name: spec.name.clone(),
                    child,
                }),
                Err(error) => eprintln!("qualia-watch: failed to spawn {}: {error}", spec.name),
            }
        }
    }

    fn shutdown(&mut self) {
        if self.children.is_empty() {
            return;
        }
        eprintln!("\nShutting down {} processes...", self.children.len());
        for child in &mut self.children {
            #[cfg(not(windows))]
            unsafe {
                libc::kill(child.child.id() as i32, libc::SIGTERM);
            }
            #[cfg(windows)]
            {
                let _ = child.child.kill();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let all_done = self
                .children
                .iter_mut()
                .all(|child| matches!(child.child.try_wait(), Ok(Some(_))));
            if all_done || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for entry in self.children.drain(..) {
            let mut child = entry.child;
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    eprintln!("qualia-watch: killing {}", entry.name);
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        eprintln!("All processes stopped.");
    }
}

fn executable_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
}

fn load_manifest(path: &str) -> Result<StackManifest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    parse_stack_manifest(&text)
}

/// Remove a region a dead stack left behind, so `create` is not refused.
fn cleanup_stale_shm(name: &str) {
    #[cfg(windows)]
    {
        let _ = name;
    }
    #[cfg(not(windows))]
    {
        let Ok(c_name) = std::ffi::CString::new(name) else {
            return;
        };
        // SAFETY: `c_name` is a valid NUL-terminated name; both calls tolerate
        // the object being absent.
        unsafe {
            let fd = libc::shm_open(c_name.as_ptr(), libc::O_RDONLY, 0);
            if fd >= 0 {
                libc::close(fd);
                libc::shm_unlink(c_name.as_ptr());
            }
        }
    }
}

#[cfg(not(windows))]
extern "C" fn handle_signal(_signal: libc::c_int) {
    RUNNING.store(false, Ordering::Relaxed);
}

fn install_signal_handlers() {
    #[cfg(not(windows))]
    // SAFETY: both handlers only store to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
    }
}

// ── App state and refresh ───────────────────────────────────────────────

struct App {
    shm: ShmRegion,
    shm_name: String,
    socket_path: String,
    attach: bool,
    start: Instant,
    runner_count: usize,
    view: ViewState,
    ledger_cursor: SeqCursor,
    events: Vec<LedgerRecord>,
    thought_cursor: SeqCursor,
    thoughts: Vec<ThoughtRecord>,
    belief: [BeliefSlot; NUM_LAYERS],
    vfe_history: [[f64; VFE_HISTORY_LEN]; NUM_LAYERS],
    vfe_head: usize,
    hex_bytes: Vec<u8>,
    hex_layer: usize,
    hex_stamp: u64,
}

/// Borrows the region as a change-detection source.
struct Region<'a>(&'a ShmRegion);

impl LedgerSource for Region<'_> {
    fn ledger_seq(&self) -> u64 {
        self.0.ledger_seq()
    }

    fn ledger_entry(&self, index: usize) -> Option<&qualia_types::LedgerEntry> {
        (index < MAX_LEDGER_ENTRIES).then(|| self.0.ledger_entry(index))
    }
}

impl ThoughtSource for Region<'_> {
    fn thought_seq(&self) -> u64 {
        self.0.thought_buffer().write_seq.load(Ordering::Acquire)
    }

    fn thought(&self, index: usize) -> Option<&qualia_types::ThoughtEntry> {
        (index < MAX_THOUGHTS).then(|| &self.0.thought_buffer().entries[index])
    }
}

/// Read every ring and snapshot, and report whether anything visible changed.
fn refresh(app: &mut App) -> bool {
    let mut changed = false;
    for layer in 0..NUM_LAYERS {
        let belief = *LayerReader::new(app.shm.layer_slot(layer)).read();
        if belief.timestamp_ns != app.belief[layer].timestamp_ns
            || belief.vfe != app.belief[layer].vfe
            || belief.cycle_us != app.belief[layer].cycle_us
        {
            changed = true;
        }
        app.belief[layer] = belief;
        app.vfe_history[layer][app.vfe_head] = belief.vfe as f64;
    }
    app.vfe_head = (app.vfe_head + 1) % VFE_HISTORY_LEN;

    {
        let source = Region(&app.shm);
        if ring::drain_ledger(
            &source,
            &mut app.ledger_cursor,
            MAX_LEDGER_ENTRIES as u64,
            &mut app.events,
        ) > 0
        {
            changed = true;
        }
        if ring::drain_thoughts(
            &source,
            &mut app.thought_cursor,
            MAX_THOUGHTS as u64,
            &mut app.thoughts,
        ) > 0
        {
            changed = true;
        }
    }
    ring::trim_front(&mut app.events, MAX_DISPLAY_EVENTS);
    ring::trim_front(&mut app.thoughts, MAX_DISPLAY_THOUGHTS);

    if app.view.view() == ViewMode::Hex {
        let stamp = app.belief[app.view.layer()].timestamp_ns;
        if app.hex_layer != app.view.layer() || app.hex_stamp != stamp {
            belief_bytes(&app.belief[app.view.layer()], &mut app.hex_bytes);
            app.hex_layer = app.view.layer();
            app.hex_stamp = stamp;
        }
    }

    changed
}

/// The typed fields of a belief slot, concatenated in declaration order. The
/// offsets match the `#[repr(C)]` layout, so this is a byte-accurate dump —
/// without casting a reference to raw memory, and therefore without `unsafe`.
fn belief_bytes(belief: &BeliefSlot, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(4 * STATE_DIM * 4 + 32);
    for value in &belief.mean {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in &belief.precision {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&belief.vfe.to_le_bytes());
    for value in &belief.prediction {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in &belief.residual {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&belief.challenge_vfe.to_le_bytes());
    out.extend_from_slice(&belief.confirm_streak.to_le_bytes());
    out.push(belief.compression);
    out.push(belief.layer);
    out.extend_from_slice(&belief._pad);
    out.extend_from_slice(&belief.timestamp_ns.to_le_bytes());
    out.extend_from_slice(&belief.cycle_us.to_le_bytes());
    out.extend_from_slice(&belief._pad2);
}

// ── Layout ──────────────────────────────────────────────────────────────

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let mode = if app.attach { "MONITOR" } else { "SUPERVISOR" };
    let mode_color = if app.attach { Color::Yellow } else { Color::Cyan };
    let runners = if app.attach {
        format!("shm:{}", app.shm_name)
    } else {
        format!("shm:{} | {} runners", app.shm_name, app.runner_count)
    };
    let title = Line::from(vec![
        Span::styled(
            " QUALIA ENGINE v0.1.0 ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{mode}] "),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("| {runners} | "), Style::default().fg(Color::DarkGray)),
    ]);
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // panel tabs
            Constraint::Min(1),    // panel body
            Constraint::Length(1), // panel status
        ])
        .split(inner);

    render_tab_bar(frame, app, rows[0]);
    match app.view.view() {
        ViewMode::Overview => render_overview(frame, app, rows[1]),
        ViewMode::Detail => render_detail(frame, app, rows[1]),
        ViewMode::Hex => render_hex(frame, app, rows[1]),
        ViewMode::Sparklines => render_sparklines(frame, app, rows[1]),
        ViewMode::Residuals => render_residuals(frame, app, rows[1]),
        ViewMode::Weights => render_weights(frame, app, rows[1]),
        ViewMode::World => render_world(frame, app, rows[1]),
    }
    render_status_bar(frame, app, rows[2]);
}

// ── Tab bar and status bar ──────────────────────────────────────────────

fn render_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw("  ")];
    for (index, (label, mode)) in VIEW_LABELS.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
        let style = if *mode == app.view.view() {
            Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        spans.push(Span::styled(*label, style));
    }
    spans.push(Span::styled(
        format!(
            "    layer {} ↑↓ · tab switches · PgUp/PgDn scrolls · r restarts · q quits",
            app.view.layer()
        ),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let elapsed = app.start.elapsed().as_secs();
    let uptime = format!(
        "{:02}:{:02}:{:02}",
        elapsed / 3600,
        (elapsed % 3600) / 60,
        elapsed % 60
    );
    let bar = Line::from(vec![
        Span::styled(
            format!("  shm {} ", app.shm_name),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("up {uptime} "),
            Style::default().fg(Color::Green),
        ),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("ledger {} ", app.shm.ledger_seq()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("layer {} {}", app.view.layer(), LAYER_NAMES[app.view.layer()]),
            Style::default().fg(Color::Yellow),
        ),
    ]);
    frame.render_widget(Paragraph::new(bar), area);
}

// ── Panel: overview ────────────────────────────────────────────────────

fn render_overview(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(12)])
        .split(area);
    render_layer_table(frame, app, rows[0]);
    render_events(frame, app, rows[1]);
}

fn render_layer_table(frame: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(
        [
            "Idx", "Layer", "Rate", "VFE", "Comp", "Streak", "μs", "Flags", "Confirms",
            "Challenges",
        ]
        .iter()
        .map(|label| {
            Cell::from(*label).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        }),
    );

    let mut rows = Vec::with_capacity(LAYER_COUNT);
    for (layer, belief) in app.belief.iter().enumerate() {
        let slot = app.shm.layer_slot(layer);
        let challenge = slot.challenge_flag.load(Ordering::Relaxed);
        let escalate = slot.escalate_flag.load(Ordering::Relaxed);
        let (flag_text, flag_style) = match (challenge, escalate) {
            (true, true) => ("C+E", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            (true, false) => ("C", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            (false, true) => (
                "E",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            (false, false) => ("·", Style::default().fg(Color::DarkGray)),
        };
        let live = belief.timestamp_ns != 0;
        let (vfe_text, vfe_tint) = if live {
            (format!("{:.4}", belief.vfe), vfe_tint(belief.vfe))
        } else {
            ("---".to_string(), Style::default().fg(Color::DarkGray))
        };
        let (comp, streak, cycle) = if live {
            (
                compression_gauge(belief.compression),
                belief.confirm_streak.to_string(),
                belief.cycle_us.to_string(),
            )
        } else {
            ("---".to_string(), "---".to_string(), "---".to_string())
        };
        let row = Row::new(vec![
            Cell::from(format!("  {layer}")).style(Style::default().fg(Color::White)),
            Cell::from(LAYER_NAMES[layer]).style(Style::default().fg(Color::White)),
            Cell::from(LAYER_FREQ[layer]).style(Style::default().fg(Color::DarkGray)),
            Cell::from(vfe_text).style(vfe_tint),
            Cell::from(comp).style(Style::default().fg(Color::Blue)),
            Cell::from(streak).style(Style::default().fg(Color::White)),
            Cell::from(cycle).style(Style::default().fg(Color::White)),
            Cell::from(flag_text).style(flag_style),
            Cell::from(slot.confirm_total.load(Ordering::Relaxed).to_string())
                .style(Style::default().fg(Color::Green)),
            Cell::from(slot.challenge_total.load(Ordering::Relaxed).to_string())
                .style(Style::default().fg(Color::Red)),
        ])
        .style(if layer == app.view.layer() {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        });
        rows.push(row);
    }

    let widths = [
        Constraint::Length(7),
        Constraint::Length(16),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(10),
    ];
    let frame_style = Style::default().fg(Color::DarkGray);
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .block(Block::default().borders(Borders::BOTTOM).border_style(frame_style));
    frame.render_widget(table, area);
}

fn render_events(frame: &mut Frame, app: &App, area: Rect) {
    let frame_style = Style::default().fg(Color::DarkGray);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(frame_style)
        .title(Span::styled(
            " Ledger ",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows_visible = inner.height as usize;
    let first = app.events.len().saturating_sub(rows_visible);
    let items: Vec<ListItem> = app.events[first..]
        .iter()
        .rev()
        .map(|entry| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {}  ", format_clock(entry.timestamp_ns)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("L{}  ", entry.layer),
                    Style::default().fg(Color::White),
                ),
                Span::styled(format!("{:<12}", event_label(entry.event)), event_tint(entry.event)),
                Span::styled(entry.detail.clone(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}

// ── Panel: detail ──────────────────────────────────────────────────────

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let layer = app.view.layer();
    let belief = &app.belief[layer];
    let slot = app.shm.layer_slot(layer);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" Layer {layer} · {} · detail ", LAYER_NAMES[layer]),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(1)])
        .split(inner);

    let stats = weight_stats(&slot.weights);

    let scalars = vec![
        Line::from(vec![
            Span::styled("  vfe ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.6}", belief.vfe), vfe_tint(belief.vfe)),
            Span::styled("   challenge ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.6}", belief.challenge_vfe),
                vfe_tint(belief.challenge_vfe),
            ),
            Span::styled("   compression ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}/255 ", belief.compression),
                Style::default().fg(Color::Blue),
            ),
            Span::styled(
                compression_gauge(belief.compression),
                Style::default().fg(Color::Blue),
            ),
        ]),
        Line::from(vec![
            Span::styled("  streak ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                belief.confirm_streak.to_string(),
                Style::default().fg(Color::Green),
            ),
            Span::styled("   cycle ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}μs", belief.cycle_us),
                Style::default().fg(Color::White),
            ),
            Span::styled("   diag mean ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.3}", stats.diag_mean),
                Style::default().fg(Color::Green),
            ),
            Span::styled("   off-diag norm ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.3}", stats.off_diag_norm),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "   dim       mean    precision    prediction      residual",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
    ];
    frame.render_widget(Paragraph::new(scalars), rows[0]);

    let visible = rows[1].height as usize;
    let first = view::first_visible_row(app.view.detail_scroll(), STATE_DIM, visible);
    let last = (first + visible).min(STATE_DIM);
    let mut lines = Vec::with_capacity(last.saturating_sub(first));
    for dim in first..last {
        let residual = belief.residual[dim];
        let residual_style = residual_style(residual);
        lines.push(Line::from(vec![
            Span::styled(format!("  {dim:3}   "), Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>10.6}  ", belief.mean[dim]),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{:>10.4}  ", belief.precision[dim]),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{:>10.6}  ", belief.prediction[dim]),
                Style::default().fg(Color::Blue),
            ),
            Span::styled(format!("{residual:>10.6}"), residual_style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), rows[1]);
}

// ── Panel: hex ─────────────────────────────────────────────────────────

fn render_hex(frame: &mut Frame, app: &App, area: Rect) {
    let layer = app.view.layer();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" Layer {layer} · {} · hex view ", LAYER_NAMES[layer]),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    let header_area = Rect { height: 1, ..inner };
    let body_area = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "      offset  00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  ascii            region",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))),
        header_area,
    );

    let bytes = &app.hex_bytes;
    let total_rows = bytes.len().div_ceil(HEX_ROW_BYTES);
    let visible = body_area.height as usize;
    let first = view::first_visible_row(app.view.hex_scroll(), total_rows, visible);
    let last = (first + visible).min(total_rows);
    let mut lines = Vec::with_capacity(last - first);
    for row in first..last {
        let offset = row * HEX_ROW_BYTES;
        let end = (offset + HEX_ROW_BYTES).min(bytes.len());
        let (field, field_style) = field_at(offset);
        let mut spans = vec![Span::styled(
            format!("  {offset:04X}   "),
            Style::default().fg(Color::DarkGray),
        )];
        for index in 0..HEX_ROW_BYTES {
            if index == 8 {
                spans.push(Span::raw(" "));
            }
            if offset + index < end {
                let byte = bytes[offset + index];
                let style = if byte == 0 {
                    Style::default().fg(Color::DarkGray)
                } else {
                    field_style
                };
                spans.push(Span::styled(format!("{byte:02X} "), style));
            } else {
                spans.push(Span::raw("   "));
            }
        }
        spans.push(Span::raw("  "));
        let ascii: String = bytes[offset..end]
            .iter()
            .map(|&byte| {
                if (0x20..=0x7E).contains(&byte) {
                    byte as char
                } else {
                    '·'
                }
            })
            .collect();
        spans.push(Span::styled(
            format!("{ascii:<16}"),
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            format!("  {field}"),
            field_style.add_modifier(Modifier::DIM),
        ));
        lines.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(lines).scroll((0, 0)),
        body_area,
    );
}

const HEX_ROW_BYTES: usize = 16;

/// Name and colour of the belief field region starting at `offset`.
fn field_at(offset: usize) -> (&'static str, Style) {
    let vector = STATE_DIM * 4;
    let base = 4 * vector;
    let (name, color) = if offset < vector {
        ("mean[1024]", Color::Green)
    } else if offset < 2 * vector {
        ("precision[1024]", Color::Cyan)
    } else if offset < 2 * vector + 4 {
        ("vfe", Color::Yellow)
    } else if offset < 3 * vector + 4 {
        ("prediction[1024]", Color::Blue)
    } else if offset < base + 4 {
        ("residual[1024]", Color::Red)
    } else if offset < base + 8 {
        ("challenge_vfe", Color::Yellow)
    } else if offset < base + 12 {
        ("confirm_streak", Color::Green)
    } else if offset < base + 13 {
        ("compression", Color::Magenta)
    } else if offset < base + 14 {
        ("layer", Color::White)
    } else if offset < base + 16 {
        ("_pad", Color::DarkGray)
    } else if offset < base + 24 {
        ("timestamp_ns", Color::DarkGray)
    } else if offset < base + 28 {
        ("cycle_us", Color::DarkGray)
    } else {
        ("_pad2", Color::DarkGray)
    };
    (name, Style::default().fg(color))
}

// ── Panel: sparklines ──────────────────────────────────────────────────

fn render_sparklines(frame: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(Span::styled(
        " VFE history · every layer ",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Ratio(1, NUM_LAYERS as u32); NUM_LAYERS])
        .split(inner);

    for layer in 0..NUM_LAYERS {
        let mut data = Vec::with_capacity(VFE_HISTORY_LEN);
        for step in 0..VFE_HISTORY_LEN {
            data.push(app.vfe_history[layer][(app.vfe_head + step) % VFE_HISTORY_LEN]);
        }
        let peak = data.iter().copied().fold(0.001_f64, f64::max);
        let scaled: Vec<u64> = data
            .iter()
            .map(|value| ((value / peak) * 100.0) as u64)
            .collect();
        let color = layer_tint(layer);
        let belief = &app.belief[layer];
        let label = format!(
            " L{layer} {} · vfe {:.4} · peak {peak:.4} ",
            LAYER_NAMES[layer], belief.vfe
        );
        let sparkline = Sparkline::default()
            .style(Style::new().fg(color))
            .data(scaled)
            .block(Block::default().title(Span::styled(label, Style::new().fg(color))));
        frame.render_widget(sparkline, rows[layer]);
    }
}

fn layer_tint(layer: usize) -> Color {
    const TINTS: [Color; NUM_LAYERS] = [
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Cyan,
        Color::Blue,
        Color::Magenta,
        Color::LightRed,
        Color::DarkGray,
    ];
    TINTS.get(layer).copied().unwrap_or(Color::DarkGray)
}

// ── Panel: residuals ───────────────────────────────────────────────────

fn render_residuals(frame: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(Span::styled(
        " Residuals · layers x dimensions ",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ));
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);

    let legend = Line::from(vec![
        Span::raw("  "),
        Span::styled("·", Style::default().fg(Color::DarkGray)),
        Span::styled(" <1e-3  ", Style::default().fg(Color::DarkGray)),
        Span::styled("░", Style::default().fg(Color::Green)),
        Span::styled(" <5e-3  ", Style::default().fg(Color::DarkGray)),
        Span::styled("▒", Style::default().fg(Color::Yellow)),
        Span::styled(" <1e-2  ", Style::default().fg(Color::DarkGray)),
        Span::styled("▓", Style::default().fg(Color::Red)),
        Span::styled(" <5e-2  ", Style::default().fg(Color::DarkGray)),
        Span::styled("█", Style::default().fg(Color::LightRed)),
        Span::styled(" ≥5e-2   (absolute residual per dimension)", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(vec![Line::from(""), legend]), rows[0]);

    let columns = (rows[1].width as usize).saturating_sub(20).max(1).min(STATE_DIM);
    let mut lines = Vec::new();
    let mut header = vec![Span::styled(
        "          ",
        Style::default().fg(Color::DarkGray),
    )];
    for dim in 0..columns {
        if dim % 8 == 0 {
            header.push(Span::styled(format!("{dim:<8}"), Style::default().fg(Color::DarkGray)));
        }
    }
    lines.push(Line::from(header));

    for (layer, belief) in app.belief.iter().enumerate() {
        let mut spans = vec![Span::styled(
            format!(
                "  L{layer} {:5} ",
                &LAYER_NAMES[layer][..5.min(LAYER_NAMES[layer].len())]
            ),
            Style::default().fg(Color::White),
        )];
        for dim in 0..columns {
            let (glyph, color) = residual_cell(belief.residual[dim].abs());
            spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
        }
        let mean = belief.residual.iter().map(|value| value.abs()).sum::<f32>() / STATE_DIM as f32;
        spans.push(Span::styled(
            format!("  mean={mean:.4}"),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), rows[1]);
}

fn residual_cell(value: f32) -> (char, Color) {
    if value < 0.001 {
        ('·', Color::DarkGray)
    } else if value < 0.005 {
        ('░', Color::Green)
    } else if value < 0.01 {
        ('▒', Color::Yellow)
    } else if value < 0.05 {
        ('▓', Color::Red)
    } else {
        ('█', Color::LightRed)
    }
}

// ── Panel: weights ─────────────────────────────────────────────────────

fn render_weights(frame: &mut Frame, app: &App, area: Rect) {
    let layer = app.view.layer();
    let weights = &app.shm.layer_slot(layer).weights;
    let bias = &app.shm.layer_slot(layer).bias;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" Layer {layer} · {} · generative weights 1024x1024 ", LAYER_NAMES[layer]),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);

    let stats = weight_stats(weights);
    let bias_norm = (bias.iter().map(|value| value * value).sum::<f32>() as f64).sqrt();

    let summary = vec![
        Line::from(vec![
            Span::styled("  diag mean ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.4}", stats.diag_mean),
                Style::default().fg(Color::Green),
            ),
            Span::styled("   off-diag norm ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.4}", stats.off_diag_norm),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("   bias norm ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{bias_norm:.4}"), Style::default().fg(Color::Cyan)),
            Span::styled("   range ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("[{:.3}, {:.3}]", stats.min, stats.max),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(Span::styled(
            "  Brighter = larger absolute value; green = positive, red = negative.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(summary), rows[0]);

    let columns = (rows[1].width as usize).saturating_sub(12).max(1).min(STATE_DIM);
    let total_rows = STATE_DIM;
    let visible = rows[1].height as usize;
    let first = view::first_visible_row(app.view.hex_scroll(), total_rows, visible);
    let last = (first + visible).min(total_rows);
    let scale = (stats.abs_mean * 5.0).max(0.01) as f32;
    let mut lines = Vec::with_capacity(last.saturating_sub(first));
    for row in first..last {
        let mut spans = vec![Span::styled(
            format!("{row:3}  "),
            Style::default().fg(Color::DarkGray),
        )];
        for column in 0..columns {
            let weight = weights[row * STATE_DIM + column];
            if row == column {
                let (glyph, color) = if weight > 0.5 {
                    ('█', Color::Green)
                } else if weight > 0.0 {
                    ('▓', Color::Green)
                } else {
                    ('▓', Color::Red)
                };
                spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
            } else {
                const GLYPHS: [char; 5] = ['·', '░', '▒', '▓', '█'];
                let intensity = (weight.abs() / scale).min(1.0);
                let level = if intensity < 0.05 {
                    0
                } else if intensity < 0.2 {
                    1
                } else if intensity < 0.5 {
                    2
                } else if intensity < 0.8 {
                    3
                } else {
                    4
                };
                let color = if weight >= 0.0 { Color::Green } else { Color::Red };
                spans.push(Span::styled(GLYPHS[level].to_string(), Style::default().fg(color)));
            }
        }
        let norm = (0..STATE_DIM)
            .map(|column| {
                let weight = weights[row * STATE_DIM + column];
                weight * weight
            })
            .sum::<f32>()
            .sqrt();
        spans.push(Span::styled(
            format!("  ‖{norm:.2}‖"),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), rows[1]);
}

// ── Panel: world ───────────────────────────────────────────────────────

fn render_world(frame: &mut Frame, app: &App, area: Rect) {
    let world = app.shm.world_model();
    let lidar = app.shm.lidar_grid().snapshot(8).unwrap_or_default();
    let binary = app.shm.binary_map();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " World model · thought stream ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(11),
            Constraint::Length(8),
            Constraint::Min(1),
        ])
        .split(halves[0]);

    let directive = ring::read_cstr(&world.directive);
    let scene = ring::read_cstr(&world.scene);
    let activity = ring::read_cstr(&world.activity);
    let pose = &world.robot_pose;
    let goal = &world.nav_goal;
    let goal_text = if goal.active == 0 {
        "no goal set".to_string()
    } else {
        format!(
            "cell ({}, {}) -> ({:.2}, {:.2})",
            goal.cell_x, goal.cell_z, goal.x_m, goal.z_m
        )
    };
    let info = vec![
        Line::from(vec![
            Span::styled(
                " DIRECTIVE ",
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {directive}"), Style::default().fg(Color::Magenta)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  scene ", Style::default().fg(Color::Cyan)),
            Span::styled(scene, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  activity ", Style::default().fg(Color::Yellow)),
            Span::styled(activity, Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  objects {}", world.num_objects),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("  vision frames {}", world.vision_frame_count),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("  llm calls {}", world.llm_call_count),
                Style::default().fg(Color::Green),
            ),
        ]),
        Line::from(vec![
            Span::styled("  nav ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("seq {}", world.nav_seq.load(Ordering::Acquire)),
                Style::default().fg(Color::White),
            ),
            Span::styled("  pose ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "x {:.2} · z {:.2} · yaw {:.2} · conf {:.2}",
                    pose.x_m, pose.z_m, pose.yaw_rad, pose.confidence
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  goal ", Style::default().fg(Color::Yellow)),
            Span::styled(goal_text, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  maps ", Style::default().fg(Color::Green)),
            Span::styled(
                format!(
                    "lidar {} · binary {} · occupied {} free {} unknown {}",
                    lidar.seq,
                    binary.seq.load(Ordering::Acquire),
                    binary.occupied_cells,
                    binary.free_cells,
                    binary.unknown_cells
                ),
                Style::default().fg(Color::White),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(info), left[0]);

    let objects_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Objects ", Style::default().fg(Color::White)));
    let objects_inner = objects_block.inner(left[1]);
    frame.render_widget(objects_block, left[1]);

    let mut object_lines = Vec::new();
    for object in world.objects.iter().take(MAX_OBJECTS) {
        if object.active == 0 {
            continue;
        }
        let color = if object.confidence > 0.8 {
            Color::Green
        } else if object.confidence > 0.5 {
            Color::Yellow
        } else {
            Color::Red
        };
        object_lines.push(Line::from(vec![
            Span::styled(
                format!("  {:.0}% ", object.confidence * 100.0),
                Style::default().fg(color),
            ),
            Span::styled(ring::read_cstr(&object.name), Style::default().fg(Color::White)),
            Span::styled(
                format!(" ({:.1},{:.1})", object.x, object.y),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    if object_lines.is_empty() {
        object_lines.push(Line::from(Span::styled(
            "  none",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(object_lines), objects_inner);

    let embedding_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " scene embedding · first 64 ",
            Style::default().fg(Color::White),
        ));
    let embedding_inner = embedding_block.inner(left[2]);
    frame.render_widget(embedding_block, left[2]);
    let mut embedding_lines = Vec::new();
    for row in 0..8.min(embedding_inner.height as usize) {
        let mut spans = vec![Span::raw("  ")];
        for column in 0..8 {
            let (glyph, color) = embedding_cell(world.scene_embedding[row * 8 + column]);
            spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
            spans.push(Span::raw(" "));
        }
        let base = row * 8;
        spans.push(Span::styled(
            format!(
                "  [{base:2}] {:.2} {:.2} {:.2} {:.2}",
                world.scene_embedding[base],
                world.scene_embedding[base + 1],
                world.scene_embedding[base + 2],
                world.scene_embedding[base + 3],
            ),
            Style::default().fg(Color::DarkGray),
        ));
        embedding_lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(embedding_lines), embedding_inner);

    let thoughts_block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " Thoughts ",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        ));
    let thoughts_inner = thoughts_block.inner(halves[1]);
    frame.render_widget(thoughts_block, halves[1]);

    let visible = thoughts_inner.height as usize;
    let start = app.thoughts.len().saturating_sub(visible);
    let items: Vec<ListItem> = app.thoughts[start..]
        .iter()
        .map(|thought| {
            let layer = if thought.layer == 255 {
                "VIS".to_string()
            } else {
                format!("L{}", thought.layer)
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {layer:>3} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<10} ", thought_kind_label(thought.kind)),
                    Style::default()
                        .fg(thought_kind_color(thought.kind))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(thought.text.clone(), Style::default().fg(Color::White)),
            ]))
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(
            List::new(vec![ListItem::new(Line::from(Span::styled(
                "  no thoughts yet",
                Style::default().fg(Color::DarkGray),
            )))]),
            thoughts_inner,
        );
    } else {
        frame.render_widget(List::new(items), thoughts_inner);
    }
}

fn embedding_cell(value: f32) -> (char, Color) {
    let magnitude = value.abs();
    let color = if value > 0.0 { Color::Green } else { Color::Red };
    if magnitude < 0.05 {
        ('·', Color::DarkGray)
    } else if magnitude < 0.2 {
        ('░', color)
    } else if magnitude < 0.5 {
        ('▒', color)
    } else if magnitude < 0.8 {
        ('▓', color)
    } else {
        ('█', color)
    }
}

// ── Formatting helpers ──────────────────────────────────────────────────

/// One pass over a layer's generative weights, shared by the Detail and
/// Weights panels.
struct WeightStats {
    diag_mean: f64,
    off_diag_norm: f64,
    abs_mean: f64,
    min: f32,
    max: f32,
}

fn weight_stats(weights: &[f32]) -> WeightStats {
    let mut diagonal = 0.0_f64;
    let mut off_diagonal = 0.0_f64;
    let mut absolute = 0.0_f64;
    let mut min = f32::MAX;
    let mut max = f32::MIN;
    for row in 0..STATE_DIM {
        for column in 0..STATE_DIM {
            let weight = weights[row * STATE_DIM + column];
            absolute += weight.abs() as f64;
            min = min.min(weight);
            max = max.max(weight);
            if row == column {
                diagonal += weight as f64;
            } else {
                off_diagonal += (weight as f64) * (weight as f64);
            }
        }
    }
    WeightStats {
        diag_mean: diagonal / STATE_DIM as f64,
        off_diag_norm: off_diagonal.sqrt(),
        abs_mean: absolute / (STATE_DIM * STATE_DIM) as f64,
        min,
        max,
    }
}

fn compression_gauge(value: u8) -> String {
    // `value` spans 0..=255; draw it as four eighth-block cells.
    const CELLS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let mut eighths = (value as usize * 32) / 255;
    let mut gauge = String::with_capacity(4);
    for _ in 0..4 {
        let cell = eighths.min(8);
        gauge.push(CELLS[cell]);
        eighths -= cell;
    }
    gauge
}

fn vfe_tint(vfe: f32) -> Style {
    let color = if vfe < 0.01 {
        Color::Green
    } else if vfe < 0.05 {
        Color::Yellow
    } else {
        Color::Red
    };
    Style::new().fg(color)
}

fn residual_style(value: f32) -> Style {
    if value.abs() < 0.001 {
        Style::default().fg(Color::Green)
    } else if value.abs() < 0.01 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Red)
    }
}

fn event_label(event: LedgerEvent) -> &'static str {
    const LABELS: [&str; 5] = ["CHALLENGE", "CONFIRM", "HABIT", "HABIT_DECAY", "ESCALATE"];
    LABELS[event as usize]
}

fn event_tint(event: LedgerEvent) -> Style {
    let (color, emphatic) = match event {
        LedgerEvent::Challenge => (Color::Red, true),
        LedgerEvent::Confirm => (Color::Green, false),
        LedgerEvent::Habit => (Color::Cyan, false),
        LedgerEvent::HabitDecay => (Color::Yellow, false),
        LedgerEvent::Escalate => (Color::Magenta, true),
    };
    let style = Style::new().fg(color);
    if emphatic {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn thought_kind_label(kind: u8) -> &'static str {
    match kind {
        0 => "observe",
        1 => "predict",
        2 => "surprise",
        3 => "learn",
        4 => "resolve",
        5 => "escalate",
        _ => "untyped",
    }
}

fn thought_kind_color(kind: u8) -> Color {
    const COLORS: [Color; 6] = [
        Color::Cyan,
        Color::Blue,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Magenta,
    ];
    COLORS.get(kind as usize).copied().unwrap_or(Color::DarkGray)
}

fn format_clock(ns: u64) -> String {
    let millis = (ns / 1_000_000) % 1000;
    let secs = (ns / 1_000_000_000) % 60;
    let mins = (ns / 60_000_000_000) % 60;
    let hours = (ns / 3_600_000_000_000) % 24;
    format!("{hours:02}:{mins:02}:{secs:02}.{millis:03}")
}
