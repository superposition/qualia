//! `qualia-types` is the shared-memory ABI root of the workspace.
//!
//! Every type here is a plain C layout that another process maps directly:
//! field order, widths and alignment are contractual, not stylistic. Anything
//! that crosses a process boundary is `#[repr(C)]` and either POD or a
//! seqlocked snapshot; the seqlock writers bump the sequence odd before
//! copying and even after, and readers reject any read that straddles a write.
//!
//! The crate holds no state and performs no I/O. It also declares the
//! workspace's manifest vocabulary (`StackManifest` and friends) so a runner
//! and its supervisor cannot disagree about the JSON shape.

use serde::Deserialize;
use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

mod entity;
pub use entity::*;

pub const STATE_DIM: usize = 1024;
pub const SHM_MAGIC: u64 = 0x5155414C3141454E; // "QUAL1AEN"
pub const SHM_VERSION: u32 = 3;
pub const JEPA_ABI_VERSION: u32 = 1;
pub const NUM_LAYERS: usize = 8;
pub const JEPA_CORE_DIM: usize = 256;
pub const JEPA_EVIDENCE_DIM: usize = STATE_DIM;
pub const JEPA_OCCUPANCY_W: usize = 64;
pub const JEPA_OCCUPANCY_H: usize = 64;
pub const JEPA_OCCUPANCY_CELLS: usize = JEPA_OCCUPANCY_W * JEPA_OCCUPANCY_H;
pub const JEPA_ID_BYTES: usize = 64;
pub const JEPA_ERROR_BYTES: usize = 256;
pub const APPLIED_ACTION_HISTORY_CAPACITY: usize = 4_096;

/// ABI version stamped into [`FlySimPayload::abi_version`].
pub const FLY_SIM_ABI_VERSION: u32 = 1;
/// Capacity of one published fly rate vector: one `f32` per prior type.
///
/// The slot is a fixed-size mapping, so the publisher refuses a graph that
/// does not fit rather than truncating the observation. The whole-brain
/// FlyWire annotation names 8,453 cell types, which fits here with headroom.
pub const FLY_SIM_MAX_TYPES: usize = 16_384;
/// Bytes of the simulator identifier in [`FlySimPayload::sim_id`].
pub const FLY_SIM_ID_BYTES: usize = 64;
/// Bytes of the last error message in [`FlySimPayload::last_error`].
pub const FLY_SIM_ERROR_BYTES: usize = 256;

pub const JEPA_BACKEND_NONE: u32 = 0;
pub const JEPA_BACKEND_CPU: u32 = 1;
pub const JEPA_BACKEND_METAL: u32 = 2;
pub const JEPA_BACKEND_CUDA: u32 = 3;
pub const JEPA_MODE_UNAVAILABLE: u32 = 0;
pub const JEPA_MODE_REPLAY: u32 = 1;
pub const JEPA_MODE_OBSERVE_ONLY: u32 = 2;
pub const JEPA_FLAG_VALID: u32 = 1 << 0;
pub const JEPA_FLAG_SOURCES_COHERENT: u32 = 1 << 1;
pub const JEPA_FLAG_CHECKPOINT_VERIFIED: u32 = 1 << 2;
pub const JEPA_FLAG_OUTPUT_FINITE: u32 = 1 << 3;
pub const JEPA_FLAG_GROUNDING_AVAILABLE: u32 = 1 << 4;
pub const JEPA_FLAG_ACTION_COVERED: u32 = 1 << 5;

// ── World voxel grid ─────────────────────────────────────────────────
// A 32×32×12 lattice over roughly 8 m × 8 m × 3 m, filled by the vision
// runner from camera frames and object detections.

pub const VOXEL_W: usize = 32; // cells along X
pub const VOXEL_D: usize = 32; // cells along Z, away from the camera
pub const VOXEL_H: usize = 12; // cells along Y
pub const VOXEL_TOTAL: usize = VOXEL_W * VOXEL_D * VOXEL_H;

/// One occupancy cell with a representative colour.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VoxelCell {
    pub occupancy: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Room-scale voxel grid.
///
/// Index mapping: `idx = vx * VOXEL_D * VOXEL_H + vz * VOXEL_H + vy`.
#[repr(C)]
pub struct WorldVoxels {
    pub cells: [VoxelCell; VOXEL_TOTAL],
    pub update_seq: AtomicU64,
}

// ── Lidar ────────────────────────────────────────────────────────────

pub const LIDAR_MAX_POINTS: usize = 720;
pub const LIDAR_GRID_W: usize = 256;
pub const LIDAR_GRID_H: usize = 256;
pub const LIDAR_GRID_CELLS: usize = LIDAR_GRID_W * LIDAR_GRID_H;
pub const MAP_GRID_W: usize = 256;
pub const MAP_GRID_H: usize = 256;
pub const MAP_GRID_CELLS: usize = MAP_GRID_W * MAP_GRID_H;
pub const CAMERA_THUMB_W: usize = 64;
pub const CAMERA_THUMB_H: usize = 48;
pub const CAMERA_THUMB_PIXELS: usize = CAMERA_THUMB_W * CAMERA_THUMB_H;
pub const CAMERA_PREVIEW_MAX_BYTES: usize = 512 * 1024;

/// A single polar lidar return.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LidarPoint {
    pub angle_rad: f32,
    pub distance_m: f32,
    pub intensity: u8,
    pub _pad: [u8; 3],
}

/// Seqlocked polar scan published by the lidar runner.
#[repr(C)]
pub struct LidarScan {
    pub seq: AtomicU64,
    pub scan_start_ns: u64,
    pub scan_end_ns: u64,
    pub point_count: u32,
    pub _pad0: u32,
    pub points: [LidarPoint; LIDAR_MAX_POINTS],
}

/// Why a seqlocked read did not produce a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    WriterBusy,
    TornRead,
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WriterBusy => write!(formatter, "snapshot writer is already active"),
            Self::TornRead => write!(formatter, "snapshot changed while it was being read"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Owned copy of one coherent [`LidarScan`].
#[derive(Clone)]
pub struct LidarScanSnapshot {
    pub seq: u64,
    pub scan_start_ns: u64,
    pub scan_end_ns: u64,
    pub point_count: u32,
    pub points: [LidarPoint; LIDAR_MAX_POINTS],
}

impl Default for LidarScanSnapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            scan_start_ns: 0,
            scan_end_ns: 0,
            point_count: 0,
            points: [LidarPoint {
                angle_rad: 0.0,
                distance_m: 0.0,
                intensity: 0,
                _pad: [0; 3],
            }; LIDAR_MAX_POINTS],
        }
    }
}

impl LidarScan {
    /// Publishes a scan. Returns the new even sequence number.
    pub fn publish(&mut self, snapshot: &LidarScanSnapshot) -> Result<u64, SnapshotError> {
        let current = self.seq.load(Ordering::Acquire);
        if current & 1 != 0 {
            return Err(SnapshotError::WriterBusy);
        }
        let writing = current.wrapping_add(1);
        self.seq
            .compare_exchange(current, writing, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SnapshotError::WriterBusy)?;
        self.scan_start_ns = snapshot.scan_start_ns;
        self.scan_end_ns = snapshot.scan_end_ns;
        self.point_count = snapshot.point_count.min(LIDAR_MAX_POINTS as u32);
        self.points.copy_from_slice(&snapshot.points);
        let published = writing.wrapping_add(1);
        self.seq.store(published, Ordering::Release);
        Ok(published)
    }

    /// Takes an owned copy, retrying at most `max_attempts` times.
    pub fn snapshot(&self, max_attempts: usize) -> Result<LidarScanSnapshot, SnapshotError> {
        for _ in 0..max_attempts.max(1) {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            // SAFETY: the sequence word brackets the plain fields; a writer
            // holds it odd for the whole copy, so no torn value is observed.
            let scan_start_ns = unsafe { std::ptr::read_volatile(&self.scan_start_ns) };
            let scan_end_ns = unsafe { std::ptr::read_volatile(&self.scan_end_ns) };
            let point_count = unsafe { std::ptr::read_volatile(&self.point_count) };
            let mut points = LidarScanSnapshot::default().points;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.points.as_ptr(),
                    points.as_mut_ptr(),
                    LIDAR_MAX_POINTS,
                );
            }
            fence(Ordering::Acquire);
            let after = self.seq.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Ok(LidarScanSnapshot {
                    seq: after,
                    scan_start_ns,
                    scan_end_ns,
                    point_count: point_count.min(LIDAR_MAX_POINTS as u32),
                    points,
                });
            }
        }
        Err(SnapshotError::TornRead)
    }
}

/// Seqlocked occupancy grid derived from lidar.
#[repr(C)]
pub struct LidarOccupancyGrid {
    pub seq: AtomicU64,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f32,
    pub origin_x_m: f32,
    pub origin_y_m: f32,
    pub cells: [u8; LIDAR_GRID_CELLS],
}

/// Owned copy of one coherent [`LidarOccupancyGrid`].
#[derive(Clone)]
pub struct LidarOccupancyGridSnapshot {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f32,
    pub origin_x_m: f32,
    pub origin_y_m: f32,
    pub cells: [u8; LIDAR_GRID_CELLS],
}

impl Default for LidarOccupancyGridSnapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            width: LIDAR_GRID_W as u32,
            height: LIDAR_GRID_H as u32,
            resolution_m: 0.0,
            origin_x_m: 0.0,
            origin_y_m: 0.0,
            cells: [0; LIDAR_GRID_CELLS],
        }
    }
}

impl LidarOccupancyGrid {
    /// Publishes a grid. Returns the new even sequence number.
    pub fn publish(&mut self, snapshot: &LidarOccupancyGridSnapshot) -> Result<u64, SnapshotError> {
        let current = self.seq.load(Ordering::Acquire);
        if current & 1 != 0 {
            return Err(SnapshotError::WriterBusy);
        }
        let writing = current.wrapping_add(1);
        self.seq
            .compare_exchange(current, writing, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SnapshotError::WriterBusy)?;
        self.width = snapshot.width;
        self.height = snapshot.height;
        self.resolution_m = snapshot.resolution_m;
        self.origin_x_m = snapshot.origin_x_m;
        self.origin_y_m = snapshot.origin_y_m;
        self.cells.copy_from_slice(&snapshot.cells);
        let published = writing.wrapping_add(1);
        self.seq.store(published, Ordering::Release);
        Ok(published)
    }

    /// Takes an owned copy, retrying at most `max_attempts` times.
    pub fn snapshot(
        &self,
        max_attempts: usize,
    ) -> Result<LidarOccupancyGridSnapshot, SnapshotError> {
        for _ in 0..max_attempts.max(1) {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let width = unsafe { std::ptr::read_volatile(&self.width) };
            let height = unsafe { std::ptr::read_volatile(&self.height) };
            let resolution_m = unsafe { std::ptr::read_volatile(&self.resolution_m) };
            let origin_x_m = unsafe { std::ptr::read_volatile(&self.origin_x_m) };
            let origin_y_m = unsafe { std::ptr::read_volatile(&self.origin_y_m) };
            let mut cells = [0; LIDAR_GRID_CELLS];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.cells.as_ptr(),
                    cells.as_mut_ptr(),
                    LIDAR_GRID_CELLS,
                );
            }
            fence(Ordering::Acquire);
            let after = self.seq.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Ok(LidarOccupancyGridSnapshot {
                    seq: after,
                    width,
                    height,
                    resolution_m,
                    origin_x_m,
                    origin_y_m,
                    cells,
                });
            }
        }
        Err(SnapshotError::TornRead)
    }
}

/// Seqlocked log-odds map accumulated across the run.
#[repr(C)]
pub struct PersistentMapGrid {
    pub seq: AtomicU64,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f32,
    pub origin_x_m: f32,
    pub origin_y_m: f32,
    pub last_update_ns: u64,
    pub occupied_cells: u32,
    pub observed_cells: u32,
    pub _pad0: u32,
    pub log_odds: [i16; MAP_GRID_CELLS],
}

/// Seqlocked binary occupancy map.
#[repr(C)]
pub struct BinaryMapGrid {
    pub seq: AtomicU64,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f32,
    pub origin_x_m: f32,
    pub origin_y_m: f32,
    pub last_update_ns: u64,
    pub occupied_cells: u32,
    pub free_cells: u32,
    pub unknown_cells: u32,
    pub cells: [u8; MAP_GRID_CELLS],
}

/// Seqlocked camera metadata plus a downscaled luma thumbnail.
#[repr(C)]
pub struct CameraFrame {
    pub seq: AtomicU64,
    pub timestamp_ns: u64,
    pub source_width: u32,
    pub source_height: u32,
    pub thumb_width: u32,
    pub thumb_height: u32,
    pub luminance_mean: f32,
    pub luminance_stddev: f32,
    pub valid: AtomicBool,
    pub _pad0: [u8; 7],
    pub thumbnail_luma: [u8; CAMERA_THUMB_PIXELS],
}

/// Owned copy of one coherent [`CameraFrame`].
#[derive(Clone)]
pub struct CameraFrameSnapshot {
    pub seq: u64,
    pub timestamp_ns: u64,
    pub source_width: u32,
    pub source_height: u32,
    pub thumb_width: u32,
    pub thumb_height: u32,
    pub luminance_mean: f32,
    pub luminance_stddev: f32,
    pub valid: bool,
    pub thumbnail_luma: [u8; CAMERA_THUMB_PIXELS],
}

impl Default for CameraFrameSnapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            timestamp_ns: 0,
            source_width: 0,
            source_height: 0,
            thumb_width: CAMERA_THUMB_W as u32,
            thumb_height: CAMERA_THUMB_H as u32,
            luminance_mean: 0.0,
            luminance_stddev: 0.0,
            valid: false,
            thumbnail_luma: [0; CAMERA_THUMB_PIXELS],
        }
    }
}

impl CameraFrame {
    /// Publishes a frame. Returns the new even sequence number.
    pub fn publish(&mut self, snapshot: &CameraFrameSnapshot) -> Result<u64, SnapshotError> {
        let current = self.seq.load(Ordering::Acquire);
        if current & 1 != 0 {
            return Err(SnapshotError::WriterBusy);
        }
        let writing = current.wrapping_add(1);
        self.seq
            .compare_exchange(current, writing, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SnapshotError::WriterBusy)?;
        self.timestamp_ns = snapshot.timestamp_ns;
        self.source_width = snapshot.source_width;
        self.source_height = snapshot.source_height;
        self.thumb_width = snapshot.thumb_width;
        self.thumb_height = snapshot.thumb_height;
        self.luminance_mean = snapshot.luminance_mean;
        self.luminance_stddev = snapshot.luminance_stddev;
        self.thumbnail_luma
            .copy_from_slice(&snapshot.thumbnail_luma);
        self.valid.store(snapshot.valid, Ordering::Relaxed);
        let published = writing.wrapping_add(1);
        self.seq.store(published, Ordering::Release);
        Ok(published)
    }

    /// Takes an owned copy, retrying at most `max_attempts` times.
    pub fn snapshot(&self, max_attempts: usize) -> Result<CameraFrameSnapshot, SnapshotError> {
        for _ in 0..max_attempts.max(1) {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            // SAFETY: see LidarScan::snapshot — the writer keeps the sequence
            // odd across every field copy, and the equal even re-read admits
            // only complete frames.
            let timestamp_ns = unsafe { std::ptr::read_volatile(&self.timestamp_ns) };
            let source_width = unsafe { std::ptr::read_volatile(&self.source_width) };
            let source_height = unsafe { std::ptr::read_volatile(&self.source_height) };
            let thumb_width = unsafe { std::ptr::read_volatile(&self.thumb_width) };
            let thumb_height = unsafe { std::ptr::read_volatile(&self.thumb_height) };
            let luminance_mean = unsafe { std::ptr::read_volatile(&self.luminance_mean) };
            let luminance_stddev = unsafe { std::ptr::read_volatile(&self.luminance_stddev) };
            let valid = self.valid.load(Ordering::Relaxed);
            let mut thumbnail_luma = [0; CAMERA_THUMB_PIXELS];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.thumbnail_luma.as_ptr(),
                    thumbnail_luma.as_mut_ptr(),
                    CAMERA_THUMB_PIXELS,
                );
            }
            fence(Ordering::Acquire);
            let after = self.seq.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Ok(CameraFrameSnapshot {
                    seq: after,
                    timestamp_ns,
                    source_width,
                    source_height,
                    thumb_width,
                    thumb_height,
                    luminance_mean,
                    luminance_stddev,
                    valid,
                    thumbnail_luma,
                });
            }
        }
        Err(SnapshotError::TornRead)
    }
}

pub const ACTION_SAFETY_ESTOP: u32 = 1 << 0;
pub const ACTION_SAFETY_STALE_POSE: u32 = 1 << 1;
pub const ACTION_SAFETY_STALE_LIDAR: u32 = 1 << 2;
pub const ACTION_SAFETY_GOAL_INACTIVE: u32 = 1 << 3;
pub const ACTION_SAFETY_COLLISION_CLAMP: u32 = 1 << 4;
pub const ACTION_SAFETY_PROGRESS_STALL: u32 = 1 << 5;
pub const ACTION_SAFETY_GOAL_REACHED: u32 = 1 << 6;
pub const ACTION_SAFETY_DISARMED: u32 = 1 << 7;
pub const ACTION_SAFETY_TRANSPORT_ERROR: u32 = 1 << 8;

// Bits in `safety_flags` are authority-scoped. The LEASH_* bits mirror the
// external `qualia.applied-action.v1` contract Leash emits and are kept apart
// from the legacy qualia-drive planner bits above.
pub const LEASH_ACTION_SAFETY_COLLISION_CLAMP: u32 = 1 << 0;
pub const LEASH_ACTION_SAFETY_SOFT_ODOMETRY_LIMIT: u32 = 1 << 1;
pub const LEASH_ACTION_SAFETY_ESTOP: u32 = 1 << 2;
pub const LEASH_ACTION_SAFETY_DEADMAN: u32 = 1 << 3;
pub const LEASH_ACTION_SAFETY_VERIFIED_ZERO: u32 = 1 << 4;
pub const ACTION_AUTHORITY_UNKNOWN: u32 = 0;
pub const ACTION_AUTHORITY_LEASH: u32 = 1;
pub const ACTION_AUTHORITY_QUALIA_DRIVE: u32 = 2;

/// Owned copy of one completed action interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedActionSnapshot {
    pub slot_seq: u64,
    pub producer_epoch: u64,
    pub action_sequence: u64,
    pub interval_start_ns: u64,
    pub interval_end_ns: u64,
    pub requested_left: f32,
    pub requested_right: f32,
    pub clamped_left: f32,
    pub clamped_right: f32,
    pub applied_left: f32,
    pub applied_right: f32,
    pub speed_scale: f32,
    pub safety_flags: u32,
    pub authority: u32,
    pub valid: bool,
    pub armed: bool,
    pub deadman_active: bool,
    pub collision_clamped: bool,
}

impl Default for AppliedActionSnapshot {
    fn default() -> Self {
        Self {
            slot_seq: 0,
            producer_epoch: 0,
            action_sequence: 0,
            interval_start_ns: 0,
            interval_end_ns: 0,
            requested_left: 0.0,
            requested_right: 0.0,
            clamped_left: 0.0,
            clamped_right: 0.0,
            applied_left: 0.0,
            applied_right: 0.0,
            speed_scale: 0.0,
            safety_flags: 0,
            authority: ACTION_AUTHORITY_UNKNOWN,
            valid: false,
            armed: false,
            deadman_active: false,
            collision_clamped: false,
        }
    }
}

/// Post-safety completed action interval.
///
/// Every payload word is atomic so a reader and the single writer never share
/// a plain field. The seqlock accepts a snapshot only across an equal, even
/// sequence pair.
#[repr(C, align(64))]
pub struct AppliedActionSlot {
    pub seq: AtomicU64,
    producer_epoch: AtomicU64,
    action_sequence: AtomicU64,
    interval_start_ns: AtomicU64,
    interval_end_ns: AtomicU64,
    requested_left: AtomicU32,
    requested_right: AtomicU32,
    clamped_left: AtomicU32,
    clamped_right: AtomicU32,
    applied_left: AtomicU32,
    applied_right: AtomicU32,
    speed_scale: AtomicU32,
    safety_flags: AtomicU32,
    authority: AtomicU32,
    valid: AtomicU8,
    armed: AtomicU8,
    deadman_active: AtomicU8,
    collision_clamped: AtomicU8,
}

impl AppliedActionSlot {
    /// Publishes one interval. Returns the new even sequence number.
    pub fn publish(&self, snapshot: AppliedActionSnapshot) -> Result<u64, SnapshotError> {
        let current = self.seq.load(Ordering::Acquire);
        if current & 1 != 0 {
            return Err(SnapshotError::WriterBusy);
        }
        let writing = current.wrapping_add(1);
        self.seq
            .compare_exchange(current, writing, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SnapshotError::WriterBusy)?;
        self.producer_epoch
            .store(snapshot.producer_epoch, Ordering::Relaxed);
        self.action_sequence
            .store(snapshot.action_sequence, Ordering::Relaxed);
        self.interval_start_ns
            .store(snapshot.interval_start_ns, Ordering::Relaxed);
        self.interval_end_ns
            .store(snapshot.interval_end_ns, Ordering::Relaxed);
        self.requested_left
            .store(snapshot.requested_left.to_bits(), Ordering::Relaxed);
        self.requested_right
            .store(snapshot.requested_right.to_bits(), Ordering::Relaxed);
        self.clamped_left
            .store(snapshot.clamped_left.to_bits(), Ordering::Relaxed);
        self.clamped_right
            .store(snapshot.clamped_right.to_bits(), Ordering::Relaxed);
        self.applied_left
            .store(snapshot.applied_left.to_bits(), Ordering::Relaxed);
        self.applied_right
            .store(snapshot.applied_right.to_bits(), Ordering::Relaxed);
        self.speed_scale
            .store(snapshot.speed_scale.to_bits(), Ordering::Relaxed);
        self.safety_flags
            .store(snapshot.safety_flags, Ordering::Relaxed);
        self.authority.store(snapshot.authority, Ordering::Relaxed);
        self.valid
            .store(u8::from(snapshot.valid), Ordering::Relaxed);
        self.armed
            .store(u8::from(snapshot.armed), Ordering::Relaxed);
        self.deadman_active
            .store(u8::from(snapshot.deadman_active), Ordering::Relaxed);
        self.collision_clamped
            .store(u8::from(snapshot.collision_clamped), Ordering::Relaxed);
        let published = writing.wrapping_add(1);
        self.seq.store(published, Ordering::Release);
        Ok(published)
    }

    /// Takes an owned copy, retrying at most `max_attempts` times.
    pub fn snapshot(&self, max_attempts: usize) -> Result<AppliedActionSnapshot, SnapshotError> {
        for _ in 0..max_attempts.max(1) {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let snapshot = AppliedActionSnapshot {
                slot_seq: before,
                producer_epoch: self.producer_epoch.load(Ordering::Relaxed),
                action_sequence: self.action_sequence.load(Ordering::Relaxed),
                interval_start_ns: self.interval_start_ns.load(Ordering::Relaxed),
                interval_end_ns: self.interval_end_ns.load(Ordering::Relaxed),
                requested_left: f32::from_bits(self.requested_left.load(Ordering::Relaxed)),
                requested_right: f32::from_bits(self.requested_right.load(Ordering::Relaxed)),
                clamped_left: f32::from_bits(self.clamped_left.load(Ordering::Relaxed)),
                clamped_right: f32::from_bits(self.clamped_right.load(Ordering::Relaxed)),
                applied_left: f32::from_bits(self.applied_left.load(Ordering::Relaxed)),
                applied_right: f32::from_bits(self.applied_right.load(Ordering::Relaxed)),
                speed_scale: f32::from_bits(self.speed_scale.load(Ordering::Relaxed)),
                safety_flags: self.safety_flags.load(Ordering::Relaxed),
                authority: self.authority.load(Ordering::Relaxed),
                valid: self.valid.load(Ordering::Relaxed) != 0,
                armed: self.armed.load(Ordering::Relaxed) != 0,
                deadman_active: self.deadman_active.load(Ordering::Relaxed) != 0,
                collision_clamped: self.collision_clamped.load(Ordering::Relaxed) != 0,
            };
            fence(Ordering::Acquire);
            let after = self.seq.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Ok(snapshot);
            }
        }
        Err(SnapshotError::TornRead)
    }
}

/// Why an entry could not be read from [`AppliedActionHistory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppliedActionHistoryError {
    WriterBusy,
    SequenceExhausted,
    NotCommitted { requested: u64, committed: u64 },
    Overrun { requested: u64, oldest: u64 },
    TornRead { sequence: u64 },
}

impl std::fmt::Display for AppliedActionHistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WriterBusy => {
                write!(formatter, "applied-action history writer is already active")
            }
            Self::SequenceExhausted => {
                write!(formatter, "applied-action history sequence exhausted")
            }
            Self::NotCommitted {
                requested,
                committed,
            } => write!(
                formatter,
                "applied-action history sequence {requested} is not committed (latest {committed})"
            ),
            Self::Overrun { requested, oldest } => write!(
                formatter,
                "applied-action history sequence {requested} was overwritten (oldest {oldest})"
            ),
            Self::TornRead { sequence } => write!(
                formatter,
                "applied-action history sequence {sequence} changed while it was being read"
            ),
        }
    }
}

impl std::error::Error for AppliedActionHistoryError {}

/// Lossless single-writer ring of completed post-safety action intervals.
///
/// `write_seq` is the newest committed absolute sequence. Each ring entry
/// carries its own absolute tag beside the per-entry seqlock payload: a writer
/// invalidates a slot before reuse and only then publishes the global
/// sequence, so a reader either sees the exact interval it asked for or gets
/// [`AppliedActionHistoryError::Overrun`] — never a silently skipped one.
#[repr(C, align(64))]
pub struct AppliedActionHistory {
    write_seq: AtomicU64,
    _pad0: [u8; 56],
    entry_seq: [AtomicU64; APPLIED_ACTION_HISTORY_CAPACITY],
    entries: [AppliedActionSlot; APPLIED_ACTION_HISTORY_CAPACITY],
}

impl AppliedActionHistory {
    /// The newest committed sequence, or zero before the first append.
    pub fn committed_seq(&self) -> u64 {
        self.write_seq.load(Ordering::Acquire)
    }

    /// Appends one interval and returns its absolute sequence number.
    ///
    /// Callers must serialize writers across threads and processes; the
    /// per-entry seqlock only detects a contract violation, it does not
    /// remove one.
    pub fn append(
        &self,
        snapshot: AppliedActionSnapshot,
    ) -> Result<u64, AppliedActionHistoryError> {
        let committed = self.write_seq.load(Ordering::Acquire);
        let sequence = committed
            .checked_add(1)
            .ok_or(AppliedActionHistoryError::SequenceExhausted)?;
        let index = (sequence as usize - 1) % APPLIED_ACTION_HISTORY_CAPACITY;

        self.entry_seq[index].store(0, Ordering::Release);
        self.entries[index]
            .publish(snapshot)
            .map_err(|_| AppliedActionHistoryError::WriterBusy)?;
        self.entry_seq[index].store(sequence, Ordering::Release);
        self.write_seq.store(sequence, Ordering::Release);
        Ok(sequence)
    }

    /// Reads one interval by absolute sequence, retrying `max_attempts` times.
    pub fn snapshot(
        &self,
        sequence: u64,
        max_attempts: usize,
    ) -> Result<AppliedActionSnapshot, AppliedActionHistoryError> {
        let committed = self.write_seq.load(Ordering::Acquire);
        if sequence == 0 || sequence > committed {
            return Err(AppliedActionHistoryError::NotCommitted {
                requested: sequence,
                committed,
            });
        }
        let oldest = committed
            .saturating_sub(APPLIED_ACTION_HISTORY_CAPACITY as u64)
            .saturating_add(1);
        if sequence < oldest {
            return Err(AppliedActionHistoryError::Overrun {
                requested: sequence,
                oldest,
            });
        }

        let index = (sequence as usize - 1) % APPLIED_ACTION_HISTORY_CAPACITY;
        for _ in 0..max_attempts.max(1) {
            let before = self.entry_seq[index].load(Ordering::Acquire);
            if before != sequence {
                std::hint::spin_loop();
                continue;
            }
            let mut snapshot = match self.entries[index].snapshot(1) {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    std::hint::spin_loop();
                    continue;
                }
            };
            fence(Ordering::Acquire);
            let after = self.entry_seq[index].load(Ordering::Acquire);
            if before == after && after == sequence {
                // A history read reports the absolute ring sequence, not the
                // reusable entry's local seqlock counter.
                snapshot.slot_seq = sequence;
                return Ok(snapshot);
            }
        }

        let committed = self.write_seq.load(Ordering::Acquire);
        let oldest = committed
            .saturating_sub(APPLIED_ACTION_HISTORY_CAPACITY as u64)
            .saturating_add(1);
        if sequence < oldest {
            Err(AppliedActionHistoryError::Overrun {
                requested: sequence,
                oldest,
            })
        } else {
            Err(AppliedActionHistoryError::TornRead { sequence })
        }
    }
}

/// Newest bounded encoded camera frame for operator preview.
///
/// `seq` is a seqlock: odd while the writer copies bytes, even when a reader
/// may take them. Format 1 is JPEG and 2 is PNG.
#[repr(C)]
pub struct CameraPreview {
    pub seq: AtomicU64,
    pub timestamp_ns: u64,
    pub width: u32,
    pub height: u32,
    pub format: u8,
    pub _pad0: [u8; 7],
    pub len: AtomicUsize,
    pub bytes: [u8; CAMERA_PREVIEW_MAX_BYTES],
}

/// Seqlocked front-end state from the visual SLAM runner.
#[repr(C)]
pub struct VslamFrontendState {
    pub seq: AtomicU64,
    pub frame_seq: u64,
    pub timestamp_ns: u64,
    pub tracking_ok: AtomicBool,
    pub _pad0: [u8; 7],
    pub feature_count: u32,
    pub keyframe_count: u32,
    pub loop_closure_count: u32,
    pub tracking_confidence: f32,
    pub mean_gradient: f32,
    pub mean_cornerness: f32,
    pub pose_x_m: f32,
    pub pose_z_m: f32,
    pub yaw_rad: f32,
    pub pose_confidence: f32,
    pub motion_dx_cells: f32,
    pub motion_dz_cells: f32,
    pub motion_yaw_delta_rad: f32,
    pub pose_writer_active: u8,
    pub _pad1: [u8; 3],
}

/// Seqlocked floor-confidence grid derived from the camera.
#[repr(C)]
pub struct CameraFloorGrid {
    pub seq: AtomicU64,
    pub width: u32,
    pub height: u32,
    pub confidence_scale: f32,
    pub last_update_ns: u64,
    pub cells: [u8; VOXEL_W * VOXEL_D],
}

// ── World model ──────────────────────────────────────────────────────

pub const MAX_OBJECTS: usize = 16;
pub const MAX_SCENE_LEN: usize = 512;
pub const MAX_DIRECTIVE_LEN: usize = 256;
pub const MAX_ACTIVITY_LEN: usize = 128;
pub const MAX_OBJECT_NAME: usize = 32;

/// Canonical robot pose in world coordinates.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NavPose {
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub pitch_rad: f32,
    pub roll_rad: f32,
    pub confidence: f32,
    pub _pad0: f32,
    pub timestamp_ns: u64,
}

/// Operator- or planner-chosen goal in world and grid coordinates.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NavGoal {
    pub active: u8,
    pub _pad0: [u8; 3],
    pub cell_x: i32,
    pub cell_z: i32,
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
    pub timestamp_ns: u64,
}

/// A detected object in the scene.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WorldObject {
    /// Null-terminated ASCII label such as `person\0` or `mug\0`.
    pub name: [u8; MAX_OBJECT_NAME],
    pub confidence: f32,
    /// Normalised position in frame, `0.0..=1.0`.
    pub x: f32,
    pub y: f32,
    /// Whether this slot currently holds an object.
    pub active: u8,
    pub _pad: [u8; 3],
}

/// Qualia's accumulated semantic understanding of its surroundings.
///
/// It lives in shared memory so every layer and the TUI read the same copy.
#[repr(C)]
pub struct WorldModel {
    /// Detected objects in the scene.
    pub objects: [WorldObject; MAX_OBJECTS],
    pub num_objects: u32,
    pub _pad0: u32,

    /// Scene description from the language model, null-terminated UTF-8.
    pub scene: [u8; MAX_SCENE_LEN],
    /// Current activity description, null-terminated UTF-8.
    pub activity: [u8; MAX_ACTIVITY_LEN],

    /// 1024-dim embedding of the scene, injected into high layers' beliefs.
    pub scene_embedding: [f32; STATE_DIM],

    /// Top-level goal, null-terminated UTF-8; set by the operator.
    pub directive: [u8; MAX_DIRECTIVE_LEN],

    pub last_vision_ns: u64,
    pub last_llm_ns: u64,
    pub llm_call_count: u64,
    pub vision_frame_count: u64,

    pub gemini_input_tokens: u64,
    pub gemini_output_tokens: u64,
    pub gemini_embedding_tokens: u64,

    /// Canonical live pose in map/world coordinates.
    pub robot_pose: NavPose,
    /// Operator- or planner-selected goal.
    pub nav_goal: NavGoal,
    /// Sequence number for pose/goal updates.
    pub nav_seq: AtomicU64,

    /// Incremented on each world-model update.
    pub update_seq: AtomicU64,
}

// ── Thought stream ───────────────────────────────────────────────────

pub const MAX_THOUGHT_LEN: usize = 256;
pub const MAX_THOUGHTS: usize = 512;

/// One entry in a layer's thought stream.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ThoughtEntry {
    pub text: [u8; MAX_THOUGHT_LEN],
    pub layer: u8,
    /// 0=observe, 1=predict, 2=surprise, 3=learn, 4=resolve, 5=escalate.
    pub kind: u8,
    pub _pad: [u8; 2],
    pub vfe: f32,
    pub timestamp_ns: u64,
    pub seq: u64,
}

/// Append-only ring of thoughts.
#[repr(C)]
pub struct ThoughtBuffer {
    pub write_seq: AtomicU64,
    pub entries: [ThoughtEntry; MAX_THOUGHTS],
}

// ── Lore ─────────────────────────────────────────────────────────────

pub const MAX_LORE_TEXT: usize = 512;
pub const MAX_LORE_QUESTION: usize = 256;
pub const MAX_LORE_ENTRIES: usize = 128;
pub const MAX_QUESTION_TEXT: usize = 256;

/// A question a layer poses outward when it cannot explain its own error.
///
/// The vision runner harvests pending questions and batches them into model
/// calls, clearing `pending` once an answer is recorded as lore.
#[repr(C)]
pub struct QuestionSlot {
    /// Question text, null-terminated UTF-8.
    pub text: [u8; MAX_QUESTION_TEXT],
    /// Layer asking the question.
    pub layer: u8,
    /// 0=high_vfe, 1=compression_plateau, 2=novel_pattern, 3=escalation.
    pub reason: u8,
    pub _pad: [u8; 2],
    /// VFE at the moment the question was raised.
    pub vfe: f32,
    /// Set while unanswered; cleared by the harvester.
    pub pending: AtomicBool,
    pub _pad2: [u8; 7],
    pub timestamp_ns: u64,
}

/// One answered question, kept as long-term semantic memory.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoreEntry {
    /// The question that produced this entry.
    pub question: [u8; MAX_LORE_QUESTION],
    /// The answer text.
    pub answer: [u8; MAX_LORE_TEXT],
    /// Layer that asked.
    pub layer: u8,
    /// Reason code, matching [`QuestionSlot`].
    pub reason: u8,
    /// How far the embedding moved when this lore was integrated.
    pub embedding_delta: f32,
    /// Post-injection VFE divided by pre-injection VFE.
    pub effectiveness: f32,
    pub _pad: [u8; 2],
    pub timestamp_ns: u64,
    pub seq: u64,
}

/// Append-only ring of lore.
#[repr(C)]
pub struct LoreBuffer {
    pub write_seq: AtomicU64,
    pub entries: [LoreEntry; MAX_LORE_ENTRIES],
}

// ── Belief state ─────────────────────────────────────────────────────

/// One layer's belief distribution and its prediction of the layer below.
///
/// Cache-line aligned because an entire layer slot is mapped and copied by
/// GPU kernels that assume 64-byte alignment.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct BeliefSlot {
    pub mean: [f32; STATE_DIM],
    pub precision: [f32; STATE_DIM],
    pub vfe: f32,
    pub prediction: [f32; STATE_DIM],
    pub residual: [f32; STATE_DIM],
    pub challenge_vfe: f32,
    pub confirm_streak: u32,
    pub compression: u8,
    pub layer: u8,
    pub _pad: [u8; 2],
    pub timestamp_ns: u64,
    pub cycle_us: u32,
    pub _pad2: [u8; 4],
}

/// Elements per layer's generative weight matrix.
///
/// Each layer maps belief to a prediction of the layer below with a
/// `STATE_DIM × STATE_DIM` matrix plus a `STATE_DIM` bias.
pub const WEIGHT_COUNT: usize = STATE_DIM * STATE_DIM;

/// A layer's double-buffered belief, generative weights and question slot.
#[repr(C)]
pub struct LayerSlot {
    pub buffers: [BeliefSlot; 2],
    /// Generative weights, `W[i][j] = weights[i * STATE_DIM + j]`.
    pub weights: [f32; WEIGHT_COUNT],
    /// Generative bias vector.
    pub bias: [f32; STATE_DIM],
    pub write_idx: AtomicUsize,
    pub challenge_flag: AtomicBool,
    pub confirm_flag: AtomicBool,
    pub escalate_flag: AtomicBool,
    pub _pad: u8,
    pub confirm_total: AtomicU64,
    pub challenge_total: AtomicU64,
    /// Written when the layer needs outside help.
    pub question: QuestionSlot,
}

/// Event kind recorded in the belief ledger.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerEvent {
    Challenge = 0,
    Confirm = 1,
    Habit = 2,
    HabitDecay = 3,
    Escalate = 4,
}

/// One ledger row: what a layer did and the belief state around it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LedgerEntry {
    pub seq: u64,
    pub layer: u8,
    pub event: LedgerEvent,
    pub compression: u8,
    pub _pad: u8,
    pub vfe: f32,
    pub residual_norm: f32,
    pub belief_mean: [f32; STATE_DIM],
    pub timestamp_ns: u64,
}

/// One coherent JEPA inference result.
///
/// The latent, prediction, uncertainty, adapter evidence and grounding output
/// all describe the same source tuple. This is data only — synchronization
/// belongs to [`JepaEvidenceSlot`].
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct JepaEvidencePayload {
    pub abi_version: u32,
    pub backend: u32,
    pub mode: u32,
    pub flags: u32,
    pub producer_epoch: u64,
    pub runner_epoch: u64,
    pub inference_seq: u64,
    pub timestamp_ns: u64,
    pub camera_seq: u64,
    pub lidar_seq: u64,
    pub pose_seq: u64,
    pub action_seq: u64,
    pub camera_age_ms: f32,
    pub lidar_age_ms: f32,
    pub pose_age_ms: f32,
    pub action_age_ms: f32,
    pub source_skew_ns: i64,
    pub observation_quality: f32,
    pub transition_nll: f32,
    pub occupancy_confidence: f32,
    pub latent_dim: u32,
    pub evidence_dim: u32,
    pub occupancy_dim: u32,
    pub model_id: [u8; JEPA_ID_BYTES],
    pub checkpoint_id: [u8; JEPA_ID_BYTES],
    pub latent: [f32; JEPA_CORE_DIM],
    pub predicted_mean: [f32; JEPA_CORE_DIM],
    pub predicted_log_variance: [f32; JEPA_CORE_DIM],
    pub evidence: [f32; JEPA_EVIDENCE_DIM],
    pub occupancy_logits: [f32; JEPA_OCCUPANCY_CELLS],
}

impl Default for JepaEvidencePayload {
    fn default() -> Self {
        Self {
            abi_version: JEPA_ABI_VERSION,
            backend: JEPA_BACKEND_NONE,
            mode: JEPA_MODE_UNAVAILABLE,
            flags: 0,
            producer_epoch: 0,
            runner_epoch: 0,
            inference_seq: 0,
            timestamp_ns: 0,
            camera_seq: 0,
            lidar_seq: 0,
            pose_seq: 0,
            action_seq: 0,
            camera_age_ms: 0.0,
            lidar_age_ms: 0.0,
            pose_age_ms: 0.0,
            action_age_ms: 0.0,
            source_skew_ns: 0,
            observation_quality: 0.0,
            transition_nll: 0.0,
            occupancy_confidence: 0.0,
            latent_dim: JEPA_CORE_DIM as u32,
            evidence_dim: JEPA_EVIDENCE_DIM as u32,
            occupancy_dim: JEPA_OCCUPANCY_CELLS as u32,
            model_id: [0; JEPA_ID_BYTES],
            checkpoint_id: [0; JEPA_ID_BYTES],
            latent: [0.0; JEPA_CORE_DIM],
            predicted_mean: [0.0; JEPA_CORE_DIM],
            predicted_log_variance: [0.0; JEPA_CORE_DIM],
            evidence: [0.0; JEPA_EVIDENCE_DIM],
            occupancy_logits: [0.0; JEPA_OCCUPANCY_CELLS],
        }
    }
}

/// Runtime counters and accelerator timing for one JEPA producer.
///
/// Correlated to an evidence payload by
/// `(producer_epoch, runner_epoch, inference_seq)`.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct JepaTelemetryPayload {
    pub abi_version: u32,
    pub backend: u32,
    pub mode: u32,
    pub flags: u32,
    pub producer_epoch: u64,
    pub runner_epoch: u64,
    pub inference_count: u64,
    pub dropped_frames: u64,
    pub stale_frames: u64,
    pub non_finite_outputs: u64,
    pub hot_swaps: u64,
    pub last_inference_ns: u64,
    pub latency_p50_us: u32,
    pub latency_p95_us: u32,
    pub latency_last_us: u32,
    pub latency_max_us: u32,
    pub camera_age_ms: f32,
    pub lidar_age_ms: f32,
    pub pose_age_ms: f32,
    pub action_age_ms: f32,
    pub source_skew_ns: i64,
    pub model_id: [u8; JEPA_ID_BYTES],
    pub checkpoint_id: [u8; JEPA_ID_BYTES],
    pub last_error_code: u32,
    pub last_error_len: u32,
    pub last_error: [u8; JEPA_ERROR_BYTES],
}

impl Default for JepaTelemetryPayload {
    fn default() -> Self {
        Self {
            abi_version: JEPA_ABI_VERSION,
            backend: JEPA_BACKEND_NONE,
            mode: JEPA_MODE_UNAVAILABLE,
            flags: 0,
            producer_epoch: 0,
            runner_epoch: 0,
            inference_count: 0,
            dropped_frames: 0,
            stale_frames: 0,
            non_finite_outputs: 0,
            hot_swaps: 0,
            last_inference_ns: 0,
            latency_p50_us: 0,
            latency_p95_us: 0,
            latency_last_us: 0,
            latency_max_us: 0,
            camera_age_ms: 0.0,
            lidar_age_ms: 0.0,
            pose_age_ms: 0.0,
            action_age_ms: 0.0,
            source_skew_ns: 0,
            model_id: [0; JEPA_ID_BYTES],
            checkpoint_id: [0; JEPA_ID_BYTES],
            last_error_code: 0,
            last_error_len: 0,
            last_error: [0; JEPA_ERROR_BYTES],
        }
    }
}

/// One published state of the invented fly rate model, observe-only.
///
/// The JEPA runtime fills this while `QUALIA_FLY_MODE=sim`, the fly crate's
/// `sim` feature is compiled in, and the active generation pointer is approved
/// for observe-only operation. The first [`FlySimPayload::type_count`] entries
/// of [`FlySimPayload::state`] carry one rate per prior type, in the prior's
/// type order; nothing in the belief or motor path reads the slot.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct FlySimPayload {
    /// [`FLY_SIM_ABI_VERSION`] of the writer that published this state.
    pub abi_version: u32,
    /// Number of valid entries in [`Self::state`].
    pub type_count: u32,
    /// The region's flag vocabulary: [`JEPA_FLAG_VALID`],
    /// [`JEPA_FLAG_OUTPUT_FINITE`].
    pub flags: u32,
    pub _pad0: u32,
    /// Generation that produced the rates; correlates with the JEPA slots.
    pub producer_epoch: u64,
    /// Runtime epoch that published them.
    pub runner_epoch: u64,
    /// Steps the model has taken since it was loaded.
    pub sim_step: u64,
    /// Publication time, nanoseconds.
    pub timestamp_ns: u64,
    /// Length of the message in [`Self::last_error`].
    pub last_error_len: u32,
    /// Step size of the last integration.
    pub dt: f32,
    pub _pad1: [u8; 8],
    /// NUL-padded identifier of the model that produced the rates — the fly
    /// crate's `SIM_ID`, or all zero while unpublished.
    pub sim_id: [u8; FLY_SIM_ID_BYTES],
    pub last_error: [u8; FLY_SIM_ERROR_BYTES],
    /// One rate per prior type; [`Self::type_count`] entries are valid.
    pub state: [f32; FLY_SIM_MAX_TYPES],
}

impl Default for FlySimPayload {
    fn default() -> Self {
        Self {
            abi_version: FLY_SIM_ABI_VERSION,
            type_count: 0,
            flags: 0,
            _pad0: 0,
            producer_epoch: 0,
            runner_epoch: 0,
            sim_step: 0,
            timestamp_ns: 0,
            last_error_len: 0,
            dt: 0.0,
            _pad1: [0; 8],
            sim_id: [0; FLY_SIM_ID_BYTES],
            last_error: [0; FLY_SIM_ERROR_BYTES],
            state: [0.0; FLY_SIM_MAX_TYPES],
        }
    }
}

/// Declares a seqlocked slot around a payload type.
///
/// The sequence word occupies its own cache line so a reader spinning on it
/// never contends with the payload copy.
macro_rules! define_jepa_slot {
    ($name:ident, $payload:ty) => {
        #[repr(C, align(64))]
        pub struct $name {
            pub seq: AtomicU64,
            pub _pad: [u8; 56],
            payload: UnsafeCell<$payload>,
        }

        // SAFETY: one writer per slot; publication is the odd/write/even
        // protocol and readers receive owned `Copy` snapshots.
        unsafe impl Sync for $name {}

        impl $name {
            /// Publishes a payload. Returns the new even sequence number.
            pub fn publish(&self, payload: $payload) -> Result<u64, SnapshotError> {
                let current = self.seq.load(Ordering::Acquire);
                if current & 1 != 0 {
                    return Err(SnapshotError::WriterBusy);
                }
                let writing = current.wrapping_add(1);
                self.seq
                    .compare_exchange(current, writing, Ordering::AcqRel, Ordering::Acquire)
                    .map_err(|_| SnapshotError::WriterBusy)?;
                // SAFETY: one writer, and the odd sequence keeps readers from
                // accepting anything until the copy is complete.
                unsafe { std::ptr::write_volatile(self.payload.get(), payload) };
                let published = writing.wrapping_add(1);
                self.seq.store(published, Ordering::Release);
                Ok(published)
            }

            /// Takes an owned copy, retrying at most `max_attempts` times.
            pub fn snapshot(&self, max_attempts: usize) -> Result<$payload, SnapshotError> {
                for _ in 0..max_attempts.max(1) {
                    let before = self.seq.load(Ordering::Acquire);
                    if before & 1 != 0 {
                        std::hint::spin_loop();
                        continue;
                    }
                    // SAFETY: the sequence word brackets the payload; the
                    // returned value owns its bytes and never aliases SHM.
                    let payload = unsafe { std::ptr::read_volatile(self.payload.get()) };
                    fence(Ordering::Acquire);
                    let after = self.seq.load(Ordering::Acquire);
                    if before == after && after & 1 == 0 {
                        return Ok(payload);
                    }
                }
                Err(SnapshotError::TornRead)
            }
        }
    };
}

define_jepa_slot!(JepaEvidenceSlot, JepaEvidencePayload);
define_jepa_slot!(JepaTelemetrySlot, JepaTelemetryPayload);
define_jepa_slot!(FlySimSlot, FlySimPayload);

/// Header at the base of the shared-memory region.
#[repr(C)]
pub struct ShmHeader {
    pub magic: u64,
    pub version: u32,
    pub num_layers: u32,
    pub layer_slot_size: u64,
    pub ledger_offset: u64,
    pub ledger_capacity: u64,
    pub ledger_write_seq: AtomicU64,
    pub total_size: u64,
    pub jepa_region_offset: u64,
    pub jepa_region_size: u64,
    pub jepa_abi_version: u32,
    pub _pad: [u8; 4],
}

/// Compact per-layer health summary for the supervisor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HealthReport {
    pub layer: u8,
    pub compression: u8,
    pub _pad: [u8; 2],
    pub vfe: f32,
    pub challenge_vfe: f32,
    pub confirm_streak: u32,
    pub cycle_us: u32,
    pub timestamp_ns: u64,
}

// ── Layer parameters ─────────────────────────────────────────────────
// Backend-agnostic configuration shared by Metal and CUDA.

/// Tunables for one layer, independent of the compute backend.
pub struct LayerParams {
    pub threshold: f32,
    pub learning_rate: f32,
    pub layer_id: u8,
    pub freq_hz: f64,
    pub weight_decay: f32,
}

/// Returns the tuned defaults for a layer id, falling back to a generic set.
pub fn default_params(layer_id: u8) -> LayerParams {
    match layer_id {
        0 => LayerParams {
            threshold: 0.1,
            learning_rate: 0.01,
            layer_id: 0,
            freq_hz: 1000.0,
            weight_decay: 0.0001,
        },
        1 => LayerParams {
            threshold: 0.08,
            learning_rate: 0.008,
            layer_id: 1,
            freq_hz: 100.0,
            weight_decay: 0.0001,
        },
        2 => LayerParams {
            threshold: 0.05,
            learning_rate: 0.005,
            layer_id: 2,
            freq_hz: 100.0,
            weight_decay: 0.00005,
        },
        3 => LayerParams {
            threshold: 0.05,
            learning_rate: 0.005,
            layer_id: 3,
            freq_hz: 100.0,
            weight_decay: 0.00005,
        },
        4 => LayerParams {
            threshold: 0.03,
            learning_rate: 0.003,
            layer_id: 4,
            freq_hz: 1.0,
            weight_decay: 0.00001,
        },
        5 => LayerParams {
            threshold: 0.02,
            learning_rate: 0.002,
            layer_id: 5,
            freq_hz: 0.1,
            weight_decay: 0.00001,
        },
        6 => LayerParams {
            threshold: 0.01,
            learning_rate: 0.001,
            layer_id: 6,
            freq_hz: 0.05,
            weight_decay: 0.000001,
        },
        // L7 mirrors vision and keeps its weights frozen.
        7 => LayerParams {
            threshold: 0.1,
            learning_rate: 0.0,
            layer_id: 7,
            freq_hz: 30.0,
            weight_decay: 0.0,
        },
        _ => LayerParams {
            threshold: 0.1,
            learning_rate: 0.01,
            layer_id,
            freq_hz: 10.0,
            weight_decay: 0.0001,
        },
    }
}

/// The stack manifest the supervisor reads.
#[derive(Debug, Clone, Deserialize)]
pub struct StackManifest {
    pub schema_version: String,
    pub stack_name: String,
    pub shared_memory: SharedMemoryConfig,
    pub control: ControlConfig,
    pub runners: Vec<RunnerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SharedMemoryConfig {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControlConfig {
    pub socket: String,
}

/// One child process in the stack.
#[derive(Debug, Clone, Deserialize)]
pub struct RunnerConfig {
    pub name: String,
    #[serde(default)]
    pub stdout: RunnerStdout,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
}

/// Where a child's stdout is sent.
#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RunnerStdout {
    #[default]
    Inherit,
    Null,
}

/// Parses and validates a stack manifest document.
///
/// Validation is deliberately shallow: it catches the mistakes a supervisor
/// would otherwise discover only after spawning children.
pub fn parse_stack_manifest(text: &str) -> Result<StackManifest, String> {
    let manifest: StackManifest =
        serde_json::from_str(text).map_err(|e| format!("failed to parse stack manifest: {e}"))?;
    if manifest.schema_version != "qualia.stack.v1" {
        return Err(format!(
            "unsupported stack manifest schema_version '{}', expected 'qualia.stack.v1'",
            manifest.schema_version
        ));
    }
    if manifest.stack_name.trim().is_empty() {
        return Err("stack manifest stack_name cannot be empty".to_string());
    }
    if manifest.shared_memory.name.trim().is_empty() {
        return Err("stack manifest shared_memory.name cannot be empty".to_string());
    }
    if manifest.control.socket.trim().is_empty() {
        return Err("stack manifest control.socket cannot be empty".to_string());
    }
    if manifest.runners.is_empty() {
        return Err("stack manifest must include at least one runner".to_string());
    }
    for runner in &manifest.runners {
        if runner.name.trim().is_empty() {
            return Err("stack manifest runner.name cannot be empty".to_string());
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn shm_constants_are_pinned() {
        assert_eq!(SHM_MAGIC, 0x5155414C3141454E);
        assert_eq!(SHM_VERSION, 3);
        assert_eq!(JEPA_ABI_VERSION, 1);
        assert_eq!(NUM_LAYERS, 8);
        assert_eq!(WEIGHT_COUNT, 1_048_576);
        assert_eq!(VOXEL_TOTAL, 12_288);
        assert_eq!(LIDAR_GRID_CELLS, 65_536);
        assert_eq!(MAP_GRID_CELLS, 65_536);
        assert_eq!(CAMERA_THUMB_PIXELS, 3_072);
    }

    #[test]
    fn belief_slot_layout_is_cache_aligned() {
        assert_eq!(align_of::<BeliefSlot>(), 64);
        assert_eq!(offset_of!(BeliefSlot, mean), 0);
        assert_eq!(offset_of!(BeliefSlot, precision), 4_096);
        assert_eq!(offset_of!(BeliefSlot, vfe), 8_192);
        assert_eq!(offset_of!(BeliefSlot, prediction), 8_196);
        assert_eq!(offset_of!(BeliefSlot, residual), 12_292);
        assert_eq!(offset_of!(BeliefSlot, challenge_vfe), 16_388);
        assert_eq!(offset_of!(BeliefSlot, confirm_streak), 16_392);
        assert_eq!(offset_of!(BeliefSlot, compression), 16_396);
        assert_eq!(offset_of!(BeliefSlot, layer), 16_397);
        assert_eq!(offset_of!(BeliefSlot, timestamp_ns), 16_400);
        assert_eq!(offset_of!(BeliefSlot, cycle_us), 16_408);
        assert_eq!(size_of::<BeliefSlot>(), 16_448);
    }

    #[test]
    fn applied_action_slot_layout_matches_gpu_consumers() {
        assert_eq!(align_of::<AppliedActionSlot>(), 64);
        assert_eq!(offset_of!(AppliedActionSlot, seq), 0);
        assert_eq!(offset_of!(AppliedActionSlot, producer_epoch), 8);
        assert_eq!(offset_of!(AppliedActionSlot, action_sequence), 16);
        assert_eq!(offset_of!(AppliedActionSlot, interval_start_ns), 24);
        assert_eq!(offset_of!(AppliedActionSlot, interval_end_ns), 32);
        assert_eq!(offset_of!(AppliedActionSlot, requested_left), 40);
        assert_eq!(offset_of!(AppliedActionSlot, requested_right), 44);
        assert_eq!(offset_of!(AppliedActionSlot, clamped_left), 48);
        assert_eq!(offset_of!(AppliedActionSlot, clamped_right), 52);
        assert_eq!(offset_of!(AppliedActionSlot, applied_left), 56);
        assert_eq!(offset_of!(AppliedActionSlot, applied_right), 60);
        assert_eq!(offset_of!(AppliedActionSlot, speed_scale), 64);
        assert_eq!(offset_of!(AppliedActionSlot, safety_flags), 68);
        assert_eq!(offset_of!(AppliedActionSlot, authority), 72);
        assert_eq!(offset_of!(AppliedActionSlot, valid), 76);
        assert_eq!(offset_of!(AppliedActionSlot, armed), 77);
        assert_eq!(offset_of!(AppliedActionSlot, deadman_active), 78);
        assert_eq!(offset_of!(AppliedActionSlot, collision_clamped), 79);
        assert_eq!(size_of::<AppliedActionSlot>(), 128);
    }

    #[test]
    fn applied_action_history_layout_is_stable() {
        assert_eq!(align_of::<AppliedActionHistory>(), 64);
        assert_eq!(offset_of!(AppliedActionHistory, write_seq), 0);
        assert_eq!(offset_of!(AppliedActionHistory, _pad0), 8);
        assert_eq!(offset_of!(AppliedActionHistory, entry_seq), 64);
        assert_eq!(offset_of!(AppliedActionHistory, entries), 32_832);
        assert_eq!(size_of::<AppliedActionHistory>(), 557_120);
    }

    #[test]
    fn jepa_payload_and_slot_layouts_are_stable() {
        assert_eq!(align_of::<JepaEvidencePayload>(), 64);
        assert_eq!(offset_of!(JepaEvidencePayload, abi_version), 0);
        assert_eq!(offset_of!(JepaEvidencePayload, producer_epoch), 16);
        assert_eq!(offset_of!(JepaEvidencePayload, timestamp_ns), 40);
        assert_eq!(offset_of!(JepaEvidencePayload, camera_seq), 48);
        assert_eq!(offset_of!(JepaEvidencePayload, camera_age_ms), 80);
        assert_eq!(offset_of!(JepaEvidencePayload, source_skew_ns), 96);
        assert_eq!(offset_of!(JepaEvidencePayload, observation_quality), 104);
        assert_eq!(offset_of!(JepaEvidencePayload, latent_dim), 116);
        assert_eq!(offset_of!(JepaEvidencePayload, model_id), 128);
        assert_eq!(offset_of!(JepaEvidencePayload, checkpoint_id), 192);
        assert_eq!(offset_of!(JepaEvidencePayload, latent), 256);
        assert_eq!(offset_of!(JepaEvidencePayload, predicted_mean), 1_280);
        assert_eq!(
            offset_of!(JepaEvidencePayload, predicted_log_variance),
            2_304
        );
        assert_eq!(offset_of!(JepaEvidencePayload, evidence), 3_328);
        assert_eq!(offset_of!(JepaEvidencePayload, occupancy_logits), 7_424);
        assert_eq!(size_of::<JepaEvidencePayload>(), 23_808);

        assert_eq!(align_of::<JepaTelemetryPayload>(), 64);
        assert_eq!(offset_of!(JepaTelemetryPayload, abi_version), 0);
        assert_eq!(offset_of!(JepaTelemetryPayload, producer_epoch), 16);
        assert_eq!(offset_of!(JepaTelemetryPayload, inference_count), 32);
        assert_eq!(offset_of!(JepaTelemetryPayload, last_inference_ns), 72);
        assert_eq!(offset_of!(JepaTelemetryPayload, latency_p50_us), 80);
        assert_eq!(offset_of!(JepaTelemetryPayload, camera_age_ms), 96);
        assert_eq!(offset_of!(JepaTelemetryPayload, source_skew_ns), 112);
        assert_eq!(offset_of!(JepaTelemetryPayload, model_id), 120);
        assert_eq!(offset_of!(JepaTelemetryPayload, checkpoint_id), 184);
        assert_eq!(offset_of!(JepaTelemetryPayload, last_error_code), 248);
        assert_eq!(offset_of!(JepaTelemetryPayload, last_error), 256);
        assert_eq!(size_of::<JepaTelemetryPayload>(), 512);

        assert_eq!(align_of::<JepaEvidenceSlot>(), 64);
        assert_eq!(offset_of!(JepaEvidenceSlot, seq), 0);
        assert_eq!(offset_of!(JepaEvidenceSlot, payload), 64);
        assert_eq!(size_of::<JepaEvidenceSlot>(), 23_872);

        assert_eq!(align_of::<JepaTelemetrySlot>(), 64);
        assert_eq!(offset_of!(JepaTelemetrySlot, seq), 0);
        assert_eq!(offset_of!(JepaTelemetrySlot, payload), 64);
        assert_eq!(size_of::<JepaTelemetrySlot>(), 576);
    }

    #[test]
    fn shm_header_round_trips_through_bytes() {
        let header = ShmHeader {
            magic: SHM_MAGIC,
            version: SHM_VERSION,
            num_layers: NUM_LAYERS as u32,
            layer_slot_size: size_of::<LayerSlot>() as u64,
            ledger_offset: 4096,
            ledger_capacity: 512,
            ledger_write_seq: AtomicU64::new(7),
            total_size: 1 << 20,
            jepa_region_offset: 8192,
            jepa_region_size: 4096,
            jepa_abi_version: JEPA_ABI_VERSION,
            _pad: [0; 4],
        };
        assert_eq!(size_of::<ShmHeader>(), 80);
        assert_eq!(align_of::<ShmHeader>(), 8);
        assert_eq!(header.magic, SHM_MAGIC);
        assert_eq!(header.version, SHM_VERSION);
        assert_eq!(header.ledger_write_seq.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn world_model_holds_all_objects_before_the_embedding() {
        assert_eq!(offset_of!(WorldModel, objects), 0);
        assert_eq!(offset_of!(WorldModel, num_objects), 768);
        let offset = offset_of!(WorldModel, scene_embedding);
        assert_eq!(offset % 4, 0);
        assert!(offset_of!(WorldModel, directive) > offset);
        assert!(size_of::<WorldModel>() > STATE_DIM * 4);
    }

    #[test]
    fn ledger_event_discriminants_are_stable() {
        assert_eq!(LedgerEvent::Challenge as u8, 0);
        assert_eq!(LedgerEvent::Confirm as u8, 1);
        assert_eq!(LedgerEvent::Habit as u8, 2);
        assert_eq!(LedgerEvent::HabitDecay as u8, 3);
        assert_eq!(LedgerEvent::Escalate as u8, 4);
    }

    #[test]
    fn nav_structs_stay_within_a_cache_line() {
        assert!(size_of::<NavPose>() <= 64);
        assert!(size_of::<NavGoal>() <= 64);
        assert!(size_of::<HealthReport>() <= 64);
    }

    #[test]
    fn default_params_cover_every_layer_and_the_fallback() {
        for layer in 0..NUM_LAYERS as u8 {
            let params = default_params(layer);
            assert_eq!(params.layer_id, layer);
            assert!(params.freq_hz > 0.0);
            assert!(params.threshold > 0.0);
            assert!(params.learning_rate >= 0.0);
        }
        let frozen = default_params(7);
        assert_eq!(frozen.learning_rate, 0.0);
        assert_eq!(frozen.weight_decay, 0.0);
        let unknown = default_params(42);
        assert_eq!(unknown.layer_id, 42);
        assert_eq!(unknown.freq_hz, 10.0);
    }

    #[test]
    fn stack_manifest_rejects_bad_schema_version() {
        let err = parse_stack_manifest(
            r#"{
              "schema_version":"qualia.stack.v0",
              "stack_name":"bad",
              "shared_memory":{"name":"/qualia_body"},
              "control":{"socket":"127.0.0.1:1"},
              "runners":[{"name":"qualia-agent"}]
            }"#,
        )
        .expect_err("bad schema version should fail");
        assert!(err.contains("unsupported stack manifest schema_version"));
    }
}
