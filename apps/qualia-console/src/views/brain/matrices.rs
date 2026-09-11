//! The belief matrices: per-layer generative weights and belief, as heatmaps
//! with a short time axis.
//!
//! The ABI holds a 1024×1024 generative weight matrix and a 1024-element belief
//! mean per layer. A heatmap does not need every element, so each is decimated
//! onto a 32×32 grid — one sample per 32×32 block, the block's top-left element
//! — which is enough for a change to be *visible* and cheap enough to sample
//! every poll without touching the runners. The time axis is the console's own
//! short history of the layer's residual and weight scale, with the braid's
//! promotion markers drawn on it; `docs/frontend-lessons.md`'s rule is kept —
//! the axis is presentation, not measurement, and no journal number comes from
//! it.

use egui::{Rect, Sense, Ui, Vec2};
use qualia_shm::ShmRegion;
use qualia_types::{BeliefSlot, STATE_DIM};

use crate::theme;

/// Heatmap cells per axis.
pub const MATRIX_DIM: usize = 32;
/// Cells in one decimated matrix.
pub const MATRIX_CELLS: usize = MATRIX_DIM * MATRIX_DIM;
/// Elements between decimated samples (`STATE_DIM / MATRIX_DIM`).
pub const STATE_STRIDE: usize = STATE_DIM / MATRIX_DIM;
/// The short history the time axis draws: frames, not seconds.
pub const HISTORY_LEN: usize = 64;
/// One heatmap's side, in pixels.
const HEATMAP_SIDE: f32 = 116.0;

/// One layer's decimated weight matrix and belief vector.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixReading {
    pub layer: u8,
    pub weight: Vec<f32>,
    pub belief: Vec<f32>,
    pub vfe: f32,
    pub residual_norm: f32,
    pub timestamp_ns: u64,
}

impl MatrixReading {
    /// Sample `layer` out of an attached region.
    ///
    /// The belief comes from the active slot's `mean`; the weight matrix is read
    /// decimated, so a poll copies thousands of floats rather than a megabyte
    /// per layer.
    pub fn sample(region: &ShmRegion, layer: usize) -> Self {
        let reader = qualia_shm::LayerReader::new(region.layer_slot(layer));
        let slot: &BeliefSlot = reader.read();
        let weights = &region.layer_slot(layer).weights;
        let mut weight = Vec::with_capacity(MATRIX_CELLS);
        let mut belief = Vec::with_capacity(MATRIX_CELLS);
        for row in 0..MATRIX_DIM {
            for column in 0..MATRIX_DIM {
                weight.push(weights[(row * STATE_STRIDE) * STATE_DIM + column * STATE_STRIDE]);
                // The belief vector is STATE_DIM long, i.e. already one value
                // per cell of the decimated grid.
                belief.push(slot.mean[row * MATRIX_DIM + column]);
            }
        }
        Self {
            layer: layer as u8,
            weight,
            belief,
            vfe: slot.vfe,
            residual_norm: slot
                .residual
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt(),
            timestamp_ns: slot.timestamp_ns,
        }
    }

    pub fn is_written(&self) -> bool {
        self.timestamp_ns != 0
    }

    /// The largest decimated weight magnitude, the heatmap's own scale.
    pub fn weight_scale(&self) -> f32 {
        self.weight
            .iter()
            .fold(0.0_f32, |scale, value| scale.max(value.abs()))
            .max(f32::EPSILON)
    }

    /// The largest decimated belief magnitude, the heatmap's own scale.
    pub fn belief_scale(&self) -> f32 {
        self.belief
            .iter()
            .fold(0.0_f32, |scale, value| scale.max(value.abs()))
            .max(f32::EPSILON)
    }
}

/// One point on the time axis: a layer's summary at one poll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerPoint {
    pub layer: u8,
    pub timestamp_ns: u64,
    pub residual_norm: f32,
    pub weight_scale: f32,
}

impl LayerPoint {
    pub fn from_reading(reading: &MatrixReading) -> Self {
        Self {
            layer: reading.layer,
            timestamp_ns: reading.timestamp_ns,
            residual_norm: reading.residual_norm,
            weight_scale: reading.weight_scale(),
        }
    }
}

/// Draw the selected layer's two heatmaps and the time axis below them.
pub fn render(
    ui: &mut Ui,
    readings: &[MatrixReading],
    history: &[LayerPoint],
    markers: &[super::TimelineMarker],
    selected: u8,
) {
    theme::state_line(ui, "belief matrices", theme::TEXT_SECOND);
    if readings.is_empty() {
        theme::absent(ui, "no layer slots sampled");
        return;
    }
    let Some(reading) = readings.iter().find(|reading| reading.layer == selected) else {
        theme::absent(ui, "the selected layer has no slot");
        return;
    };

    ui.horizontal(|ui| {
        heatmap(
            ui,
            "weight matrix",
            &reading.weight,
            reading.weight_scale(),
        );
        ui.add_space(theme::GAP_S);
        heatmap(ui, "belief vector", &reading.belief, reading.belief_scale());
    });

    if reading.is_written() {
        theme::field(ui, "layer vfe", &format!("{:.4}", reading.vfe), None);
        theme::field(
            ui,
            "layer residual",
            &format!("{:.4}", reading.residual_norm),
            None,
        );
    } else {
        theme::state_line(ui, "layer never written", theme::MUTED);
    }

    let selected_history: Vec<LayerPoint> = history
        .iter()
        .filter(|point| point.layer == selected)
        .copied()
        .collect();
    time_axis(ui, &selected_history, markers);
}

/// One decimated matrix as a heatmap, in the accent ramp; a negative entry is
/// drawn in the failure hue so a sign change is visible, not averaged away.
fn heatmap(ui: &mut Ui, label: &str, values: &[f32], scale: f32) {
    ui.vertical(|ui| {
        theme::state_line(ui, label, theme::TEXT_SECOND);
        let side = HEATMAP_SIDE;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::BG);
        if values.len() != MATRIX_CELLS {
            return;
        }
        let cell = side / MATRIX_DIM as f32;
        for row in 0..MATRIX_DIM {
            for column in 0..MATRIX_DIM {
                let value = values[row * MATRIX_DIM + column];
                let intensity = (value.abs() / scale).clamp(0.0, 1.0);
                let colour = if value < 0.0 {
                    theme::FAIL.gamma_multiply(0.15 + 0.85 * intensity)
                } else {
                    theme::ramp(intensity)
                };
                let cell_rect = Rect::from_min_size(
                    egui::pos2(rect.left() + column as f32 * cell, rect.top() + row as f32 * cell),
                    Vec2::splat(cell.max(1.0)),
                );
                painter.rect_filled(cell_rect, 0.0, colour);
            }
        }
    });
}

/// The short time axis under the matrices: one column per poll, the layer's
/// residual brightness, and a marker line per braid event the console saw.
fn time_axis(ui: &mut Ui, history: &[LayerPoint], markers: &[super::TimelineMarker]) {
    ui.add_space(theme::GAP_S);
    theme::state_line(ui, "time axis", theme::TEXT_SECOND);
    let points: Vec<&LayerPoint> = history.iter().collect();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(HEATMAP_SIDE * 2.0 + theme::GAP_S, 40.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG);

    if points.is_empty() {
        theme::absent(ui, "no frames yet");
        return;
    }
    let peak = points
        .iter()
        .fold(0.0_f32, |peak, point| peak.max(point.residual_norm))
        .max(f32::EPSILON);
    let column = rect.width() / HISTORY_LEN as f32;
    let offset = HISTORY_LEN.saturating_sub(points.len());
    for (index, point) in points.iter().enumerate() {
        let intensity = (point.residual_norm / peak).clamp(0.0, 1.0);
        let x = rect.left() + (offset + index) as f32 * column;
        let height = rect.height() * (0.15 + 0.85 * intensity);
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(x, rect.bottom() - height),
                egui::pos2(x + column.max(1.0), rect.bottom()),
            ),
            0.0,
            theme::ramp(intensity),
        );
    }

    let first = points.first().map(|point| point.timestamp_ns).unwrap_or(0);
    let last = points.last().map(|point| point.timestamp_ns).unwrap_or(first);
    let span = last.saturating_sub(first).max(1);
    for marker in markers {
        if marker.timestamp_ns < first || marker.timestamp_ns > last {
            continue;
        }
        let x = rect.left() + rect.width() * (marker.timestamp_ns - first) as f32 / span as f32;
        let colour = marker.colour();
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, colour),
        );
    }
    theme::state_line(
        ui,
        &format!("{} frames, {} markers", points.len(), markers.len()),
        theme::MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_region_leaves_every_layer_unwritten() {
        let name = format!("/qualia_console_brain_matrix_{}", std::process::id());
        let region = ShmRegion::create(&name).expect("create region");
        let reading = MatrixReading::sample(&region, 0);
        assert!(!reading.is_written());
        assert_eq!(reading.weight.len(), MATRIX_CELLS);
        assert_eq!(reading.belief.len(), MATRIX_CELLS);
        assert_eq!(reading.weight, vec![0.0; MATRIX_CELLS]);
    }

    #[test]
    fn the_time_axis_point_carries_the_layer_summary() {
        let reading = MatrixReading {
            layer: 3,
            weight: vec![2.0; MATRIX_CELLS],
            belief: vec![0.5; MATRIX_CELLS],
            vfe: 0.25,
            residual_norm: 1.5,
            timestamp_ns: 42,
        };
        let point = LayerPoint::from_reading(&reading);
        assert_eq!(point.layer, 3);
        assert_eq!(point.timestamp_ns, 42);
        assert_eq!(point.residual_norm, 1.5);
        assert_eq!(point.weight_scale, 2.0);
    }
}
