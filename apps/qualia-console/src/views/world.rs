//! World view: map and telemetry slots out of the same shared region.
//!
//! Traceability (`docs/frontend-lessons.md`, source 2): name the cadence and
//! keep the wire/ABI types with the client, not the widget. The structs here
//! are owned copies — the view draws snapshots, never a live mapping.

use egui::Ui;
use qualia_types::{PersistentMapGrid, VslamFrontendState};

use crate::{format_age_ms, ConsoleState};

/// Pose as the visual-SLAM front end publishes it.
#[derive(Debug, Clone, PartialEq)]
pub struct PoseReading {
    pub x_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub confidence: f32,
    pub timestamp_ns: u64,
}

impl PoseReading {
    pub fn from_frontend(frontend: &VslamFrontendState) -> Self {
        Self {
            x_m: frontend.pose_x_m,
            z_m: frontend.pose_z_m,
            yaw_rad: frontend.yaw_rad,
            confidence: frontend.pose_confidence,
            timestamp_ns: frontend.timestamp_ns,
        }
    }
}

/// The persistent map's counters.
#[derive(Debug, Clone, PartialEq)]
pub struct MapReading {
    pub width: u32,
    pub height: u32,
    pub resolution_m: f32,
    pub occupied_cells: u32,
    pub observed_cells: u32,
    pub seq: u64,
    pub last_update_ns: u64,
}

impl MapReading {
    pub fn from_grid(grid: &PersistentMapGrid) -> Self {
        Self {
            width: grid.width,
            height: grid.height,
            resolution_m: grid.resolution_m,
            occupied_cells: grid.occupied_cells,
            observed_cells: grid.observed_cells,
            seq: grid.seq.load(std::sync::atomic::Ordering::Acquire),
            last_update_ns: grid.last_update_ns,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldView {
    pub region: Option<String>,
    pub pose: Option<PoseReading>,
    pub map: Option<MapReading>,
    pub voxel_update_seq: Option<u64>,
    pub error: Option<String>,
}

impl WorldView {
    pub fn unattached(region: Option<String>, error: impl Into<String>) -> Self {
        Self {
            region,
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// Copy the pose, map counters and voxel sequence out of an attached region.
    pub fn sample(region: &qualia_shm::ShmRegion) -> Self {
        Self {
            region: None,
            pose: Some(PoseReading::from_frontend(region.vslam_frontend())),
            map: Some(MapReading::from_grid(region.map_grid())),
            voxel_update_seq: Some(
                region
                    .world_voxels()
                    .update_seq
                    .load(std::sync::atomic::Ordering::Acquire),
            ),
            error: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pose.is_none() && self.map.is_none() && self.voxel_update_seq.is_none()
    }
}

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    let view = &state.world;
    ui.heading("World");

    if let Some(error) = &view.error {
        ui.colored_label(
            egui::Color32::from_rgb(224, 160, 138),
            format!("world source error: {error}"),
        );
    }

    if view.is_empty() {
        ui.label("no world data");
        return;
    }

    match &view.pose {
        Some(pose) => {
            ui.label(format!(
                "pose: x {:.3} m, z {:.3} m, yaw {:.3} rad, confidence {:.2}",
                pose.x_m, pose.z_m, pose.yaw_rad, pose.confidence
            ));
            ui.label(format!(
                "pose age: {} ms",
                format_age_ms(state.observed_at_ns, pose.timestamp_ns)
            ));
        }
        None => {
            ui.label("pose: no fix");
        }
    }

    match &view.map {
        Some(map) => {
            ui.label(format!(
                "map: {} x {} cells at {:.3} m/cell",
                map.width, map.height, map.resolution_m
            ));
            ui.label(format!(
                "map occupancy: {} occupied, {} observed (seq {})",
                map.occupied_cells, map.observed_cells, map.seq
            ));
            ui.label(format!(
                "map age: {} ms",
                format_age_ms(state.observed_at_ns, map.last_update_ns)
            ));
        }
        None => {
            ui.label("map: not published");
        }
    }

    match view.voxel_update_seq {
        Some(seq) => ui.label(format!("voxels update seq: {seq}")),
        None => ui.label("voxels: not published"),
    };
}
