//! World view: map and telemetry slots out of the same shared region.
//!
//! Traceability (`docs/frontend-lessons.md`, source 2): name the cadence and
//! keep the wire/ABI types with the client, not the widget. The structs here
//! are owned copies — the view draws snapshots, never a live mapping.

use std::sync::atomic::{AtomicU64, Ordering};

use egui::Ui;
use qualia_types::{PersistentMapGrid, VslamFrontendState};

use crate::{theme, ConsoleState};

/// A slot's publish sequence, or `None` before its first publish.
///
/// Every seqlocked slot reads zero until a runner publishes into it, which is
/// what separates "nothing published yet" from a reading whose own numbers are
/// zero. `views/telemetry.rs` gates its frames on the same rule; each World
/// slot gates on it so an attached but never-written region renders its absent
/// arms instead of a pose at the origin.
fn published_seq(seq: &AtomicU64) -> Option<u64> {
    match seq.load(Ordering::Acquire) {
        0 => None,
        published => Some(published),
    }
}

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
    /// The front end's pose, or `None` until the VSLAM runner publishes one.
    pub fn from_frontend(frontend: &VslamFrontendState) -> Option<Self> {
        published_seq(&frontend.seq).map(|_| Self {
            x_m: frontend.pose_x_m,
            z_m: frontend.pose_z_m,
            yaw_rad: frontend.yaw_rad,
            confidence: frontend.pose_confidence,
            timestamp_ns: frontend.timestamp_ns,
        })
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
    /// The map counters, or `None` until a runner publishes the grid.
    pub fn from_grid(grid: &PersistentMapGrid) -> Option<Self> {
        published_seq(&grid.seq).map(|seq| Self {
            width: grid.width,
            height: grid.height,
            resolution_m: grid.resolution_m,
            occupied_cells: grid.occupied_cells,
            observed_cells: grid.observed_cells,
            seq,
            last_update_ns: grid.last_update_ns,
        })
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
    ///
    /// Each is `None` until its own slot has been published to, so an attached
    /// but never-written region keeps the absent arms below instead of drawing
    /// zeroes as readings.
    pub fn sample(region: &qualia_shm::ShmRegion) -> Self {
        Self {
            region: None,
            pose: PoseReading::from_frontend(region.vslam_frontend()),
            map: MapReading::from_grid(region.map_grid()),
            voxel_update_seq: published_seq(&region.world_voxels().update_seq),
            error: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pose.is_none() && self.map.is_none() && self.voxel_update_seq.is_none()
    }
}

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    let view = &state.world;

    if let Some(error) = &view.error {
        theme::error_banner(ui, &format!("world source error: {error}"));
    }

    theme::field_path(
        ui,
        "shm region",
        view.region.as_deref().unwrap_or(theme::DASH),
    );

    // "No world data" means no source at all. An attached region whose slots
    // are all still unpublished is not empty: it renders the three absent arms
    // below, so the operator reads "no fix" rather than zeroes.
    if view.error.is_some() && view.is_empty() {
        theme::state_line(ui, "no world data", theme::TEXT_SECOND);
        return;
    }

    ui.add_space(theme::GAP_S);
    theme::hairline(ui);
    ui.add_space(theme::GAP_S);

    match &view.pose {
        Some(pose) => {
            theme::field(ui, "pose x", &format!("{:.3}", pose.x_m), Some("m"));
            theme::field(ui, "pose z", &format!("{:.3}", pose.z_m), Some("m"));
            theme::field(ui, "pose yaw", &format!("{:.3}", pose.yaw_rad), Some("rad"));
            theme::field(ui, "pose confidence", &format!("{:.2}", pose.confidence), None);
            theme::field(
                ui,
                "pose age",
                &theme::age(state.observed_at_ns, pose.timestamp_ns),
                None,
            );
        }
        None => theme::state_line(ui, "pose: no fix", theme::TEXT_SECOND),
    }

    ui.add_space(theme::GAP_S);
    theme::hairline(ui);
    ui.add_space(theme::GAP_S);

    match &view.map {
        Some(map) => {
            theme::field(
                ui,
                "map size",
                &format!("{} x {}", map.width, map.height),
                Some("cells"),
            );
            theme::field(ui, "map resolution", &format!("{:.3}", map.resolution_m), Some("m/cell"));
            theme::field(ui, "map occupied", &map.occupied_cells.to_string(), None);
            theme::field(ui, "map observed", &map.observed_cells.to_string(), None);
            theme::field(ui, "map seq", &map.seq.to_string(), None);
            theme::field(
                ui,
                "map age",
                &theme::age(state.observed_at_ns, map.last_update_ns),
                None,
            );
        }
        None => theme::state_line(ui, "map: not published", theme::TEXT_SECOND),
    }

    ui.add_space(theme::GAP_S);
    theme::hairline(ui);
    ui.add_space(theme::GAP_S);

    match view.voxel_update_seq {
        Some(seq) => theme::field(ui, "voxels update seq", &seq.to_string(), None),
        None => theme::state_line(ui, "voxels: not published", theme::TEXT_SECOND),
    }
}
