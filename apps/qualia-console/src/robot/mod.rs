//! Live robot views use the robot's published HTTP readings. They do not send
//! motor commands or turn a missing host producer into a simulated reading.
mod lidar;
mod exploration;
mod panels;
mod source;
mod spatial;

use egui::{Color32, TextureHandle, TextureOptions};
use source::{Data, Monitor};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialog { Camera, Lidar, Occupancy, Brain, Inputs, Coach, Cognition, Matrices, World, Perception, Mission, Evidence, Compute, Telemetry, Diagnostics, Braid, Recordings }
const DIALOGS: [(Dialog, &str); 17] = [
    (Dialog::Camera, "Camera"), (Dialog::Brain, "Brain"), (Dialog::Coach, "DeepSeek console"),
    (Dialog::Mission, "Exploration + score + MCP"), (Dialog::Perception, "Visual processing"),
    (Dialog::Lidar, "Lidar"), (Dialog::Occupancy, "Occupancy"), (Dialog::Inputs, "Fly inputs"),
    (Dialog::Evidence, "Action evidence"), (Dialog::Cognition, "Seven layers"), (Dialog::Matrices, "Belief matrices"),
    (Dialog::World, "World + route"), (Dialog::Compute, "Compute"), (Dialog::Telemetry, "Robot telemetry"),
    (Dialog::Diagnostics, "Diagnostics"), (Dialog::Braid, "Braid"), (Dialog::Recordings, "Recordings"),
];

pub struct RobotConsole {
    monitor: Monitor,
    robot_url: String,
    open: [bool; 17],
    arrange: bool,
    last_size: egui::Vec2,
    extent: f32,
    camera: Option<(u64, TextureHandle)>,
    grid: Option<(u64, f32, TextureHandle)>,
    scan: Option<lidar::Scan>,
    scan_error: Option<String>,
    scan_advanced_ms: u64,
    spatial: spatial::SpatialView,
    weight_delta: bool,
}

impl RobotConsole {
    pub fn from_env() -> Option<Self> {
        let robot_url = std::env::var("QUALIA_LEASH_BASE_URL").ok()?.trim_end_matches('/').to_owned();
        if robot_url.is_empty() { return None; }
        Some(Self {
            monitor: Monitor::new(robot_url.clone(), crate::client::agent_url()),
            robot_url, open: [true; 17], arrange: true, last_size: egui::Vec2::ZERO, extent: 6.0, camera: None,
            grid: None, scan: None, scan_error: None, scan_advanced_ms: 0,
            spatial: spatial::SpatialView::default(), weight_delta: false,
        })
    }

    fn textures(&mut self, ctx: &egui::Context, data: &Data) {
        if let Some((stamp, pixels)) = &data.camera {
            if self.camera.as_ref().map(|c| c.0) != Some(*stamp) {
                if let Some((old_stamp, texture)) = &mut self.camera {
                    texture.set((**pixels).clone(), TextureOptions::LINEAR);
                    *old_stamp = *stamp;
                } else {
                    self.camera = Some((*stamp, ctx.load_texture("robot-camera", (**pixels).clone(), TextureOptions::LINEAR)));
                }
            }
        }
        if let Some(reading) = data.sources.get("sensors") {
            match lidar::Scan::parse(&reading.value) {
                Ok(scan) => {
                    if self.scan.as_ref().map(|s| s.timestamp_ms) != Some(scan.timestamp_ms) {
                        self.scan_advanced_ms = source::now_ms();
                    }
                    self.scan = Some(scan); self.scan_error = reading.error.clone();
                }
                Err(error) => self.scan_error = Some(error),
            }
        }
        if let Some(scan) = &self.scan {
            if self.grid.as_ref().map(|g| (g.0, g.1)) != Some((scan.timestamp_ms, self.extent)) {
                let cells = scan.occupancy(self.extent);
                let pixels: Vec<u8> = cells.iter().flat_map(|cell| match cell {
                    1 => [43, 75, 83, 255], 2 => [88, 234, 190, 255], _ => [17, 25, 38, 255],
                }).collect();
                let image = egui::ColorImage::from_rgba_unmultiplied([lidar::SIDE, lidar::SIDE], &pixels);
                if let Some((stamp, extent, texture)) = &mut self.grid {
                    texture.set(image, TextureOptions::NEAREST);
                    *stamp = scan.timestamp_ms; *extent = self.extent;
                } else {
                    self.grid = Some((scan.timestamp_ms, self.extent, ctx.load_texture("lidar-occupancy", image, TextureOptions::NEAREST)));
                }
            }
        }
    }

    pub fn render(&mut self, ctx: &egui::Context, state: &mut crate::ConsoleState) {
        crate::theme::apply(ctx);
        let data = self.monitor.data.lock().unwrap().clone();
        self.textures(ctx, &data);
        egui::TopBottomPanel::top("robot-header").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Qualia");
                ui.label(&self.robot_url);
                if let Some(health) = data.sources.get("health") {
                    panels::freshness(ui, health, 5000);
                    ui.label(format!("{} · {}", panels::text(&health.value["role"]), panels::text(&health.value["mode"])));
                }
                ui.separator();
                ui.label("Console controls locked · acknowledgement repair unconfirmed");
            });
            ui.horizontal(|ui| {
                ui.menu_button("Dialogs", |ui| {
                    for (index, (_, label)) in DIALOGS.iter().enumerate() { ui.checkbox(&mut self.open[index], *label); }
                });
                if ui.button("Show all").clicked() { self.open = [true; 17]; self.arrange = true; }
                if ui.button("Arrange dialogs").clicked() { self.arrange = true; }
                if let Some(health) = data.sources.get("health") {
                    ui.label(format!("Reported estop: {} | deadman OK: {}", panels::text(&health.value["estop"]), panels::text(&health.value["deadman_ok"])));
                }
            });
            let mut issues: Vec<String> = data.sources.iter().filter_map(|(name, reading)| {
                reading.error.as_ref().map(|error| format!("{name}: {error}"))
            }).collect();
            if let Some(error) = &data.camera_error { issues.push(format!("Camera: {error}")); }
            if let Some(error) = &self.scan_error { issues.push(error.clone()); }
            if data.sources.get("host-health").is_some_and(|r| r.value["ready"] == false) {
                issues.push("Host integration is not ready; inspect Compute and Diagnostics.".into());
            }
            egui::CollapsingHeader::new(format!("Issues ({})", issues.len())).default_open(false).show(ui, |ui| {
                for issue in issues { ui.colored_label(Color32::YELLOW, issue); }
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(crate::theme::BG)).show(ctx, |_| {});
        let bounds = ctx.available_rect();
        if (bounds.size() - self.last_size).length() > 8.0 {
            self.arrange = true; self.last_size = bounds.size();
        }
        let count = self.open.iter().filter(|open| **open).count().max(1);
        let columns = if bounds.width() >= 2400.0 { 5 } else if bounds.width() >= 1500.0 { 4 } else { 3 };
        let rows = count.div_ceil(columns);
        let cell = egui::vec2(bounds.width() / columns as f32, bounds.height() / rows as f32);
        let mut visible = 0;
        for (index, (dialog, title)) in DIALOGS.iter().copied().enumerate() {
            let mut open = self.open[index];
            if !open { continue; }
            let position = bounds.min + egui::vec2((visible % columns) as f32 * cell.x + 5.0, (visible / columns) as f32 * cell.y + 5.0);
            let size = egui::vec2((cell.x - 20.0).max(240.0), (cell.y - 42.0).max(120.0));
            visible += 1;
            let mut window = egui::Window::new(title).id(egui::Id::new(("live-dialog", index)))
                .open(&mut open).collapsible(false).resizable(true)
                .default_pos(position).default_size(size).min_size([230.0, 110.0]);
            if self.arrange { window = window.current_pos(position).fixed_size(size); }
            window.show(ctx, |ui| {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.dialog(ui, dialog, &data, state));
            });
            self.open[index] = open;
        }
        self.arrange = false;
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn dialog(&mut self, ui: &mut egui::Ui, dialog: Dialog, data: &Data, state: &mut crate::ConsoleState) {
        match dialog {
            Dialog::Camera => self.camera(ui, data, ui.available_height().max(140.0) - 30.0),
            Dialog::Lidar => self.lidar_panel(ui, false),
            Dialog::Occupancy => self.lidar_panel(ui, true),
            Dialog::Brain => panels::brain(ui, state),
            Dialog::Inputs => panels::fly_inputs(ui, data),
            Dialog::Coach => crate::views::coach::render(ui, state),
            Dialog::Cognition => panels::cognition(ui, data),
            Dialog::Matrices => self.matrices(ui, data),
            Dialog::World => self.spatial.render(ui, data),
            Dialog::Perception => panels::perception(ui, data),
            Dialog::Mission => {
                exploration::render(ui, data);
                if ui.add_enabled(!data.observing, egui::Button::new("MCP observe (read only)")).clicked() { self.monitor.observe(); }
            }
            Dialog::Evidence => panels::evidence(ui, data),
            Dialog::Compute => panels::compute(ui, data),
            Dialog::Telemetry => {
                if let Some(reading) = data.sources.get("sensors") {
                    panels::freshness(ui, reading, 1500);
                    let sensors = &reading.value["sensors"];
                    for key in ["battery", "odometry"] { panels::object(ui, key, &sensors[key]); }
                    panels::object(ui, "IMU", &sensors["imu"]["sample"]);
                }
                if let Some(spatial) = data.sources.get("spatial") { panels::object(ui, "Odometry pose", &spatial.value["odometry_pose"]); }
            }
            Dialog::Diagnostics => {
                for (name, reading) in &data.sources {
                    ui.push_id(name, |ui| {
                        ui.horizontal(|ui| { ui.strong(name); panels::freshness(ui, reading, 5000); });
                        egui::CollapsingHeader::new("Response details").show(ui, |ui| panels::object(ui, name, &reading.value));
                    });
                }
            }
            Dialog::Braid => {
                if state.connection.is_live() {
                    crate::views::mission::render(ui, state);
                } else {
                    ui.colored_label(Color32::YELLOW, "Braid unavailable from the host agent");
                    ui.label("No live session, generation or mission counts have been received.");
                    ui.label(&state.agent_url);
                }
            }
            Dialog::Recordings => crate::views::evidence::render(ui, state),
        }
    }

    fn matrices(&mut self, ui: &mut egui::Ui, data: &Data) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.weight_delta, false, "Activity");
            ui.selectable_value(&mut self.weight_delta, true, "Weight delta");
        });
        if let Some(sketch) = data.sources.get("host-sketch") {
            panels::freshness(ui, sketch, 5000);
            if let Some(layers) = sketch.value["canonical"].as_array() {
                for pair in layers.chunks(2) {
                    ui.columns(2, |columns| {
                        for (layer, column) in pair.iter().zip(columns.iter_mut()) {
                            column.label(format!("L{} | seq {}", panels::text(&layer["layer"]), panels::text(&layer["sequence"])));
                            if layer["sequence"].as_u64().unwrap_or(0) == 0 { column.colored_label(Color32::YELLOW, "No published activity"); }
                            panels::matrix(column, if self.weight_delta {"Weight delta"} else {"Activation"}, &layer[if self.weight_delta {"weight_delta"} else {"activation"}]);
                            column.label(format!("Learning signal {}", panels::text(&layer["learning_signal"])));
                        }
                    });
                }
            }
            panels::object(ui, "Persistence", &sketch.value["persistence"]);
        }
    }

    fn lidar_panel(&mut self, ui: &mut egui::Ui, occupancy: bool) {
        ui.add(egui::Slider::new(&mut self.extent, 2.0..=12.0).text("radius (m)"));
        let Some(scan) = &self.scan else { ui.label("Waiting for timestamped scan"); return; };
        let now = source::now_ms();
        let fresh = scan.is_fresh(now, self.scan_advanced_ms) && self.scan_error.is_none();
        ui.colored_label(if fresh {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!(
            "{} | {} returns | {:.1} Hz | {} ms", if fresh {"Live lidar"} else {"STALE lidar"}, scan.points.len(), scan.rate_hz.unwrap_or(0.0), now.saturating_sub(scan.timestamp_ms)));
        ui.label(format!("{} | current scan | +/-{:.1} m", scan.frame, self.extent));
        if occupancy {
            if let Some((_, _, texture)) = &self.grid {
                let size = ui.available_width().min(ui.available_height().max(160.0) - 30.0).min(470.0);
                ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(size, size)));
                ui.label("Green occupied | teal measured free | dark unknown");
                ui.label(format!("160 x 160 | {:.3} m/cell | no accumulated map", 2.0 * self.extent / lidar::SIDE as f32));
            }
        } else { panels::scan_plot(ui, scan, self.extent); }
    }

    fn camera(&self, ui: &mut egui::Ui, data: &Data, height: f32) {
        ui.heading("Live camera");
        if let Some((stamp, texture)) = &self.camera {
            let age = source::now_ms().saturating_sub(*stamp);
            let stale = age > 1500 || data.camera_error.is_some();
            ui.colored_label(if stale {Color32::YELLOW} else {Color32::LIGHT_GREEN},
                format!("{} · received {} ms ago", if stale {"STALE camera"} else {"JPEG received"}, age));
            ui.add(egui::Image::new(texture).max_size(egui::vec2(ui.available_width(), height)));
        } else { ui.label("Waiting for camera/snapshot"); }
    }

}
