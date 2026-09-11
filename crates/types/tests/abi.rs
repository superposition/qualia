//! Behavioural tests for the shared-memory ABI.
//!
//! Layout assertions are hard numbers on purpose: a change to any of them is a
//! silent break for every mapped consumer, so a failing value here is the
//! point. Seqlock tests exercise the public reader/writer paths, and the
//! history test drives the ring past capacity to prove it fails closed on
//! evidence it has overwritten rather than returning a stale interval.

use qualia_types::*;
use std::alloc::{alloc_zeroed, Layout};
use std::mem::{align_of, size_of};
use std::sync::atomic::Ordering;

/// Allocates a zeroed, correctly aligned instance of an ABI struct.
///
/// These types have no constructors by design — they are mapped, not built —
/// so tests obtain references the same way a consumer does.
fn zeroed<T>() -> &'static mut T {
    let layout = Layout::from_size_align(size_of::<T>(), align_of::<T>()).expect("valid layout");
    // SAFETY: the layout is non-empty for every type under test, the
    // allocation is zeroed, and zero is a valid bit pattern for each field.
    let pointer = unsafe { alloc_zeroed(layout) } as *mut T;
    assert!(!pointer.is_null(), "allocation failed");
    unsafe { &mut *pointer }
}

#[test]
fn repr_c_sizes_and_alignments_are_pinned() {
    assert_eq!(size_of::<VoxelCell>(), 4);
    assert_eq!(align_of::<VoxelCell>(), 1);

    assert_eq!(size_of::<WorldVoxels>(), 49_160);
    assert_eq!(align_of::<WorldVoxels>(), 8);

    assert_eq!(size_of::<LidarPoint>(), 12);
    assert_eq!(size_of::<LidarScan>(), 8_672);
    assert_eq!(align_of::<LidarScan>(), 8);
    assert_eq!(size_of::<LidarOccupancyGrid>(), 65_568);
    assert_eq!(align_of::<LidarOccupancyGrid>(), 8);

    assert_eq!(size_of::<PersistentMapGrid>(), 131_128);
    assert_eq!(size_of::<BinaryMapGrid>(), 65_592);

    assert_eq!(size_of::<CameraFrame>(), 3_120);
    assert_eq!(size_of::<CameraPreview>(), 524_328);
    assert_eq!(size_of::<VslamFrontendState>(), 88);
    assert_eq!(size_of::<CameraFloorGrid>(), 1_056);

    assert_eq!(size_of::<NavPose>(), 40);
    assert_eq!(size_of::<NavGoal>(), 40);
    assert_eq!(size_of::<WorldObject>(), 48);
    assert_eq!(size_of::<WorldModel>(), 5_920);

    assert_eq!(size_of::<ThoughtEntry>(), 280);
    assert_eq!(size_of::<ThoughtBuffer>(), 143_368);
    assert_eq!(size_of::<QuestionSlot>(), 280);
    assert_eq!(size_of::<LoreEntry>(), 800);
    assert_eq!(size_of::<LoreBuffer>(), 102_408);

    assert_eq!(size_of::<BeliefSlot>(), 16_448);
    assert_eq!(align_of::<BeliefSlot>(), 64);
    assert_eq!(size_of::<LayerSlot>(), 4_231_616);
    assert_eq!(size_of::<LedgerEntry>(), 4_128);

    assert_eq!(size_of::<JepaEvidencePayload>(), 23_808);
    assert_eq!(align_of::<JepaEvidencePayload>(), 64);
    assert_eq!(size_of::<JepaTelemetryPayload>(), 512);
    assert_eq!(align_of::<JepaTelemetryPayload>(), 64);
    assert_eq!(size_of::<JepaEvidenceSlot>(), 23_872);
    assert_eq!(align_of::<JepaEvidenceSlot>(), 64);
    assert_eq!(size_of::<JepaTelemetrySlot>(), 576);
    assert_eq!(align_of::<JepaTelemetrySlot>(), 64);

    assert_eq!(size_of::<ShmHeader>(), 80);
    assert_eq!(size_of::<HealthReport>(), 32);

    assert_eq!(size_of::<AppliedActionSlot>(), 128);
    assert_eq!(align_of::<AppliedActionSlot>(), 64);
    assert_eq!(size_of::<AppliedActionHistory>(), 557_120);
    assert_eq!(align_of::<AppliedActionHistory>(), 64);
}

#[test]
fn lidar_scan_round_trips_and_rejects_an_odd_sequence() {
    let scan = zeroed::<LidarScan>();
    let mut snapshot = LidarScanSnapshot::default();
    snapshot.scan_start_ns = 10;
    snapshot.scan_end_ns = 20;
    snapshot.point_count = 3;
    snapshot.points[0] = LidarPoint {
        angle_rad: 0.5,
        distance_m: 1.25,
        intensity: 200,
        _pad: [0; 3],
    };

    assert_eq!(scan.publish(&snapshot).expect("publish"), 2);
    let read = scan.snapshot(8).expect("snapshot");
    assert_eq!(read.seq, 2);
    assert_eq!(read.point_count, 3);
    assert_eq!(read.points[0].distance_m, 1.25);

    // A writer that has begun but not finished leaves the sequence odd; a
    // reader must refuse rather than observe half a scan.
    scan.seq.store(1, Ordering::Release);
    assert!(matches!(
        scan.snapshot(4),
        Err(SnapshotError::TornRead)
    ));

    // The point count is clamped rather than trusted.
    scan.seq.store(2, Ordering::Release);
    snapshot.point_count = u32::MAX;
    assert_eq!(scan.publish(&snapshot).expect("publish"), 4);
    assert_eq!(scan.snapshot(4).expect("snapshot").point_count, 720);
}

#[test]
fn camera_frame_round_trips_and_reports_a_torn_read() {
    let frame = zeroed::<CameraFrame>();
    let mut snapshot = CameraFrameSnapshot::default();
    snapshot.timestamp_ns = 99;
    snapshot.source_width = 1280;
    snapshot.thumb_width = CAMERA_THUMB_W as u32;
    snapshot.thumbnail_luma[0] = 7;
    snapshot.valid = true;

    assert_eq!(frame.publish(&snapshot).expect("publish"), 2);
    let read = frame.snapshot(8).expect("snapshot");
    assert_eq!(read.timestamp_ns, 99);
    assert_eq!(read.source_width, 1280);
    assert_eq!(read.thumbnail_luma[0], 7);
    assert!(read.valid);

    frame.seq.store(3, Ordering::Release);
    assert!(matches!(
        frame.snapshot(2),
        Err(SnapshotError::TornRead)
    ));
}

#[test]
fn occupancy_grid_round_trips() {
    let grid = zeroed::<LidarOccupancyGrid>();
    let mut snapshot = LidarOccupancyGridSnapshot::default();
    snapshot.resolution_m = 0.05;
    snapshot.cells[0] = 100;
    snapshot.cells[LIDAR_GRID_CELLS - 1] = 200;
    assert_eq!(grid.publish(&snapshot).expect("publish"), 2);
    let read = grid.snapshot(4).expect("snapshot");
    assert_eq!(read.resolution_m, 0.05);
    assert_eq!(read.cells[LIDAR_GRID_CELLS - 1], 200);
}

#[test]
fn applied_action_slot_round_trips_every_field() {
    let slot = zeroed::<AppliedActionSlot>();
    let snapshot = AppliedActionSnapshot {
        slot_seq: 0,
        producer_epoch: 3,
        action_sequence: 44,
        interval_start_ns: 1_000,
        interval_end_ns: 2_000,
        requested_left: 0.4,
        requested_right: -0.4,
        clamped_left: 0.3,
        clamped_right: -0.3,
        applied_left: 0.25,
        applied_right: -0.25,
        speed_scale: 0.5,
        safety_flags: LEASH_ACTION_SAFETY_COLLISION_CLAMP,
        authority: ACTION_AUTHORITY_LEASH,
        valid: true,
        armed: true,
        deadman_active: false,
        collision_clamped: true,
    };
    assert_eq!(slot.publish(snapshot).expect("publish"), 2);
    let read = slot.snapshot(4).expect("snapshot");
    assert_eq!(read.slot_seq, 2);
    assert_eq!(read.producer_epoch, 3);
    assert_eq!(read.action_sequence, 44);
    assert_eq!(read.applied_left, 0.25);
    assert_eq!(read.authority, ACTION_AUTHORITY_LEASH);
    assert!(read.valid && read.armed && read.collision_clamped);
    assert!(!read.deadman_active);
}

#[test]
fn applied_action_history_fails_closed_on_overwritten_entries() {
    let history = zeroed::<AppliedActionHistory>();
    assert_eq!(history.committed_seq(), 0);

    for sequence in 1..=(APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1) {
        let mut snapshot = AppliedActionSnapshot::default();
        snapshot.action_sequence = sequence;
        snapshot.applied_left = sequence as f32;
        assert_eq!(history.append(snapshot).expect("append"), sequence);
    }
    assert_eq!(
        history.committed_seq(),
        APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1
    );

    // The newest interval is readable and carries its absolute ring sequence.
    let newest = history
        .snapshot(APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1, 4)
        .expect("newest entry");
    assert_eq!(newest.slot_seq, APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1);
    assert_eq!(newest.action_sequence, APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1);
    assert_eq!(
        newest.applied_left,
        (APPLIED_ACTION_HISTORY_CAPACITY + 1) as f32
    );

    // Sequence 1 was overwritten by the last append: refuse, never return the
    // interval that now lives in that slot.
    assert_eq!(
        history.snapshot(1, 4).unwrap_err(),
        AppliedActionHistoryError::Overrun {
            requested: 1,
            oldest: 2,
        }
    );
    assert_eq!(
        history.snapshot(0, 4).unwrap_err(),
        AppliedActionHistoryError::NotCommitted {
            requested: 0,
            committed: APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1,
        }
    );
    assert_eq!(
        history.snapshot(u64::MAX, 4).unwrap_err(),
        AppliedActionHistoryError::NotCommitted {
            requested: u64::MAX,
            committed: APPLIED_ACTION_HISTORY_CAPACITY as u64 + 1,
        }
    );
}

#[test]
fn jepa_slots_keep_payload_and_telemetry_correlated() {
    let evidence = zeroed::<JepaEvidenceSlot>();
    let telemetry = zeroed::<JepaTelemetrySlot>();

    let mut payload = JepaEvidencePayload::default();
    payload.producer_epoch = 11;
    payload.runner_epoch = 22;
    payload.inference_seq = 33;
    payload.backend = JEPA_BACKEND_CUDA;
    payload.mode = JEPA_MODE_OBSERVE_ONLY;
    payload.flags = JEPA_FLAG_VALID | JEPA_FLAG_SOURCES_COHERENT;
    payload.latent[0] = 1.5;
    payload.observation_quality = 0.9;

    let mut counters = JepaTelemetryPayload::default();
    counters.producer_epoch = 11;
    counters.runner_epoch = 22;
    counters.inference_count = 33;
    counters.backend = JEPA_BACKEND_CUDA;
    counters.mode = JEPA_MODE_OBSERVE_ONLY;

    assert_eq!(evidence.publish(payload).expect("publish evidence"), 2);
    assert_eq!(telemetry.publish(counters).expect("publish telemetry"), 2);

    let read_payload = evidence.snapshot(4).expect("evidence snapshot");
    let read_counters = telemetry.snapshot(4).expect("telemetry snapshot");

    let evidence_key = (
        read_payload.producer_epoch,
        read_payload.runner_epoch,
        read_payload.inference_seq,
    );
    let telemetry_key = (
        read_counters.producer_epoch,
        read_counters.runner_epoch,
        read_counters.inference_count,
    );
    assert_eq!(evidence_key, (11, 22, 33));
    assert_eq!(telemetry_key, evidence_key);
    assert_eq!(read_payload.latent[0], 1.5);
    assert_eq!(read_payload.observation_quality, 0.9);
    assert_eq!(read_payload.flags, JEPA_FLAG_VALID | JEPA_FLAG_SOURCES_COHERENT);

    // Torn evidence must fail closed rather than pair with telemetry.
    evidence.seq.store(5, Ordering::Release);
    assert!(matches!(
        evidence.snapshot(2),
        Err(SnapshotError::TornRead)
    ));
}

#[test]
fn parse_stack_manifest_accepts_the_documented_shape() {
    let text = r#"{
      "schema_version": "qualia.stack.v1",
      "stack_name": "zero-motion",
      "shared_memory": { "name": "/qualia_body" },
      "control": { "socket": "127.0.0.1:7100" },
      "runners": [
        { "name": "qualia-agent" },
        { "name": "qualia-lidar", "stdout": "null", "env_passthrough": ["QUALIA_FLY_MODE"] }
      ]
    }"#;
    let manifest = parse_stack_manifest(text).expect("documented shape parses");
    assert_eq!(manifest.schema_version, "qualia.stack.v1");
    assert_eq!(manifest.stack_name, "zero-motion");
    assert_eq!(manifest.shared_memory.name, "/qualia_body");
    assert_eq!(manifest.control.socket, "127.0.0.1:7100");
    assert_eq!(manifest.runners.len(), 2);
    assert!(matches!(manifest.runners[0].stdout, RunnerStdout::Inherit));
    assert!(matches!(manifest.runners[1].stdout, RunnerStdout::Null));
    assert_eq!(
        manifest.runners[1].env_passthrough,
        vec!["QUALIA_FLY_MODE".to_string()]
    );
}

#[test]
fn parse_stack_manifest_rejects_malformed_and_incomplete_documents() {
    assert!(parse_stack_manifest("{ not json")
        .unwrap_err()
        .contains("failed to parse"));

    let no_runners = r#"{
      "schema_version": "qualia.stack.v1",
      "stack_name": "empty",
      "shared_memory": { "name": "/qualia_body" },
      "control": { "socket": "127.0.0.1:7100" },
      "runners": []
    }"#;
    assert!(parse_stack_manifest(no_runners)
        .unwrap_err()
        .contains("at least one runner"));

    let unnamed_runner = no_runners.replace("[]", r#"[{ "name": "  " }]"#);
    assert!(parse_stack_manifest(&unnamed_runner)
        .unwrap_err()
        .contains("runner.name cannot be empty"));

    let blank_socket = no_runners.replace("127.0.0.1:7100", "");
    assert!(parse_stack_manifest(&blank_socket)
        .unwrap_err()
        .contains("control.socket cannot be empty"));

    let missing_field = no_runners.replace(r#""control": { "socket": "127.0.0.1:7100" },"#, "");
    assert!(parse_stack_manifest(&missing_field)
        .unwrap_err()
        .contains("failed to parse"));
}
