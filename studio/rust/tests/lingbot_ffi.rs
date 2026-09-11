//! Behavioural tests for the C ABI the native Qualia Studio application links
//! against. Everything here goes through the crate's public surface, so it
//! pins the contract rather than the implementation.

use qualia_studio_rust::{
    qualia_studio_lingbot_abi_version, qualia_studio_lingbot_context_create,
    qualia_studio_lingbot_context_destroy, qualia_studio_lingbot_context_observe,
    qualia_studio_lingbot_context_reset, qualia_studio_lingbot_decode_pose,
    qualia_studio_lingbot_encode_pose, qualia_studio_lingbot_inverse_se3,
    qualia_studio_lingbot_normalize_quaternion, qualia_studio_lingbot_unproject_depth,
    qualia_studio_lingbot_voxel_downsample, FfiContextDecision, FfiContextPolicy,
    FfiExtrinsic3x4, FfiIntrinsics, FfiPoint, FfiPoseEncoding9, FfiQuaternion, FfiTransform4x4,
    LINGBOT_ABI_VERSION,
};
use std::mem::{align_of, size_of};
use std::ptr;

fn assert_near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.0e-4,
        "expected {expected}, got {actual}"
    );
}

fn identity_rotation_extrinsic(translation: [f32; 3]) -> FfiExtrinsic3x4 {
    FfiExtrinsic3x4 {
        values: [
            1.0,
            0.0,
            0.0,
            translation[0],
            0.0,
            1.0,
            0.0,
            translation[1],
            0.0,
            0.0,
            1.0,
            translation[2],
        ],
    }
}

const PIXEL_INTRINSICS: FfiIntrinsics = FfiIntrinsics {
    fx: 400.0,
    fy: 420.0,
    cx: 320.0,
    cy: 240.0,
    width: 640,
    height: 480,
};

#[test]
fn exported_symbols_carry_their_c_names() {
    // The public constant is the version the symbol must report.
    assert_eq!(LINGBOT_ABI_VERSION, 1);
    // Re-declaring the symbols with their C spelling makes the linker resolve
    // them by name; a renamed or mangled export fails to link.
    extern "C" {
        fn qualia_studio_lingbot_abi_version() -> u32;
        fn qualia_studio_lingbot_context_reset(context: *mut std::ffi::c_void) -> bool;
        fn qualia_studio_lingbot_voxel_downsample(
            input: *const FfiPoint,
            input_len: usize,
            voxel_size: f32,
            output: *mut FfiPoint,
            output_capacity: usize,
        ) -> usize;
    }
    unsafe {
        assert_eq!(qualia_studio_lingbot_abi_version(), LINGBOT_ABI_VERSION);
        assert!(!qualia_studio_lingbot_context_reset(ptr::null_mut()));
        assert_eq!(
            qualia_studio_lingbot_voxel_downsample(ptr::null(), 0, 0.1, ptr::null_mut(), 0),
            0
        );
    }
}

#[test]
fn ffi_value_types_keep_their_layout() {
    assert_eq!((size_of::<FfiQuaternion>(), align_of::<FfiQuaternion>()), (16, 4));
    assert_eq!(
        (size_of::<FfiPoseEncoding9>(), align_of::<FfiPoseEncoding9>()),
        (36, 4)
    );
    assert_eq!((size_of::<FfiIntrinsics>(), align_of::<FfiIntrinsics>()), (24, 4));
    assert_eq!(
        (size_of::<FfiExtrinsic3x4>(), align_of::<FfiExtrinsic3x4>()),
        (48, 4)
    );
    assert_eq!(
        (size_of::<FfiTransform4x4>(), align_of::<FfiTransform4x4>()),
        (64, 4)
    );
    assert_eq!((size_of::<FfiPoint>(), align_of::<FfiPoint>()), (20, 4));
    assert_eq!(
        (size_of::<FfiContextPolicy>(), align_of::<FfiContextPolicy>()),
        (32, 8)
    );
    assert_eq!(
        (size_of::<FfiContextDecision>(), align_of::<FfiContextDecision>()),
        (56, 8)
    );
}

#[test]
fn normalize_quaternion_scales_to_unit_length_and_handles_null() {
    let mut output = FfiQuaternion::default();
    let scaled = FfiQuaternion {
        w: 2.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    assert!(unsafe { qualia_studio_lingbot_normalize_quaternion(&scaled, &mut output) });
    assert_eq!((output.w, output.x, output.y, output.z), (1.0, 0.0, 0.0, 0.0));

    // A degenerate quaternion falls back to the identity rotation.
    let zero = FfiQuaternion {
        w: 0.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    assert!(unsafe { qualia_studio_lingbot_normalize_quaternion(&zero, &mut output) });
    assert_eq!(output.w, 1.0);

    assert!(!unsafe {
        qualia_studio_lingbot_normalize_quaternion(ptr::null(), &mut output)
    });
    assert!(!unsafe {
        qualia_studio_lingbot_normalize_quaternion(&scaled, ptr::null_mut())
    });
}

#[test]
fn pose_encoding_round_trips_through_the_intrinsics_contract() {
    let extrinsic = identity_rotation_extrinsic([1.25, -0.5, 2.0]);
    let mut encoding = FfiPoseEncoding9::default();
    assert!(unsafe {
        qualia_studio_lingbot_encode_pose(&extrinsic, &PIXEL_INTRINSICS, &mut encoding)
    });
    assert_eq!(encoding.tx, 1.25);
    assert_eq!(encoding.ty, -0.5);
    assert_eq!(encoding.tz, 2.0);
    assert_near(encoding.qw, 1.0);
    assert_near(encoding.fov_w, 2.0 * (320.0f32).atan2(400.0));
    assert_near(encoding.fov_h, 2.0 * (240.0f32).atan2(420.0));

    let mut decoded_extrinsic = FfiExtrinsic3x4::default();
    let mut decoded_intrinsics = FfiIntrinsics::default();
    assert!(unsafe {
        qualia_studio_lingbot_decode_pose(
            &encoding,
            480,
            640,
            &mut decoded_extrinsic,
            &mut decoded_intrinsics,
        )
    });
    for (actual, expected) in decoded_extrinsic.values.iter().zip(extrinsic.values) {
        assert_near(*actual, expected);
    }
    assert_near(decoded_intrinsics.fx, PIXEL_INTRINSICS.fx);
    assert_near(decoded_intrinsics.fy, PIXEL_INTRINSICS.fy);
    assert_eq!(decoded_intrinsics.cx, 320.0);
    assert_eq!(decoded_intrinsics.cy, 240.0);
    assert_eq!(decoded_intrinsics.width, 640);
    assert_eq!(decoded_intrinsics.height, 480);
}

#[test]
fn pose_functions_reject_null_and_degenerate_input() {
    let extrinsic = identity_rotation_extrinsic([0.0, 0.0, 0.0]);
    let mut encoding = FfiPoseEncoding9::default();
    assert!(!unsafe {
        qualia_studio_lingbot_encode_pose(ptr::null(), &PIXEL_INTRINSICS, &mut encoding)
    });
    assert!(!unsafe {
        qualia_studio_lingbot_encode_pose(&extrinsic, ptr::null(), &mut encoding)
    });
    assert!(!unsafe {
        qualia_studio_lingbot_encode_pose(&extrinsic, &PIXEL_INTRINSICS, ptr::null_mut())
    });

    assert!(unsafe {
        qualia_studio_lingbot_encode_pose(&extrinsic, &PIXEL_INTRINSICS, &mut encoding)
    });
    let mut decoded_extrinsic = FfiExtrinsic3x4::default();
    let mut decoded_intrinsics = FfiIntrinsics::default();
    for (height, width) in [(0, 640), (480, 0), (-1, 640), (480, -3)] {
        assert!(!unsafe {
            qualia_studio_lingbot_decode_pose(
                &encoding,
                height,
                width,
                &mut decoded_extrinsic,
                &mut decoded_intrinsics,
            )
        });
    }
    assert!(!unsafe {
        qualia_studio_lingbot_decode_pose(
            ptr::null(),
            480,
            640,
            &mut decoded_extrinsic,
            &mut decoded_intrinsics,
        )
    });
}

#[test]
fn inverse_se3_transposes_rotation_and_negates_translation() {
    let extrinsic = identity_rotation_extrinsic([1.0, 2.0, 3.0]);
    let mut output = FfiTransform4x4::default();
    assert!(unsafe { qualia_studio_lingbot_inverse_se3(&extrinsic, &mut output) });
    assert_eq!(
        output.values,
        [
            1.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0, -2.0, 0.0, 0.0, 1.0, -3.0, 0.0, 0.0, 0.0, 1.0,
        ]
    );
    assert!(!unsafe { qualia_studio_lingbot_inverse_se3(ptr::null(), &mut output) });
    assert!(!unsafe { qualia_studio_lingbot_inverse_se3(&extrinsic, ptr::null_mut()) });
}

#[test]
fn unproject_depth_lifts_pixels_into_the_world_frame() {
    let depth = [2.0f32];
    let confidence = [0.75f32];
    let rgb = [10u8, 20, 30];
    let extrinsic = identity_rotation_extrinsic([-1.0, -2.0, -3.0]);
    let intrinsics = FfiIntrinsics {
        fx: 1.0,
        fy: 1.0,
        cx: 0.0,
        cy: 0.0,
        width: 1,
        height: 1,
    };
    let mut points = [FfiPoint::default(); 1];
    let written = unsafe {
        qualia_studio_lingbot_unproject_depth(
            1,
            1,
            depth.as_ptr(),
            confidence.as_ptr(),
            rgb.as_ptr(),
            &extrinsic,
            &intrinsics,
            0.0,
            points.as_mut_ptr(),
            points.len(),
        )
    };
    assert_eq!(written, 1);
    assert_near(points[0].x, 1.0);
    assert_near(points[0].y, 2.0);
    assert_near(points[0].z, 5.0);
    assert_near(points[0].confidence, 0.75);
    assert_eq!((points[0].r, points[0].g, points[0].b), (10, 20, 30));
}

#[test]
fn unproject_depth_reports_the_point_count_when_no_buffer_is_given() {
    let depth = [1.0f32, 2.0, 3.0, 4.0];
    let no_confidence = ptr::null();
    let no_rgb = ptr::null();
    let extrinsic = identity_rotation_extrinsic([0.0, 0.0, 0.0]);
    let intrinsics = FfiIntrinsics {
        fx: 1.0,
        fy: 1.0,
        cx: 0.0,
        cy: 0.0,
        width: 2,
        height: 2,
    };
    let count = unsafe {
        qualia_studio_lingbot_unproject_depth(
            2,
            2,
            depth.as_ptr(),
            no_confidence,
            no_rgb,
            &extrinsic,
            &intrinsics,
            0.5,
            ptr::null_mut(),
            0,
        )
    };
    assert_eq!(count, 4);
    // A zero-capacity buffer still reports the true count.
    let mut points = [FfiPoint::default(); 1];
    let count = unsafe {
        qualia_studio_lingbot_unproject_depth(
            2,
            2,
            depth.as_ptr(),
            no_confidence,
            no_rgb,
            &extrinsic,
            &intrinsics,
            0.5,
            points.as_mut_ptr(),
            0,
        )
    };
    assert_eq!(count, 4);
}

#[test]
fn unproject_depth_filters_by_min_depth_and_missing_frames() {
    let depth = [0.4f32, 0.6];
    let extrinsic = identity_rotation_extrinsic([0.0, 0.0, 0.0]);
    let intrinsics = FfiIntrinsics {
        fx: 1.0,
        fy: 1.0,
        cx: 0.0,
        cy: 0.0,
        width: 2,
        height: 1,
    };
    let mut points = [FfiPoint::default(); 2];
    let written = unsafe {
        qualia_studio_lingbot_unproject_depth(
            2,
            1,
            depth.as_ptr(),
            ptr::null(),
            ptr::null(),
            &extrinsic,
            &intrinsics,
            0.5,
            points.as_mut_ptr(),
            points.len(),
        )
    };
    assert_eq!(written, 1);
    assert_near(points[0].z, 0.6);
    // No RGB frame yields an uncoloured point.
    assert_eq!((points[0].r, points[0].g, points[0].b), (0, 0, 0));

    assert_eq!(
        unsafe {
            qualia_studio_lingbot_unproject_depth(
                2,
                1,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                &extrinsic,
                &intrinsics,
                0.5,
                points.as_mut_ptr(),
                points.len(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_unproject_depth(
                0,
                1,
                depth.as_ptr(),
                ptr::null(),
                ptr::null(),
                &extrinsic,
                &intrinsics,
                0.5,
                points.as_mut_ptr(),
                points.len(),
            )
        },
        0
    );
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_unproject_depth(
                2,
                1,
                depth.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                &intrinsics,
                0.5,
                points.as_mut_ptr(),
                points.len(),
            )
        },
        0
    );
}

#[test]
fn voxel_downsample_averages_within_a_voxel_and_is_deterministic() {
    let points = [
        FfiPoint {
            x: 0.01,
            y: 0.01,
            z: 0.01,
            confidence: 0.5,
            r: 10,
            g: 20,
            b: 30,
            _pad: 0,
        },
        FfiPoint {
            x: 0.02,
            y: 0.02,
            z: 0.02,
            confidence: 1.0,
            r: 30,
            g: 40,
            b: 50,
            _pad: 0,
        },
        FfiPoint {
            x: 1.0,
            y: 0.0,
            z: 0.0,
            confidence: 0.25,
            r: 1,
            g: 2,
            b: 3,
            _pad: 0,
        },
    ];
    let mut output = [FfiPoint::default(); 4];
    let written = unsafe {
        qualia_studio_lingbot_voxel_downsample(
            points.as_ptr(),
            points.len(),
            0.1,
            output.as_mut_ptr(),
            output.len(),
        )
    };
    assert_eq!(written, 2);
    for value in [output[0].x, output[0].y, output[0].z] {
        assert_near(value, 0.015);
    }
    assert_near(output[0].confidence, 0.75);
    assert_eq!((output[0].r, output[0].g, output[0].b), (20, 30, 40));
    assert_eq!(output[1].x, 1.0);

    // Same input, same order, same output.
    let mut again = [FfiPoint::default(); 4];
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_voxel_downsample(
                points.as_ptr(),
                points.len(),
                0.1,
                again.as_mut_ptr(),
                again.len(),
            )
        },
        2
    );
    for (left, right) in output.iter().zip(again.iter()) {
        assert_eq!((left.x, left.y, left.z), (right.x, right.y, right.z));
        assert_eq!((left.r, left.g, left.b), (right.r, right.g, right.b));
    }
}

#[test]
fn voxel_downsample_handles_empty_and_undersized_buffers() {
    let points = [
        FfiPoint {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            confidence: 1.0,
            r: 0,
            g: 0,
            b: 0,
            _pad: 0,
        },
        FfiPoint {
            x: 5.0,
            y: 0.0,
            z: 0.0,
            confidence: 1.0,
            r: 0,
            g: 0,
            b: 0,
            _pad: 0,
        },
    ];
    // A non-positive voxel size leaves the cloud untouched.
    let mut untouched = [FfiPoint::default(); 2];
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_voxel_downsample(
                points.as_ptr(),
                points.len(),
                0.0,
                untouched.as_mut_ptr(),
                untouched.len(),
            )
        },
        2
    );
    assert_eq!(untouched[1].x, 5.0);

    // Null input with a non-zero length is rejected outright.
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_voxel_downsample(
                ptr::null(),
                3,
                0.1,
                untouched.as_mut_ptr(),
                untouched.len(),
            )
        },
        0
    );
    // Null input with zero length is an empty cloud.
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_voxel_downsample(ptr::null(), 0, 0.1, ptr::null_mut(), 0)
        },
        0
    );
    // A buffer smaller than the cloud is filled to capacity and reports how
    // many points it wrote.
    let mut one = [FfiPoint::default(); 1];
    assert_eq!(
        unsafe {
            qualia_studio_lingbot_voxel_downsample(
                points.as_ptr(),
                points.len(),
                0.1,
                one.as_mut_ptr(),
                one.len(),
            )
        },
        1
    );
    assert_eq!(one[0].x, 0.0);
}

#[test]
fn stream_context_assigns_roles_and_compresses_evicted_frames() {
    let policy = FfiContextPolicy {
        anchor_frames: 2,
        recent_full_frames: 2,
        keyframe_interval: 2,
        trajectory_tokens_per_frame: 6,
        _pad: 0,
    };
    let context = unsafe { qualia_studio_lingbot_context_create(&policy) };
    assert!(!context.is_null());

    let mut decision = FfiContextDecision::default();
    assert!(unsafe { qualia_studio_lingbot_context_observe(context, 1, &mut decision) });
    assert_eq!(decision.role, 0);
    assert_eq!(decision.anchor_frames, 1);
    assert_eq!(decision.has_compressed_frame, 0);

    assert!(unsafe { qualia_studio_lingbot_context_observe(context, 2, &mut decision) });
    assert_eq!(decision.role, 0);
    assert_eq!(decision.anchor_frames, 2);

    assert!(unsafe { qualia_studio_lingbot_context_observe(context, 3, &mut decision) });
    assert_eq!(decision.role, 1);
    assert_eq!(decision.recent_full_frames, 1);

    assert!(unsafe { qualia_studio_lingbot_context_observe(context, 4, &mut decision) });
    assert_eq!(decision.role, 2);

    assert!(unsafe { qualia_studio_lingbot_context_observe(context, 5, &mut decision) });
    assert_eq!(decision.role, 1);
    assert_eq!(decision.frame_seq, 5);
    assert_eq!(decision.compressed_frame_seq, 3);
    assert_eq!(decision.has_compressed_frame, 1);
    assert_eq!(decision.trajectory_frames, 1);
    assert_eq!(decision.trajectory_tokens, 6);

    // Frames must advance strictly.
    assert!(!unsafe { qualia_studio_lingbot_context_observe(context, 5, &mut decision) });
    assert!(!unsafe { qualia_studio_lingbot_context_observe(context, 0, &mut decision) });

    assert!(unsafe { qualia_studio_lingbot_context_reset(context) });
    assert!(unsafe { qualia_studio_lingbot_context_observe(context, 5, &mut decision) });
    assert_eq!(decision.role, 0);

    unsafe { qualia_studio_lingbot_context_destroy(context) };
}

#[test]
fn stream_context_rejects_null_handles_and_bad_policies() {
    assert!(unsafe { qualia_studio_lingbot_context_create(ptr::null()).is_null() });

    let invalid = [
        FfiContextPolicy {
            anchor_frames: 0,
            ..FfiContextPolicy::default()
        },
        FfiContextPolicy {
            recent_full_frames: 0,
            ..FfiContextPolicy::default()
        },
        FfiContextPolicy {
            keyframe_interval: 0,
            ..FfiContextPolicy::default()
        },
        FfiContextPolicy {
            trajectory_tokens_per_frame: 0,
            ..FfiContextPolicy::default()
        },
    ];
    for policy in invalid {
        assert!(unsafe { qualia_studio_lingbot_context_create(&policy).is_null() });
    }

    let mut decision = FfiContextDecision::default();
    assert!(!unsafe { qualia_studio_lingbot_context_observe(ptr::null_mut(), 1, &mut decision) });
    assert!(!unsafe { qualia_studio_lingbot_context_observe(ptr::null_mut(), 1, ptr::null_mut()) });
    assert!(!unsafe { qualia_studio_lingbot_context_reset(ptr::null_mut()) });

    let minimal = FfiContextPolicy {
        anchor_frames: 1,
        recent_full_frames: 1,
        keyframe_interval: 1,
        trajectory_tokens_per_frame: 1,
        _pad: 0,
    };
    let context = unsafe { qualia_studio_lingbot_context_create(&minimal) };
    assert!(!context.is_null());
    assert!(!unsafe { qualia_studio_lingbot_context_observe(context, 1, ptr::null_mut()) });
    unsafe { qualia_studio_lingbot_context_destroy(context) };

    // Destroying a null handle is a no-op.
    unsafe { qualia_studio_lingbot_context_destroy(ptr::null_mut()) };
}
