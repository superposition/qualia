//! Qualia Studio's Rust half.
//!
//! The native application links this crate as a static library and calls the
//! `qualia_studio_lingbot_*` symbols below. Every `#[repr(C)]` type here is
//! part of that linkage contract: field order, size and alignment are frozen,
//! and the `#[no_mangle] extern "C"` symbols keep their exact spelling. The
//! arithmetic behind them lives in the private [`lingbot`] module.

mod lingbot;

use lingbot::{
    CameraIntrinsics, ContextPolicy, ContextRole, DepthFrame, Extrinsic3x4, PoseEncoding9,
    Quaternion, ReconstructionPoint, StreamContext,
};
use std::ffi::c_void;

/// Version of the exported ABI. Bumped whenever a symbol or layout changes.
pub const LINGBOT_ABI_VERSION: u32 = 1;

/// Quaternion crossing the FFI boundary.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiQuaternion {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Pose encoding as nine contiguous values, translation first.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiPoseEncoding9 {
    pub tx: f32,
    pub ty: f32,
    pub tz: f32,
    pub qw: f32,
    pub qx: f32,
    pub qy: f32,
    pub qz: f32,
    pub fov_h: f32,
    pub fov_w: f32,
}

/// Pinhole camera parameters crossing the FFI boundary.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiIntrinsics {
    pub fx: f32,
    pub fy: f32,
    pub cx: f32,
    pub cy: f32,
    pub width: i32,
    pub height: i32,
}

/// Row-major 3x4 rigid transform, translation in lanes 3, 7 and 11.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiExtrinsic3x4 {
    pub values: [f32; 12],
}

/// Column-major 4x4 transform.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiTransform4x4 {
    pub values: [f32; 16],
}

/// One reconstructed point with its colour and confidence.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub confidence: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub _pad: u8,
}

/// Context retention rules as the native side declares them.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiContextPolicy {
    pub anchor_frames: usize,
    pub recent_full_frames: usize,
    pub keyframe_interval: u64,
    pub trajectory_tokens_per_frame: u32,
    pub _pad: u32,
}

/// One context decision, flattened for the native side.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FfiContextDecision {
    pub frame_seq: u64,
    pub role: u32,
    pub anchor_frames: u32,
    pub recent_full_frames: u32,
    pub _pad: u32,
    pub trajectory_frames: u64,
    pub trajectory_tokens: u64,
    pub compressed_frame_seq: u64,
    pub has_compressed_frame: u8,
    pub _reserved: [u8; 7],
}

#[no_mangle]
pub extern "C" fn qualia_studio_lingbot_abi_version() -> u32 {
    LINGBOT_ABI_VERSION
}

/// # Safety
///
/// `input` must be readable and `output` writable for one [`FfiQuaternion`].
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_normalize_quaternion(
    input: *const FfiQuaternion,
    output: *mut FfiQuaternion,
) -> bool {
    let (Some(input), Some(output)) = (input.as_ref(), output.as_mut()) else {
        return false;
    };
    *output = quaternion_out(lingbot::normalize_quaternion(quaternion_in(*input)));
    true
}

/// # Safety
///
/// All three pointers must be readable or writable for one value of their
/// respective types.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_encode_pose(
    extrinsic: *const FfiExtrinsic3x4,
    intrinsics: *const FfiIntrinsics,
    output: *mut FfiPoseEncoding9,
) -> bool {
    let (Some(extrinsic), Some(intrinsics), Some(output)) =
        (extrinsic.as_ref(), intrinsics.as_ref(), output.as_mut())
    else {
        return false;
    };
    *output = pose_out(lingbot::encode_pose(
        Extrinsic3x4(extrinsic.values),
        intrinsics_in(*intrinsics),
    ));
    true
}

/// # Safety
///
/// `encoding` must be readable and both outputs writable. The image dimensions
/// must be positive.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_decode_pose(
    encoding: *const FfiPoseEncoding9,
    image_height: i32,
    image_width: i32,
    extrinsic_output: *mut FfiExtrinsic3x4,
    intrinsics_output: *mut FfiIntrinsics,
) -> bool {
    if image_height <= 0 || image_width <= 0 {
        return false;
    }
    let (Some(encoding), Some(extrinsic_output), Some(intrinsics_output)) = (
        encoding.as_ref(),
        extrinsic_output.as_mut(),
        intrinsics_output.as_mut(),
    ) else {
        return false;
    };
    let (extrinsic, intrinsics) =
        lingbot::decode_pose(pose_in(*encoding), image_height, image_width);
    extrinsic_output.values = extrinsic.0;
    *intrinsics_output = intrinsics_out(intrinsics);
    true
}

/// # Safety
///
/// `extrinsic` must be readable and `output` writable for one value each.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_inverse_se3(
    extrinsic: *const FfiExtrinsic3x4,
    output: *mut FfiTransform4x4,
) -> bool {
    let (Some(extrinsic), Some(output)) = (extrinsic.as_ref(), output.as_mut()) else {
        return false;
    };
    output.values = lingbot::inverse_se3(Extrinsic3x4(extrinsic.values)).0;
    true
}

/// # Safety
///
/// `depth`, and `confidence`/`rgb` when non-null, must cover `width * height`
/// (depth, confidence) and `width * height * 3` (rgb) elements. A non-null
/// `output` must be writable for `output_capacity` points.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_unproject_depth(
    width: i32,
    height: i32,
    depth: *const f32,
    confidence: *const f32,
    rgb: *const u8,
    extrinsic: *const FfiExtrinsic3x4,
    intrinsics: *const FfiIntrinsics,
    min_depth: f32,
    output: *mut FfiPoint,
    output_capacity: usize,
) -> usize {
    if width <= 0 || height <= 0 || depth.is_null() {
        return 0;
    }
    let (Some(extrinsic), Some(intrinsics)) = (extrinsic.as_ref(), intrinsics.as_ref()) else {
        return 0;
    };
    let Some(pixel_count) = (width as usize).checked_mul(height as usize) else {
        return 0;
    };
    let depth = std::slice::from_raw_parts(depth, pixel_count);
    // The optional planes are validated against the declared frame size, not
    // trusted to match it.
    let confidence = (!confidence.is_null()).then(|| std::slice::from_raw_parts(confidence, pixel_count));
    let Some(rgb_len) = pixel_count.checked_mul(3) else {
        return 0;
    };
    let rgb = (!rgb.is_null()).then(|| std::slice::from_raw_parts(rgb, rgb_len));
    let points = lingbot::unproject_depth(
        DepthFrame {
            width: width as usize,
            height: height as usize,
            depth,
            confidence,
            rgb,
        },
        Extrinsic3x4(extrinsic.values),
        intrinsics_in(*intrinsics),
        min_depth,
    );
    copy_points(&points, output, output_capacity)
}

/// # Safety
///
/// `input` must cover `input_len` points (null only when `input_len` is zero),
/// and a non-null `output` must be writable for `output_capacity` points.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_voxel_downsample(
    input: *const FfiPoint,
    input_len: usize,
    voxel_size: f32,
    output: *mut FfiPoint,
    output_capacity: usize,
) -> usize {
    if input.is_null() && input_len != 0 {
        return 0;
    }
    let input = if input_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(input, input_len)
    };
    let points: Vec<_> = input.iter().copied().map(point_in).collect();
    let downsampled = lingbot::voxel_downsample(&points, voxel_size);
    copy_points(&downsampled, output, output_capacity)
}

/// # Safety
///
/// `policy` must be readable for one [`FfiContextPolicy`], or null to fail.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_context_create(
    policy: *const FfiContextPolicy,
) -> *mut c_void {
    let Some(policy) = policy.as_ref() else {
        return std::ptr::null_mut();
    };
    let Some(context) = StreamContext::new(ContextPolicy {
        anchor_frames: policy.anchor_frames,
        recent_full_frames: policy.recent_full_frames,
        keyframe_interval: policy.keyframe_interval,
        trajectory_tokens_per_frame: policy.trajectory_tokens_per_frame,
    }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(context)).cast::<c_void>()
}

/// # Safety
///
/// `context` must be null or a handle from [`qualia_studio_lingbot_context_create`]
/// that has not been destroyed yet.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_context_destroy(context: *mut c_void) {
    if !context.is_null() {
        drop(Box::from_raw(context.cast::<StreamContext>()));
    }
}

/// # Safety
///
/// `context` must be a live handle and `output` writable for one decision.
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_context_observe(
    context: *mut c_void,
    frame_seq: u64,
    output: *mut FfiContextDecision,
) -> bool {
    let (Some(context), Some(output)) = (context.cast::<StreamContext>().as_mut(), output.as_mut())
    else {
        return false;
    };
    let Some(decision) = context.observe(frame_seq) else {
        return false;
    };
    *output = FfiContextDecision {
        frame_seq: decision.frame_seq,
        role: match decision.role {
            ContextRole::Anchor => 0,
            ContextRole::RecentKeyframe => 1,
            ContextRole::Recent => 2,
        },
        anchor_frames: decision.anchor_frames,
        recent_full_frames: decision.recent_full_frames,
        _pad: 0,
        trajectory_frames: decision.trajectory_frames,
        trajectory_tokens: decision.trajectory_tokens,
        compressed_frame_seq: decision.compressed_frame_seq.unwrap_or_default(),
        has_compressed_frame: u8::from(decision.compressed_frame_seq.is_some()),
        _reserved: [0; 7],
    };
    true
}

/// # Safety
///
/// `context` must be a live handle from [`qualia_studio_lingbot_context_create`].
#[no_mangle]
pub unsafe extern "C" fn qualia_studio_lingbot_context_reset(context: *mut c_void) -> bool {
    let Some(context) = context.cast::<StreamContext>().as_mut() else {
        return false;
    };
    context.reset();
    true
}

/// Writes as many points as the buffer accepts and always reports how many the
/// caller would need. A null or zero-capacity buffer copies nothing.
///
/// # Safety
///
/// `output` must be null or writable for `output_capacity` points.
unsafe fn copy_points(
    points: &[ReconstructionPoint],
    output: *mut FfiPoint,
    output_capacity: usize,
) -> usize {
    if output.is_null() || output_capacity == 0 {
        return points.len();
    }
    let written = points.len().min(output_capacity);
    for (index, point) in points.iter().take(written).enumerate() {
        std::ptr::write(output.add(index), point_out(*point));
    }
    written
}

fn quaternion_in(value: FfiQuaternion) -> Quaternion {
    Quaternion {
        w: value.w,
        x: value.x,
        y: value.y,
        z: value.z,
    }
}

fn quaternion_out(value: Quaternion) -> FfiQuaternion {
    FfiQuaternion {
        w: value.w,
        x: value.x,
        y: value.y,
        z: value.z,
    }
}

fn pose_in(value: FfiPoseEncoding9) -> PoseEncoding9 {
    PoseEncoding9 {
        translation: [value.tx, value.ty, value.tz],
        rotation: Quaternion {
            w: value.qw,
            x: value.qx,
            y: value.qy,
            z: value.qz,
        },
        fov_h: value.fov_h,
        fov_w: value.fov_w,
    }
}

fn pose_out(value: PoseEncoding9) -> FfiPoseEncoding9 {
    FfiPoseEncoding9 {
        tx: value.translation[0],
        ty: value.translation[1],
        tz: value.translation[2],
        qw: value.rotation.w,
        qx: value.rotation.x,
        qy: value.rotation.y,
        qz: value.rotation.z,
        fov_h: value.fov_h,
        fov_w: value.fov_w,
    }
}

fn intrinsics_in(value: FfiIntrinsics) -> CameraIntrinsics {
    CameraIntrinsics {
        fx: value.fx,
        fy: value.fy,
        cx: value.cx,
        cy: value.cy,
        width: value.width,
        height: value.height,
    }
}

fn intrinsics_out(value: CameraIntrinsics) -> FfiIntrinsics {
    FfiIntrinsics {
        fx: value.fx,
        fy: value.fy,
        cx: value.cx,
        cy: value.cy,
        width: value.width,
        height: value.height,
    }
}

fn point_in(value: FfiPoint) -> ReconstructionPoint {
    ReconstructionPoint {
        position: [value.x, value.y, value.z],
        confidence: value.confidence,
        color: [value.r, value.g, value.b],
    }
}

fn point_out(value: ReconstructionPoint) -> FfiPoint {
    FfiPoint {
        x: value.position[0],
        y: value.position[1],
        z: value.position[2],
        confidence: value.confidence,
        r: value.color[0],
        g: value.color[1],
        b: value.color[2],
        _pad: 0,
    }
}
