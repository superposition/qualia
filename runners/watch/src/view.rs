//! The panel table and the view state machine.
//!
//! The tab bar, the digit keys and the layout dispatch all read
//! [`VIEW_LABELS`]. Adding a panel is therefore four edits in two files: a
//! [`ViewMode`] variant and a [`VIEW_LABELS`] row here, then a `match` arm and
//! a render function in `main.rs`.

use qualia_types::NUM_LAYERS;

/// How many belief layers the engine publishes.
pub const LAYER_COUNT: usize = NUM_LAYERS;

/// One panel of the operator view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// Layer table plus the ledger event stream.
    Overview,
    /// One layer's scalars and all `STATE_DIM` dimensions.
    Detail,
    /// A typed byte map of one layer's belief slot.
    Hex,
    /// VFE history for all layers.
    Sparklines,
    /// Absolute residual heatmap across layers and dimensions.
    Residuals,
    /// One layer's generative weight matrix.
    Weights,
    /// World model and the thought stream.
    World,
    /// The braid the agent reports on `GET /braid`.
    Mission,
}

/// The ordered panel table.
///
/// Position `i` is selected by digit `i + 1`, and the label is what the tab bar
/// renders. Nothing else may define a panel's key or its label.
pub const VIEW_LABELS: &[(&str, ViewMode)] = &[
    ("1:Overview", ViewMode::Overview),
    ("2:Detail", ViewMode::Detail),
    ("3:Hex", ViewMode::Hex),
    ("4:Spark", ViewMode::Sparklines),
    ("5:Residual", ViewMode::Residuals),
    ("6:Weights", ViewMode::Weights),
    ("7:World", ViewMode::World),
    ("8:Mission", ViewMode::Mission),
];

/// Where `view` sits in [`VIEW_LABELS`].
pub fn view_index(view: ViewMode) -> Option<usize> {
    VIEW_LABELS.iter().position(|(_, candidate)| *candidate == view)
}

/// The digit key that selects `view`, derived from the table.
pub fn digit_for_view(view: ViewMode) -> Option<char> {
    let index = view_index(view)?;
    char::from_digit(index as u32 + 1, 10)
}

/// The panel a digit key selects, or `None` if the key is unbound.
pub fn view_for_digit(digit: char) -> Option<ViewMode> {
    let index = digit.to_digit(10)? as usize;
    if index == 0 {
        return None;
    }
    VIEW_LABELS.get(index - 1).map(|(_, view)| *view)
}

/// The tab-bar label for a panel.
pub fn label_for(view: ViewMode) -> &'static str {
    view_index(view)
        .map(|index| VIEW_LABELS[index].0)
        .unwrap_or("?")
}

/// Which panel is showing and which layer it is looking at.
#[derive(Clone, Copy, Debug)]
pub struct ViewState {
    view: ViewMode,
    selected_layer: usize,
    detail_scroll: u16,
    hex_scroll: u16,
}

impl Default for ViewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            view: VIEW_LABELS[0].1,
            selected_layer: 0,
            detail_scroll: 0,
            hex_scroll: 0,
        }
    }

    pub fn view(&self) -> ViewMode {
        self.view
    }

    pub fn layer(&self) -> usize {
        self.selected_layer
    }

    pub fn detail_scroll(&self) -> u16 {
        self.detail_scroll
    }

    pub fn hex_scroll(&self) -> u16 {
        self.hex_scroll
    }

    /// Show `view`, resetting the scroll of the panel being left and entered.
    pub fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        self.reset_scroll();
    }

    /// Tab: the next panel in [`VIEW_LABELS`], wrapping.
    pub fn next_view(&mut self) {
        let index = view_index(self.view).unwrap_or(0);
        self.set_view(VIEW_LABELS[(index + 1) % VIEW_LABELS.len()].1);
    }

    /// Back-tab: the previous panel in [`VIEW_LABELS`], wrapping.
    pub fn prev_view(&mut self) {
        let index = view_index(self.view).unwrap_or(0);
        self.set_view(VIEW_LABELS[(index + VIEW_LABELS.len() - 1) % VIEW_LABELS.len()].1);
    }

    /// Jump to `layer`, clamped to the published range.
    pub fn select_layer(&mut self, layer: usize) {
        self.selected_layer = layer.min(LAYER_COUNT.saturating_sub(1));
        self.reset_scroll();
    }

    /// Move the layer selection by `delta`, clamped at both ends.
    pub fn move_layer(&mut self, delta: i32) {
        let target = (self.selected_layer as i32 + delta).max(0) as usize;
        self.select_layer(target);
    }

    /// Scroll the current panel by `delta` lines, clamped at zero.
    ///
    /// The Hex and Weights panels share the second offset because they are the
    /// two long dumps; Detail keeps its own.
    pub fn scroll(&mut self, delta: i32) {
        match self.view {
            ViewMode::Detail => self.detail_scroll = clamp_scroll(self.detail_scroll, delta),
            ViewMode::Hex | ViewMode::Weights => {
                self.hex_scroll = clamp_scroll(self.hex_scroll, delta)
            }
            _ => {}
        }
    }

    /// Reset every panel's scroll offset.
    pub fn reset_scroll(&mut self) {
        self.detail_scroll = 0;
        self.hex_scroll = 0;
    }
}

fn clamp_scroll(current: u16, delta: i32) -> u16 {
    (current as i32 + delta).clamp(0, u16::MAX as i32) as u16
}

/// The first row a scrolling panel shows.
///
/// The offset is parked on the last page — `total_rows - visible_rows` — so a
/// PgDn past the end of the content keeps the tail visible instead of blanking
/// the panel. Content shorter than the panel always starts at row zero.
pub fn first_visible_row(scroll: u16, total_rows: usize, visible_rows: usize) -> usize {
    (scroll as usize).min(total_rows.saturating_sub(visible_rows))
}

/// One engine timestamp as `HH:MM:SS.mmm` — the clock the ledger rows and the
/// Mission panel's promotion and quarantine stamps are read with.
pub fn format_clock(ns: u64) -> String {
    let millis = (ns / 1_000_000) % 1000;
    let secs = (ns / 1_000_000_000) % 60;
    let mins = (ns / 60_000_000_000) % 60;
    let hours = (ns / 3_600_000_000_000) % 24;
    format!("{hours:02}:{mins:02}:{secs:02}.{millis:03}")
}
