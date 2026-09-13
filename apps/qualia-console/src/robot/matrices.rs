//! Frozen effective connectivity and the model's actual last Euler substep.
use std::collections::HashSet;
use egui::{Color32, Ui};
use serde_json::Value;
use super::{panels, pretrained, source::{now_ms, Data}};

fn finite(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite()) }
fn decimal(value: &Value) -> String { finite(value).map(|v| format!("{v:.4}")).unwrap_or_else(|| "unavailable".into()) }

pub fn valid_status(value: &Value) -> bool {
    if value["schema"] != "qualia.flyvis-matrices.v1" || !pretrained::valid_envelope(value)
        || value["cell_type"].as_str().is_none_or(|v| v.is_empty() || v.len() > 128)
        || value["recurrence"]["method"] != "explicit_euler"
        || value["recurrence"]["optimizer_present"] != false || value["recurrence"]["deepseek_updates_model"] != false
        || !value["recurrence"]["beliefs"].is_null() || !value["recurrence"]["probabilities"].is_null()
        || finite(&value["recurrence"]["dt_s"]).is_none_or(|v| (v - 0.02).abs() > 1e-8) { return false; }
    let Some(cells) = value["cells"].as_array().filter(|v| !v.is_empty() && v.len() <= 721) else { return false; };
    let mut ids = HashSet::new();
    if !cells.iter().all(|cell| {
        cell["model_index"].as_u64().is_some_and(|v| v <= u32::MAX as u64 && ids.insert(v))
            && ["u", "v"].iter().all(|key| cell[key].as_i64().is_some_and(|v| (-1000..=1000).contains(&v)))
            && ["before_voltage", "input_drive", "recurrent_drive", "bias", "tau_s", "alpha", "after_voltage", "delta_voltage"].iter().all(|key| finite(&cell[key]).is_some())
            && finite(&cell["tau_s"]).is_some_and(|v| v > 0.0) && finite(&cell["alpha"]).is_some_and(|v| v > 0.0 && v <= 1.00001)
    }) { return false; }
    let matrix = &value["weight_matrix"];
    if matrix["aggregation"] != "mean signed effective edge weight" || matrix["axis_order"] != "target_rows_source_columns" { return false; }
    let Some(types) = matrix["cell_types"].as_array().filter(|v| !v.is_empty() && v.len() <= 128) else { return false; };
    let mut names = HashSet::new();
    if !types.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128 && names.insert(s))) { return false; }
    let side = types.len();
    let Some(means) = matrix["mean_signed"].as_array().filter(|v| v.len() == side) else { return false; };
    let Some(counts) = matrix["edge_counts"].as_array().filter(|v| v.len() == side) else { return false; };
    means.iter().zip(counts).all(|(means, counts)| {
        let Some(means) = means.as_array().filter(|v| v.len() == side) else { return false; };
        let Some(counts) = counts.as_array().filter(|v| v.len() == side) else { return false; };
        means.iter().zip(counts).all(|(mean, count)| count.as_u64().is_some_and(|n| if n == 0 { mean.is_null() } else { finite(mean).is_some() }))
    })
}

pub struct MatrixView {
    mode: u8,
    cell: usize,
    field: &'static str,
    host_layer: u64,
    host_field: &'static str,
}
impl Default for MatrixView {
    fn default() -> Self { Self { mode: 0, cell: 0, field: "after_voltage", host_layer: 3, host_field: "weight_rms" } }
}
impl MatrixView {
    pub fn render(&mut self, ui: &mut Ui, data: &Data) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, 0, "Weights");
            ui.selectable_value(&mut self.mode, 1, "Recurrence");
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, 3, "Robot L2");
            ui.selectable_value(&mut self.mode, 2, "Host beliefs");
        });
        if self.mode == 2 { self.host(ui, data); return; }
        if self.mode == 3 { self.robot(ui, data); return; }
        ui.small("Frozen weights; no optimizer or DeepSeek updates.");
        let Some(reading) = data.sources.get("model-matrices") else { ui.label("Waiting for model matrix evidence"); return; };
        let value = &reading.value;
        if !valid_status(value) || value["cell_type"].as_str() != Some(data.neuron_type.as_str()) {
            ui.colored_label(Color32::YELLOW, "Matrix source unavailable or contract invalid");
            if let Some(error) = &reading.error { ui.small(error); } return;
        }
        let now = now_ms();
        let same_model = data.sources.get("pretrained").is_some_and(|summary| {
            value["run_id"] == summary.value["run_id"] && value["model"]["checkpoint_sha256"] == summary.value["model"]["checkpoint_sha256"]
                && value["model"]["export_sha256"] == summary.value["model"]["export_sha256"]
        });
        let live = reading.fresh_at(now, 1500) && pretrained::fresh_link(value, now) && same_model;
        ui.colored_label(if live {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!("{} · {} · tick {}",
            if live {"Live substep"} else {"Historical"}, panels::text(&value["cell_type"]), panels::text(&value["tick"])));
        let cells = value["cells"].as_array().expect("validated cells");
        self.cell = self.cell.min(cells.len() - 1);
        if self.mode == 0 { self.weights(ui, value, live); }
        else {
            let cell = &cells[self.cell];
            ui.label(format!("Cell {}: {:.6} → {:.6}", panels::text(&cell["model_index"]), finite(&cell["before_voltage"]).unwrap(), finite(&cell["after_voltage"]).unwrap()));
            ui.label(format!("Δ {:.3e} · tau {} s", finite(&cell["delta_voltage"]).unwrap(), decimal(&cell["tau_s"])));
            ui.label(format!("alpha {} · dt {} s", decimal(&cell["alpha"]), decimal(&value["recurrence"]["dt_s"])));
            self.recurrence(ui, value, live);
        }
        egui::CollapsingHeader::new("Model and source evidence").show(ui, |ui| {
            ui.small("alpha = dt / max(tau, dt). Last real substep; voltage is not a probability or belief vector.");
            if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
            for key in ["run_id", "model", "recurrence", "source", "output"] { panels::object(ui, key, &value[key]); }
        });
    }

    fn weights(&self, ui: &mut Ui, value: &Value, live: bool) {
        let matrix = &value["weight_matrix"];
        let types = matrix["cell_types"].as_array().expect("validated types");
        let side = types.len();
        let means: Vec<Option<f64>> = matrix["mean_signed"].as_array().unwrap().iter().flat_map(|row| row.as_array().unwrap().iter().map(finite)).collect();
        ui.small(format!("{side} × {side} signed mean weights · green+ / red−"));
        let limit = (ui.clip_rect().bottom() - ui.cursor().top() - 32.0).clamp(70.0, 205.0);
        let mut hovered = None;
        ui.columns(2, |columns| {
            hovered = heatmap_sized(&mut columns[0], side, &means, live, limit);
            let cell = &value["cells"][self.cell];
            columns[1].small(format!("Cell {}", panels::text(&cell["model_index"])));
            for (label, key) in [("pre", "before_voltage"), ("post", "after_voltage")] {
                columns[1].small(format!("{label} {:.6}", finite(&cell[key]).unwrap()));
            }
            columns[1].small(format!("Δ {:.3e}", finite(&cell["delta_voltage"]).unwrap()));
            columns[1].small(format!("tau {} s", decimal(&cell["tau_s"])));
            columns[1].small(format!("alpha {}", decimal(&cell["alpha"])));
            columns[1].small(format!("dt {} s", decimal(&value["recurrence"]["dt_s"])));
        });
        if let Some(index) = hovered {
            let row = index / side; let col = index % side;
            let label = format!("{} → {} | mean {} | {} edges", panels::text(&types[col]), panels::text(&types[row]), decimal(&matrix["mean_signed"][row][col]), panels::text(&matrix["edge_counts"][row][col]));
            ui.label(label);
        }
        egui::CollapsingHeader::new("Connectivity key").show(ui, |ui| {
            ui.small("Target rows / source columns. Gray means absent edges. Each square is the mean signed effective weight of actual edges between two cell types.");
        });
    }

    fn recurrence(&mut self, ui: &mut Ui, value: &Value, live: bool) {
        let cells = value["cells"].as_array().expect("validated cells");
        egui::ComboBox::from_id_salt("recurrence-field").selected_text(self.field).show_ui(ui, |ui| {
            for field in ["before_voltage", "input_drive", "recurrent_drive", "bias", "tau_s", "alpha", "after_voltage", "delta_voltage"] {
                ui.selectable_value(&mut self.field, field, field);
            }
        });
        ui.add(egui::Slider::new(&mut self.cell, 0..=cells.len()-1).text("Cell row"));
        let values: Vec<_> = cells.iter().map(|cell| finite(&cell[self.field])).collect();
        // Rows are actual model indices in producer order, not a spatial claim.
        let side = (cells.len() as f32).sqrt().ceil() as usize;
        if let Some(index) = heatmap(ui, side, &values, live) { if index < cells.len() {
            ui.label(format!("Model index {} · {} = {}", panels::text(&cells[index]["model_index"]), self.field, decimal(&cells[index][self.field])));
        }}
        ui.small("Row-major model-index display; inspect a cell's real integration terms below.");
        ui.small("Direct camera drive enters R1–R8. T4a receives it through recurrent connections, so its direct input_drive is zero.");
        ui.small("Euler: after = before + alpha × (−before + bias + recurrent + input). Producer retains its float32 operation order.");
        egui::Grid::new("cell-recurrence-terms").striped(true).show(ui, |ui| {
            for field in ["before_voltage", "input_drive", "recurrent_drive", "bias", "alpha", "after_voltage", "delta_voltage"] {
                ui.label(field); ui.label(format!("{:.6e}", finite(&cells[self.cell][field]).unwrap())); ui.end_row();
            }
        });
    }

    fn robot(&self, ui: &mut Ui, data: &Data) {
        ui.strong("Robot L2 cognition boundary");
        ui.small("Actual robot latent state and precision; separate from pretrained visual voltage.");
        let Some(reading) = data.sources.get("cognition") else { ui.label("Waiting for robot cognition"); return; };
        let now = now_ms();
        let value = &reading.value;
        let boundary = &value["boundary"];
        let Some(values) = boundary["latent"].as_array().filter(|v| v.len() == 1024 && v.iter().all(|v| finite(v).is_some())) else {
            ui.colored_label(Color32::YELLOW, "Robot has no valid 1024-component L2 boundary"); return;
        };
        let layer = value["layers"].as_array().and_then(|rows| rows.iter().find(|row| row["layer"] == 2));
        let live = reading.fresh_at(now, 500) && boundary["layer"] == 2
            && boundary["sequence"].as_u64().is_some_and(|v| v > 0)
            && boundary["ts_ms"].as_u64().is_some_and(|v| v > 0 && v <= now.saturating_add(100) && now.saturating_sub(v) <= 500)
            && boundary["expires_at_ms"].as_u64().is_some_and(|v| v > now)
            && layer.is_some_and(|layer| layer["sequence"] == boundary["sequence"] && layer["fresh"] == true
                && layer["source_ts_ms"].as_u64().is_some_and(|v| v > 0 && v <= now.saturating_add(100) && now.saturating_sub(v) <= 1500));
        ui.colored_label(if live {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!("{} · sequence {}", if live {"Current L2 state"} else {"Historical / unavailable L2 state"}, panels::text(&boundary["sequence"])));
        ui.label(format!("Precision {} · backend {}", decimal(&boundary["precision"]), panels::text(&value["capabilities"]["backend"])));
        let values: Vec<_> = values.iter().map(finite).collect();
        if let Some(index) = heatmap(ui, 32, &values, live) { ui.label(format!("Latent component {index} = {:.6}", values[index].unwrap())); }
        ui.small("1024 latent components laid out by index; this is not a spatial map or a probability matrix.");
        if let Some(layer) = layer {
            ui.label(format!("Prediction error {} · source age {} ms", decimal(&layer["prediction_error_l2"]), panels::text(&layer["source_age_ms"])));
        }
        egui::CollapsingHeader::new("Robot belief provenance").show(ui, |ui| {
            if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
            for key in ["source", "destination", "ts_ms", "expires_at_ms", "state_digest"] { panels::object(ui, key, &boundary[key]); }
        });
    }

    fn host(&mut self, ui: &mut Ui, data: &Data) {
        ui.small("Host L3–L6 cognition sketches; separate from the pretrained visual network.");
        let Some(reading) = data.sources.get("host-sketch") else { ui.label("Host belief source unavailable"); return; };
        panels::freshness(ui, reading, 5000);
        ui.horizontal_wrapped(|ui| {
            for layer in 3..=6 { ui.selectable_value(&mut self.host_layer, layer, format!("L{layer}")); }
        });
        egui::ComboBox::from_id_salt("host-matrix-field").selected_text(self.host_field).show_ui(ui, |ui| {
            for field in ["activation", "weight_rms", "weight_delta"] { ui.selectable_value(&mut self.host_field, field, field); }
        });
        let Some(layer) = reading.value["canonical"].as_array().and_then(|rows| rows.iter().find(|r| r["layer"].as_u64() == Some(self.host_layer))) else { ui.label("No published layer"); return; };
        let side = layer["side"].as_u64().unwrap_or(0) as usize;
        let Some(values) = layer[self.host_field].as_array().filter(|v| (1..=128).contains(&side) && v.len() == side * side && v.iter().all(|n| finite(n).is_some())) else { ui.label("Published sketch has no valid matrix shape"); return; };
        let sequence = layer["sequence"].as_u64().unwrap_or(0);
        let stepped = sequence > 0;
        ui.colored_label(if stepped {Color32::WHITE} else {Color32::YELLOW}, format!("Sequence {sequence} · {}", if stepped {"published layer state"} else {"initialized; no model step published"}));
        let current = data.sources.get("host-belief").is_some_and(|belief| belief.fresh_at(now_ms(), 1500)
            && belief.value["layers"].as_array().is_some_and(|rows| rows.iter().any(|r| r["layer"] == layer["layer"] && r["sequence"] == layer["sequence"] && r["fresh"] == true)));
        let values: Vec<_> = values.iter().map(finite).collect();
        heatmap(ui, side, &values, stepped && current && reading.fresh_at(now_ms(), 5000));
        ui.small("Published sketches: weight_rms is a nonnegative RMS summary, not individual signed synapses.");
        ui.label(format!("Precision {} · prediction error {} · learning signal {}", decimal(&layer["precision"]), decimal(&layer["prediction_error_l2"]), decimal(&layer["learning_signal"])));
        egui::CollapsingHeader::new("Host source and persistence").show(ui, |ui| {
            if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
            panels::object(ui, "Persistence", &reading.value["persistence"]);
        });
    }
}

fn heatmap(ui: &mut Ui, side: usize, values: &[Option<f64>], live: bool) -> Option<usize> {
    heatmap_sized(ui, side, values, live, 205.0)
}

fn heatmap_sized(ui: &mut Ui, side: usize, values: &[Option<f64>], live: bool, limit: f32) -> Option<usize> {
    let size = ui.available_width().clamp(40.0, limit);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let cell = size / side as f32;
    let scale = values.iter().flatten().map(|v| v.abs()).fold(0.0, f64::max);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(25));
    for (index, value) in values.iter().enumerate() {
        let color = match value {
            None => Color32::from_gray(35),
            Some(value) => {
                let brightness = if scale > 0.0 { (value.abs()/scale*210.0) as u8 } else { 0 };
                if !live { Color32::from_gray(30 + brightness/2) }
                else if *value >= 0.0 { Color32::from_rgb(20,30+brightness,40+brightness/2) }
                else { Color32::from_rgb(30+brightness,20,40+brightness/2) }
            }
        };
        let pos = rect.min + egui::vec2((index%side) as f32*cell,(index/side) as f32*cell);
        painter.rect_filled(egui::Rect::from_min_size(pos,egui::vec2(cell,cell)),0.0,color);
    }
    ui.small(format!("Linear color scale ±{scale:.5}; no interpolated values."));
    response.hover_pos().and_then(|position| {
        let col = ((position.x-rect.left())/cell).floor() as usize;
        let row = ((position.y-rect.top())/cell).floor() as usize;
        let index = row*side+col;
        (col < side && row < side && index < values.len()).then_some(index)
    })
}
