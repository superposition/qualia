//! Brain view: the 3D firing model and the belief matrices.
//!
//! This is the sixth view. One egui scene over three independently toggleable
//! layers, with the per-layer belief matrices beneath it:
//!
//! - **world** — the occupancy voxels the vision runner writes, and the floor
//!   grid, so the map is visible filling in;
//! - **cloud** — the live lidar returns (`runners/lidar`), per-point bearing and
//!   range, drawn as points in the accent ramp;
//! - **brain** — the connectome prior (`QUALIA_FLY_PRIOR_PATH`, or the committed
//!   `assets/brain/prior`) in its committed layout: a node is lit by the belief
//!   activity (`BeliefSlot.mean`) of the layer its type is assigned to, and an
//!   edge pulses by `weight × rate[source]` from the fly model's observe-only
//!   slot (`FlySimSlot`), which is exactly the coupling term `crates/fly-circuit`
//!   integrates;
//! - **connectome** — the released male-CNS connectome itself, read from the
//!   artifact the importer writes (`QUALIA_CONNECTOME_DIR`): 139,662 placed somata
//!   of 166,700 CSR nodes, coloured by cell type, with the firing set of the tick
//!   that is showing (`QUALIA_CONNECTOME_SPIKES`, a recorded `spikes.bin` or the
//!   runner's socket) drawn bright and larger on top — see
//!   [`connectome`];
//! - **matrices** — the per-layer generative weight matrix and belief vector as
//!   decimated heatmaps with a short time axis, and the braid markers on it.
//!
//! The scene is presentation and never measurement (`docs/frontend-lessons.md`
//! §"the 20 fps poll loop … presentation, not measurement"). It reads the region
//! and never writes it, every layer is capped, and the frame happens beside the
//! poller's own thread, so drawing cannot pace the runners.
//!
//! The prior carries no type-to-layer map, so the crossing is stated rather than
//! implied: type `t` reads layer `t % NUM_LAYERS`' belief slot, and its intensity
//! is that layer's decimated belief mean at index `t / NUM_LAYERS`. Node intensity
//! therefore follows live belief activity; the edge pulses render the published
//! fly rate vector faithfully, so while the fly drive is not wired (T30/T31) the
//! vector is zero and the pulses are flat — the view never fabricates motion.

pub mod connectome;
pub mod layout;
pub mod matrices;
pub mod prior;
pub mod scene;

use std::path::Path;
use std::sync::Arc;

use egui::{Color32, Ui};
use qualia_shm::ShmRegion;
use qualia_types::{
    FLY_SIM_MAX_TYPES, NUM_LAYERS, VOXEL_D, VOXEL_H, VOXEL_TOTAL, VOXEL_W,
};

use crate::{theme, BraidState, ConsoleState};

pub use layout::Layout;
pub use matrices::{LayerPoint, MatrixReading};
pub use prior::{PriorGraph, PriorSource};
pub use scene::{Camera, SceneCounts, SceneToggles};

/// The environment key a deployment points at its prior artifact.
pub const PRIOR_DIR_ENV: &str = prior::PRIOR_DIR_ENV;
/// The coupling dial's key (`QUALIA_FLY_COUPLING_SCALE`, T30).
pub const COUPLING_SCALE_ENV: &str = "QUALIA_FLY_COUPLING_SCALE";
/// The dial's default when nothing declares one.
pub const COUPLING_SCALE_DEFAULT: f32 = 1.0;
/// The manifest key that names the stack whose dial is observed.
pub const STACK_MANIFEST_ENV: &str = "QUALIA_STACK_MANIFEST";
/// Side of one voxel, in metres: the lattice covers roughly 8 m in each axis.
const VOXEL_METERS: f32 = 8.0 / VOXEL_W as f32;
/// Recent markers the panel lists.
const MARKERS_SHOWN: usize = 6;

/// What kind of event a timeline marker records.
///
/// The braid's wire state carries only the *last* promotion and the *last*
/// partials quarantine (`BraidState::last_promotion_ns`,
/// `BraidState::last_quarantine_ns`); a `PromotionRolledBack` has no timestamp
/// in that payload, so it is not invented here — the timeline marks what the
/// braid actually reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// The coupling dial was observed to change.
    CouplingScale,
    /// A generation was promoted (`BraidEvent::PromotionAccepted`).
    PromotionAccepted,
    /// Partial segments were quarantined (`BraidEvent::Quarantined`).
    PartialsQuarantined,
}

/// One mark on the belief-matrix time axis.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineMarker {
    pub kind: MarkerKind,
    pub timestamp_ns: u64,
    pub generation: Option<u64>,
    pub label: String,
}

impl TimelineMarker {
    pub fn colour(&self) -> Color32 {
        match self.kind {
            MarkerKind::CouplingScale => theme::TEXT_SECOND,
            MarkerKind::PromotionAccepted => theme::ACCENT,
            MarkerKind::PartialsQuarantined => theme::WARN,
        }
    }
}

/// The fly model's observe-only state, copied out of the region.
#[derive(Debug, Clone, PartialEq)]
pub struct FiringReading {
    pub sim_id: String,
    pub type_count: usize,
    pub sim_step: u64,
    pub producer_epoch: u64,
    pub timestamp_ns: u64,
    pub flags: u32,
    pub rates: Vec<f32>,
}

impl FiringReading {
    /// The largest rate magnitude, or zero while the model is at rest.
    pub fn peak(&self) -> f32 {
        self.rates
            .iter()
            .fold(0.0_f32, |peak, rate| peak.max(rate.abs()))
    }
}

/// The robot's pose on the floor plane, as the world layer needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanPose {
    pub x_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
}

/// The world layers: lidar returns, occupied voxels and the floor grid's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloudReading {
    /// Lidar returns in the world frame, as `[x, y, z]`.
    pub points: Vec<[f32; 3]>,
    /// Occupied voxel centres, capped at the scene's draw limit.
    pub voxels: Vec<[f32; 3]>,
    pub voxel_total: usize,
    pub lidar_timestamp_ns: u64,
    pub voxel_seq: Option<u64>,
    pub floor_seq: Option<u64>,
    pub pose: Option<PlanPose>,
}

/// Everything the brain view can say without a region or a braid.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainView {
    pub region: Option<String>,
    pub error: Option<String>,
    pub firing: Option<FiringReading>,
    pub layers: Vec<MatrixReading>,
    pub cloud: CloudReading,
    pub markers: Vec<TimelineMarker>,
    pub history: Vec<LayerPoint>,
    pub coupling_scale: Option<f32>,
    pub camera: Camera,
    pub toggles: SceneToggles,
    pub selected_layer: u8,
    pub counts: SceneCounts,
}

impl Default for BrainView {
    fn default() -> Self {
        Self {
            region: None,
            error: None,
            firing: None,
            layers: Vec::new(),
            cloud: CloudReading::default(),
            markers: Vec::new(),
            history: Vec::new(),
            coupling_scale: None,
            camera: Camera::default(),
            toggles: SceneToggles::default(),
            selected_layer: 0,
            counts: SceneCounts::default(),
        }
    }
}

impl BrainView {
    /// The region could not be attached; every layer is unavailable.
    pub fn unattached(region: Option<String>, error: impl Into<String>) -> Self {
        Self {
            region,
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// Copy the fly model, the belief matrices and the world layers out of an
    /// attached region. Pure: no environment, no clock, no braid.
    pub fn sample(region: &ShmRegion) -> Self {
        let firing = region
            .fly_sim()
            .snapshot(4)
            .ok()
            .filter(|payload| payload.type_count > 0 && payload.timestamp_ns != 0)
            .map(|payload| {
                let count = (payload.type_count as usize).min(FLY_SIM_MAX_TYPES);
                let end = payload.sim_id.iter().position(|byte| *byte == 0).unwrap_or(payload.sim_id.len());
                FiringReading {
                    sim_id: String::from_utf8_lossy(&payload.sim_id[..end]).into_owned(),
                    type_count: count,
                    sim_step: payload.sim_step,
                    producer_epoch: payload.producer_epoch,
                    timestamp_ns: payload.timestamp_ns,
                    flags: payload.flags,
                    rates: payload.state[..count].to_vec(),
                }
            });

        let layers = (0..NUM_LAYERS)
            .map(|layer| MatrixReading::sample(region, layer))
            .collect();

        Self {
            region: None,
            error: None,
            firing,
            layers,
            cloud: sample_cloud(region),
            markers: Vec::new(),
            history: Vec::new(),
            coupling_scale: None,
            camera: Camera::default(),
            toggles: SceneToggles::default(),
            selected_layer: 0,
            counts: SceneCounts::default(),
        }
    }

    /// The intensity the scene paints for each prior type, from the belief
    /// slots' activity.
    ///
    /// #173 step 2 names the belief slot as the node source, and the prior
    /// carries no type-to-layer map, so the crossing is fixed and stated: type
    /// `t` reads the `belief` slot of layer `t % NUM_LAYERS`, and its intensity
    /// is that layer's decimated belief mean (`BeliefSlot.mean`, the same 32×32
    /// decimation the matrix panels read) at index `t / NUM_LAYERS`. A layer the
    /// stack has not written leaves its types dark — the guard is the value
    /// itself, not a timestamp, so a zero belief is honestly zero.
    pub fn node_intensity(&self, type_count: usize) -> Vec<f32> {
        let mut intensity = vec![0.0_f32; type_count];
        for (node, value) in intensity.iter_mut().enumerate() {
            let layer = node % NUM_LAYERS;
            let index = (node / NUM_LAYERS) % matrices::MATRIX_CELLS;
            if let Some(reading) = self.layers.get(layer) {
                if let Some(belief) = reading.belief.get(index) {
                    *value = belief.abs();
                }
            }
        }
        intensity
    }

    /// Record the braid's promotion events as timeline markers.
    pub fn record_braid(&mut self, braid: &BraidState) {
        self.push_marker(TimelineMarker {
            kind: MarkerKind::PromotionAccepted,
            timestamp_ns: braid.last_promotion_ns,
            generation: Some(braid.generation),
            label: format!("PromotionAccepted g{}", braid.generation),
        });
        if let Some(quarantine) = braid.last_quarantine_ns {
            self.push_marker(TimelineMarker {
                kind: MarkerKind::PartialsQuarantined,
                timestamp_ns: quarantine,
                generation: Some(braid.generation),
                label: format!("PartialsQuarantined g{}", braid.generation),
            });
        }
    }

    /// Observe the coupling dial and mark a change on the axis.
    ///
    /// T30's dial lives in the environment the supervisor spawns the belief
    /// runners with, and the agent persists it in the stack manifest; the panel
    /// reads whichever is present, so a change the operator or an agent makes is
    /// visible as a marker rather than only as a different number.
    pub fn observe_coupling_scale(&mut self, observed_at_ns: u64) {
        let Some(scale) = coupling_scale() else {
            return;
        };
        let changed = self
            .coupling_scale
            .map(|previous| (previous - scale).abs() > f32::EPSILON)
            .unwrap_or(true);
        if changed && !self.timeline_has(MarkerKind::CouplingScale, observed_at_ns) {
            self.push_marker(TimelineMarker {
                kind: MarkerKind::CouplingScale,
                timestamp_ns: observed_at_ns,
                generation: None,
                label: format!("coupling scale {scale:.3}"),
            });
        }
        self.coupling_scale = Some(scale);
    }

    /// Fold a newer poll in, keeping the camera, the short history and the
    /// markers that earlier polls produced.
    pub fn absorb(&mut self, newer: BrainView) {
        for reading in &self.layers {
            if reading.is_written() {
                self.history.push(LayerPoint::from_reading(reading));
            }
        }
        self.trim_history();
        let camera = self.camera;
        let toggles = self.toggles;
        let selected_layer = self.selected_layer;
        let mut history = std::mem::take(&mut self.history);
        let markers = std::mem::take(&mut self.markers);
        *self = newer;
        self.camera = camera;
        self.toggles = toggles;
        self.selected_layer = selected_layer;
        history.append(&mut self.history);
        self.history = history;
        self.trim_history();
        for marker in markers {
            self.push_marker(marker);
        }
    }

    fn push_marker(&mut self, marker: TimelineMarker) {
        if marker.timestamp_ns == 0 {
            return;
        }
        if self
            .markers
            .iter()
            .any(|existing| existing.kind == marker.kind && existing.timestamp_ns == marker.timestamp_ns)
        {
            return;
        }
        self.markers.push(marker);
        if self.markers.len() > 256 {
            self.markers.remove(0);
        }
    }

    fn timeline_has(&self, kind: MarkerKind, timestamp_ns: u64) -> bool {
        self.markers
            .iter()
            .any(|marker| marker.kind == kind && marker.timestamp_ns == timestamp_ns)
    }

    fn trim_history(&mut self) {
        let cap = matrices::HISTORY_LEN * NUM_LAYERS;
        if self.history.len() > cap {
            let excess = self.history.len() - cap;
            self.history.drain(..excess);
        }
    }

    pub fn marker_count(&self) -> usize {
        self.markers.len()
    }
}

/// The prior and its layout, loaded once and shared by every frame.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainAssets {
    pub prior: Option<PriorGraph>,
    pub layout: Option<Layout>,
    pub error: Option<String>,
}

impl BrainAssets {
    /// The deployment's prior (`QUALIA_FLY_PRIOR_PATH`), or the committed one.
    pub fn load() -> Self {
        let (prior, mut error) = match prior_dir() {
            Some(path) => match PriorGraph::load_dir(&path) {
                Ok(prior) => (Some(prior), None),
                Err(reason) => (
                    PriorGraph::committed().ok(),
                    Some(format!("{reason}; drawing the committed prior")),
                ),
            },
            None => (PriorGraph::committed().ok(), None),
        };
        let layout = Layout::committed().ok().filter(|layout| {
            match prior.as_ref() {
                Some(prior) if layout.matches(prior) => true,
                Some(prior) => {
                    if error.is_none() {
                        error = Some(format!(
                            "the committed layout is not the layout of {}; \
                             regenerate it with `python assets/brain/make_layout.py`",
                            prior.source.label()
                        ));
                    }
                    false
                }
                None => true,
            }
        });
        if prior.is_none() && error.is_none() {
            error = Some("no prior artifact could be read".to_owned());
        }
        Self {
            prior,
            layout,
            error,
        }
    }
}

/// The prior directory a deployment names, if any.
pub fn prior_dir() -> Option<std::path::PathBuf> {
    std::env::var_os(PRIOR_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

/// The coupling dial, from the manifest the agent persists it in, else the
/// console's own environment.
pub fn coupling_scale() -> Option<f32> {
    if let Some(path) = std::env::var_os(STACK_MANIFEST_ENV).filter(|value| !value.is_empty()) {
        if let Ok(text) = std::fs::read_to_string(Path::new(&path)) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(scale) = value
                    .get("env")
                    .and_then(|env| env.get(COUPLING_SCALE_ENV))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|text| text.parse::<f32>().ok())
                {
                    return Some(scale);
                }
            }
        }
    }
    std::env::var(COUPLING_SCALE_ENV)
        .ok()
        .and_then(|text| text.parse::<f32>().ok())
}

fn sample_cloud(region: &ShmRegion) -> CloudReading {
    let pose = pose_reading(region);
    let lidar = region.lidar_scan().snapshot(4).ok();
    let (points, lidar_timestamp_ns) = match lidar {
        Some(scan) => {
            let count = (scan.point_count as usize).min(scan.points.len());
            let mut points = Vec::with_capacity(count);
            for point in &scan.points[..count] {
                let (sin, cos) = point.angle_rad.sin_cos();
                let (dx, dz) = (point.distance_m * cos, point.distance_m * sin);
                let (x, z) = match pose {
                    Some(pose) => {
                        let (sin_yaw, cos_yaw) = pose.yaw_rad.sin_cos();
                        (
                            pose.x_m + dx * cos_yaw - dz * sin_yaw,
                            pose.z_m + dx * sin_yaw + dz * cos_yaw,
                        )
                    }
                    None => (dx, dz),
                };
                points.push([x, 0.0, z]);
            }
            (points, scan.scan_end_ns)
        }
        None => (Vec::new(), 0),
    };

    let voxels = region.world_voxels();
    let voxel_seq = published_seq(&voxels.update_seq);
    let mut centres = Vec::new();
    let mut total = 0;
    for index in 0..VOXEL_TOTAL {
        if voxels.cells[index].occupancy == 0 {
            continue;
        }
        total += 1;
        if centres.len() >= scene::MAX_VOXELS_DRAWN {
            continue;
        }
        let vx = index / (VOXEL_D * VOXEL_H);
        let remainder = index % (VOXEL_D * VOXEL_H);
        let vz = remainder / VOXEL_H;
        let vy = remainder % VOXEL_H;
        centres.push([
            (vx as f32 - VOXEL_W as f32 / 2.0) * VOXEL_METERS,
            (vy as f32 - VOXEL_H as f32 / 2.0) * VOXEL_METERS,
            (vz as f32 - VOXEL_D as f32 / 2.0) * VOXEL_METERS,
        ]);
    }

    CloudReading {
        points,
        voxels: centres,
        voxel_total: total,
        lidar_timestamp_ns,
        voxel_seq,
        floor_seq: published_seq(&region.camera_floor().seq),
        pose,
    }
}

fn pose_reading(region: &ShmRegion) -> Option<PlanPose> {
    let frontend = region.vslam_frontend();
    published_seq(&frontend.seq).map(|_| PlanPose {
        x_m: frontend.pose_x_m,
        z_m: frontend.pose_z_m,
        yaw_rad: frontend.yaw_rad,
    })
}

fn published_seq(seq: &std::sync::atomic::AtomicU64) -> Option<u64> {
    match seq.load(std::sync::atomic::Ordering::Acquire) {
        0 => None,
        published => Some(published),
    }
}

/// Draw the sixth panel: the scene, its layer toggles, and the matrices.
pub fn render(ui: &mut Ui, state: &mut ConsoleState) {
    let assets: Arc<BrainAssets> = Arc::clone(&state.brain_assets);
    let view = &mut state.brain;

    if let Some(error) = &view.error {
        theme::error_banner(ui, &format!("brain source error: {error}"));
    }
    theme::field_path(ui, "shm region", view.region.as_deref().unwrap_or(theme::DASH));
    match assets.prior.as_ref() {
        Some(prior) => {
            theme::field_path(ui, "prior", &prior.source.label());
            theme::field(
                ui,
                "prior graph",
                &format!("{} types, {} edges", prior.type_count(), prior.edge_count()),
                None,
            );
        }
        None => theme::state_line(ui, "prior: none readable", theme::TEXT_SECOND),
    }
    if let Some(error) = &assets.error {
        theme::state_line(ui, error, theme::WARN);
    }
    match assets.layout.as_ref() {
        Some(layout) => theme::field(
            ui,
            "layout",
            &format!(
                "{} ({} nodes, seed {})",
                layout.algorithm,
                layout.node_count(),
                layout.seed
            ),
            None,
        ),
        None => theme::state_line(ui, "layout: not derived from this prior", theme::WARN),
    }

    let cloud_state = connectome::cloud();
    let connectome_cloud = match &cloud_state {
        connectome::CloudState::Ready(cloud) => Some(Arc::clone(cloud)),
        _ => None,
    };
    let connectome_frame = connectome::frame();

    match &cloud_state {
        connectome::CloudState::Ready(cloud) => {
            theme::field_path(ui, "cloud artifact", &cloud.directory);
            theme::field(
                ui,
                "cloud counts",
                &format!(
                    "{} nodes, {} placed, {} types",
                    cloud.node_count, cloud.placed, cloud.types
                ),
                Some("male CNS v1.0"),
            );
            theme::field(ui, "cloud source", &cloud.source, None);
        }
        connectome::CloudState::Unset => theme::state_line(
            ui,
            "cloud: set QUALIA_CONNECTOME_DIR to the artifact directory",
            theme::TEXT_SECOND,
        ),
        connectome::CloudState::Failed(error) => {
            theme::state_line(ui, &format!("cloud: {error}"), theme::WARN)
        }
    }
    if let Some(error) = &connectome_frame.error {
        theme::state_line(ui, &format!("spike stream: {error}"), theme::WARN);
    } else if connectome_frame.ticks_read == 0 {
        theme::state_line(
            ui,
            "spike stream: set QUALIA_CONNECTOME_SPIKES to spikes.bin or tcp://host:port",
            theme::TEXT_SECOND,
        );
    } else {
        theme::field(
            ui,
            "spike stream",
            &format!(
                "tick {}, {:.1} frames/s, {} firing",
                connectome_frame.tick,
                connectome_frame.rate_hz,
                connectome_frame.firing_count()
            ),
            None,
        );
        theme::field(ui, "spike source", &connectome_frame.source, None);
    }

    ui.add_space(theme::GAP_S);
    ui.horizontal(|ui| {
        ui.checkbox(&mut view.toggles.brain, "connectome");
        ui.checkbox(&mut view.toggles.connectome, "male-CNS cloud");
        ui.checkbox(&mut view.toggles.cloud, "point cloud");
        ui.checkbox(&mut view.toggles.world, "voxels");
        ui.checkbox(&mut view.toggles.floor, "floor grid");
    });

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), scene::SCENE_HEIGHT), egui::Sense::drag());
    if response.dragged() {
        view.camera.orbit(response.drag_delta());
    }
    if response.hovered() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            view.camera.zoom_by((scroll * 0.002).exp());
        }
    }
    let painter = ui.painter_at(rect);
    let counts = scene::paint(
        &painter,
        rect,
        &view.camera,
        view.toggles,
        view,
        &assets,
        connectome_cloud.as_deref(),
        &connectome_frame,
    );
    view.counts = counts;

    match view.firing.as_ref() {
        Some(firing) => {
            theme::field(
                ui,
                "fly sim",
                &format!(
                    "{} types, step {}, epoch {}",
                    firing.type_count, firing.sim_step, firing.producer_epoch
                ),
                None,
            );
            theme::field(ui, "fly peak rate", &format!("{:.4}", firing.peak()), None);
            theme::field(ui, "fly age", &theme::age(state.observed_at_ns, firing.timestamp_ns), None);
        }
        None => theme::state_line(
            ui,
            "fly sim: not published (QUALIA_FLY_MODE=sim needed)",
            theme::TEXT_SECOND,
        ),
    }
    theme::field(
        ui,
        "node intensity",
        &format!("belief mean (layer = type mod {NUM_LAYERS}, index = type div {NUM_LAYERS})"),
        None,
    );
    theme::field(
        ui,
        "lidar points",
        &view.cloud.points.len().to_string(),
        Some("returns"),
    );
    theme::field(
        ui,
        "voxels",
        &format!("{} of {} drawn", view.counts.voxels_drawn, view.cloud.voxel_total),
        None,
    );
    if let Some(pose) = view.cloud.pose {
        theme::field(
            ui,
            "robot pose",
            &format!("{:.2}, {:.2}, {:.2}", pose.x_m, pose.z_m, pose.yaw_rad),
            Some("m, m, rad"),
        );
    }
    theme::field(
        ui,
        "scene draw",
        &format!(
            "{} nodes, {} edges (of {} / {})",
            view.counts.nodes_drawn,
            view.counts.edges_drawn,
            view.counts.nodes_total,
            view.counts.edges_total
        ),
        None,
    );

    theme::field(
        ui,
        "cloud draw",
        &format!(
            "{} of {} points, {} of {} firing",
            view.counts.connectome_drawn,
            view.counts.connectome_placed,
            view.counts.firing_drawn,
            view.counts.firing_total
        ),
        None,
    );

    ui.add_space(theme::GAP_S);
    ui.horizontal(|ui| {
        theme::state_line(ui, "layer", theme::MUTED);
        for layer in 0..NUM_LAYERS {
            let selected = view.selected_layer == layer as u8;
            if ui.selectable_label(selected, format!("L{layer}")).clicked() {
                view.selected_layer = layer as u8;
            }
        }
    });

    ui.add_space(theme::GAP_S);
    matrices::render(
        ui,
        &view.layers,
        &view.history,
        &view.markers,
        view.selected_layer,
    );

    ui.add_space(theme::GAP_S);
    theme::state_line(ui, "braid markers", theme::MUTED);
    match view.coupling_scale {
        Some(scale) => theme::field(ui, "coupling scale", &format!("{scale:.3}"), None),
        None => theme::state_line(ui, "coupling scale: not declared", theme::TEXT_SECOND),
    }
    for marker in view.markers.iter().rev().take(MARKERS_SHOWN) {
        theme::state_line(ui, &marker.label, marker.colour());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_region_reports_absent_layers_not_zeroes() {
        let name = format!("/qualia_console_brain_{}", std::process::id());
        let region = ShmRegion::create(&name).expect("create region");
        let view = BrainView::sample(&region);
        assert!(view.firing.is_none(), "an unpublished sim is absent");
        assert!(view.cloud.points.is_empty());
        assert_eq!(view.layers.len(), NUM_LAYERS);
        assert!(view.layers.iter().all(|layer| !layer.is_written()));
    }

    #[test]
    fn node_intensity_follows_the_belief_slot_of_its_layer() {
        let name = format!("/qualia_console_brain_intensity_{}", std::process::id());
        let region = ShmRegion::create(&name).expect("create region");
        let type_count = 5;

        // Nothing written: every node is dark, not merely small.
        let dark = BrainView::sample(&region);
        assert!(
            dark.node_intensity(type_count)
                .iter()
                .all(|value| *value == 0.0),
            "an unwritten region must not light a node"
        );

        // Write layer 0's belief mean: type 0 (layer 0, index 0) lights, while
        // type 1 reads layer 1 — still unwritten — and stays dark.
        {
            let writer = qualia_shm::LayerWriter::new(region.layer_slot(0));
            let slot = writer.back_buffer();
            slot.mean[0] = 0.75;
            slot.layer = 0;
            slot.timestamp_ns = 42;
            writer.publish();
        }
        let lit = BrainView::sample(&region);
        let intensity = lit.node_intensity(type_count);
        assert!(
            (intensity[0] - 0.75).abs() < 1e-6,
            "type 0 must read layer 0's belief mean, got {}",
            intensity[0]
        );
        assert_eq!(intensity[1], 0.0, "an unwritten layer leaves its type dark");

        // Zero the layer and publish: the node goes dark again.
        {
            let writer = qualia_shm::LayerWriter::new(region.layer_slot(0));
            let slot = writer.back_buffer();
            slot.mean[0] = 0.0;
            slot.timestamp_ns = 43;
            writer.publish();
        }
        let zeroed = BrainView::sample(&region);
        assert_eq!(
            zeroed.node_intensity(type_count)[0],
            0.0,
            "a zero belief frame is dark, not carried over"
        );
    }

    #[test]
    fn the_braid_marks_only_events_that_happened() {
        let braid = BraidState {
            schema_version: "qualia.braid-state.v1".to_owned(),
            generation: 4,
            session_id: "s".to_owned(),
            open_missions: 0,
            last_promotion_ns: 1_000,
            last_quarantine_ns: None,
        };
        let mut view = BrainView::default();
        view.record_braid(&braid);
        assert_eq!(view.markers.len(), 1);
        assert_eq!(view.markers[0].kind, MarkerKind::PromotionAccepted);
        view.record_braid(&braid);
        assert_eq!(view.markers.len(), 1, "the same event is not marked twice");
    }

    fn written_layers(residual: f32, timestamp_ns: u64) -> Vec<MatrixReading> {
        (0..NUM_LAYERS)
            .map(|layer| MatrixReading {
                layer: layer as u8,
                weight: vec![1.0; matrices::MATRIX_CELLS],
                belief: vec![0.0; matrices::MATRIX_CELLS],
                vfe: 0.0,
                residual_norm: residual + layer as f32,
                timestamp_ns,
            })
            .collect()
    }

    #[test]
    fn absorb_keeps_the_camera_and_appends_history() {
        let mut older = BrainView {
            camera: Camera {
                yaw: 1.25,
                ..Camera::default()
            },
            layers: written_layers(0.5, 10),
            ..BrainView::default()
        };
        let newer = BrainView {
            camera: Camera {
                yaw: -3.0,
                ..Camera::default()
            },
            layers: written_layers(1.5, 20),
            ..BrainView::default()
        };
        older.absorb(newer);
        assert_eq!(older.camera.yaw, 1.25, "the operator's camera survives a poll");
        assert_eq!(older.history.len(), NUM_LAYERS, "the outgoing frame is kept");
        assert_eq!(older.layers[0].residual_norm, 1.5, "the newer reading lands");
        assert_eq!(older.history[0].residual_norm, 0.5);
    }

    #[test]
    fn absorb_appends_history_and_caps_it() {
        let mut view = BrainView {
            layers: written_layers(0.0, 1),
            ..BrainView::default()
        };
        for step in 2..=100 {
            view.absorb(BrainView {
                layers: written_layers(0.0, step),
                ..BrainView::default()
            });
        }
        assert_eq!(
            view.history.len(),
            matrices::HISTORY_LEN * NUM_LAYERS,
            "the short history is capped, not unbounded"
        );
    }
}
