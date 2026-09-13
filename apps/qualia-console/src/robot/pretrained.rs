//! Read-only display of the pretrained visual model's continuous response.
use std::collections::VecDeque;
use egui::{Color32, Stroke, Ui};
use serde_json::Value;
use super::{panels, source::{now_ms, Data, Reading}};

const FRESH_MS: u64 = 1500;

pub fn endpoint(robot: &str) -> String {
    std::env::var("QUALIA_FLYVIS_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| {
        reqwest::Url::parse(robot).ok().and_then(|mut url| {
            url.set_port(Some(8091)).ok()?;
            url.set_path("/status"); url.set_query(None); url.set_fragment(None);
            Some(url.to_string())
        }).unwrap_or_else(|| "invalid pretrained producer URL".into())
    })
}

fn number(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite()) }
fn hash(value: &Value) -> bool {
    value.as_str().is_some_and(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit()))
}
pub fn valid_status(value: &Value) -> bool {
    value["schema"] == "qualia.flyvis-live.v1"
        && valid_envelope(value)
        && (value["output"].is_null() || value["output"]["per_type"].as_array().is_some_and(|rows| {
            rows.len() <= 256 && rows.iter().all(|row| {
                row["cell_type"].as_str().is_some_and(|v| !v.is_empty() && v.len() <= 128)
                    && row["count"].as_u64().is_some_and(|n| n > 0)
                    && ["mean_voltage", "min_voltage", "max_voltage", "std_voltage", "mean_abs_delta_from_previous"]
                        .iter().all(|key| number(&row[key]).is_some())
            })
        }))
}

pub(super) fn valid_envelope(value: &Value) -> bool {
    value["activity_kind"] == "graded_model_voltage"
        && value["parameters_frozen"] == true && value["controls_locked"] == true
        && value["run_id"].as_str().is_some_and(|v| !v.is_empty())
        && value["published_unix_ns"].as_u64().is_some()
        && matches!(value["state"].as_str(), Some("starting" | "waiting_for_camera" | "warming_up" | "running" | "stale" | "memory_hold" | "stopped" | "failed"))
        && value["model"].is_object() && value["source"].is_object()
}

fn fresh_ns(value: &Value, now: u64) -> bool {
    value.as_u64().is_some_and(|ns| {
        let stamp = ns / 1_000_000;
        stamp > 0 && stamp <= now.saturating_add(100) && now.saturating_sub(stamp) <= FRESH_MS
    })
}
fn fresh_age(value: &Value) -> bool {
    number(value).is_some_and(|age| (0.0..=FRESH_MS as f64).contains(&age))
}
fn fresh(reading: &Reading, now: u64) -> bool {
    reading.fresh_at(now, FRESH_MS) && valid_status(&reading.value) && fresh_link(&reading.value, now)
}
pub(super) fn fresh_link(value: &Value, now: u64) -> bool {
    let source = &value["source"]; let output = &value["output"];
    value["state"] == "running"
        && value["error"].is_null() && fresh_ns(&value["published_unix_ns"], now)
        && fresh_ns(&source["request_started_unix_ns"], now) && fresh_ns(&source["received_unix_ns"], now)
        && fresh_ns(&output["completed_unix_ns"], now) && fresh_ns(&output["input_received_unix_ns"], now)
        && source["request_started_unix_ns"].as_u64() <= source["received_unix_ns"].as_u64()
        && source["received_unix_ns"].as_u64() <= output["completed_unix_ns"].as_u64()
        && output["completed_unix_ns"].as_u64() <= value["published_unix_ns"].as_u64()
        && source["received_unix_ns"] == output["input_received_unix_ns"]
        && hash(&source["jpeg_sha256"]) && source["jpeg_sha256"] == output["input_sha256"]
        && fresh_age(&source["received_age_ms"]) && fresh_age(&output["age_ms"]) && fresh_age(&output["input_age_ms"])
        && number(&output["voltage_mean"]).is_some() && number(&output["mean_abs_delta_from_previous"]).is_some()
}

fn decimal(value: &Value) -> String {
    number(value).map(|v| format!("{v:.3}")).unwrap_or_else(|| "unavailable".into())
}
fn age_text(value: &Value, elapsed_ms: u64) -> String {
    number(value).filter(|v| *v >= 0.0).map(|v| format!("{:.0} ms", v + elapsed_ms as f64)).unwrap_or_else(|| "unavailable".into())
}

#[derive(Default)]
pub struct PretrainedView {
    run: String,
    last_output: u64,
    history: VecDeque<(u64, f64)>,
    pub neurons: super::neurons::NeuronView,
}
impl PretrainedView {
    pub fn render(&mut self, ui: &mut Ui, data: &Data, producer: &str) {
        ui.strong("CNS visual response · graded model voltage");
        ui.small(producer);
        let Some(reading) = data.sources.get("pretrained") else {
            ui.label("Waiting for the pretrained model's published status"); return;
        };
        let now = now_ms(); let value = &reading.value;
        if !valid_status(value) {
            ui.colored_label(Color32::YELLOW, "Pretrained producer unavailable or status contract invalid");
            egui::CollapsingHeader::new("Connection details").show(ui, |ui| {
                if let Some(error) = &reading.error { ui.label(error); }
            }); return;
        }
        let live = fresh(reading, now);
        let run = value["run_id"].as_str().unwrap_or_default();
        let output = &value["output"]; let stamp = output["completed_unix_ns"].as_u64().unwrap_or(0);
        if self.run != run || (stamp > 0 && stamp < self.last_output) {
            self.run = run.into(); self.history.clear(); self.last_output = 0;
        }
        if live && stamp > self.last_output {
            self.history.push_back((stamp, number(&output["voltage_mean"]).unwrap_or(0.0)));
            if self.history.len() > 180 { self.history.pop_front(); }
            self.last_output = stamp;
        }
        let state = value["state"].as_str().unwrap_or("unknown");
        ui.colored_label(if live {Color32::LIGHT_GREEN} else {Color32::YELLOW},
            format!("{} · state {state} · frame {} · tick {}", if live {"LIVE measured response"} else {"Response unverified / stale"}, panels::text(&value["camera_frames"]), panels::text(&value["tick"])));
        ui.label(format!("Model {} {} · weights frozen · controls locked", panels::text(&value["model"]["name"]), panels::text(&value["model"]["id"])));
        let elapsed = now.saturating_sub(reading.received_ms);
        ui.label(format!("Camera receipt {} · output {} · output input {}",
            age_text(&value["source"]["received_age_ms"], elapsed), age_text(&output["age_ms"], elapsed), age_text(&output["input_age_ms"], elapsed)));
        let change = number(&output["mean_abs_delta_from_previous"]).map(|v| format!("{v:.3e}")).unwrap_or_else(|| "unavailable".into());
        ui.label(format!("Compute {} ms · mean voltage {} · mean |change| {change}", decimal(&value["compute_ms"]), decimal(&output["voltage_mean"])));
        self.neurons.render(ui, data, value);
        self.plot(ui, live);
        egui::CollapsingHeader::new("Cell-type summary").default_open(false).show(ui, |ui| {
            if let Some(rows) = output["per_type"].as_array() {
                egui::Grid::new("pretrained-types").striped(true).show(ui, |ui| {
                    for label in ["Cell type", "Cells", "Mean voltage", "Mean |change|"] { ui.strong(label); } ui.end_row();
                    for row in rows {
                        for field in ["cell_type", "count"] { ui.label(panels::text(&row[field])); }
                        ui.label(decimal(&row["mean_voltage"])); ui.label(decimal(&row["mean_abs_delta_from_previous"])); ui.end_row();
                    }
                });
            } else { ui.label("No computed response published"); }
        });
        egui::CollapsingHeader::new("Source, model and errors").default_open(false).show(ui, |ui| {
            ui.label("Camera luminance drives this model. Values are not spikes or Hz; camera exposure time is unknown.");
            ui.label(format!("Run {run}"));
            if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
            if let Some(error) = value["error"].as_str() { ui.colored_label(Color32::YELLOW, error); }
            for key in ["model", "source", "latest_camera", "model_inputs", "measured_context_drives_model", "identity_space"] { panels::object(ui, key, &value[key]); }
            ui.label("No wheel readout, learned driving, or physical action is established by this display.");
        });
    }

    fn plot(&self, ui: &mut Ui, live: bool) {
        ui.small(format!("Mean graded voltage · {} received outputs · history", self.history.len()));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(20.0), 68.0), egui::Sense::hover());
        if self.history.is_empty() { return; }
        let low = self.history.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let high = self.history.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        let first = self.history.front().unwrap().0; let last = self.history.back().unwrap().0;
        let span = (high-low).max(1e-9); let time_span = last.saturating_sub(first).max(1);
        let points: Vec<_> = self.history.iter().map(|(stamp, mean)| egui::pos2(
            rect.left()+((*stamp-first) as f64/time_span as f64) as f32*rect.width(),
            rect.bottom()-if high == low {rect.height()*0.5} else {((*mean-low)/span) as f32*rect.height()}
        )).collect();
        let color = if live {Color32::LIGHT_GREEN} else {Color32::GRAY};
        for pair in points.windows(2) { ui.painter().line_segment([pair[0],pair[1]],Stroke::new(1.5,color)); }
        if points.len()==1 { ui.painter().circle_filled(points[0],2.0,color); }
        ui.small(format!("Observed range {low:.4}..{high:.4}; {:.1} s shown", time_span as f64/1e9));
    }
}
