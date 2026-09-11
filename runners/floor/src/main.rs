//! `qualia-floor`: derives a floor-confidence grid from the camera thumbnail.
//!
//! The runner reads the shared camera slot and is the only writer of
//! `CameraFloorGrid`. Every valid frame is sampled once, the grid is stamped
//! with that frame's timestamp, and the published sequence advances so a reader
//! can tell a fresh grid from a stale one.
//!
//! The mapping from thumbnail to grid is deliberately cheap and deterministic:
//! a grid cell reads the thumbnail pixel in the lower half of the image that
//! sits under it, and treats bright, near-camera pixels as likely floor. The
//! published layout (`width`, `height`, `confidence_scale`, `last_update_ns`,
//! `cells`) is ABI and does not change.

use qualia_shm::{ShmError, ShmRegion};
use qualia_types::{
    CameraFloorGrid, CameraFrameSnapshot, CAMERA_THUMB_H, CAMERA_THUMB_W, VOXEL_D, VOXEL_W,
};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

/// Arena every runner attaches to unless the stack names another one.
const DEFAULT_SHM_NAME: &str = "/qualia_body";

/// Wait between camera samples when `QUALIA_FLOOR_POLL_MS` is unset.
const DEFAULT_POLL_MS: u64 = 100;

/// Seqlock read attempts per frame before the loop yields a tick.
const SNAPSHOT_ATTEMPTS: usize = 8;

/// One telemetry line for the first published grid and every this-many after.
const LOG_INTERVAL: u64 = 30;

/// Share of a cell's confidence that comes from thumbnail brightness.
const LUMA_WEIGHT: f32 = 0.75;

/// Share of a cell's confidence that comes from being near the camera.
const DEPTH_WEIGHT: f32 = 0.25;

/// Full-confidence value published alongside the grid.
const CONFIDENCE_SCALE: f32 = 255.0;

/// Publish an empty grid of the declared shape.
fn init_grid(grid: &mut CameraFloorGrid) {
    grid.width = VOXEL_W as u32;
    grid.height = VOXEL_D as u32;
    grid.confidence_scale = CONFIDENCE_SCALE;
    grid.last_update_ns = 0;
    grid.cells.fill(0);
    grid.seq.store(0, Ordering::Release);
}

/// The thumbnail pixel a grid cell reads.
///
/// Columns spread across the image and rows fall in the lower half, which is
/// where floor pixels are; both are clamped so the index stays in the
/// thumbnail even if the grid and thumbnail constants ever drift apart.
fn sampled_pixel(gx: usize, gz: usize) -> usize {
    let column = (gx * CAMERA_THUMB_W / VOXEL_W).min(CAMERA_THUMB_W - 1);
    let row = (CAMERA_THUMB_H / 2 + gz * (CAMERA_THUMB_H / 2) / VOXEL_D).min(CAMERA_THUMB_H - 1);
    row * CAMERA_THUMB_W + column
}

/// Derive the whole grid from one camera frame and publish it.
///
/// `cells` is row-major over depth: `gz * VOXEL_W + gx`.
fn write_floor_grid(grid: &mut CameraFloorGrid, frame: &CameraFrameSnapshot) {
    grid.width = VOXEL_W as u32;
    grid.height = VOXEL_D as u32;
    grid.confidence_scale = CONFIDENCE_SCALE;
    grid.last_update_ns = frame.timestamp_ns;

    for gz in 0..VOXEL_D {
        // Rows nearer the camera are weighted higher; all of these values are
        // exact f32 fractions of the grid depth.
        let near_bias = (VOXEL_D - gz) as f32 / VOXEL_D as f32;
        for gx in 0..VOXEL_W {
            let luma = frame.thumbnail_luma[sampled_pixel(gx, gz)] as f32 / CONFIDENCE_SCALE;
            let confidence = (luma * LUMA_WEIGHT + near_bias * DEPTH_WEIGHT) * CONFIDENCE_SCALE;
            grid.cells[gz * VOXEL_W + gx] = confidence.clamp(0.0, CONFIDENCE_SCALE) as u8;
        }
    }

    let published = grid.seq.load(Ordering::Acquire).wrapping_add(1);
    grid.seq.store(published, Ordering::Release);
}

/// Whether this frame is worth deriving a grid from.
///
/// A frame is published only when it is valid and carries a sequence the last
/// derivation has not already consumed, so a stalled camera cannot make the
/// grid look fresher than it is.
fn should_publish(frame: &CameraFrameSnapshot, last_seq: u64) -> bool {
    frame.valid && frame.seq != 0 && frame.seq != last_seq
}

/// Whether the published sequence deserves a telemetry line.
fn should_log(seq: u64) -> bool {
    seq == 1 || seq % LOG_INTERVAL == 0
}

/// Mean confidence over every cell, on the published `0..=255` scale.
fn grid_average(cells: &[u8]) -> f32 {
    let total: u32 = cells.iter().copied().map(u32::from).sum();
    total as f32 / (VOXEL_W * VOXEL_D) as f32
}

/// Confidence at the cell ahead of the camera.
fn center_confidence(cells: &[u8]) -> u8 {
    cells[(VOXEL_D / 2) * VOXEL_W + VOXEL_W / 2]
}

/// Poll interval in milliseconds, or the default when unset or malformed.
fn poll_ms_from(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_POLL_MS)
}

/// Region name to attach to, or the stack default when unset.
fn shm_name_from(raw: Option<&str>) -> String {
    raw.unwrap_or(DEFAULT_SHM_NAME).to_string()
}

/// The one fatal line the runner emits when the arena is unreachable.
fn open_failure_message(name: &str, err: &ShmError) -> String {
    format!("qualia-floor: failed to open shm '{name}': {err}")
}

/// Attach to the arena the stack created, or die reporting why.
fn attach(name: &str) -> ShmRegion {
    match ShmRegion::open(name) {
        Ok(region) => region,
        Err(err) => {
            eprintln!("{}", open_failure_message(name, &err));
            std::process::exit(1);
        }
    }
}

fn main() {
    let shm_name = shm_name_from(std::env::var("QUALIA_SHM_NAME").ok().as_deref());
    let poll_ms = poll_ms_from(std::env::var("QUALIA_FLOOR_POLL_MS").ok().as_deref());
    let shm = attach(&shm_name);

    let mut last_seq = 0u64;
    init_grid(shm.camera_floor_mut());
    println!("qualia-floor: deriving floor confidence from camera thumbnails");

    loop {
        if let Ok(frame) = shm.camera_frame().snapshot(SNAPSHOT_ATTEMPTS) {
            if should_publish(&frame, last_seq) {
                write_floor_grid(shm.camera_floor_mut(), &frame);
                last_seq = frame.seq;

                let grid = shm.camera_floor();
                let published = grid.seq.load(Ordering::Acquire);
                if should_log(published) {
                    println!(
                        "qualia-floor: seq={} avg_conf={:.1} center_conf={}",
                        published,
                        grid_average(&grid.cells),
                        center_confidence(&grid.cells),
                    );
                }
            }
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_types::CAMERA_THUMB_PIXELS;
    use std::sync::atomic::AtomicU64;

    /// Names stay unique per test *and* per run, so the tests never share a
    /// region with each other or with a region a killed run left behind.
    static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

    fn region_name(tag: &str) -> String {
        let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
        format!("/qualia_floor_test_{}_{}_{}", std::process::id(), tag, index)
    }

    fn empty_grid() -> CameraFloorGrid {
        CameraFloorGrid {
            seq: AtomicU64::new(0),
            width: 0,
            height: 0,
            confidence_scale: 0.0,
            last_update_ns: 0,
            cells: [0; VOXEL_W * VOXEL_D],
        }
    }

    fn frame(
        seq: u64,
        timestamp_ns: u64,
        valid: bool,
        luma: [u8; CAMERA_THUMB_PIXELS],
    ) -> CameraFrameSnapshot {
        CameraFrameSnapshot {
            seq,
            timestamp_ns,
            valid,
            thumbnail_luma: luma,
            ..Default::default()
        }
    }

    fn flat(luma: u8) -> [u8; CAMERA_THUMB_PIXELS] {
        [luma; CAMERA_THUMB_PIXELS]
    }

    /// Every thumbnail column carries its own index, so a cell's confidence
    /// reveals which column the runner sampled.
    fn column_ramp() -> [u8; CAMERA_THUMB_PIXELS] {
        let mut luma = [0u8; CAMERA_THUMB_PIXELS];
        for row in 0..CAMERA_THUMB_H {
            for col in 0..CAMERA_THUMB_W {
                luma[row * CAMERA_THUMB_W + col] = col as u8;
            }
        }
        luma
    }

    #[test]
    fn init_grid_stamps_the_published_shape_and_clears_the_cells() {
        let mut grid = empty_grid();
        grid.width = 7;
        grid.height = 9;
        grid.confidence_scale = 1.0;
        grid.last_update_ns = 55;
        grid.cells.fill(200);
        grid.seq.store(41, Ordering::Release);

        init_grid(&mut grid);

        assert_eq!(grid.width, VOXEL_W as u32);
        assert_eq!(grid.height, VOXEL_D as u32);
        assert_eq!(grid.confidence_scale, 255.0);
        assert_eq!(grid.last_update_ns, 0);
        assert!(grid.cells.iter().all(|&cell| cell == 0));
        assert_eq!(grid.seq.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_bright_thumbnail_saturates_the_near_rows_and_dims_with_depth() {
        let mut grid = empty_grid();
        write_floor_grid(&mut grid, &frame(1, 4242, true, flat(255)));

        // Near rows saturate; the farthest row keeps only its luma share and
        // the quarter-weight depth bias, and confidence falls with depth.
        assert_eq!(grid.cells[0], 255);
        assert_eq!(grid.cells[(VOXEL_D - 1) * VOXEL_W], 193);
        for gz in 1..VOXEL_D {
            assert!(grid.cells[gz * VOXEL_W] <= grid.cells[(gz - 1) * VOXEL_W]);
        }
        assert!(grid.cells[0] > grid.cells[VOXEL_D * VOXEL_W - 1]);
    }

    #[test]
    fn a_dark_thumbnail_keeps_only_the_near_camera_bias() {
        let mut grid = empty_grid();
        write_floor_grid(&mut grid, &frame(3, 99, true, flat(0)));

        // Luma contributes nothing, so a cell is the depth ramp alone.
        assert_eq!(grid.cells[0], 63);
        assert_eq!(grid.cells[(VOXEL_D - 1) * VOXEL_W], 1);
    }

    #[test]
    fn columns_sample_the_thumbnail_from_the_left_edge_rightwards() {
        let mut grid = empty_grid();
        write_floor_grid(&mut grid, &frame(1, 1, true, column_ramp()));

        // Cell column gx reads thumbnail column 2*gx, and luma rises with the
        // column, so confidence rises left to right.
        assert_eq!(grid.cells[0], 63);
        assert_eq!(grid.cells[16], 87);
        assert_eq!(grid.cells[VOXEL_W - 1], 110);
        for gx in 1..VOXEL_W {
            assert!(grid.cells[gx] > grid.cells[gx - 1]);
        }
    }

    #[test]
    fn publication_stamps_the_frame_and_advances_the_sequence() {
        let name = region_name("publish");
        let region = ShmRegion::create(&name).expect("create");
        init_grid(region.camera_floor_mut());

        write_floor_grid(region.camera_floor_mut(), &frame(7, 4242, true, flat(128)));

        // A reader attaching to the same region sees the published bytes.
        let reader = ShmRegion::open(&name).expect("attach");
        let published = reader.camera_floor();
        assert_eq!(published.seq.load(Ordering::Acquire), 1);
        assert_eq!(published.last_update_ns, 4242);
        assert_eq!(published.width, VOXEL_W as u32);
        assert_eq!(published.height, VOXEL_D as u32);
        assert_eq!(published.confidence_scale, 255.0);
        assert_eq!(published.cells.len(), VOXEL_W * VOXEL_D);
        assert!(published.cells.iter().any(|&cell| cell != 0));

        write_floor_grid(region.camera_floor_mut(), &frame(8, 5000, true, flat(0)));
        assert_eq!(published.seq.load(Ordering::Acquire), 2);
        assert_eq!(published.last_update_ns, 5000);
        assert_eq!(published.cells[0], 63);
    }

    #[test]
    fn only_a_fresh_valid_frame_is_published() {
        let fresh = frame(9, 1, true, flat(0));
        assert!(should_publish(&fresh, 8));
        assert!(!should_publish(&fresh, 9));
        assert!(!should_publish(&frame(10, 1, false, flat(0)), 9));
        assert!(!should_publish(&frame(0, 1, true, flat(0)), 0));
        assert!(!should_publish(&frame(0, 1, true, flat(0)), 5));
    }

    #[test]
    fn telemetry_reports_the_first_grid_and_every_thirtieth() {
        assert!(should_log(1));
        assert!(should_log(30));
        assert!(should_log(60));
        assert!(!should_log(2));
        assert!(!should_log(29));
        assert!(!should_log(31));
    }

    #[test]
    fn telemetry_averages_every_cell_and_reads_the_centre_one() {
        let mut cells = [0u8; VOXEL_W * VOXEL_D];
        cells[0] = 200;
        cells[VOXEL_W * VOXEL_D - 1] = 100;
        assert_eq!(grid_average(&cells), 300.0 / (VOXEL_W * VOXEL_D) as f32);
        assert_eq!(center_confidence(&cells), 0);

        cells[(VOXEL_D / 2) * VOXEL_W + VOXEL_W / 2] = 77;
        assert_eq!(center_confidence(&cells), 77);

        let mut grid = empty_grid();
        write_floor_grid(&mut grid, &frame(1, 1, true, flat(255)));
        assert!(grid_average(&grid.cells) > 200.0);
    }

    #[test]
    fn poll_interval_is_the_configured_milliseconds_or_the_default() {
        assert_eq!(poll_ms_from(None), 100);
        assert_eq!(poll_ms_from(Some("250")), 250);
        assert_eq!(poll_ms_from(Some("0")), 0);
        assert_eq!(poll_ms_from(Some("soon")), 100);
        assert_eq!(poll_ms_from(Some("")), 100);
    }

    #[test]
    fn shm_name_is_the_configured_region_or_the_stack_default() {
        assert_eq!(shm_name_from(None), "/qualia_body");
        assert_eq!(shm_name_from(Some("/qualia_other")), "/qualia_other");
    }

    #[test]
    fn an_unreachable_region_names_itself_and_its_cause() {
        let name = region_name("absent");
        let err = match ShmRegion::open(&name) {
            Ok(_) => panic!("an absent region cannot be attached"),
            Err(err) => err,
        };

        let message = open_failure_message(&name, &err);
        assert!(message.starts_with(&format!("qualia-floor: failed to open shm '{name}': ")));
        assert!(message.ends_with(&err.to_string()));
        assert!(message.len() > "qualia-floor: failed to open shm ''".len());
    }
}
