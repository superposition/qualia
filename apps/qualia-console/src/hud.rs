//! The HUD: the operator's floating debug panels, one per runner's telemetry
//! frame.
//!
//! Each panel is the operator's own surface, additive to the six views: a
//! movable, resizable, collapsible window fed by one runner's binary stats frame
//! (`crate::views::stats`), toggled from the `HUD` menu or `Ctrl+H`. Panels
//! appear for the runners that actually publish a frame; a runner the stack
//! declares that publishes nothing is named as a gap in the menu instead of
//! getting an empty panel.
//!
//! The layout — which panels are open and where they sit — is remembered
//! between runs in a small fixed-width binary file (`QUALIA_CONSOLE_LAYOUT`,
//! default beside the user's local state), written at most twice a second and
//! only when it changed: a restart opens the console the operator left it, and
//! a crash costs at most the last half-second of dragging.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use crate::theme;
use crate::views::stats::StatsRow;
use crate::ConsoleState;

/// A frame older than this reads stale: the same 1.5 s the agent uses for a
/// stale pose or lidar scan.
pub const STALE_NS: u64 = 1_500_000_000;

/// Environment key naming the layout file; unset means the per-user default.
pub const LAYOUT_ENV: &str = "QUALIA_CONSOLE_LAYOUT";

/// Layout file magic: `QLHD`.
const LAYOUT_MAGIC: u32 = 0x514C_4844;

/// Layout file version; a file this build does not know is ignored.
const LAYOUT_VERSION: u16 = 1;

/// Bytes of a runner name in the layout file.
const NAME_BYTES: usize = 32;

/// Bytes one layout entry occupies: name, open flag, padding, then the rect.
const ENTRY_BYTES: usize = NAME_BYTES + 4 + 4 * 4;

/// Bytes before the first entry.
const HEADER_BYTES: usize = 12;

/// Default panel size — wide enough for the label/value columns, tall enough
/// for the numbers, the sparkline and four values.
const PANEL_W: f32 = 272.0;
const PANEL_H: f32 = 216.0;

/// At most two layout writes a second, and only after a change.
const SAVE_INTERVAL_NS: u64 = 500_000_000;

/// Which HUD panels are showing and where they sit.
#[derive(Debug, Clone, PartialEq)]
pub struct HudLayout {
    /// Whether the HUD is showing at all (`Ctrl+H`).
    pub on: bool,
    open: BTreeMap<String, bool>,
    rects: BTreeMap<String, [f32; 4]>,
    path: Option<PathBuf>,
    dirty: bool,
    last_save_ns: u64,
}

impl Default for HudLayout {
    fn default() -> Self {
        Self {
            on: true,
            open: BTreeMap::new(),
            rects: BTreeMap::new(),
            path: None,
            dirty: false,
            last_save_ns: 0,
        }
    }
}

impl HudLayout {
    /// Load the remembered layout, or the defaults when there is none.
    ///
    /// A missing, short, foreign or truncated file is not an error: the console
    /// opens with every panel showing at its cascade position and rewrites the
    /// file as the operator moves things.
    pub fn load() -> Self {
        let path = layout_path();
        let mut layout = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        if let Ok(bytes) = std::fs::read(&path) {
            layout.decode(&bytes);
        }
        layout
    }

    /// Whether `runner`'s panel is showing. Unset means showing: the HUD's
    /// whole point is that the frames are visible.
    pub fn is_open(&self, runner: &str) -> bool {
        self.open.get(runner).copied().unwrap_or(true)
    }

    /// Show or hide `runner`'s panel.
    pub fn set(&mut self, runner: &str, open: bool) {
        if self.is_open(runner) != open {
            self.open.insert(runner.to_owned(), open);
            self.dirty = true;
        }
    }

    /// Turn the whole HUD on or off; the per-panel choices are kept.
    pub fn set_on(&mut self, on: bool) {
        if self.on != on {
            self.on = on;
            self.dirty = true;
        }
    }

    /// Show or hide every panel in `runners`.
    pub fn set_all(&mut self, runners: &[String], open: bool) {
        for runner in runners {
            self.set(runner, open);
        }
    }

    /// The remembered rect of `runner`'s panel, `[x, y, width, height]`.
    pub fn rect(&self, runner: &str) -> Option<[f32; 4]> {
        self.rects.get(runner).copied()
    }

    /// Record where egui put `runner`'s panel this frame.
    pub fn record_rect(&mut self, runner: &str, rect: egui::Rect) {
        let next = [
            rect.min.x,
            rect.min.y,
            rect.width(),
            rect.height(),
        ];
        let changed = match self.rects.get(runner) {
            Some(previous) => previous
                .iter()
                .zip(next.iter())
                .any(|(before, after)| (before - after).abs() > 0.5),
            None => true,
        };
        if changed {
            self.rects.insert(runner.to_owned(), next);
            self.dirty = true;
        }
    }

    /// The cascade position a panel takes the first time it is shown.
    fn default_rect(&self, index: usize) -> [f32; 4] {
        let step = 28.0;
        [
            24.0 + step * (index % 6) as f32,
            44.0 + step * index as f32,
            PANEL_W,
            PANEL_H,
        ]
    }

    /// Write the layout if it changed and the debounce interval has passed.
    pub fn save_if_due(&mut self, now_ns: u64) {
        if !self.dirty || now_ns.saturating_sub(self.last_save_ns) < SAVE_INTERVAL_NS {
            return;
        }
        self.save_now(now_ns);
    }

    /// Write the layout now.
    pub fn save_now(&mut self, now_ns: u64) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let bytes = self.encode();
        // A layout that cannot be written is not worth failing a console over;
        // the operator keeps working from the remembered in-memory state.
        if std::fs::write(&path, bytes).is_ok() {
            self.dirty = false;
            self.last_save_ns = now_ns;
        }
    }

    /// Encode the layout as the fixed-width binary the file holds.
    fn encode(&self) -> Vec<u8> {
        let mut runners: Vec<(&String, bool)> = self
            .open
            .iter()
            .map(|(runner, open)| (runner, *open))
            .collect();
        for runner in self.rects.keys() {
            if !self.open.contains_key(runner) {
                runners.push((runner, self.is_open(runner)));
            }
        }
        runners.sort_by(|left, right| left.0.cmp(right.0));

        let mut bytes = Vec::with_capacity(HEADER_BYTES + runners.len() * ENTRY_BYTES);
        bytes.extend_from_slice(&LAYOUT_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&LAYOUT_VERSION.to_le_bytes());
        bytes.push(u8::from(self.on));
        bytes.push(0);
        bytes.extend_from_slice(&(runners.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&[0u8; 2]);
        for (runner, open) in runners {
            let mut name = [0u8; NAME_BYTES];
            qualia_types::write_fixed(&mut name, runner);
            bytes.extend_from_slice(&name);
            bytes.push(u8::from(open));
            bytes.extend_from_slice(&[0u8; 3]);
            let rect = self.rects.get(runner).copied().unwrap_or_default();
            for value in rect {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    /// Decode a layout file, leaving the defaults where the file says nothing.
    fn decode(&mut self, bytes: &[u8]) {
        if bytes.len() < HEADER_BYTES {
            return;
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if magic != LAYOUT_MAGIC || version != LAYOUT_VERSION {
            return;
        }
        self.on = bytes[6] != 0;
        let count = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        for index in 0..count {
            let start = HEADER_BYTES + index * ENTRY_BYTES;
            let Some(entry) = bytes.get(start..start + ENTRY_BYTES) else {
                return;
            };
            let runner = qualia_types::read_fixed(&entry[..NAME_BYTES]);
            if runner.is_empty() {
                continue;
            }
            let open = entry[NAME_BYTES] != 0;
            let mut rect = [0.0f32; 4];
            for (slot, value) in rect.iter_mut().enumerate() {
                let at = NAME_BYTES + 4 + slot * 4;
                *value = f32::from_le_bytes([
                    entry[at],
                    entry[at + 1],
                    entry[at + 2],
                    entry[at + 3],
                ]);
            }
            self.open.insert(runner.clone(), open);
            if rect[2] > 0.0 && rect[3] > 0.0 {
                self.rects.insert(runner, rect);
            }
        }
        self.dirty = false;
    }
}

/// The layout file: `QUALIA_CONSOLE_LAYOUT` when set, otherwise the per-user
/// state directory, falling back to the working directory.
fn layout_path() -> PathBuf {
    if let Some(path) = std::env::var_os(LAYOUT_ENV).filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("qualia").join("console-layout.bin")
}

/// Draw every open HUD panel for the runners this poll found.
pub fn render(ctx: &egui::Context, state: &mut ConsoleState) {
    if !state.hud_layout.on {
        return;
    }
    // Disjoint field borrows: the frames, the history and the layout are three
    // parts of one state and never alias.
    let ConsoleState {
        hud,
        hud_history,
        hud_layout,
        observed_at_ns,
        ..
    } = state;

    for (index, row) in hud.rows.iter().enumerate() {
        let runner = row.runner.clone();
        let mut open = hud_layout.is_open(&runner);
        let rect = hud_layout.rect(&runner).unwrap_or_else(|| hud_layout.default_rect(index));
        let window = egui::Window::new(runner.as_str())
            .id(egui::Id::new(("qualia_hud_panel", runner.as_str())))
            .default_pos(egui::pos2(rect[0], rect[1]))
            .default_size(egui::vec2(rect[2], rect[3]))
            .resizable(true)
            .collapsible(true)
            .movable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                draw_panel(ui, row, hud_history.get(&runner), *observed_at_ns);
            });
        if let Some(shown) = window {
            hud_layout.record_rect(&runner, shown.response.rect);
        }
        if !open {
            hud_layout.set(&runner, false);
        }
    }
}

/// One panel's body: the state word, the numbers, the rate bar and sparkline,
/// then the values the runner last emitted.
fn draw_panel(
    ui: &mut egui::Ui,
    row: &StatsRow,
    history: Option<&VecDeque<f32>>,
    observed_at_ns: u64,
) {
    let age_ns = observed_at_ns.saturating_sub(row.published_at_ns);
    let state_word = if !row.publishing {
        "no frame"
    } else if age_ns > STALE_NS {
        "stale"
    } else {
        "live"
    };
    let state_color = if row.errors > 0 {
        theme::FAIL
    } else if age_ns > STALE_NS || !row.publishing {
        theme::WARN
    } else {
        theme::ACCENT
    };
    ui.horizontal(|ui| {
        theme::status_pill(ui, state_word, state_color);
        ui.label(theme::text(
            &format!("{:.2} Hz", row.rate_hz),
            theme::SIZE_BODY,
            theme::TEXT,
        ));
    });
    ui.add_space(theme::GAP_S);

    theme::field(ui, "rate", &format!("{:.2}", row.rate_hz), Some("Hz"));
    theme::field(ui, "byte rate", &bytes_per_sec(row.bytes_per_sec), None);
    theme::field(ui, "backlog", &row.backlog.to_string(), None);
    theme::field(ui, "ticks", &row.ticks.to_string(), None);
    theme::field(ui, "errors", &row.errors.to_string(), None);
    theme::field(
        ui,
        "updated",
        &theme::age(observed_at_ns, row.published_at_ns),
        None,
    );

    ui.add_space(theme::GAP_S);
    let ceiling = history
        .map(|series| series.iter().copied().fold(0.0f32, f32::max))
        .unwrap_or(0.0)
        .max(row.rate_hz as f32)
        .max(1.0);
    rate_bar(ui, row.rate_hz as f32, ceiling);
    sparkline(ui, history, ceiling);

    if !row.values.is_empty() {
        ui.add_space(theme::GAP_S);
        theme::hairline(ui);
        for (label, value) in &row.values {
            theme::field(ui, label, &format!("{value:.3}"), None);
        }
    }
}

/// A bar whose fill is the runner's current rate against the busiest sample in
/// the window the panel is showing.
fn rate_bar(ui: &mut egui::Ui, rate: f32, ceiling: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::SEP);
    let fill = (rate / ceiling).clamp(0.0, 1.0);
    let filled = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(rect.width() * fill, rect.height()),
    );
    ui.painter().rect_filled(filled, 0.0, theme::ACCENT);
}

/// The rate history as a polyline from `0` to `ceiling`.
fn sparkline(ui: &mut egui::Ui, history: Option<&std::collections::VecDeque<f32>>, ceiling: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::PANEL);
    let Some(series) = history.filter(|series| series.len() >= 2) else {
        return;
    };
    let span = (series.len() - 1) as f32;
    let points: Vec<egui::Pos2> = series
        .iter()
        .enumerate()
        .map(|(index, rate)| {
            let x = rect.left() + rect.width() * (index as f32 / span);
            let y = rect.bottom() - rect.height() * (rate / ceiling).clamp(0.0, 1.0);
            egui::pos2(x, y)
        })
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.0, theme::ACCENT)));
}

/// A byte rate an operator can read at a glance.
fn bytes_per_sec(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB/s", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.2} MiB/s", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.2} KiB/s", bytes / KIB)
    } else {
        format!("{bytes:.0} B/s")
    }
}
