//! Actual flyvis cells: signed published change by default, with optional voltage.
use std::collections::HashSet;
use egui::{Color32, Stroke, Ui};
use serde_json::Value;
use super::{pretrained, source::{now_ms, Data}};

fn finite(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite()) }

pub fn valid_status(value: &Value) -> bool {
    if value["schema"] != "qualia.flyvis-cells.v1" || !pretrained::valid_envelope(value)
        || !value["cell_type"].as_str().is_some_and(|name| !name.is_empty() && name.len() <= 128) {
        return false;
    }
    let Some(cells) = value["cells"].as_array() else { return false; };
    if cells.is_empty() || cells.len() > 721 { return false; }
    let mut ids = HashSet::with_capacity(cells.len());
    cells.iter().all(|cell| {
        let Some(index) = cell["model_index"].as_u64().filter(|v| *v <= u32::MAX as u64) else { return false; };
        ids.insert(index)
            && ["u", "v"].iter().all(|key| cell[key].as_i64().is_some_and(|v| (-1000..=1000).contains(&v)))
            && finite(&cell["voltage"]).is_some() && finite(&cell["delta_voltage"]).is_some()
    })
}

pub fn valid_retina(value: &Value) -> bool {
    if value["schema"] != "qualia.flyvis-retina.v1" || value["cell_type"] != "R1"
        || value["input_kind"] != "retinal_luminance" || !pretrained::valid_envelope(value) { return false; }
    let Some(cells) = value["cells"].as_array().filter(|v| !v.is_empty() && v.len() <= 721) else { return false; };
    let mut ids = HashSet::new();
    cells.iter().all(|cell| cell["model_index"].as_u64().is_some_and(|v| v <= u32::MAX as u64 && ids.insert(v))
        && ["u", "v"].iter().all(|key| cell[key].as_i64().is_some_and(|v| (-1000..=1000).contains(&v)))
        && finite(&cell["input_drive"]).is_some())
}

pub struct NeuronView {
    pub selected_type: String,
    full_brightness: f64,
    delta_scale: f64,
    changes: bool,
}
impl Default for NeuronView {
    fn default() -> Self { Self { selected_type: "T4a".into(), full_brightness: 3.0, delta_scale: 0.003, changes: true } }
}

impl NeuronView {
    pub fn render(&mut self, ui: &mut Ui, data: &Data, summary: &Value) {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("neuron-type").selected_text(&self.selected_type).show_ui(ui, |ui| {
                if let Some(rows) = summary["output"]["per_type"].as_array() {
                    for row in rows { if let Some(name) = row["cell_type"].as_str() {
                        ui.selectable_value(&mut self.selected_type, name.to_owned(), name);
                    }}
                }
            });
            ui.selectable_value(&mut self.changes, true, "Change per tick");
            ui.selectable_value(&mut self.changes, false, "Voltage");
        });
        let Some(reading) = data.sources.get("neurons") else { ui.label("Waiting for measured cell updates"); return; };
        let value = &reading.value;
        if !valid_status(value) || value["cell_type"].as_str() != Some(self.selected_type.as_str()) {
            ui.label(format!("Waiting for {} cells", self.selected_type)); return;
        }
        let now = now_ms();
        let live = reading.fresh_at(now, 1500) && pretrained::fresh_link(value, now) && same_model(value, summary);
        let cells = value["cells"].as_array().expect("validated cell array");
        let mean_delta = cells.iter().map(|c| finite(&c["delta_voltage"]).unwrap().abs()).sum::<f64>() / cells.len() as f64;
        let peak_delta = cells.iter().map(|c| finite(&c["delta_voltage"]).unwrap().abs()).fold(0.0, f64::max);
        ui.colored_label(if live {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!(
            "{} | {} cells | tick {} | mean |ΔV| {mean_delta:.2e}", if live {"Current response"} else {"Historical response"}, cells.len(), value["tick"]));
        // Reserve room for the real temporal traces below, even inside an
        // unbounded ScrollArea. clip_rect is the actual visible viewport.
        let height = (ui.clip_rect().bottom() - ui.cursor().top() - 190.0).clamp(95.0, 260.0);
        ui.columns(2, |columns| {
            let eye = &mut columns[0];
            eye.strong("Eye input · R1");
            if let Some(retina) = data.sources.get("retina").filter(|r| valid_retina(&r.value)) {
                let input = &retina.value;
                let fresh = retina.fresh_at(now, 1500) && pretrained::fresh_link(input, now) && same_model(input, summary);
                eye.colored_label(if fresh {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!("{} · tick {}", if fresh {"Measured"} else {"Historical"}, input["tick"]));
                paint_cells(eye, input["cells"].as_array().unwrap(), "input_drive", height, fresh, 1.0);
                eye.small("Luminance · black 0 / white 1");
                if input["tick"] == value["tick"] && input["source"]["jpeg_sha256"] == value["source"]["jpeg_sha256"] {
                    eye.small("Same image and tick");
                } else { eye.small("Different ticks; independent samples"); }
            } else {
                eye.colored_label(Color32::YELLOW, "Waiting for exact retinal input");
                if let Some(error) = data.sources.get("retina").and_then(|r| r.error.as_ref()) {
                    egui::CollapsingHeader::new("Eye source").show(eye, |ui| { ui.small(error); });
                }
            }
            let response = &mut columns[1];
            response.strong(if self.changes {"Cell voltage change"} else {"Cell voltage"});
            response.small(format!("{} · tick {}", self.selected_type, value["tick"]));
            paint_cells(response, cells, if self.changes {"delta_voltage"} else {"voltage"}, height, live,
                if self.changes {self.delta_scale} else {self.full_brightness});
            response.small(if self.changes { format!("Cyan rises / orange falls · fixed ±{:.3} model units", self.delta_scale) }
                else { format!("Rectified voltage · fixed full scale {:.2}", self.full_brightness) });
        });
        ui.small(format!("Peak |ΔV| {peak_delta:.2e} · real tick-to-tick change, held until next tick. No generated spikes."));
        egui::CollapsingHeader::new("Response scales and source evidence").show(ui, |ui| {
            ui.add(egui::Slider::new(&mut self.delta_scale, 0.0001..=0.1).logarithmic(true).text("Full-scale |ΔV|"));
            ui.add(egui::Slider::new(&mut self.full_brightness, 0.1..=10.0).text("Full-scale voltage"));
            ui.label("Display scales change no model parameters. Camera exposure is unknown; receipt/hash identify the sampled image.");
            if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
            for key in ["cell_type", "run_id", "tick", "source", "output", "identity_space"] { super::panels::object(ui, key, &value[key]); }
        });
    }
}

fn same_model(value: &Value, summary: &Value) -> bool {
    value["run_id"] == summary["run_id"] && value["model"]["checkpoint_sha256"] == summary["model"]["checkpoint_sha256"]
        && value["model"]["export_sha256"] == summary["model"]["export_sha256"]
}

fn paint_cells(ui: &mut Ui, cells: &[Value], field: &str, height: f32, live: bool, full_scale: f64) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(40.0), height), egui::Sense::hover());
    let project = |cell: &Value| {
        let u = cell["u"].as_i64().unwrap() as f32;
        let v = cell["v"].as_i64().unwrap() as f32;
        egui::vec2(v, u + 0.5*v) // BoxEye x=13*v, y=13*(u+v/2), y down.
    };
    let mut minimum = egui::vec2(f32::INFINITY, f32::INFINITY);
    let mut maximum = egui::vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for cell in cells { let p=project(cell); minimum=minimum.min(p); maximum=maximum.max(p); }
    let extent=maximum-minimum;
    let scale=((rect.width()-12.0)/extent.x.max(1.0)).min((rect.height()-12.0)/extent.y.max(1.0));
    let center=(minimum+maximum)*0.5;
    let radius=(scale*0.46).clamp(0.7, 8.0);
    let hover=response.hover_pos();
    let mut hovered=None;
    let mut nearest=f32::INFINITY;
    let painter=ui.painter_at(rect);
    for cell in cells {
        let point=rect.center()+(project(cell)-center)*scale;
        let signal=finite(&cell[field]).unwrap();
        let amplitude=if field=="delta_voltage" {signal.abs()} else {signal.max(0.0)};
        let magnitude=(amplitude/full_scale).clamp(0.0,1.0) as f32;
        let color=if !live {Color32::from_gray((28.0+70.0*magnitude) as u8)}
            else if field=="input_drive" {Color32::from_gray((255.0*magnitude) as u8)}
            else if field=="delta_voltage" && signal < 0.0 {Color32::from_rgb((30.0+225.0*magnitude) as u8,(35.0+90.0*magnitude) as u8,30)}
            else {Color32::from_rgb(20,(35.0+195.0*magnitude) as u8,(45.0+210.0*magnitude) as u8)};
        painter.circle_filled(point,radius,color);
        if let Some(mouse)=hover {
            let distance=mouse.distance_sq(point);
            if distance<nearest && distance<=(radius+3.0).powi(2) {nearest=distance; hovered=Some((point,cell));}
        }
    }
    if let Some((point, cell))=hovered {
        painter.circle_stroke(point,radius+1.0,Stroke::new(1.0,Color32::WHITE));
        response.on_hover_ui(|ui| {
            ui.label(format!("Model index {} · u={} v={}",cell["model_index"],cell["u"],cell["v"]));
            ui.label(format!("{field}: {:.6e}",finite(&cell[field]).unwrap()));
            if field != "input_drive" {
                ui.label(format!("voltage: {:.7} · ΔV: {:.6e}",finite(&cell["voltage"]).unwrap(),finite(&cell["delta_voltage"]).unwrap()));
            }
        });
    }
}
