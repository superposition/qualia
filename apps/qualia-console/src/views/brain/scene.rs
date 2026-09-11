//! The 3D scene: one orthographic camera over the world layers and the graph.
//!
//! egui is a 2D painter, so the scene is drawn by projecting each 3D point
//! through a small camera and painting lines and circles — no second GPU pass,
//! no `unsafe`. The layers are independently toggleable and each is capped, so a
//! prior with thousands of types cannot make the frame unbounded; the panel
//! reports what it drew and what it dropped.

use egui::{Painter, Pos2, Rect, Stroke, Vec2};

use crate::theme;

use super::layout::Layout;
use super::prior::PriorGraph;
use super::{BrainAssets, BrainView};

/// Height of the scene canvas, above the matrix panels.
pub const SCENE_HEIGHT: f32 = 216.0;
/// Nodes drawn per frame; the rest are counted as dropped.
pub const MAX_NODES_DRAWN: usize = 4096;
/// Edges drawn per frame; the rest are counted as dropped.
pub const MAX_EDGES_DRAWN: usize = 6000;
/// Lidar returns drawn per frame (the slot itself holds 720).
pub const MAX_POINTS_DRAWN: usize = 720;
/// Occupied voxels drawn per frame; the rest are counted as dropped.
pub const MAX_VOXELS_DRAWN: usize = 3000;

/// Which of the scene's layers are showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneToggles {
    pub world: bool,
    pub cloud: bool,
    pub brain: bool,
    pub floor: bool,
}

impl Default for SceneToggles {
    fn default() -> Self {
        Self {
            world: true,
            cloud: true,
            brain: true,
            floor: true,
        }
    }
}

/// An orbit camera: yaw and pitch around the origin, a zoom on the projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -0.6,
            pitch: 0.5,
            zoom: 1.0,
        }
    }
}

impl Camera {
    /// Rotate `point` into camera space: yaw about Y, then pitch about X.
    pub fn rotate(&self, point: [f32; 3]) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let x = point[0] * cos_yaw + point[2] * sin_yaw;
        let z = -point[0] * sin_yaw + point[2] * cos_yaw;
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let y = point[1] * cos_pitch - z * sin_pitch;
        let z = point[1] * sin_pitch + z * cos_pitch;
        [x, y, z]
    }

    /// Orbit by a drag, in pixels.
    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw += delta.x * 0.01;
        self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.45, 1.45);
    }

    /// Scale the zoom, clamped so the scene cannot be lost.
    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.25, 8.0);
    }
}

/// Projects world points into a screen rectangle, orthographically.
pub struct Projector {
    centre: Pos2,
    scale: f32,
    camera: Camera,
}

impl Projector {
    pub fn new(rect: Rect, camera: Camera) -> Self {
        let scale = rect.width().min(rect.height()) * 0.42 * camera.zoom;
        Self {
            centre: rect.center(),
            scale,
            camera,
        }
    }

    /// A world point to a screen position and a depth (`+z` toward the camera).
    pub fn project(&self, point: [f32; 3]) -> (Pos2, f32) {
        let [x, y, z] = self.camera.rotate(point);
        (
            Pos2::new(self.centre.x + x * self.scale, self.centre.y - y * self.scale),
            z,
        )
    }

    /// Nearer points read brighter; the range is the unit layout's own span.
    pub fn depth_alpha(depth: f32) -> f32 {
        (0.45 + 0.27 * (depth + 1.0).clamp(0.0, 2.0)).clamp(0.2, 1.0)
    }
}

/// The scene's own counts for one frame: what was drawn and what was dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SceneCounts {
    pub nodes_drawn: usize,
    pub nodes_total: usize,
    pub edges_drawn: usize,
    pub edges_total: usize,
    pub points_drawn: usize,
    pub voxels_drawn: usize,
    pub voxels_total: usize,
}

/// Draw the scene into `rect`, returning what was drawn.
pub fn paint(
    painter: &Painter,
    rect: Rect,
    camera: &Camera,
    toggles: SceneToggles,
    view: &BrainView,
    assets: &BrainAssets,
) -> SceneCounts {
    painter.rect_filled(rect, 0.0, theme::BG);
    let projector = Projector::new(rect, *camera);
    let mut counts = SceneCounts::default();

    if toggles.floor {
        paint_floor(painter, &projector);
    }
    if toggles.world {
        counts.voxels_total = view.cloud.voxels.len();
        for (index, voxel) in view.cloud.voxels.iter().enumerate().take(MAX_VOXELS_DRAWN) {
            let (position, depth) = projector.project(*voxel);
            if !rect.contains(position) {
                continue;
            }
            let intensity = 0.45 + 0.55 * ((index % 7) as f32 / 6.0);
            let colour = theme::ramp(intensity).gamma_multiply(Projector::depth_alpha(depth) * 0.9);
            painter.rect_filled(Rect::from_center_size(position, Vec2::splat(3.0)), 0.0, colour);
            counts.voxels_drawn += 1;
        }
    }
    if toggles.cloud {
        counts.points_drawn = view.cloud.points.len().min(MAX_POINTS_DRAWN);
        for point in view.cloud.points.iter().take(MAX_POINTS_DRAWN) {
            let (position, depth) = projector.project(*point);
            if !rect.contains(position) {
                continue;
            }
            let colour = theme::ACCENT.gamma_multiply(Projector::depth_alpha(depth));
            painter.circle_filled(position, 1.6, colour);
        }
    }
    if toggles.brain {
        if let (Some(prior), Some(layout)) = (assets.prior.as_ref(), assets.layout.as_ref()) {
            paint_graph(painter, rect, &projector, view, prior, layout, &mut counts);
        }
    }

    counts
}

fn paint_floor(painter: &Painter, projector: &Projector) {
    let stroke = Stroke::new(1.0, theme::SEP);
    let half = 1.0_f32;
    for step in -4..=4 {
        let offset = step as f32 * (half / 4.0);
        let a = projector.project([-half, -1.0, offset]).0;
        let b = projector.project([half, -1.0, offset]).0;
        painter.line_segment([a, b], stroke);
        let c = projector.project([offset, -1.0, -half]).0;
        let d = projector.project([offset, -1.0, half]).0;
        painter.line_segment([c, d], stroke);
    }
}

fn paint_graph(
    painter: &Painter,
    rect: Rect,
    projector: &Projector,
    view: &BrainView,
    prior: &PriorGraph,
    layout: &Layout,
    counts: &mut SceneCounts,
) {
    let node_count = prior.type_count().min(layout.node_count());
    let rates = view.firing.as_ref().map(|firing| firing.rates.as_slice());
    let peak = rates
        .map(|rates| {
            rates[..rates.len().min(node_count)]
                .iter()
                .fold(0.0_f32, |m, v| m.max(v.abs()))
        })
        .unwrap_or(0.0)
        .max(f32::EPSILON);
    let max_weight = prior.weights.iter().copied().max().unwrap_or(1).max(1) as f32;

    // Edges first, so nodes sit on top of the pulses they send.
    counts.edges_total = prior.edge_count();
    let stride = prior.edge_count().div_ceil(MAX_EDGES_DRAWN).max(1);
    for (edge, (source, destination, weight)) in prior.edges().enumerate() {
        if edge % stride != 0
            || source >= layout.node_count()
            || destination >= layout.node_count()
        {
            continue;
        }
        let source_rate = rates
            .map(|rates| rates.get(source).copied().unwrap_or(0.0).abs())
            .unwrap_or(0.0);
        let flux = (weight as f32 * source_rate / (peak * max_weight)).clamp(0.0, 1.0);
        let from = projector.project(layout.positions[source]).0;
        let to = projector.project(layout.positions[destination]).0;
        if !rect.intersects(Rect::from_two_pos(from, to)) {
            continue;
        }
        let colour = theme::ramp(0.35 + 0.65 * flux).gamma_multiply(0.12 + 0.55 * flux);
        painter.line_segment([from, to], Stroke::new(0.8 + 1.4 * flux, colour));
        counts.edges_drawn += 1;
    }

    counts.nodes_total = prior.type_count();
    for node in 0..node_count.min(MAX_NODES_DRAWN) {
        let (position, depth) = projector.project(layout.positions[node]);
        if !rect.contains(position) {
            continue;
        }
        let rate = rates
            .map(|rates| rates.get(node).copied().unwrap_or(0.0).abs())
            .unwrap_or(0.0);
        let intensity = (rate / peak).clamp(0.0, 1.0);
        let colour = theme::ramp(intensity).gamma_multiply(Projector::depth_alpha(depth));
        painter.circle_filled(position, 3.0 + 4.0 * intensity, colour);
        counts.nodes_drawn += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_camera_projects_the_origin_to_the_centre() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(200.0, 100.0));
        let projector = Projector::new(rect, Camera::default());
        let (origin, depth) = projector.project([0.0, 0.0, 0.0]);
        assert_eq!(origin, rect.center());
        assert_eq!(depth, 0.0);
    }

    #[test]
    fn the_pitch_is_clamped_so_the_scene_cannot_flip() {
        let mut camera = Camera::default();
        camera.orbit(Vec2::new(0.0, 10_000.0));
        assert!(camera.pitch <= 1.45 && camera.pitch >= -1.45);
    }

    #[test]
    fn a_full_turn_is_the_identity() {
        let camera = Camera {
            yaw: std::f32::consts::TAU,
            pitch: 0.0,
            zoom: 1.0,
        };
        let rotated = camera.rotate([0.3, -0.2, 0.5]);
        for (value, expected) in rotated.iter().zip([0.3_f32, -0.2, 0.5]) {
            assert!((value - expected).abs() < 1e-5, "{rotated:?}");
        }
    }
}
