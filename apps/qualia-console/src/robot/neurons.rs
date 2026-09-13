//! Actual flyvis model cells; brightness is rectified published voltage.
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

pub struct NeuronView {
    pub selected_type: String,
    full_brightness: f64,
}
impl Default for NeuronView {
    fn default() -> Self { Self { selected_type: "T4a".into(), full_brightness: 3.0 } }
}

impl NeuronView {
    pub fn render(&mut self, ui: &mut Ui, data: &Data, summary: &Value) {
        ui.horizontal(|ui| {
            ui.strong("Neural activity");
            egui::ComboBox::from_id_salt("neuron-type").selected_text(&self.selected_type).show_ui(ui, |ui| {
                if let Some(rows) = summary["output"]["per_type"].as_array() {
                    for row in rows {
                        if let Some(name) = row["cell_type"].as_str() {
                            ui.selectable_value(&mut self.selected_type, name.to_owned(), name);
                        }
                    }
                }
            });
            ui.small("graded response");
        });
        let Some(reading) = data.sources.get("neurons") else {
            ui.label("Waiting for published neuron coordinates and values"); return;
        };
        let value = &reading.value;
        if !valid_status(value) || value["cell_type"].as_str() != Some(self.selected_type.as_str()) {
            ui.label(format!("Waiting for {} cells", self.selected_type));
            egui::CollapsingHeader::new("Neuron connection details").show(ui, |ui| {
                if let Some(error) = &reading.error { ui.label(error); }
            }); return;
        }
        let now = now_ms();
        let same_model = value["run_id"] == summary["run_id"]
            && value["model"]["checkpoint_sha256"] == summary["model"]["checkpoint_sha256"]
            && value["model"]["export_sha256"] == summary["model"]["export_sha256"];
        let live = reading.fresh_at(now, 1500) && pretrained::fresh_link(value, now) && same_model;
        let cells = value["cells"].as_array().expect("validated cell array");
        let positive = cells.iter().filter(|cell| finite(&cell["voltage"]).unwrap_or(0.0) > 0.0).count();
        let low = cells.iter().filter_map(|cell| finite(&cell["voltage"])).fold(f64::INFINITY, f64::min);
        let high = cells.iter().filter_map(|cell| finite(&cell["voltage"])).fold(f64::NEG_INFINITY, f64::max);
        ui.colored_label(if live {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!(
            "{} · {} cells · {} positive · tick {}", if live {"Current cell values"} else {"Historical cells / source unavailable"},
            cells.len(), positive, value["tick"]));
        ui.small(format!("Voltage {low:.4}..{high:.4} · model retinal sample coordinates"));

        // The surrounding scroll area has unbounded height; size from the
        // visible width so model details cannot push the cell field offscreen.
        let height = (ui.available_width() * 0.65).clamp(180.0, 330.0);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(40.0), height), egui::Sense::hover());
        let project = |cell: &Value| {
            let u = cell["u"].as_i64().unwrap_or(0) as f32;
            let v = cell["v"].as_i64().unwrap_or(0) as f32;
            // Exported BoxEye centers: x=13*v, y=13*(u+v/2), y down.
            // The common factor 13 cancels when fitting the real lattice.
            egui::vec2(v, u + 0.5 * v)
        };
        let mut minimum = egui::vec2(f32::INFINITY, f32::INFINITY);
        let mut maximum = egui::vec2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for cell in cells {
            let p = project(cell); minimum = minimum.min(p); maximum = maximum.max(p);
        }
        let extent = maximum - minimum;
        let scale = ((rect.width()-24.0)/extent.x.max(1.0)).min((rect.height()-24.0)/extent.y.max(1.0));
        let center = (minimum+maximum)*0.5;
        let radius = (scale*0.60).clamp(1.0, 9.0);
        let hover = response.hover_pos();
        let mut hovered = None;
        let mut nearest = f32::INFINITY;
        for cell in cells {
            let point = rect.center() + (project(cell)-center)*scale;
            let voltage = finite(&cell["voltage"]).unwrap_or(0.0);
            let brightness = (voltage.max(0.0)/self.full_brightness).clamp(0.0, 1.0) as f32;
            let color = if live {
                Color32::from_rgb((27.0+67.0*brightness) as u8, (40.0+207.0*brightness) as u8, (55.0+127.0*brightness) as u8)
            } else { Color32::from_gray((32.0+70.0*brightness) as u8) };
            ui.painter().circle_filled(point, radius, color);
            if let Some(mouse) = hover {
                let distance = mouse.distance_sq(point);
                if distance < nearest && distance <= (radius+3.0).powi(2) { nearest=distance; hovered=Some((point,cell)); }
            }
        }
        if let Some((point, cell)) = hovered {
            ui.painter().circle_stroke(point,radius+2.0,Stroke::new(1.0,Color32::WHITE));
            response.on_hover_ui(|ui| {
                ui.strong(format!("{} · model index {}",self.selected_type,cell["model_index"]));
                ui.label(format!("Hex u={} v={}",cell["u"],cell["v"]));
                ui.label(format!("Published voltage {:.7}",finite(&cell["voltage"]).unwrap_or(0.0)));
                ui.label(format!("Signed update Δ {:.4e}",finite(&cell["delta_voltage"]).unwrap_or(0.0)));
                ui.label("A graded model state; no spike event inferred");
            });
        }
        ui.small(format!("Brightness: max(voltage, 0); {:.2} model units = full. No generated spike events.", self.full_brightness));
        egui::CollapsingHeader::new("Display scale and neuron evidence").show(ui, |ui| {
            ui.add(egui::Slider::new(&mut self.full_brightness,0.1..=10.0).text("Model voltage at full brightness"));
            ui.label("This display setting changes no model parameter.");
            if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW,error); }
            for key in ["cell_type","run_id","tick","source","output","identity_space"] {
                super::panels::object(ui,key,&value[key]);
            }
        });
    }
}
