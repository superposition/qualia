//! Belief view: the `BeliefSlot` ABI, read through `qualia-shm`.
//!
//! Traceability (`docs/frontend-lessons.md`):
//!
//! - source 1 — render the ABI straight out of shared memory (sequence-number
//!   staleness detection included), and the explicit "no `unsafe` in the
//!   console" decision: the pointer arithmetic lives in `qualia-shm`'s
//!   [`LayerReader`](qualia_shm::LayerReader), never here;
//! - source 3 — stale is a named state, not a colour.

use egui::Ui;
use qualia_shm::LayerReader;
use qualia_types::{BeliefSlot, NUM_LAYERS};

use crate::{theme, ConsoleState};

/// A belief layer that has not been republished for this long is stale.
///
/// The stack's pacing rule (Step 21) lets navigation degrade past
/// `QUALIA_BELIEF_PACE_MS * 8` (2 s at the 250 ms default); the console draws
/// the same line the runner does.
pub const BELIEF_STALE_NS: u64 = 2_000_000_000;

/// Column widths of the layer table, on the 8 px rhythm and sized to the
/// panel's default width.
const COL_LAYER: f32 = 72.0;
const COL_VFE: f32 = 80.0;
const COL_RESIDUAL: f32 = 88.0;
const COL_COMPRESSION: f32 = 96.0;
const COL_AGE: f32 = 96.0;
const COL_STATE: f32 = 72.0;

/// One coherent layer reading, copied out of the region so the view is pure.
#[derive(Debug, Clone, PartialEq)]
pub struct BeliefReading {
    pub layer: u8,
    pub vfe: f32,
    pub residual_norm: f32,
    pub compression: u8,
    pub cycle_us: u32,
    pub timestamp_ns: u64,
}

impl BeliefReading {
    pub fn from_slot(slot: &BeliefSlot) -> Self {
        Self {
            layer: slot.layer,
            vfe: slot.vfe,
            residual_norm: slot.residual.iter().map(|x| x * x).sum::<f32>().sqrt(),
            compression: slot.compression,
            cycle_us: slot.cycle_us,
            timestamp_ns: slot.timestamp_ns,
        }
    }

    /// A layer the stack has never written is not stale, it is absent.
    pub fn is_written(&self) -> bool {
        self.timestamp_ns != 0
    }

    pub fn is_stale(&self, observed_at_ns: u64) -> bool {
        self.is_written() && observed_at_ns.saturating_sub(self.timestamp_ns) > BELIEF_STALE_NS
    }

    pub fn status(&self, observed_at_ns: u64) -> &'static str {
        if !self.is_written() {
            "never"
        } else if self.is_stale(observed_at_ns) {
            "stale"
        } else {
            "live"
        }
    }

    /// The colour the named state carries: live, degraded, or absent.
    fn state_color(&self, observed_at_ns: u64) -> egui::Color32 {
        match self.status(observed_at_ns) {
            "live" => theme::ACCENT,
            "stale" => theme::WARN,
            _ => theme::MUTED,
        }
    }
}

/// Everything the belief panel can say without the region.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeliefView {
    /// The region name from `QUALIA_SHM_NAME`, or `None` when unset.
    pub region: Option<String>,
    pub readings: Vec<BeliefReading>,
    pub error: Option<String>,
}

impl BeliefView {
    /// The region could not be attached; every layer is unavailable.
    pub fn unattached(region: Option<String>, error: impl Into<String>) -> Self {
        Self {
            region,
            readings: Vec::new(),
            error: Some(error.into()),
        }
    }

    /// Copy every layer slot out of an attached region.
    pub fn sample(region: &qualia_shm::ShmRegion) -> Self {
        let readings = (0..NUM_LAYERS)
            .map(|layer| BeliefReading::from_slot(LayerReader::new(region.layer_slot(layer)).read()))
            .collect();
        Self {
            region: None,
            readings,
            error: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.readings.is_empty()
    }
}

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    let view = &state.belief;

    if let Some(error) = &view.error {
        theme::error_banner(ui, &format!("belief source error: {error}"));
    }

    theme::field_path(
        ui,
        "shm region",
        view.region.as_deref().unwrap_or(theme::DASH),
    );

    if view.is_empty() {
        theme::state_line(ui, "no belief slots sampled", theme::TEXT_SECOND);
        theme::absent(
            ui,
            &format!("expected {NUM_LAYERS} layers from the region named by QUALIA_SHM_NAME"),
        );
        return;
    }

    ui.add_space(theme::GAP_S);
    theme::header(
        ui,
        &[
            ("layer", COL_LAYER, false),
            ("vfe", COL_VFE, true),
            ("residual", COL_RESIDUAL, true),
            ("compression", COL_COMPRESSION, true),
            ("age", COL_AGE, true),
            ("state", COL_STATE, false),
        ],
    );

    for reading in &view.readings {
        let layer = format!("layer {}", reading.layer);
        let state_word = reading.status(state.observed_at_ns).to_owned();
        let color = reading.state_color(state.observed_at_ns);
        let (vfe, residual, compression, age) = if reading.is_written() {
            (
                format!("{:.4}", reading.vfe),
                format!("{:.4}", reading.residual_norm),
                reading.compression.to_string(),
                theme::age(state.observed_at_ns, reading.timestamp_ns),
            )
        } else {
            (
                theme::DASH.to_owned(),
                theme::DASH.to_owned(),
                theme::DASH.to_owned(),
                theme::DASH.to_owned(),
            )
        };
        let absent = !reading.is_written();
        let value_color = if absent { theme::MUTED } else { theme::TEXT };
        theme::cells(
            ui,
            &[
                (layer.as_str(), COL_LAYER, value_color, false),
                (vfe.as_str(), COL_VFE, value_color, true),
                (residual.as_str(), COL_RESIDUAL, value_color, true),
                (compression.as_str(), COL_COMPRESSION, value_color, true),
                (age.as_str(), COL_AGE, value_color, true),
                (state_word.as_str(), COL_STATE, color, false),
            ],
        );
    }
}
