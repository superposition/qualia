//! `qualia-vslam`: the visual front end that turns camera thumbnails into odometry.
//!
//! The runner is deliberately small. It consumes the two seqlocked camera
//! products the rest of the stack already publishes — the downscaled luma
//! thumbnail and the camera-derived floor grid — and estimates three things
//! per frame: how far the view translated, how far it turned, and how much the
//! frame can be trusted. Those are written into
//! [`qualia_types::VslamFrontendState`], and when the front end owns the pose
//! it also writes [`qualia_types::NavPose`] into the world model.
//!
//! Two properties matter for the rest of the stack:
//!
//! * **The pose is shared, not sovereign.** The front end publishes a pose only
//!   while nobody else is publishing one, and it yields as soon as an external
//!   writer, such as the LiDAR localiser, takes over. Re-acquisition is by
//!   deadline, never by a stomping write.
//! * **Every match is quality-scored.** A flat image yields no shift, a weak
//!   match yields a low score, and confidence is the published summary of that
//!   evidence, so a consumer never has to re-derive it from raw pixels.
//!
//! Nothing here allocates per frame: the working buffers live in [`Frontend`].

use qualia_shm::ShmRegion;
use qualia_types::{
    NavPose, CAMERA_THUMB_H, CAMERA_THUMB_PIXELS, CAMERA_THUMB_W, VOXEL_D, VOXEL_W,
};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

/// Environment variable naming the shared-memory region to attach to.
pub const ENV_SHM_NAME: &str = "QUALIA_SHM_NAME";
/// Environment variable overriding the poll interval in milliseconds.
pub const ENV_POLL_MS: &str = "QUALIA_VSLAM_POLL_MS";
/// Environment variable that lets the front end publish a pose at all.
pub const ENV_PUBLISH_POSE: &str = "QUALIA_VSLAM_PUBLISH_POSE";
/// Environment variable that makes the front end publish a pose unconditionally.
pub const ENV_FORCE_POSE: &str = "QUALIA_VSLAM_FORCE_POSE";

/// Region name used when [`ENV_SHM_NAME`] is unset.
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";
/// Poll interval used when [`ENV_POLL_MS`] is unset or unparsable.
pub const DEFAULT_POLL_MS: u64 = 33;

/// Frame-to-frame difference that alone justifies a new keyframe.
pub const KEYFRAME_TRANSLATION_PROXY: f32 = 0.06;
/// Side length of one floor-grid cell in metres.
pub const FLOOR_CELL_M: f32 = 0.4;
/// Age at which a pose nobody refreshes is considered stale.
pub const VSLAM_POSE_STALE_NS: u64 = 500_000_000;
/// Largest floor-grid translation, in cells, considered in one frame.
pub const MAX_FLOOR_SHIFT_CELLS: isize = 3;
/// Largest horizontal thumbnail shift, in pixels, considered in one frame.
pub const MAX_THUMB_SHIFT_PX: isize = 6;
/// Largest vertical thumbnail shift, in pixels, considered in one frame.
pub const MAX_THUMB_SHIFT_Y_PX: isize = 4;
/// Radians of yaw per pixel of opposed half-frame displacement.
pub const YAW_RAD_PER_PX: f32 = 0.010;
/// Largest body-frame translation integrated from a single frame.
pub const MAX_STEP_M: f32 = FLOOR_CELL_M * 2.0;
/// Largest yaw change integrated from a single frame.
pub const MAX_YAW_STEP_RAD: f32 = 0.20;

/// Seqlock read attempts before a frame is treated as unavailable.
const SNAPSHOT_ATTEMPTS: usize = 8;
/// Translation score below which floor evidence still counts, at this weight.
const FLOOR_MOTION_FLOOR: f32 = 0.25;
/// Translation score below which thumbnail evidence still counts, at this weight.
const THUMB_MOTION_FLOOR: f32 = 0.15;
/// Thumbnail pixels per cell used when only the thumbnail is available.
const THUMB_METRES_PER_PX: f32 = 0.15;
/// Published confidence at or above which tracking is considered healthy.
const TRACKING_OK_THRESHOLD: f32 = 0.55;
/// Smallest floor match, in cells, that may move the pose.
const MIN_MATCH_SAMPLES: usize = 64;
/// Fraction of the baseline sample count a candidate shift must retain.
const MIN_SAMPLE_FRACTION: usize = 2;
/// Baseline error floor, so a perfect match is not divided by zero.
const MIN_ERROR_FLOOR: f32 = 0.001;
/// Interior gradient magnitude, summed over both axes, that counts as a corner.
const FEATURE_GRADIENT_THRESHOLD: i32 = 48;
/// Corner count at which the texture term saturates.
const FEATURE_SATURATION: f32 = 900.0;
/// Mean gradient at which the sharpness term saturates.
const GRADIENT_SATURATION: f32 = 0.18;
/// Frame difference at which the motion term saturates.
const MOTION_SATURATION: f32 = 0.12;
/// Confidence a featureless, motionless frame still reports.
const BASE_TRACKING_CONFIDENCE: f32 = 0.20;
/// Both confidences saturate here, never at certainty.
const CONFIDENCE_CEILING: f32 = 0.98;
/// Translation evidence's share of the published pose confidence.
const POSE_TRANSLATION_WEIGHT: f32 = 0.20;
/// Yaw evidence's share of the published pose confidence.
const POSE_YAW_WEIGHT: f32 = 0.10;
/// Fixed bonus for a ready floor grid.
const FLOOR_EVIDENCE_BONUS: f32 = 0.10;

/// The front end estimates a planar, level pose, so every field it does not
/// compute itself is zero. This is that zero template.
const LEVEL_POSE: NavPose = NavPose {
    x_m: 0.0,
    y_m: 0.0,
    z_m: 0.0,
    yaw_rad: 0.0,
    pitch_rad: 0.0,
    roll_rad: 0.0,
    confidence: 0.0,
    _pad0: 0.0,
    timestamp_ns: 0,
};

/// Floor-grid window matched between frames, inset from the border so the
/// search offsets never need pixels outside it.
const FLOOR_REGION: MatchRegion = MatchRegion {
    x0: 2,
    x1: VOXEL_W - 2,
    y0: 2,
    y1: VOXEL_D - 2,
};

/// Vertical extent of the thumbnail matched for translation: the lower part of
/// the frame, where the floor recedes and the parallax is largest.
fn thumbnail_band() -> (usize, usize) {
    (CAMERA_THUMB_H / 3, CAMERA_THUMB_H - 4)
}

/// Dead-reckoned pose in the front end's own frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LocalPose {
    pub x_m: f32,
    pub z_m: f32,
    pub yaw_rad: f32,
}

/// Half-open pixel window matched between consecutive frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchRegion {
    pub x0: usize,
    pub x1: usize,
    pub y0: usize,
    pub y1: usize,
}

/// Pixel dimensions of the two images a match runs over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageGrid {
    pub width: usize,
    pub height: usize,
}

impl ImageGrid {
    /// The camera thumbnail's geometry.
    pub const THUMB: Self = Self {
        width: CAMERA_THUMB_W,
        height: CAMERA_THUMB_H,
    };
    /// The camera-derived floor grid's geometry.
    pub const FLOOR: Self = Self {
        width: VOXEL_W,
        height: VOXEL_D,
    };
}

/// Result of matching one region at one shift.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShiftMatch {
    pub dx: isize,
    pub dy: isize,
    /// Mean absolute luma difference at this shift, in `0.0..=1.0`.
    pub error: f32,
    /// Improvement over the zero shift, in `0.0..=1.0`.
    pub score: f32,
    /// Pixels that contributed to the error.
    pub samples: usize,
}

/// Per-frame texture and motion summary.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackingStats {
    pub feature_count: usize,
    pub mean_gradient: f32,
    pub mean_cornerness: f32,
    pub frame_delta: f32,
    pub confidence: f32,
}

impl TrackingStats {
    /// Measures `thumb`, comparing it against `prev` when one is available.
    pub fn measure(prev: Option<&[u8]>, thumb: &[u8]) -> Self {
        let feature_count = count_features(thumb);
        let mean_gradient = mean_gradient(thumb);
        let mean_cornerness = mean_cornerness(thumb);
        let frame_delta = prev.map_or(0.0, |previous| mean_abs_delta(previous, thumb));
        let confidence = tracking_confidence(feature_count, mean_gradient, frame_delta);
        Self {
            feature_count,
            mean_gradient,
            mean_cornerness,
            frame_delta,
            confidence,
        }
    }
}

/// Runtime configuration, read once at start-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontendConfig {
    pub shm_name: String,
    pub poll_ms: u64,
    pub publish_pose: bool,
    pub force_pose: bool,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self {
            shm_name: String::from(DEFAULT_SHM_NAME),
            poll_ms: DEFAULT_POLL_MS,
            publish_pose: true,
            force_pose: false,
        }
    }
}

impl FrontendConfig {
    /// Reads the documented environment keys, falling back to defaults.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Resolves the configuration through `lookup`; separated from the
    /// environment so the mapping is testable without mutating the process.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let shm_name = lookup(ENV_SHM_NAME)
            .filter(|value| !value.is_empty())
            .unwrap_or(defaults.shm_name);
        let poll_ms = lookup(ENV_POLL_MS)
            .and_then(|value| value.parse().ok())
            .filter(|ms| *ms > 0)
            .unwrap_or(defaults.poll_ms);
        Self {
            shm_name,
            poll_ms,
            publish_pose: flag(&lookup, ENV_PUBLISH_POSE, true),
            force_pose: flag(&lookup, ENV_FORCE_POSE, false),
        }
    }

    /// The poll interval as a duration.
    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.poll_ms)
    }
}

/// True-ish spellings accepted for boolean environment keys.
fn flag(lookup: &impl Fn(&str) -> Option<String>, name: &str, default: bool) -> bool {
    match lookup(name) {
        Some(value) if default => !matches!(value.as_str(), "0" | "false" | "FALSE" | "no" | "NO"),
        Some(value) => matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        None => default,
    }
}

/// Owns the pose slot while the front end is the active pose source.
///
/// The front end only ever becomes the writer when nobody else has published a
/// pose, or when the published one has gone stale; once established it steps
/// aside for any newer pose that differs from its own.
#[derive(Clone, Copy, Debug, Default)]
pub struct PosePublisher {
    publishing: bool,
    last_write_ns: u64,
}

impl PosePublisher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this publisher currently considers itself the pose source.
    pub fn is_publishing(&self) -> bool {
        self.publishing
    }

    /// Timestamp of the last pose this publisher wrote, or zero.
    pub fn last_write_ns(&self) -> u64 {
        self.last_write_ns
    }

    /// Writes `pose` when the front end is entitled to, returning whether it did.
    pub fn publish_if_needed(
        &mut self,
        shm: &ShmRegion,
        pose: LocalPose,
        timestamp_ns: u64,
        confidence: f32,
        force: bool,
    ) -> bool {
        let current = shm.world_model().robot_pose;
        let decided = if force {
            true
        } else if self.publishing {
            let external_took_over = current.timestamp_ns > self.last_write_ns
                && !same_pose(current, pose, 0.05, 0.05);
            !external_took_over
        } else {
            current.timestamp_ns == 0
                || current.confidence <= 0.05
                || timestamp_ns.saturating_sub(current.timestamp_ns) > VSLAM_POSE_STALE_NS
        };

        if !decided {
            if !force
                && current.timestamp_ns > self.last_write_ns
                && !same_pose(current, pose, 0.05, 0.05)
            {
                self.publishing = false;
            }
            return false;
        }

        shm.set_robot_pose(NavPose {
            x_m: pose.x_m,
            z_m: pose.z_m,
            yaw_rad: pose.yaw_rad,
            confidence,
            timestamp_ns,
            ..LEVEL_POSE
        });
        self.publishing = true;
        self.last_write_ns = timestamp_ns;
        true
    }
}

/// What one tracked frame published, for the operator log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameReport {
    pub frame_seq: u64,
    pub timestamp_ns: u64,
    pub feature_count: u32,
    pub keyframe_count: u32,
    pub tracking_confidence: f32,
    pub mean_gradient: f32,
    pub mean_cornerness: f32,
    pub pose: LocalPose,
    pub pose_confidence: f32,
    pub motion_dx_cells: f32,
    pub motion_dz_cells: f32,
    pub motion_yaw_delta_rad: f32,
    pub pose_writer_active: bool,
}

impl FrameReport {
    /// The per-frame log record operators and tooling grep for.
    pub fn log_line(&self) -> String {
        format!(
            "qualia-vslam: frame_seq={} track_conf={:.3} pose=({:.2},{:.2}) yaw={:.2} \
             pose_conf={:.3} floor_shift=({:.1},{:.1}) yaw_delta={:.3} features={} keyframes={} \
             writer={}",
            self.frame_seq,
            self.tracking_confidence,
            self.pose.x_m,
            self.pose.z_m,
            self.pose.yaw_rad,
            self.pose_confidence,
            self.motion_dx_cells,
            self.motion_dz_cells,
            self.motion_yaw_delta_rad,
            self.feature_count,
            self.keyframe_count,
            u8::from(self.pose_writer_active),
        )
    }
}

/// The front end proper: one instance holds every buffer the loop reuses.
pub struct Frontend {
    config: FrontendConfig,
    last_frame_seq: u64,
    prev_thumb: [u8; CAMERA_THUMB_PIXELS],
    prev_floor: [u8; VOXEL_W * VOXEL_D],
    has_prev_thumb: bool,
    has_prev_floor: bool,
    pose: LocalPose,
    publisher: PosePublisher,
    keyframe_count: u32,
}

impl Frontend {
    pub fn new(config: FrontendConfig) -> Self {
        Self {
            config,
            last_frame_seq: 0,
            prev_thumb: [0; CAMERA_THUMB_PIXELS],
            prev_floor: [0; VOXEL_W * VOXEL_D],
            has_prev_thumb: false,
            has_prev_floor: false,
            pose: LocalPose::default(),
            publisher: PosePublisher::new(),
            keyframe_count: 0,
        }
    }

    pub fn config(&self) -> &FrontendConfig {
        &self.config
    }

    /// Zeroes the published state, so a restarted front end never leaves a
    /// half-filled slot behind for its readers.
    pub fn init_state(&self, shm: &ShmRegion) {
        let state = shm.vslam_frontend_mut();
        // Nobody is tracking yet; the sequence word is written last, so a
        // reader that sees a non-zero sequence sees an initialized slot.
        state.tracking_ok.store(false, Ordering::Release);
        state.pose_writer_active = 0;

        // Which frame this slot describes.
        state.frame_seq = 0;
        state.timestamp_ns = 0;
        state.loop_closure_count = 0;

        // Where the front end thinks it is, and how much it trusts that.
        state.tracking_confidence = 0.0;
        state.pose_confidence = 0.0;
        state.pose_x_m = 0.0;
        state.pose_z_m = 0.0;
        state.yaw_rad = 0.0;

        // What the frame itself looked like.
        state.mean_gradient = 0.0;
        state.mean_cornerness = 0.0;
        state.feature_count = 0;
        state.keyframe_count = 0;

        // The last motion integrated into the pose.
        state.motion_dx_cells = 0.0;
        state.motion_dz_cells = 0.0;
        state.motion_yaw_delta_rad = 0.0;

        state.seq.store(0, Ordering::Release);
    }

    /// Processes the newest camera frame once.
    ///
    /// Returns `None` when nothing new is available — no frame yet, the same
    /// sequence twice, or a frame the camera marked invalid.
    pub fn tick(&mut self, shm: &ShmRegion) -> Option<FrameReport> {
        let frame = shm.camera_frame().snapshot(SNAPSHOT_ATTEMPTS).ok()?;
        if !frame.valid || frame.seq == 0 || frame.seq == self.last_frame_seq {
            return None;
        }

        let thumb = frame.thumbnail_luma;
        let tracking = TrackingStats::measure(
            self.has_prev_thumb.then_some(self.prev_thumb.as_slice()),
            &thumb,
        );

        let floor = shm.camera_floor();
        let floor_seq = floor.seq.load(Ordering::Acquire);
        let floor_ready = floor_seq != 0
            && floor.width == VOXEL_W as u32
            && floor.height == VOXEL_D as u32
            && floor.last_update_ns != 0;
        let floor_cells = floor_ready.then(|| floor.cells);

        let floor_shift = match floor_cells.as_ref() {
            Some(cells) if self.has_prev_floor => estimate_shift(
                &self.prev_floor,
                cells,
                ImageGrid::FLOOR,
                FLOOR_REGION,
                MAX_FLOOR_SHIFT_CELLS,
                MAX_FLOOR_SHIFT_CELLS,
            ),
            _ => ShiftMatch::default(),
        };

        let (band_y0, band_y1) = thumbnail_band();
        let full_region = MatchRegion {
            x0: 4,
            x1: CAMERA_THUMB_W - 4,
            y0: band_y0,
            y1: band_y1,
        };
        let left_region = MatchRegion {
            x0: 4,
            x1: CAMERA_THUMB_W / 2,
            y0: band_y0,
            y1: band_y1,
        };
        let right_region = MatchRegion {
            x0: CAMERA_THUMB_W / 2,
            x1: CAMERA_THUMB_W - 4,
            y0: band_y0,
            y1: band_y1,
        };
        let (full_shift, left_shift, right_shift) = if self.has_prev_thumb {
            (
                estimate_shift(
                    &self.prev_thumb,
                    &thumb,
                    ImageGrid::THUMB,
                    full_region,
                    MAX_THUMB_SHIFT_PX,
                    MAX_THUMB_SHIFT_Y_PX,
                ),
                estimate_shift(
                    &self.prev_thumb,
                    &thumb,
                    ImageGrid::THUMB,
                    left_region,
                    MAX_THUMB_SHIFT_PX,
                    MAX_THUMB_SHIFT_Y_PX,
                ),
                estimate_shift(
                    &self.prev_thumb,
                    &thumb,
                    ImageGrid::THUMB,
                    right_region,
                    MAX_THUMB_SHIFT_PX,
                    MAX_THUMB_SHIFT_Y_PX,
                ),
            )
        } else {
            (
                ShiftMatch::default(),
                ShiftMatch::default(),
                ShiftMatch::default(),
            )
        };

        let motion_dx_cells = -(floor_shift.dx as f32);
        let motion_dz_cells = -(floor_shift.dy as f32);
        let (yaw_delta_rad, yaw_score) = estimate_yaw_delta(left_shift, right_shift, full_shift);

        if self.has_prev_floor && floor_shift.samples > 0 {
            integrate_local_pose(
                &mut self.pose,
                motion_dx_cells,
                motion_dz_cells,
                yaw_delta_rad,
                floor_shift.score.max(FLOOR_MOTION_FLOOR),
            );
        } else if self.has_prev_thumb && full_shift.samples > 0 {
            integrate_local_pose(
                &mut self.pose,
                -(full_shift.dx as f32) * THUMB_METRES_PER_PX,
                -(full_shift.dy as f32) * THUMB_METRES_PER_PX,
                yaw_delta_rad,
                full_shift.score.max(THUMB_MOTION_FLOOR),
            );
        }

        let pose_confidence = pose_confidence(
            tracking.confidence,
            floor_shift.score,
            yaw_score,
            floor_ready,
        );
        let wrote_pose = if self.config.publish_pose {
            self.publisher.publish_if_needed(
                shm,
                self.pose,
                frame.timestamp_ns,
                pose_confidence,
                self.config.force_pose,
            )
        } else {
            false
        };

        self.keyframe_count = if tracking.frame_delta >= KEYFRAME_TRANSLATION_PROXY
            || floor_shift.score >= 0.18
            || yaw_delta_rad.abs() >= 0.04
        {
            self.keyframe_count.saturating_add(1)
        } else {
            self.keyframe_count.max(1)
        };

        let state = shm.vslam_frontend_mut();
        let previous_seq = state.seq.load(Ordering::Acquire);
        state.frame_seq = frame.seq;
        state.timestamp_ns = frame.timestamp_ns;
        state.feature_count = tracking.feature_count as u32;
        state.keyframe_count = self.keyframe_count;
        state.loop_closure_count = 0;
        state.mean_gradient = tracking.mean_gradient;
        state.mean_cornerness = tracking.mean_cornerness;
        state.tracking_confidence = tracking.confidence;
        state.pose_x_m = self.pose.x_m;
        state.pose_z_m = self.pose.z_m;
        state.yaw_rad = self.pose.yaw_rad;
        state.pose_confidence = pose_confidence;
        state.motion_dx_cells = motion_dx_cells;
        state.motion_dz_cells = motion_dz_cells;
        state.motion_yaw_delta_rad = yaw_delta_rad;
        state.pose_writer_active = u8::from(wrote_pose);
        state
            .tracking_ok
            .store(pose_confidence >= TRACKING_OK_THRESHOLD, Ordering::Release);
        state.seq.store(previous_seq.wrapping_add(1), Ordering::Release);

        self.prev_thumb.copy_from_slice(&thumb);
        self.has_prev_thumb = true;
        if let Some(cells) = floor_cells {
            self.prev_floor.copy_from_slice(&cells);
            self.has_prev_floor = true;
        }
        self.last_frame_seq = frame.seq;

        Some(FrameReport {
            frame_seq: frame.seq,
            timestamp_ns: frame.timestamp_ns,
            feature_count: tracking.feature_count as u32,
            keyframe_count: self.keyframe_count,
            tracking_confidence: tracking.confidence,
            mean_gradient: tracking.mean_gradient,
            mean_cornerness: tracking.mean_cornerness,
            pose: self.pose,
            pose_confidence,
            motion_dx_cells,
            motion_dz_cells,
            motion_yaw_delta_rad: yaw_delta_rad,
            pose_writer_active: wrote_pose,
        })
    }

    /// Runs until the process is stopped, logging every thirtieth frame.
    pub fn run(&mut self, shm: &ShmRegion) -> ! {
        let interval = self.config.poll_interval();
        let mut published: u64 = 0;
        loop {
            if let Some(report) = self.tick(shm) {
                published += 1;
                if published == 1 || published % 30 == 0 {
                    println!("{}", report.log_line());
                }
            }
            thread::sleep(interval);
        }
    }
}

/// Best integer shift of `curr` against `prev` within `max_dx`/`max_dy`.
///
/// The zero shift is measured first; a region that cannot supply
/// [`MIN_MATCH_SAMPLES`] overlapping pixels yields the default match, whose
/// `samples` of zero tells the caller there is no evidence. The returned score
/// is the fraction of the zero-shift error the best shift removed.
pub fn estimate_shift(
    prev: &[u8],
    curr: &[u8],
    grid: ImageGrid,
    region: MatchRegion,
    max_dx: isize,
    max_dy: isize,
) -> ShiftMatch {
    let baseline = shift_error(prev, curr, grid, region, 0, 0);
    if baseline.samples < MIN_MATCH_SAMPLES {
        return ShiftMatch::default();
    }

    let samples_needed = baseline.samples / MIN_SAMPLE_FRACTION;
    let mut chosen = baseline.error;
    let mut chosen_offset = (0isize, 0isize, baseline.samples);
    for offset_y in -max_dy..=max_dy {
        for offset_x in -max_dx..=max_dx {
            let candidate = shift_error(prev, curr, grid, region, offset_x, offset_y);
            if candidate.samples < samples_needed {
                continue;
            }
            if candidate.error < chosen {
                chosen = candidate.error;
                chosen_offset = (offset_x, offset_y, candidate.samples);
            }
        }
    }

    let explained = (baseline.error - chosen) / baseline.error.max(MIN_ERROR_FLOOR);
    ShiftMatch {
        dx: chosen_offset.0,
        dy: chosen_offset.1,
        error: chosen,
        score: explained.clamp(0.0, 1.0),
        samples: chosen_offset.2,
    }
}

/// Mean absolute luma difference of `prev` against `curr` shifted by `(dx, dy)`.
///
/// Only pixels whose shifted counterpart falls inside the image contribute;
/// with no overlap the error saturates at `1.0` and `samples` is zero.
pub fn shift_error(
    prev: &[u8],
    curr: &[u8],
    grid: ImageGrid,
    region: MatchRegion,
    dx: isize,
    dy: isize,
) -> ShiftMatch {
    let mut difference = 0.0f32;
    let mut counted = 0usize;
    for row in region.y0..region.y1 {
        let Some(shifted_row) = in_bounds(row as isize + dy, grid.height) else {
            continue;
        };
        for column in region.x0..region.x1 {
            let Some(shifted_column) = in_bounds(column as isize + dx, grid.width) else {
                continue;
            };
            let before = prev[row * grid.width + column] as f32 / 255.0;
            let after = curr[shifted_row * grid.width + shifted_column] as f32 / 255.0;
            difference += (before - after).abs();
            counted += 1;
        }
    }
    ShiftMatch {
        dx,
        dy,
        error: if counted == 0 {
            1.0
        } else {
            difference / counted as f32
        },
        score: 0.0,
        samples: counted,
    }
}

/// A non-negative index inside an image of `limit` pixels, or nothing.
fn in_bounds(index: isize, limit: usize) -> Option<usize> {
    (index >= 0 && (index as usize) < limit).then_some(index as usize)
}

/// Yaw change implied by the two half-frames sliding against each other.
///
/// Returns the integrated step and the score behind it; without evidence in
/// both halves the answer is `(0.0, 0.0)` rather than a guess.
pub fn estimate_yaw_delta(left: ShiftMatch, right: ShiftMatch, full: ShiftMatch) -> (f32, f32) {
    if left.samples == 0 || right.samples == 0 {
        return (0.0, 0.0);
    }
    let opposed_px = (right.dx - left.dx) as f32 * 0.5;
    let confidence = ((left.score + right.score) * 0.5).max(full.score * 0.5);
    let step = (opposed_px * YAW_RAD_PER_PX * confidence).clamp(-MAX_YAW_STEP_RAD, MAX_YAW_STEP_RAD);
    (step, confidence)
}

/// Advances `pose` by a body-frame step, clamped and wrapped.
///
/// `dx_cells`/`dz_cells` are floor-grid cells and rotate into the world frame
/// through the current yaw; a single frame can never move the pose further than
/// [`MAX_STEP_M`] or turn it further than [`MAX_YAW_STEP_RAD`].
pub fn integrate_local_pose(
    pose: &mut LocalPose,
    dx_cells: f32,
    dz_cells: f32,
    yaw_delta_rad: f32,
    motion_weight: f32,
) {
    let along_x = (dx_cells * FLOOR_CELL_M * motion_weight).clamp(-MAX_STEP_M, MAX_STEP_M);
    let along_z = (dz_cells * FLOOR_CELL_M * motion_weight).clamp(-MAX_STEP_M, MAX_STEP_M);
    let cosine = pose.yaw_rad.cos();
    let sine = pose.yaw_rad.sin();
    let (world_x, world_z) = (
        cosine * along_x - sine * along_z,
        sine * along_x + cosine * along_z,
    );
    pose.x_m += world_x;
    pose.z_m += world_z;
    pose.yaw_rad = normalize_angle(pose.yaw_rad + yaw_delta_rad);
}

/// Published pose confidence for one frame.
///
/// Texture tracking dominates; translation and yaw matches add evidence, and a
/// ready floor grid adds a fixed bonus. The value saturates below certainty.
pub fn pose_confidence(
    tracking_confidence: f32,
    translation_score: f32,
    yaw_score: f32,
    floor_ready: bool,
) -> f32 {
    let floor_evidence = if floor_ready { FLOOR_EVIDENCE_BONUS } else { 0.0 };
    (tracking_confidence
        + POSE_TRANSLATION_WEIGHT * translation_score
        + POSE_YAW_WEIGHT * yaw_score
        + floor_evidence)
        .clamp(0.0, CONFIDENCE_CEILING)
}

/// Wraps an angle into `(-π, π]`.
pub fn normalize_angle(angle: f32) -> f32 {
    let full_turn = 2.0 * std::f32::consts::PI;
    let mut wrapped = angle;
    while wrapped > std::f32::consts::PI {
        wrapped -= full_turn;
    }
    while wrapped < -std::f32::consts::PI {
        wrapped += full_turn;
    }
    wrapped
}

/// Corner-like pixels: gradient magnitude at or above a fixed threshold.
pub fn count_features(thumb: &[u8]) -> usize {
    let mut corners = 0usize;
    for row in 1..CAMERA_THUMB_H - 1 {
        let base = row * CAMERA_THUMB_W;
        for column in 1..CAMERA_THUMB_W - 1 {
            let at = base + column;
            let horizontal = thumb[at + 1] as i32 - thumb[at - 1] as i32;
            let vertical = thumb[at + CAMERA_THUMB_W] as i32 - thumb[at - CAMERA_THUMB_W] as i32;
            if horizontal.abs() + vertical.abs() >= FEATURE_GRADIENT_THRESHOLD {
                corners += 1;
            }
        }
    }
    corners
}

/// Mean gradient magnitude over the interior, normalised to `0.0..=1.0`.
pub fn mean_gradient(thumb: &[u8]) -> f32 {
    let mut total = 0.0f32;
    let mut counted = 0usize;
    for row in 1..CAMERA_THUMB_H - 1 {
        let base = row * CAMERA_THUMB_W;
        for column in 1..CAMERA_THUMB_W - 1 {
            let at = base + column;
            let horizontal = thumb[at + 1] as f32 - thumb[at - 1] as f32;
            let vertical = thumb[at + CAMERA_THUMB_W] as f32 - thumb[at - CAMERA_THUMB_W] as f32;
            total += (horizontal * horizontal + vertical * vertical).sqrt() / 255.0;
            counted += 1;
        }
    }
    if counted == 0 {
        0.0
    } else {
        total / counted as f32
    }
}

/// Mean four-neighbour contrast over the interior, normalised to `0.0..=1.0`.
pub fn mean_cornerness(thumb: &[u8]) -> f32 {
    let mut total = 0.0f32;
    let mut counted = 0usize;
    for row in 1..CAMERA_THUMB_H - 1 {
        let base = row * CAMERA_THUMB_W;
        for column in 1..CAMERA_THUMB_W - 1 {
            let at = base + column;
            let centre = thumb[at] as f32;
            let neighbours = [
                thumb[at - 1] as f32,
                thumb[at + 1] as f32,
                thumb[at - CAMERA_THUMB_W] as f32,
                thumb[at + CAMERA_THUMB_W] as f32,
            ];
            let contrast: f32 = neighbours
                .iter()
                .map(|neighbour| (centre - neighbour).abs())
                .sum();
            total += contrast / (4.0 * 255.0);
            counted += 1;
        }
    }
    if counted == 0 {
        0.0
    } else {
        total / counted as f32
    }
}

/// Mean absolute luma change between two equal-length frames, in `0.0..=1.0`.
pub fn mean_abs_delta(prev: &[u8], curr: &[u8]) -> f32 {
    let mut total = 0.0f32;
    for (previous, current) in prev.iter().zip(curr.iter()) {
        total += (*previous as f32 - *current as f32).abs() / 255.0;
    }
    total / prev.len() as f32
}

/// Confidence in a frame from its texture and its motion since the last one.
pub fn tracking_confidence(feature_count: usize, gradient: f32, frame_delta: f32) -> f32 {
    let texture = (feature_count as f32 / FEATURE_SATURATION).clamp(0.0, 1.0);
    let sharpness = (gradient / GRADIENT_SATURATION).clamp(0.0, 1.0);
    let motion = (frame_delta / MOTION_SATURATION).clamp(0.0, 1.0);
    (BASE_TRACKING_CONFIDENCE + 0.45 * texture + 0.20 * sharpness + 0.15 * motion)
        .clamp(0.0, CONFIDENCE_CEILING)
}

/// Whether two poses agree in position and yaw within the given epsilons.
fn same_pose(current: NavPose, pose: LocalPose, position_epsilon_m: f32, yaw_epsilon_rad: f32) -> bool {
    let position_close = (current.x_m - pose.x_m).abs() <= position_epsilon_m
        && (current.z_m - pose.z_m).abs() <= position_epsilon_m;
    let yaw_close = normalize_angle(current.yaw_rad - pose.yaw_rad).abs() <= yaw_epsilon_rad;
    position_close && yaw_close
}
