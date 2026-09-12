use egui::{Color32, Stroke, Ui};
use serde_json::Value;
use super::{lidar::Scan, source::{Data, Reading, now_ms}};

pub fn text(value: &Value) -> String {
    match value { Value::Null => "unavailable".into(), Value::String(s) => s.clone(), _ => value.to_string() }
}

pub fn freshness(ui: &mut Ui, reading: &Reading, deadline: u64) {
    let age = now_ms().saturating_sub(reading.received_ms);
    let label = if reading.received_ms == 0 { "No reading".into() }
        else if reading.error.is_some() || age > deadline { format!("STALE · received {age} ms ago") }
        else { format!("Connected · received {age} ms ago") };
    ui.colored_label(if reading.error.is_some() || age > deadline {Color32::YELLOW} else {Color32::LIGHT_GREEN}, label);
}

/// Summary fields stay visible; large arrays and nested objects are expandable.
pub fn object(ui: &mut Ui, name: &str, value: &Value) {
    ui.push_id(name, |ui| match value {
        Value::Object(fields) => {
            ui.strong(name);
            for (key, value) in fields {
                if ["token", "secret", "bearer", "password", "authorization"].iter().any(|s| key.to_lowercase().contains(s)) { continue; }
                if value.is_object() || value.is_array() {
                    egui::CollapsingHeader::new(format!("{key}{}", value.as_array().map(|v| format!(" ({})", v.len())).unwrap_or_default()))
                        .show(ui, |ui| object(ui, key, value));
                } else { ui.label(format!("{key}: {}", text(value))); }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().take(128).enumerate() { object(ui, &index.to_string(), item); }
            if items.len() > 128 { ui.label(format!("{} more items; response is truncated for display", items.len() - 128)); }
        }
        _ => { ui.label(format!("{name}: {}", text(value))); }
    });
}

pub fn scan_plot(ui: &mut Ui, scan: &Scan, extent: f32) {
    let size = ui.available_width().min(ui.available_height().max(160.0) - 25.0).min(470.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(17, 25, 38));
    let center = rect.center();
    let scale = size / (2.0 * extent);
    for meter in 1..=extent as i32 {
        painter.circle_stroke(center, meter as f32 * scale, Stroke::new(1.0, Color32::from_gray(55)));
    }
    painter.line_segment([egui::pos2(rect.left(), center.y), egui::pos2(rect.right(), center.y)], Stroke::new(1.0, Color32::GRAY));
    painter.line_segment([egui::pos2(center.x, rect.top()), egui::pos2(center.x, rect.bottom())], Stroke::new(1.0, Color32::GRAY));
    for point in &scan.points {
        let position = center + egui::vec2(point[0] * scale, -point[1] * scale);
        if rect.contains(position) { painter.circle_filled(position, 2.0, Color32::from_rgb(88, 234, 190)); }
    }
    painter.circle_filled(center, 4.0, Color32::WHITE);
    painter.arrow(center, egui::vec2(22.0, 0.0), Stroke::new(2.0, Color32::WHITE));
    ui.label(format!("±{extent:.1} m · range rings 1 m"));
}

pub fn matrix(ui: &mut Ui, title: &str, value: &Value) {
    ui.strong(title);
    let Some(values) = value.as_array().filter(|v| !v.is_empty()) else { ui.label("No published matrix"); return; };
    let side = (values.len() as f32).sqrt().ceil() as usize;
    let size = ui.available_width().min(310.0);
    let cell = size / side as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let max = values.iter().filter_map(Value::as_f64).map(f64::abs).fold(0.0, f64::max);
    for (index, value) in values.iter().take(4096).enumerate() {
        let normalized = if max > 0.0 { value.as_f64().unwrap_or(0.0) / max } else { 0.0 };
        let c = (normalized.abs().min(1.0) * 220.0) as u8;
        let color = if normalized >= 0.0 {Color32::from_rgb(20, 25 + c, 40 + c / 2)} else {Color32::from_rgb(25 + c, 25, 40 + c / 2)};
        let position = rect.min + egui::vec2((index % side) as f32 * cell, (index / side) as f32 * cell);
        ui.painter().rect_filled(egui::Rect::from_min_size(position, egui::vec2(cell, cell)), 0.0, color);
    }
    ui.label(format!("{} values · color scale ±{max:.4}", values.len()));
}

pub fn cognition(ui: &mut Ui, data: &Data) {
    ui.heading("Seven cognition layers");
    for (key, title) in [("cognition", "Robot L0–L2"), ("host-belief", "Host L3–L6")] {
        ui.strong(title);
        let Some(reading) = data.sources.get(key) else { ui.label("Waiting for source"); continue; };
        freshness(ui, reading, 5000);
        ui.label(format!("Backend: {} · motor authority: {}", text(&reading.value["capabilities"]["backend"]), text(&reading.value["capabilities"]["motor_authority"])));
        egui::Grid::new(key).striped(true).show(ui, |ui| {
            for label in ["Layer", "Sequence", "Source", "Hz", "Precision", "Prediction error", "Activation RMS"] { ui.strong(label); } ui.end_row();
            if let Some(layers) = reading.value["layers"].as_array() { for layer in layers {
                for field in ["layer", "sequence", "fresh", "cadence_hz", "precision", "prediction_error_l2", "activation_rms"] {
                    let now = now_ms();
                    let producer_fresh = layer["ts_ms"].as_u64().is_some_and(|ts| ts <= now.saturating_add(1000) && now.saturating_sub(ts) <= 1500);
                    ui.label(if field == "fresh" { if layer[field] == true && reading.fresh_at(now, 5000) && producer_fresh { "fresh".into() } else { "STALE".into() } } else { text(&layer[field]) });
                } ui.end_row();
            }}
        });
        ui.separator();
    }
    ui.columns(3, |columns| {
        if let Some(robot) = data.sources.get("cognition") {
            freshness(&mut columns[0], robot, 5000);
            if robot.value["boundary"]["expires_at_ms"].as_u64().unwrap_or(0) < now_ms() { columns[0].colored_label(Color32::YELLOW, "Boundary expired"); }
            matrix(&mut columns[0], "Robot L2 boundary", &robot.value["boundary"]["latent"]);
        }
        if let Some(host) = data.sources.get("host-belief") {
            freshness(&mut columns[1], host, 5000);
            freshness(&mut columns[2], host, 5000);
            matrix(&mut columns[1], "Grounded L3", &host.value["posterior"]["grounded_l3"]);
            matrix(&mut columns[2], "Semantic delta L3", &host.value["posterior"]["semantic_delta_l3"]);
        }
    });
}

pub fn evidence(ui: &mut Ui, data: &Data) {
    ui.heading("Leash applied-action evidence");
    let Some(reading) = data.sources.get("actions") else { ui.label("Waiting for action ledger"); return; };
    freshness(ui, reading, 5000);
    ui.label(format!("Epoch {} · latest sequence {}", text(&reading.value["producer_epoch"]), text(&reading.value["latest_sequence"])));
    ui.label("Published ledger entries; requested speed alone does not establish movement.");
    egui::Grid::new("actions").striped(true).show(ui, |ui| {
        for label in ["Sequence", "Requested L", "Requested R", "Applied L", "Applied R", "Armed", "Valid", "Deadman"] { ui.strong(label); } ui.end_row();
        if let Some(entries) = reading.value["entries"].as_array() { for entry in entries.iter().rev().take(16) {
            for key in ["action_sequence", "requested_left", "requested_right", "applied_left", "applied_right", "armed", "valid", "deadman_active"] { ui.label(text(&entry[key])); } ui.end_row();
        }}
    });
}

pub fn compute(ui: &mut Ui, data: &Data) {
    ui.heading("Compute + integration");
    if let Some(host) = data.sources.get("host-health") {
        freshness(ui, host, 5000);
        ui.label(format!("Host healthy: {} · integration ready: {}", text(&host.value["healthy"]), text(&host.value["ready"])));
        object(ui, "Host compute", &host.value["compute"]);
        object(ui, "Integration", &host.value["integration"]);
        object(ui, "Planner", &host.value["planner"]);
    }
    if let Some(robot) = data.sources.get("cognition") { freshness(ui, robot, 5000); object(ui, "Robot cognition backend", &robot.value["backend_status"]); }
}

pub fn brain(ui: &mut Ui, state: &mut crate::ConsoleState) {
    use crate::views::brain::{connectome, scene};
    let cloud_state = connectome::cloud();
    let cloud = match &cloud_state {
        connectome::CloudState::Ready(cloud) => Some(cloud.as_ref()),
        connectome::CloudState::Failed(error) => { ui.colored_label(Color32::YELLOW, error); None }
        connectome::CloudState::Unset => { ui.label("Connectome artifact is not configured"); None }
    };
    let mut frame = connectome::frame();
    let stale = frame.finished || frame.error.is_some() || frame.ticks_read == 0
        || frame.last_advanced.is_none_or(|time| time.elapsed().as_millis() > 1500);
    ui.colored_label(if stale {Color32::YELLOW} else {Color32::LIGHT_GREEN}, format!(
        "{} · tick {} · {:.1} frames/s · {} firing", if stale {"STALE stream"} else {"Receiving model spikes"}, frame.tick, frame.rate_hz, frame.firing_count()));
    ui.label(&frame.source);
    if let Some(cloud) = cloud {
        let placed = frame.firing.iter().filter(|id| cloud.point_of(**id).is_some()).count();
        ui.label(format!("Latest frame: {placed} spikes with coordinates, {} without coordinates", frame.firing.len() - placed));
    }
    let samples: Vec<_> = frame.activity.iter().filter(|sample| sample.received.elapsed().as_secs_f32() < 10.0).collect();
    let peak = samples.iter().map(|sample| sample.count).max().unwrap_or(0);
    let total: usize = samples.iter().map(|sample| sample.count).sum();
    ui.label(format!("Observed spikes, last 10 seconds: {total} | peak {peak}/frame | {} received frames", samples.len()));
    let (history, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 65.0), egui::Sense::hover());
    let painter = ui.painter_at(history);
    painter.rect_filled(history, 0.0, Color32::from_rgb(17, 25, 38));
    for sample in samples {
        let x = history.right() - sample.received.elapsed().as_secs_f32() / 10.0 * history.width();
        let height = if sample.count == 0 {1.0} else {(sample.count as f32).ln_1p() / (peak.max(1) as f32).ln_1p() * (history.height()-4.0)};
        painter.line_segment([egui::pos2(x, history.bottom()), egui::pos2(x, history.bottom()-height)],
            egui::Stroke::new(2.0, if sample.count == 0 {Color32::GRAY} else {Color32::LIGHT_GREEN}));
    }
    ui.small("Spikes per received frame, logarithmic scale; includes cells without 3D coordinates. Up to 512 frames over 10 seconds; gaps have no readings.");
    if stale { ui.colored_label(Color32::YELLOW, "Retained activity is historical; no fresh frame is being received."); }
    ui.label("Model activity; wheel acknowledgements are in Action evidence.");
    if stale { frame.firing.clear(); }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ui.available_height().max(160.0) - 5.0), egui::Sense::drag());
    if response.dragged() { state.brain.camera.orbit(response.drag_delta()); }
    if response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll != 0.0 { state.brain.camera.zoom_by((scroll * 0.002).exp()); }
    }
    let toggles = scene::SceneToggles {connectome: true, brain: false, world: false, cloud: false, floor: false};
    state.brain.counts = scene::paint(&ui.painter_at(rect), rect, &state.brain.camera, toggles,
        &state.brain, &state.brain_assets, cloud, &frame);
}

pub fn perception(ui: &mut Ui, data: &Data) {
    if let Some(reading) = data.sources.get("perception-observation") {
        freshness(ui, reading, 1500);
        let p = &reading.value;
        ui.strong("Camera -> visual frontend (observation)");
        ui.label(format!("{} | frame {} | {} features | confidence {}", text(&p["state"]), text(&p["frame_seq"]), text(&p["features"]), text(&p["tracking_confidence"])));
        ui.label(format!("Brightness {} | contrast {} | keyframes {}", text(&p["luminance_mean"]), text(&p["luminance_stddev"]), text(&p["keyframes"])));
        ui.colored_label(Color32::YELLOW, text(&p["limitation"]));
        if p["error"].is_string() { ui.colored_label(Color32::YELLOW, text(&p["error"])); }
        ui.separator();
    }
    if let Some(perception) = data.sources.get("host-perception") {
        freshness(ui, perception, 5000);
        let camera = &perception.value["camera"];
        let vslam = &perception.value["vslam"];
        ui.label(format!("Host camera: {} · seq {} · {} × {}", text(&camera["available"]), text(&camera["frame_seq"]), text(&camera["source_width"]), text(&camera["source_height"])));
        ui.label(format!("VSLAM tracking: {} · {} features · pose confidence {}", text(&vslam["tracking"]), text(&vslam["feature_count"]), text(&vslam["pose_confidence"])));
        if perception.value["visual_mapping_ready"] != true { ui.colored_label(Color32::YELLOW, text(&perception.value["visual_mapping_warning"])); }
    }
    ui.strong("Detected objects");
    if let Some(world) = data.sources.get("host-world") {
        freshness(ui, world, 5000);
        if let Some(objects) = world.value["objects"].as_array() {
            if objects.is_empty() { ui.label("No objects published by the host"); }
            for item in objects.iter().take(128) {
                ui.label(format!("{} · confidence {} · {}", text(item.get("label").or_else(|| item.get("kind")).unwrap_or(&Value::Null)), text(&item["confidence"]), text(&item["source"])));
            }
        } else { ui.label("Object source unavailable"); }
    }
    if let Some(costmap) = data.sources.get("host-costmap") {
        freshness(ui, costmap, 5000);
        ui.label(format!("Metric visual landmarks: {}", costmap.value["visual_landmarks"].as_array().map(|v| v.len().to_string()).unwrap_or_else(|| "unavailable".into())));
        object(ui, "Calibration and fusion", &costmap.value["fusion"]);
    }
}

pub fn mission(ui: &mut Ui, data: &Data) {
    ui.label("Motion controls locked until acknowledgement repair and operator presence are confirmed.");
    ui.horizontal_wrapped(|ui| {
        for label in ["Set goal", "Clear goal", "Explore", "Stop explore", "Start arena", "End arena", "Operator stop"] {
            ui.add_enabled(false, egui::Button::new(label));
        }
    });
    if let Some(arena) = data.sources.get("host-arena") {
        freshness(ui, arena, 5000);
        ui.label(format!("Arena {} · running {} · session {}", text(&arena.value["phase"]), text(&arena.value["running"]), text(&arena.value["session_id"])));
        ui.label(format!("Last tool: {}", text(&arena.value["last_tool"])));
        ui.strong("Activity posts");
        if let Some(events) = arena.value["events"].as_array() {
            if events.is_empty() { ui.label("No arena events published"); }
            for (index, event) in events.iter().rev().take(32).enumerate() { object(ui, &format!("Event {index}"), event); }
        }
        object(ui, "Model influence", &arena.value["model_feedback"]);
    }
    if let Some(explore) = data.sources.get("host-explore") {
        freshness(ui, explore, 5000);
        ui.label(format!("Explore available: {} · running: {}", text(&explore.value["available"]), text(&explore.value["running"])));
    }
}


pub fn fly_inputs(ui: &mut Ui, data: &Data) {
    let Some(reading) = data.sources.get("fly-inputs") else {
        ui.label("Waiting for live producer input evidence"); return;
    };
    freshness(ui, reading, 1500);
    let status = &reading.value;
    let input = &status["input"];
    let output = &status["output"];
    let now = now_ms();
    let age = input["received_ms"].as_u64().map(|t| now.saturating_sub(t));
    let fresh = reading.fresh_at(now, 1500) && age.is_some_and(|v| v <= 1500) && input["received_ms"].as_u64().is_some_and(|t| t <= now.saturating_add(100)) && status["state"] == "live"
        && input["measured"].is_object() && input["model_stepped"] != false;
    ui.colored_label(if fresh {Color32::LIGHT_GREEN} else {Color32::YELLOW},
        format!("{} | tick {} | age {} ms", if fresh {"Measured inputs"} else {"STALE inputs"}, text(&input["tick"]), age.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into())));
    ui.label("Required sources: camera + IMU + odometry + lidar; engineering encoder");
    if let Some(reason) = input["hold_reason"].as_str() { ui.colored_label(Color32::YELLOW, format!("Output held: {reason}")); }
    if input["model_stepped"] == false { ui.colored_label(Color32::YELLOW, "No neural step for this acquisition"); }
    let measured = &input["measured"];
    egui::Grid::new("fly-input-evidence").num_columns(2).show(ui, |ui| {
        for (label, value) in [
            ("IMU timestamp", &measured["imu_ts_ms"]),
            ("Lidar timestamp", &measured["lidar_ts_ms"]),
            ("Odometry timestamp", &measured["odometry_ts_ms"]),
            ("Gyroscope (rad/s)", &measured["angular_velocity_radps"]),
            ("Acceleration incl. gravity (m/s2)", &measured["acceleration_including_gravity_mps2"]),
            ("Odometry speed (m/s)", &input["odometry_speed_mps"]),
            ("Fused yaw rate (rad/s)", &input["fused_yaw_rate_radps"]),
            ("Nearest lidar return (m)", &measured["nearest_return_m"]),
            ("Motion attenuation", &input["motion_attenuation"]),
            ("Proximity attention", &input["proximity_attention"]),
        ] { ui.label(label); ui.monospace(text(value)); ui.end_row(); }
    });
    if let Some(columns) = input["conditioned_columns"].as_array() {
        ui.label("Sensor-conditioned camera columns");
        for (i, value) in columns.iter().enumerate() {
            ui.add(egui::ProgressBar::new(value.as_f64().unwrap_or(0.0) as f32).text(format!("{}: {:.3}", i + 1, value.as_f64().unwrap_or(0.0))));
        }
    }
    ui.label("Camera reports retrieval time; capture timestamp is unavailable.");
    let paired = output["tick"] == input["tick"] && output["tick"].is_u64() && output["run_id"].is_string() && output["run_id"] == input["run_id"];
    if paired {
        ui.label(format!("Model decision {} | gated {} | proposal L {} / R {}", text(&output["neural_command"]), text(&output["gated_command"]), text(&output["proposed_left"]), text(&output["proposed_right"])));
    } else { ui.label("No paired model output for the latest input"); }
    ui.label(format!("Transport: {} | wheel limit {}", if status["transport_attached"] == false { "disconnected" } else if status["transport_attached"] == true { "attached" } else { "unknown" }, text(&output["max_wheel_speed"])));
    ui.colored_label(Color32::YELLOW, "Model limitation: inhibitory receptor outputs have no calibrated downstream resting activity. Motor firing is not established.");
}
