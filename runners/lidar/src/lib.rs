//! LiDAR ingest for the qualia body bus.
//!
//! The runner opens the ranging device as a serial stream, reframes its
//! 47-byte packets, stitches consecutive packets into whole rotations, and
//! publishes each rotation twice: as the polar [`LidarScan`] the perception
//! stack reads, and as the [`LidarOccupancyGrid`] the navigation stack reads.
//! Both land in the shared arena `qualia-shm` maps, under the seqlocks
//! `qualia-types` defines, so this crate owns no state of its own — the bytes
//! in the arena are the state.
//!
//! Everything the process needs comes from its environment: the port, the
//! line rate, the startup deadline, the logging cadence and the region name.
//! [`LidarConfig`] reads and defaults those keys, so the binary is a thin
//! driver over the logic collected here.
//!
//! [`LidarScan`]: qualia_types::LidarScan
//! [`LidarOccupancyGrid`]: qualia_types::LidarOccupancyGrid

use std::collections::VecDeque;
use std::time::Duration;

use qualia_shm::ShmRegion;
use qualia_types::{
    LidarOccupancyGridSnapshot, LidarScanSnapshot, LIDAR_GRID_CELLS, LIDAR_GRID_H, LIDAR_GRID_W,
    LIDAR_MAX_POINTS,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Serial device the runner opens when `QUALIA_LIDAR_PORT` is unset.
pub const DEFAULT_PORT: &str = "/dev/ttyACM0";
/// Line rate used when `QUALIA_LIDAR_BAUD` is unset or unparseable.
pub const DEFAULT_BAUD: u32 = 230_400;
/// Seconds without a first complete rotation before the runner gives up.
pub const DEFAULT_TIMEOUT_SECS: u64 = 5;
/// Rotations between summary lines when `QUALIA_LIDAR_LOG_EVERY_SCANS` is unset.
pub const DEFAULT_LOG_EVERY_SCANS: u64 = 10;
/// Shared arena the runner attaches to when `QUALIA_SHM_NAME` is unset.
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";

/// The runner's environment-derived settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LidarConfig {
    pub port: String,
    pub baud: u32,
    pub timeout: Duration,
    pub log_every_scans: u64,
    pub shm_name: String,
}

impl LidarConfig {
    /// Reads every key from the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads every key through `lookup`, substituting the documented default
    /// whenever a key is absent or does not parse. A malformed value must
    /// never stop the runner from starting.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let port = lookup("QUALIA_LIDAR_PORT").unwrap_or_else(|| DEFAULT_PORT.to_string());
        let baud = lookup("QUALIA_LIDAR_BAUD")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_BAUD);
        let timeout_secs =
            number(&mut lookup, "QUALIA_LIDAR_TIMEOUT_SECS").unwrap_or(DEFAULT_TIMEOUT_SECS);
        let log_every_scans =
            number(&mut lookup, "QUALIA_LIDAR_LOG_EVERY_SCANS").unwrap_or(DEFAULT_LOG_EVERY_SCANS);
        let shm_name = lookup("QUALIA_SHM_NAME").unwrap_or_else(|| DEFAULT_SHM_NAME.to_string());

        Self {
            port,
            baud,
            timeout: Duration::from_secs(timeout_secs),
            log_every_scans,
            shm_name,
        }
    }
}

fn number(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str) -> Option<u64> {
    lookup(key)?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

/// First byte of every device packet.
pub const FRAME_HEADER: u8 = 0x54;
/// Second byte of every device packet, the version/length code.
pub const FRAME_VER_LEN: u8 = 0x2C;
/// Returns packed into one packet.
pub const POINTS_PER_PACKET: usize = 12;
/// Bytes in one packet, including the trailing checksum.
pub const FRAME_SIZE: usize = 47;

/// Metres per occupancy-grid cell.
pub const GRID_RESOLUTION_M: f32 = 0.05;
/// Cell value written where a return landed.
pub const GRID_OCCUPIED: u8 = 255;

/// A rotation is cut where an angle below this follows one above
/// [`SCAN_WRAP_MIN_DEG`]; the device reports whole turns in one direction.
const SCAN_WRAP_MAX_DEG: f32 = 20.0;
const SCAN_WRAP_MIN_DEG: f32 = 340.0;

/// Centidegrees in a full turn.
const FULL_TURN_CDEG: i64 = 36_000;

/// One polar return as the device reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DevicePoint {
    pub angle_deg: f32,
    pub distance_mm: u16,
    pub intensity: u8,
}

/// One decoded device packet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Packet {
    pub speed_deg_per_sec: u16,
    pub start_angle_cdeg: u16,
    pub end_angle_cdeg: u16,
    pub timestamp_ms: u16,
    pub points: [DevicePoint; POINTS_PER_PACKET],
}

/// CRC-8 over `data` with the device's polynomial (`0x4D`, MSB first).
pub fn crc8(data: &[u8]) -> u8 {
    let mut remainder = 0u8;
    for byte in data {
        remainder ^= byte;
        for _ in 0..8 {
            remainder = match remainder & 0x80 {
                0 => remainder.wrapping_shl(1),
                _ => remainder.wrapping_shl(1) ^ 0x4D,
            };
        }
    }
    remainder
}

/// Decodes one checksum-verified packet.
pub fn parse_packet(raw: &[u8; FRAME_SIZE]) -> Packet {
    let speed_deg_per_sec = read_u16(raw, 2);
    let start_angle_cdeg = read_u16(raw, 4);
    let end_angle_cdeg = read_u16(raw, 42);
    let timestamp_ms = read_u16(raw, 44);

    // The sweep runs forward from the start angle to the end angle, wrapping
    // through zero; centidegrees make the span exact before it becomes f32.
    let span_cdeg = (end_angle_cdeg as i64 - start_angle_cdeg as i64).rem_euclid(FULL_TURN_CDEG);
    let per_point_deg = span_cdeg as f32 / (POINTS_PER_PACKET as f32 - 1.0) / 100.0;
    let origin_deg = start_angle_cdeg as f32 / 100.0;

    let mut points = [DevicePoint {
        angle_deg: 0.0,
        distance_mm: 0,
        intensity: 0,
    }; POINTS_PER_PACKET];

    for (index, point) in points.iter_mut().enumerate() {
        let base = 6 + index * 3;
        let distance_mm = read_u16(raw, base);
        let intensity = raw[base + 2];
        let angle_deg = origin_deg + index as f32 * per_point_deg;
        let angle_deg = if angle_deg >= 360.0 {
            angle_deg - 360.0
        } else {
            angle_deg
        };
        *point = DevicePoint {
            angle_deg,
            distance_mm,
            intensity,
        };
    }

    Packet {
        speed_deg_per_sec,
        start_angle_cdeg,
        end_angle_cdeg,
        timestamp_ms,
        points,
    }
}

/// Pulls the next checksum-valid packet out of `stream`, sliding one byte at a
/// time past anything that is not a whole frame so a mid-stream join costs at
/// most one corrupted rotation.
pub fn extract_packet(stream: &mut VecDeque<u8>) -> Option<Packet> {
    loop {
        if stream.len() < FRAME_SIZE {
            return None;
        }
        if stream[0] != FRAME_HEADER || stream[1] != FRAME_VER_LEN {
            stream.pop_front();
            continue;
        }

        let mut raw = [0u8; FRAME_SIZE];
        for (slot, byte) in raw.iter_mut().zip(stream.iter()) {
            *slot = *byte;
        }

        let last = FRAME_SIZE - 1;
        if crc8(&raw[..last]) != raw[last] {
            stream.pop_front();
            continue;
        }

        for _ in 0..FRAME_SIZE {
            stream.pop_front();
        }
        let packet = parse_packet(&raw);
        return Some(packet);
    }
}

fn read_u16(raw: &[u8], offset: usize) -> u16 {
    u16::from(raw[offset]) | (u16::from(raw[offset + 1]) << 8)
}

// ---------------------------------------------------------------------------
// Rotation assembly
// ---------------------------------------------------------------------------

/// Stitches a point stream into whole rotations.
///
/// The device reports angles continuously within a turn and jumps back near
/// zero at the seam. The rotation that was already in flight when the runner
/// attached has no known start, so the first seam discards it and only later
/// seams hand back a complete rotation.
#[derive(Debug, Default)]
pub struct ScanAssembler {
    points: Vec<DevicePoint>,
    last_angle_deg: Option<f32>,
    seam_seen: bool,
}

impl ScanAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one device point. Returns the rotation that just completed when
    /// `point` starts a new one, and `None` otherwise.
    pub fn push(&mut self, point: DevicePoint) -> Option<Vec<DevicePoint>> {
        let mut completed = None;
        if let Some(previous) = self.last_angle_deg {
            if point.angle_deg < SCAN_WRAP_MAX_DEG && previous > SCAN_WRAP_MIN_DEG {
                if self.seam_seen && !self.points.is_empty() {
                    completed = Some(std::mem::take(&mut self.points));
                }
                self.seam_seen = true;
                self.points.clear();
            }
        }
        self.last_angle_deg = Some(point.angle_deg);
        self.points.push(point);
        completed
    }

    /// Points buffered for the rotation currently being assembled.
    pub fn buffered(&self) -> usize {
        self.points.len()
    }
}

// ---------------------------------------------------------------------------
// Startup deadline
// ---------------------------------------------------------------------------

/// What an idle serial read should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleAction {
    /// Nothing has timed out yet.
    Idle,
    /// No rotation ever arrived inside the deadline: exit non-zero.
    Fail,
    /// A rotation arrived once and the stream has since stalled: warn.
    Warn,
}

/// Decides what an idle read means.
///
/// A stream that never produced a rotation is fatal once `timeout` has passed
/// since startup. A stream that did produce one only warns when it stalls, so
/// a device that goes quiet cannot kill a runner that is already reporting.
pub fn idle_action(
    published_scans: u64,
    since_start: Duration,
    since_last_publish: Duration,
    timeout: Duration,
) -> IdleAction {
    if published_scans == 0 && since_start >= timeout {
        IdleAction::Fail
    } else if since_last_publish >= timeout {
        IdleAction::Warn
    } else {
        IdleAction::Idle
    }
}

// ---------------------------------------------------------------------------
// Publication
// ---------------------------------------------------------------------------

/// Publishes one complete rotation as the polar scan and the occupancy grid,
/// replacing both frames.
pub fn publish_scan(shm: &ShmRegion, points: &[DevicePoint]) {
    let now_ns = unix_time_ns();

    let mut scan = LidarScanSnapshot {
        scan_start_ns: now_ns,
        scan_end_ns: now_ns,
        point_count: points.len().min(LIDAR_MAX_POINTS) as u32,
        ..LidarScanSnapshot::default()
    };
    for (slot, point) in scan
        .points
        .iter_mut()
        .zip(points.iter().take(LIDAR_MAX_POINTS))
    {
        slot.angle_rad = point.angle_deg.to_radians();
        slot.distance_m = point.distance_mm as f32 / 1000.0;
        slot.intensity = point.intensity;
    }
    shm.lidar_scan_mut()
        .publish(&scan)
        .expect("the lidar runner is the scan's only writer");

    let mut grid = LidarOccupancyGridSnapshot {
        width: LIDAR_GRID_W as u32,
        height: LIDAR_GRID_H as u32,
        resolution_m: GRID_RESOLUTION_M,
        ..LidarOccupancyGridSnapshot::default()
    };
    grid.origin_x_m = -(LIDAR_GRID_W as f32 * GRID_RESOLUTION_M * 0.5);
    grid.origin_y_m = -(LIDAR_GRID_H as f32 * GRID_RESOLUTION_M * 0.5);

    for point in points {
        let carries_return = point.distance_mm > 0 && point.intensity > 0;
        if !carries_return {
            continue;
        }
        let range_m = f32::from(point.distance_mm) / 1000.0;
        let angle_rad = point.angle_deg.to_radians();
        let x_m = angle_rad.cos() * range_m;
        let y_m = angle_rad.sin() * range_m;
        let column = ((x_m - grid.origin_x_m) / grid.resolution_m).floor() as i32;
        let row = ((y_m - grid.origin_y_m) / grid.resolution_m).floor() as i32;
        if column < 0
            || row < 0
            || column >= LIDAR_GRID_W as i32
            || row >= LIDAR_GRID_H as i32
        {
            continue;
        }
        let index = row as usize * LIDAR_GRID_W + column as usize;
        if index < LIDAR_GRID_CELLS {
            grid.cells[index] = GRID_OCCUPIED;
        }
    }
    shm.lidar_grid_mut()
        .publish(&grid)
        .expect("the lidar runner is the grid's only writer");
}

// ---------------------------------------------------------------------------
// Summaries
// ---------------------------------------------------------------------------

/// Counts over one rotation, as the runner reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanSummary {
    pub total: usize,
    pub valid: usize,
    pub min_mm: u16,
    pub max_mm: u16,
}

/// Summarises a rotation: how many returns carry both range and intensity, and
/// the range extremes.
pub fn summarize(points: &[DevicePoint]) -> ScanSummary {
    ScanSummary {
        total: points.len(),
        valid: points
            .iter()
            .filter(|point| point.distance_mm > 0 && point.intensity > 0)
            .count(),
        min_mm: points
            .iter()
            .filter(|point| point.distance_mm > 0)
            .map(|point| point.distance_mm)
            .min()
            .unwrap_or(0),
        max_mm: points.iter().map(|point| point.distance_mm).max().unwrap_or(0),
    }
}

fn unix_time_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0)
}
