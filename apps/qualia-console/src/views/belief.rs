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

use crate::{format_age_ms, ConsoleState};

/// A belief layer that has not been republished for this long is stale.
///
/// The stack's pacing rule (Step 21) lets navigation degrade past
/// `QUALIA_BELIEF_PACE_MS * 8` (2 s at the 250 ms default); the console draws
/// the same line the runner does.
pub const BELIEF_STALE_NS: u64 = 2_000_000_000;

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
    ui.heading("Belief layers");

    match &view.region {
        Some(name) => {
            ui.label(format!("shm region: {name}"));
        }
        None => {
            ui.label("shm region: unattached");
        }
    }

    if let Some(error) = &view.error {
        ui.colored_label(
            egui::Color32::from_rgb(224, 160, 138),
            format!("belief source error: {error}"),
        );
    }

    if view.is_empty() {
        ui.label("no belief slots sampled");
        ui.separator();
        ui.label(format!(
            "expected {NUM_LAYERS} layers from the region named by QUALIA_SHM_NAME"
        ));
        return;
    }

    egui::Grid::new("belief_layers")
        .num_columns(6)
        .striped(true)
        .show(ui, |ui| {
            ui.label("layer");
            ui.label("vfe");
            ui.label("residual");
            ui.label("compression");
            ui.label("age");
            ui.label("state");
            ui.end_row();

            for reading in &view.readings {
                ui.label(format!("layer {}", reading.layer));
                if reading.is_written() {
                    ui.label(format!("{:.4}", reading.vfe));
                    ui.label(format!("{:.4}", reading.residual_norm));
                    ui.label(format!("{}", reading.compression));
                    ui.label(format!(
                        "{} ms",
                        format_age_ms(state.observed_at_ns, reading.timestamp_ns)
                    ));
                } else {
                    ui.label("—");
                    ui.label("—");
                    ui.label("—");
                    ui.label("—");
                }
                ui.label(reading.status(state.observed_at_ns));
                ui.end_row();
            }
        });
}
