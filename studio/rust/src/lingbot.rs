//! Geometry and stream-context logic behind the Lingbot C ABI.
//!
//! Nothing in here is `extern "C"`; the ABI surface in [`crate`] only marshals
//! values in and out of these types. The module is deliberately free of I/O so
//! the same code can be exercised from a host test harness.

use std::collections::{BTreeMap, VecDeque};

/// Smallest magnitude accepted before a division is considered degenerate.
const GUARD: f32 = 1.0e-6;

/// Rotation expressed as a unit quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quaternion {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Default for Quaternion {
    fn default() -> Self {
        Self {
            w: 1.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
}

impl Quaternion {
    fn length(self) -> f32 {
        (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    fn scaled(self, factor: f32) -> Self {
        Self {
            w: self.w * factor,
            x: self.x * factor,
            y: self.y * factor,
            z: self.z * factor,
        }
    }
}

/// Nine-value pose the model consumes: translation, rotation and both fields of
/// view, in that order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoseEncoding9 {
    pub translation: [f32; 3],
    pub rotation: Quaternion,
    pub fov_h: f32,
    pub fov_w: f32,
}

/// Pinhole camera parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraIntrinsics {
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub width: i32,
    pub height: i32,
}

/// Row-major 3x4 world-to-camera matrix.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extrinsic3x4(pub [f32; 12]);

/// Column-major 4x4 transform.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform4x4(pub [f32; 16]);

/// One reconstructed world-space sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReconstructionPoint {
    pub position: [f32; 3],
    pub confidence: f32,
    pub color: [u8; 3],
}

/// Borrowed depth frame; `confidence` and `rgb` may be absent.
pub struct DepthFrame<'a> {
    pub width: usize,
    pub height: usize,
    pub depth: &'a [f32],
    pub confidence: Option<&'a [f32]>,
    pub rgb: Option<&'a [u8]>,
}

/// Returns the identity rotation when the input cannot be normalised.
pub fn normalize_quaternion(quaternion: Quaternion) -> Quaternion {
    let length = quaternion.length();
    if !length.is_finite() || length <= f32::EPSILON {
        return Quaternion::default();
    }
    quaternion.scaled(1.0 / length)
}

/// Packs a world-to-camera matrix and its camera parameters into the nine
/// values the model sees.
pub fn encode_pose(extrinsic: Extrinsic3x4, intrinsics: CameraIntrinsics) -> PoseEncoding9 {
    let v = extrinsic.0;
    let rotation = [
        v[0], v[1], v[2], v[4], v[5], v[6], v[8], v[9], v[10],
    ];
    let half_width = intrinsics.width.max(1) as f32 * 0.5;
    let half_height = intrinsics.height.max(1) as f32 * 0.5;
    PoseEncoding9 {
        translation: [v[3], v[7], v[11]],
        rotation: rotation_to_quaternion(rotation),
        fov_h: 2.0 * half_height.atan2(intrinsics.fy.max(GUARD)),
        fov_w: 2.0 * half_width.atan2(intrinsics.fx.max(GUARD)),
    }
}

/// Inverse of [`encode_pose`]. The image dimensions are needed because the
/// encoding does not carry the resolution, only the field of view it implies.
pub fn decode_pose(
    encoding: PoseEncoding9,
    image_height: i32,
    image_width: i32,
) -> (Extrinsic3x4, CameraIntrinsics) {
    let rotation = quaternion_to_rotation(normalize_quaternion(encoding.rotation));
    let half_width = image_width.max(1) as f32 * 0.5;
    let half_height = image_height.max(1) as f32 * 0.5;
    let half_fov_w = (encoding.fov_w.max(GUARD) * 0.5).tan();
    let half_fov_h = (encoding.fov_h.max(GUARD) * 0.5).tan();
    let extrinsic = Extrinsic3x4([
        rotation[0],
        rotation[1],
        rotation[2],
        encoding.translation[0],
        rotation[3],
        rotation[4],
        rotation[5],
        encoding.translation[1],
        rotation[6],
        rotation[7],
        rotation[8],
        encoding.translation[2],
    ]);
    let intrinsics = CameraIntrinsics {
        fx: half_width / half_fov_w,
        fy: half_height / half_fov_h,
        cx: image_width as f32 * 0.5,
        cy: image_height as f32 * 0.5,
        width: image_width,
        height: image_height,
    };
    (extrinsic, intrinsics)
}

/// Inverts a rigid transform: transpose the rotation, then rotate and negate
/// the translation.
pub fn inverse_se3(extrinsic: Extrinsic3x4) -> Transform4x4 {
    let v = extrinsic.0;
    let translation = [v[3], v[7], v[11]];
    let rotation = [
        [v[0], v[1], v[2]],
        [v[4], v[5], v[6]],
        [v[8], v[9], v[10]],
    ];
    let inverse_translation = [
        -(rotation[0][0] * translation[0]
            + rotation[1][0] * translation[1]
            + rotation[2][0] * translation[2]),
        -(rotation[0][1] * translation[0]
            + rotation[1][1] * translation[1]
            + rotation[2][1] * translation[2]),
        -(rotation[0][2] * translation[0]
            + rotation[1][2] * translation[1]
            + rotation[2][2] * translation[2]),
    ];
    Transform4x4([
        rotation[0][0],
        rotation[1][0],
        rotation[2][0],
        inverse_translation[0],
        rotation[0][1],
        rotation[1][1],
        rotation[2][1],
        inverse_translation[1],
        rotation[0][2],
        rotation[1][2],
        rotation[2][2],
        inverse_translation[2],
        0.0,
        0.0,
        0.0,
        1.0,
    ])
}

/// Lifts every valid depth pixel into world space. Pixels at or below
/// `min_depth`, non-finite samples and frames that do not match their declared
/// size are dropped.
pub fn unproject_depth(
    frame: DepthFrame<'_>,
    extrinsic: Extrinsic3x4,
    intrinsics: CameraIntrinsics,
    min_depth: f32,
) -> Vec<ReconstructionPoint> {
    let Some(pixel_count) = frame.width.checked_mul(frame.height) else {
        return Vec::new();
    };
    if frame.depth.len() < pixel_count {
        return Vec::new();
    }
    if !intrinsics.fx.is_finite()
        || !intrinsics.fy.is_finite()
        || intrinsics.fx.abs() <= GUARD
        || intrinsics.fy.abs() <= GUARD
        || !min_depth.is_finite()
    {
        return Vec::new();
    }

    let camera_to_world = inverse_se3(extrinsic).0;
    let mut points = Vec::with_capacity(pixel_count / 4 + 1);
    for row in 0..frame.height {
        for column in 0..frame.width {
            let index = row * frame.width + column;
            let depth = frame.depth[index];
            if !depth.is_finite() || depth <= min_depth {
                continue;
            }
            let camera_x = (column as f32 - intrinsics.cx) * depth / intrinsics.fx;
            let camera_y = (row as f32 - intrinsics.cy) * depth / intrinsics.fy;
            let confidence = frame
                .confidence
                .and_then(|values| values.get(index).copied())
                .unwrap_or(1.0);
            if !confidence.is_finite() {
                continue;
            }
            let color = frame
                .rgb
                .and_then(|values| values.get(index * 3..index * 3 + 3))
                .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .unwrap_or([0, 0, 0]);
            points.push(ReconstructionPoint {
                position: [
                    camera_to_world[0] * camera_x
                        + camera_to_world[1] * camera_y
                        + camera_to_world[2] * depth
                        + camera_to_world[3],
                    camera_to_world[4] * camera_x
                        + camera_to_world[5] * camera_y
                        + camera_to_world[6] * depth
                        + camera_to_world[7],
                    camera_to_world[8] * camera_x
                        + camera_to_world[9] * camera_y
                        + camera_to_world[10] * depth
                        + camera_to_world[11],
                ],
                confidence: confidence.clamp(0.0, 1.0),
                color,
            });
        }
    }
    points
}

/// Averages each axis-aligned voxel into a single point. The output order is
/// the ascending voxel index, so repeated calls on the same cloud agree.
pub fn voxel_downsample(
    points: &[ReconstructionPoint],
    voxel_size: f32,
) -> Vec<ReconstructionPoint> {
    if points.is_empty() || !voxel_size.is_finite() || voxel_size <= f32::EPSILON {
        return points.to_vec();
    }

    #[derive(Default)]
    struct Cell {
        position: [f64; 3],
        confidence: f64,
        color: [u64; 3],
        samples: u64,
    }

    let mut cells: BTreeMap<(i64, i64, i64), Cell> = BTreeMap::new();
    for point in points {
        if point.position.iter().any(|axis| !axis.is_finite()) || !point.confidence.is_finite() {
            continue;
        }
        let key = (
            (point.position[0] / voxel_size).floor() as i64,
            (point.position[1] / voxel_size).floor() as i64,
            (point.position[2] / voxel_size).floor() as i64,
        );
        let cell = cells.entry(key).or_default();
        for axis in 0..3 {
            cell.position[axis] += f64::from(point.position[axis]);
            cell.color[axis] += u64::from(point.color[axis]);
        }
        cell.confidence += f64::from(point.confidence);
        cell.samples += 1;
    }

    cells
        .into_values()
        .map(|cell| {
            let samples = cell.samples.max(1) as f64;
            ReconstructionPoint {
                position: [
                    (cell.position[0] / samples) as f32,
                    (cell.position[1] / samples) as f32,
                    (cell.position[2] / samples) as f32,
                ],
                confidence: (cell.confidence / samples) as f32,
                color: [
                    (cell.color[0] / cell.samples.max(1)) as u8,
                    (cell.color[1] / cell.samples.max(1)) as u8,
                    (cell.color[2] / cell.samples.max(1)) as u8,
                ],
            }
        })
        .collect()
}

fn quaternion_to_rotation(q: Quaternion) -> [f32; 9] {
    let (w, x, y, z) = (q.w, q.x, q.y, q.z);
    let (ww, xx, yy, zz) = (w * w, x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        ww + xx - yy - zz,
        2.0 * (xy - wz),
        2.0 * (xz + wy),
        2.0 * (xy + wz),
        ww - xx + yy - zz,
        2.0 * (yz - wx),
        2.0 * (xz - wy),
        2.0 * (yz + wx),
        ww - xx - yy + zz,
    ]
}

/// Shepperd's method: pick the largest diagonal term so the square root never
/// cancels catastrophically.
fn rotation_to_quaternion(rotation: [f32; 9]) -> Quaternion {
    let (m00, m11, m22) = (rotation[0], rotation[4], rotation[8]);
    let trace = m00 + m11 + m22;
    let quaternion = if trace > 0.0 {
        let scale = ((trace + 1.0).sqrt()) * 2.0;
        Quaternion {
            w: 0.25 * scale,
            x: (rotation[7] - rotation[5]) / scale,
            y: (rotation[2] - rotation[6]) / scale,
            z: (rotation[3] - rotation[1]) / scale,
        }
    } else if m00 > m11 && m00 > m22 {
        let scale = ((1.0 + m00 - m11 - m22).sqrt()) * 2.0;
        Quaternion {
            w: (rotation[7] - rotation[5]) / scale,
            x: 0.25 * scale,
            y: (rotation[1] + rotation[3]) / scale,
            z: (rotation[2] + rotation[6]) / scale,
        }
    } else if m11 > m22 {
        let scale = ((1.0 + m11 - m00 - m22).sqrt()) * 2.0;
        Quaternion {
            w: (rotation[2] - rotation[6]) / scale,
            x: (rotation[1] + rotation[3]) / scale,
            y: 0.25 * scale,
            z: (rotation[5] + rotation[7]) / scale,
        }
    } else {
        let scale = ((1.0 + m22 - m00 - m11).sqrt()) * 2.0;
        Quaternion {
            w: (rotation[3] - rotation[1]) / scale,
            x: (rotation[2] + rotation[6]) / scale,
            y: (rotation[5] + rotation[7]) / scale,
            z: 0.25 * scale,
        }
    };
    normalize_quaternion(quaternion)
}

/// Which slot a frame occupies in the model's context window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextRole {
    Anchor,
    RecentKeyframe,
    Recent,
}

/// Retention rules for a rolling frame context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextPolicy {
    pub anchor_frames: usize,
    pub recent_full_frames: usize,
    pub keyframe_interval: u64,
    pub trajectory_tokens_per_frame: u32,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            anchor_frames: 8,
            recent_full_frames: 64,
            keyframe_interval: 5,
            trajectory_tokens_per_frame: 6,
        }
    }
}

impl ContextPolicy {
    fn is_valid(self) -> bool {
        self.anchor_frames > 0
            && self.recent_full_frames > 0
            && self.keyframe_interval > 0
            && self.trajectory_tokens_per_frame > 0
    }
}

/// What the context decided about one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextDecision {
    pub frame_seq: u64,
    pub role: ContextRole,
    pub anchor_frames: u32,
    pub recent_full_frames: u32,
    pub trajectory_frames: u64,
    pub trajectory_tokens: u64,
    pub compressed_frame_seq: Option<u64>,
}

/// Rolling context window driven by an increasing frame sequence.
pub struct StreamContext {
    policy: ContextPolicy,
    anchors: Vec<u64>,
    recent: VecDeque<u64>,
    trajectory_frames: u64,
    last_frame_seq: u64,
}

impl StreamContext {
    /// Returns `None` for a policy with a zero-valued field.
    pub fn new(policy: ContextPolicy) -> Option<Self> {
        policy.is_valid().then(|| Self {
            policy,
            anchors: Vec::with_capacity(policy.anchor_frames),
            recent: VecDeque::with_capacity(policy.recent_full_frames),
            trajectory_frames: 0,
            last_frame_seq: 0,
        })
    }

    /// Admits one frame. Sequences must advance; a repeat or a zero is
    /// rejected with `None`.
    pub fn observe(&mut self, frame_seq: u64) -> Option<ContextDecision> {
        if frame_seq == 0 || frame_seq <= self.last_frame_seq {
            return None;
        }
        self.last_frame_seq = frame_seq;

        let mut compressed_frame_seq = None;
        let role = if self.anchors.len() < self.policy.anchor_frames {
            self.anchors.push(frame_seq);
            ContextRole::Anchor
        } else {
            let index_since_anchor = self.recent.len() as u64 + self.trajectory_frames;
            self.recent.push_back(frame_seq);
            if self.recent.len() > self.policy.recent_full_frames {
                compressed_frame_seq = self.recent.pop_front();
                self.trajectory_frames += 1;
            }
            if index_since_anchor % self.policy.keyframe_interval == 0 {
                ContextRole::RecentKeyframe
            } else {
                ContextRole::Recent
            }
        };

        Some(ContextDecision {
            frame_seq,
            role,
            anchor_frames: self.anchors.len() as u32,
            recent_full_frames: self.recent.len() as u32,
            trajectory_frames: self.trajectory_frames,
            trajectory_tokens: self
                .trajectory_frames
                .saturating_mul(u64::from(self.policy.trajectory_tokens_per_frame)),
            compressed_frame_seq,
        })
    }

    /// Drops every frame and restarts the sequence.
    pub fn reset(&mut self) {
        self.anchors.clear();
        self.recent.clear();
        self.trajectory_frames = 0;
        self.last_frame_seq = 0;
    }
}
