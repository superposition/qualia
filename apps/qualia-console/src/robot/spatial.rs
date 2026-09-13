use egui::{Color32, Stroke, Ui};
use serde_json::Value;
use super::{panels, source::Data};

pub struct SpatialView { perspective: bool, yaw: f32, radius: f32 }
impl Default for SpatialView {
    fn default() -> Self { Self { perspective: false, yaw: 0.5, radius: 8.0 } }
}

fn point(v: &Value) -> Option<[f32; 3]> {
    let component = |index: usize, key: &str| v.get(index).or_else(|| v.get(key))
        .or_else(|| v.get(key.trim_end_matches("_m")))?.as_f64()
        .filter(|v| v.is_finite() && v.abs() <= f32::MAX as f64).map(|v| v as f32);
    Some([component(0, "x_m")?, component(1, "y_m").unwrap_or(0.0), component(2, "z_m")?])
}

impl SpatialView {
    pub fn render(&mut self, ui: &mut Ui, data: &Data) {
        ui.horizontal(|ui| {
            ui.heading("Spatial map + routes");
            ui.selectable_value(&mut self.perspective, false, "Top");
            ui.selectable_value(&mut self.perspective, true, "Perspective");
            ui.add(egui::Slider::new(&mut self.radius, 2.0..=30.0).text("radius (m)"));
        });
        let Some(reading) = data.sources.get("host-costmap") else { ui.label("Waiting for planner/costmap"); return; };
        panels::freshness(ui, reading, 5000);
        let map = &reading.value;
        ui.label(format!("Frame {} · {} observed cells · {} occupied · {}", panels::text(&map["frame_id"]),
            panels::text(&map["observed_count"]), panels::text(&map["occupied_count"]), panels::text(&map["voxel_source"])));
        if reading.error.is_some() || map.is_null() { ui.label("Map source unavailable; current lidar remains available below."); return; }
        if map["observed_count"].as_u64().unwrap_or(0) == 0 && map["lidar_timestamp_ns"].as_u64().unwrap_or(0) == 0 {
            ui.colored_label(Color32::YELLOW, "The host has received no spatial observations.");
        }
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 380.0), egui::Sense::drag());
        if response.dragged() { self.yaw += response.drag_delta().x * 0.008; }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(17, 25, 38));
        let center = point(&map["robot_pose"]).unwrap_or([0.0; 3]);
        let scale = rect.height() / (self.radius * 2.0);
        let project = |p: [f32; 3]| {
            let x = p[0] - center[0]; let y = p[1] - center[1]; let z = p[2] - center[2];
            let (x, z) = if self.perspective { (x * self.yaw.cos() - z * self.yaw.sin(), x * self.yaw.sin() + z * self.yaw.cos()) } else { (x, z) };
            rect.center() + egui::vec2(x * scale, if self.perspective {z * scale * 0.5 - y * scale} else {-z * scale})
        };
        for meter in -(self.radius as i32)..=self.radius as i32 {
            let m = meter as f32;
            painter.line_segment([project([center[0] + m, center[1], center[2] - self.radius]), project([center[0] + m, center[1], center[2] + self.radius])], Stroke::new(1.0, Color32::from_gray(45)));
            painter.line_segment([project([center[0] - self.radius, center[1], center[2] + m]), project([center[0] + self.radius, center[1], center[2] + m])], Stroke::new(1.0, Color32::from_gray(45)));
        }
        for (key, color, size) in [
            ("lidar_points", Color32::from_rgb(88,234,190), 2.0),
            ("sensor_voxel_points", Color32::from_rgb(88,180,220), 3.0),
            ("voxel_points", Color32::from_rgb(88,180,220), 3.0),
            ("visual_landmarks", Color32::from_rgb(255,180,90), 3.0),
        ] {
            if let Some(points) = map[key].as_array() { for p in points.iter().take(100000).filter_map(point) {
                let position = project(p);
                if rect.contains(position) { painter.circle_filled(position, size, color); }
            }}
        }
        for (key, color) in [("actual_travel_points", Color32::WHITE), ("leash_path_points", Color32::YELLOW), ("path", Color32::LIGHT_BLUE)] {
            if let Some(points) = map[key].as_array() {
                let projected: Vec<_> = points.iter().take(10000).filter_map(point).map(project).collect();
                for pair in projected.windows(2) { painter.line_segment([pair[0], pair[1]], Stroke::new(2.0, color)); }
            }
        }
        if map["lidar_timestamp_ns"].as_u64().unwrap_or(0) > 0 {
            let origin = project(center);
            painter.circle_filled(origin, 5.0, Color32::WHITE);
        }
        ui.label("White: actual travel · yellow: Leash route · blue: planner route · green: lidar · orange: visual landmarks");
        egui::CollapsingHeader::new("Fusion, calibration and spatial evidence").show(ui, |ui| panels::object(ui, "Fusion", &map["fusion"]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn point_accepts_wire_objects_and_triples_but_not_missing_axes() {
        assert_eq!(point(&serde_json::json!({"x_m":1,"y_m":2,"z_m":3})), Some([1.0,2.0,3.0]));
        assert_eq!(point(&serde_json::json!({"x":1,"y":2,"z":3})), Some([1.0,2.0,3.0]));
        assert_eq!(point(&serde_json::json!([1,2,3])), Some([1.0,2.0,3.0]));
        assert_eq!(point(&serde_json::json!({"x_m":1})), None);
    }
}
