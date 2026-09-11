//! Behavioural tests for the shared-memory arena.
//!
//! Each test creates its own uniquely named region and drops every handle it
//! holds, so the suite is deterministic and leaves neither a mapping nor a name
//! behind. Everything asserted here is observable through the public API: the
//! header written at creation, the byte offsets the accessors return, the
//! seqlock-free ring writes, and the fact that a second attach reads the bytes
//! the first handle left in place instead of a fresh zeroed region.

use std::sync::atomic::{AtomicU64, Ordering};

use qualia_shm::*;

/// Names are unique per test *and* per run: two threads of the same test binary
/// never share a region, and a region left over from a killed earlier process is
/// not reused because the process id and counter both move on.
static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region_name(tag: &str) -> String {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    format!("/qualia_shm_test_{}_{}_{}", std::process::id(), tag, index)
}

fn offset_in(region: &ShmRegion, address: *const u8) -> usize {
    address as usize - region.as_ptr() as usize
}

fn ledger_row(seq: u64, vfe: f32) -> LedgerEntry {
    LedgerEntry {
        seq,
        layer: 3,
        event: LedgerEvent::Confirm,
        compression: 1,
        _pad: 0,
        vfe,
        residual_norm: 0.5,
        belief_mean: [0.0; STATE_DIM],
        timestamp_ns: 7,
    }
}

#[test]
fn region_is_send_and_sync() {
    fn assert_shared<T: Send + Sync>() {}
    assert_shared::<ShmRegion>();
}

#[test]
fn create_open_detach_round_trip() {
    let name = region_name("roundtrip");

    let region = ShmRegion::create(&name).expect("create");
    assert_eq!(region.len(), SHM_SIZE);
    assert!(!region.is_empty());
    assert!(!region.as_ptr().is_null());

    let header = region.header();
    assert_eq!(header.magic, SHM_MAGIC);
    assert_eq!(header.version, SHM_VERSION);
    assert_eq!(header.num_layers, NUM_LAYERS as u32);
    assert_eq!(header.layer_slot_size, LAYER_SLOT_SIZE as u64);
    assert_eq!(header.ledger_offset, LEDGER_OFFSET as u64);
    assert_eq!(header.ledger_capacity, MAX_LEDGER_ENTRIES as u64);
    assert_eq!(header.total_size, SHM_SIZE as u64);
    assert_eq!(header.jepa_region_offset, JEPA_REGION_OFFSET as u64);
    assert_eq!(header.jepa_region_size, JEPA_REGION_SIZE as u64);
    assert_eq!(header.jepa_abi_version, JEPA_ABI_VERSION);
    assert_eq!(region.ledger_seq(), 0);

    {
        let attached = ShmRegion::open(&name).expect("attach");
        assert_eq!(attached.header().magic, SHM_MAGIC);
        assert_eq!(attached.header().version, SHM_VERSION);
        assert_eq!(attached.len(), SHM_SIZE);
    }

    drop(region);
    assert!(
        matches!(ShmRegion::open(&name), Err(ShmError::OsError(_))),
        "the name must disappear once the creator detaches"
    );
}

#[test]
fn offsets_place_the_header_slots_and_ledger_where_the_layout_says() {
    let name = region_name("offsets");
    let region = ShmRegion::create(&name).expect("create");

    assert_eq!(SHM_SIZE, 64 * 1024 * 1024);
    assert_eq!(LAYER_SLOTS_OFFSET, 4096);
    assert_eq!(LEDGER_SIZE, 16 * 1024 * 1024);
    assert_eq!(HEADER_SIZE, std::mem::size_of::<ShmHeader>());
    assert_eq!(LAYER_SLOT_SIZE, std::mem::size_of::<LayerSlot>());
    assert!(HEADER_SIZE <= LAYER_SLOTS_OFFSET);
    assert_eq!(
        LEDGER_OFFSET,
        LAYER_SLOTS_OFFSET + NUM_LAYERS * LAYER_SLOT_SIZE
    );

    for layer in 0..NUM_LAYERS {
        let expected = LAYER_SLOTS_OFFSET + layer * LAYER_SLOT_SIZE;
        let address = region.layer_slot(layer) as *const LayerSlot as *const u8;
        assert_eq!(offset_in(&region, address), expected, "layer {layer}");
        assert_eq!(expected % std::mem::align_of::<LayerSlot>(), 0);
    }
    assert_eq!(
        offset_in(
            &region,
            region.layer_slot(NUM_LAYERS - 1) as *const LayerSlot as *const u8
        ) + LAYER_SLOT_SIZE,
        LEDGER_OFFSET
    );

    let first = region.ledger_entry(0) as *const LedgerEntry as *const u8;
    assert_eq!(offset_in(&region, first), LEDGER_OFFSET);
    let entry_size = std::mem::size_of::<LedgerEntry>();
    let last = region.ledger_entry(MAX_LEDGER_ENTRIES - 1) as *const LedgerEntry as *const u8;
    assert_eq!(
        offset_in(&region, last),
        LEDGER_OFFSET + (MAX_LEDGER_ENTRIES - 1) * entry_size
    );
    assert!(LEDGER_OFFSET + MAX_LEDGER_ENTRIES * entry_size <= LEDGER_OFFSET + LEDGER_SIZE);
    assert_eq!(LEDGER_OFFSET % std::mem::align_of::<LedgerEntry>(), 0);

    // The typed accessor and raw base-pointer arithmetic agree on one address.
    assert_eq!(
        offset_in(&region, region.header() as *const ShmHeader as *const u8),
        0
    );
    assert_eq!(
        offset_in(&region, region.world_model() as *const WorldModel as *const u8),
        WORLD_MODEL_OFFSET
    );
    assert_eq!(
        offset_in(
            &region,
            region.thought_buffer() as *const ThoughtBuffer as *const u8
        ),
        THOUGHT_BUFFER_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.lore_buffer() as *const LoreBuffer as *const u8),
        LORE_BUFFER_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.world_voxels() as *const WorldVoxels as *const u8),
        WORLD_VOXELS_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.lidar_scan() as *const LidarScan as *const u8),
        LIDAR_SCAN_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.map_grid() as *const PersistentMapGrid as *const u8),
        MAP_GRID_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.camera_frame() as *const CameraFrame as *const u8),
        CAMERA_FRAME_OFFSET
    );
    assert_eq!(
        offset_in(
            &region,
            region.applied_action() as *const AppliedActionSlot as *const u8
        ),
        APPLIED_ACTION_OFFSET
    );
    assert_eq!(
        offset_in(
            &region,
            region.applied_action_history() as *const AppliedActionHistory as *const u8
        ),
        APPLIED_ACTION_HISTORY_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.jepa_evidence() as *const JepaEvidenceSlot as *const u8),
        JEPA_REGION_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.jepa_telemetry() as *const JepaTelemetrySlot as *const u8),
        JEPA_TELEMETRY_OFFSET
    );
    assert_eq!(
        offset_in(&region, region.fly_sim() as *const FlySimSlot as *const u8),
        FLY_SIM_OFFSET
    );

    assert!(WORLD_MODEL_OFFSET + std::mem::size_of::<WorldModel>() <= SHM_SIZE);
    assert!(THOUGHT_BUFFER_OFFSET + std::mem::size_of::<ThoughtBuffer>() <= SHM_SIZE);
    assert!(LORE_BUFFER_OFFSET + std::mem::size_of::<LoreBuffer>() <= SHM_SIZE);
    assert!(WORLD_VOXELS_OFFSET + std::mem::size_of::<WorldVoxels>() <= SHM_SIZE);
    assert!(LIDAR_SCAN_OFFSET + std::mem::size_of::<LidarScan>() <= SHM_SIZE);
    assert!(JEPA_REGION_OFFSET + JEPA_REGION_SIZE <= SHM_SIZE);
    assert!(FLY_SIM_OFFSET + std::mem::size_of::<FlySimSlot>() <= SHM_SIZE);
    assert!(APPLIED_ACTION_HISTORY_OFFSET + std::mem::size_of::<AppliedActionHistory>() <= SHM_SIZE);

    // A write through the region base is what `layer_slot` reads back.
    unsafe {
        let raw = region.as_ptr().add(LAYER_SLOTS_OFFSET) as *mut LayerSlot;
        (*raw).bias[0] = 1234.0;
    }
    assert_eq!(region.layer_slot(0).bias[0], 1234.0);

    drop(region);
}

#[test]
fn open_rejects_a_foreign_header() {
    let name = region_name("foreign");
    let region = ShmRegion::create(&name).expect("create");

    // SAFETY: this test is the sole owner of a fresh region; it deliberately
    // corrupts each field and restores it before finishing.
    let header = unsafe { &mut *(region.as_ptr() as *mut ShmHeader) };

    header.magic = SHM_MAGIC ^ 0xFFFF;
    assert!(matches!(ShmRegion::open(&name), Err(ShmError::BadMagic)));
    header.magic = SHM_MAGIC;

    header.version = SHM_VERSION + 1;
    match ShmRegion::open(&name) {
        Err(ShmError::VersionMismatch { expected, found }) => {
            assert_eq!(expected, SHM_VERSION);
            assert_eq!(found, SHM_VERSION + 1);
        }
        _ => panic!("a newer version must be rejected with VersionMismatch"),
    }
    header.version = SHM_VERSION;

    header.ledger_offset += 8;
    assert!(matches!(
        ShmRegion::open(&name),
        Err(ShmError::LayoutMismatch)
    ));
    header.ledger_offset = LEDGER_OFFSET as u64;

    header.jepa_region_size = JEPA_REGION_SIZE as u64 + 64;
    assert!(matches!(
        ShmRegion::open(&name),
        Err(ShmError::LayoutMismatch)
    ));
    header.jepa_region_size = JEPA_REGION_SIZE as u64;

    assert!(ShmRegion::open(&name).is_ok());
    drop(region);
}

#[test]
fn reattach_reads_the_existing_region_instead_of_zeroing_it() {
    let name = region_name("reattach");

    let owner = ShmRegion::create(&name).expect("create");
    owner.emit_thought(2, 3, 1.25, "persisted");
    owner.emit_lore("why", "because", 4, 1, 0.5, 0.75);
    owner.append_ledger(&ledger_row(9, 2.5));
    owner.world_model_mut().last_vision_ns = 4242;

    let attached = ShmRegion::open(&name).expect("reattach");
    assert_eq!(attached.header().magic, SHM_MAGIC);

    let thoughts = attached.thought_buffer();
    assert_eq!(thoughts.write_seq.load(Ordering::Acquire), 1);
    assert_eq!(thoughts.entries[0].text[..9], *b"persisted");
    assert_eq!(thoughts.entries[0].layer, 2);
    assert_eq!(thoughts.entries[0].kind, 3);
    assert_eq!(thoughts.entries[0].vfe, 1.25);
    assert_eq!(thoughts.entries[0].seq, 0);

    let lore = attached.lore_buffer();
    assert_eq!(lore.write_seq.load(Ordering::Acquire), 1);
    assert_eq!(lore.entries[0].question[..3], *b"why");
    assert_eq!(lore.entries[0].answer[..7], *b"because");
    assert_eq!(lore.entries[0].effectiveness, 0.75);

    assert_eq!(attached.ledger_seq(), 1);
    let row = attached.ledger_entry(0);
    assert_eq!(row.seq, 9);
    assert_eq!(row.layer, 3);
    assert_eq!(row.vfe, 2.5);
    assert_eq!(attached.world_model().last_vision_ns, 4242);

    // Detaching the creator must not take the region away from the attachment.
    drop(owner);
    assert_eq!(attached.world_model().last_vision_ns, 4242);
    assert_eq!(attached.thought_buffer().entries[0].text[..9], *b"persisted");
    drop(attached);
}

#[test]
fn layer_writer_publishes_and_reader_follows() {
    let name = region_name("doublebuffer");
    let region = ShmRegion::create(&name).expect("create");

    let slot = region.layer_slot(1);
    assert_eq!(slot.write_idx.load(Ordering::Acquire), 0);

    let writer = LayerWriter::new(slot);
    let reader = LayerReader::new(slot);

    writer.back_buffer().mean[0] = 42.0;
    writer.back_buffer().layer = 1;
    writer.publish();

    assert_eq!(slot.write_idx.load(Ordering::Acquire), 1);
    assert_eq!(reader.read().mean[0], 42.0);
    assert_eq!(reader.read().layer, 1);

    // The second publish flips the pair back and the reader follows.
    writer.back_buffer().mean[0] = 7.0;
    writer.publish();
    assert_eq!(slot.write_idx.load(Ordering::Acquire), 0);
    assert_eq!(reader.read().mean[0], 7.0);

    drop(region);
}

#[test]
fn ledger_ring_wraps_and_reports_its_sequence() {
    let name = region_name("ledger");
    let region = ShmRegion::create(&name).expect("create");

    assert_eq!(region.ledger_seq(), 0);
    region.append_ledger(&ledger_row(0, 0.0));
    assert_eq!(region.ledger_seq(), 1);
    assert_eq!(region.ledger_entry(0).seq, 0);

    // A full ring pass reuses slot 0 for the newest sequence and leaves the
    // slots written exactly once in this pass untouched.
    for seq in 1..=MAX_LEDGER_ENTRIES as u64 {
        region.append_ledger(&ledger_row(seq, seq as f32));
    }
    assert_eq!(region.ledger_seq(), MAX_LEDGER_ENTRIES as u64 + 1);
    assert_eq!(region.ledger_entry(0).seq, MAX_LEDGER_ENTRIES as u64);
    assert_eq!(region.ledger_entry(0).vfe, MAX_LEDGER_ENTRIES as f32);
    assert_eq!(region.ledger_entry(1).seq, 1);
    assert_eq!(
        region.ledger_entry(MAX_LEDGER_ENTRIES - 1).seq,
        MAX_LEDGER_ENTRIES as u64 - 1
    );

    drop(region);
}

#[test]
fn weight_tiles_round_trip_through_a_layer_slot() {
    let name = region_name("weights");
    let region = ShmRegion::create(&name).expect("create");

    let values: Vec<f32> = (0..16).map(|value| value as f32 * 0.5).collect();
    region.write_weight_tile(0, 2, 3, 4, &values);
    assert_eq!(region.read_weight_tile(0, 2, 3, 4), values);

    // An untouched tile reads back as zeros, not as the tile beside it.
    assert!(region.read_weight_tile(0, 0, 0, 4).iter().all(|v| *v == 0.0));

    drop(region);
}

#[test]
fn open_of_an_unknown_name_reports_an_os_error() {
    let name = region_name("missing");
    assert!(matches!(ShmRegion::open(&name), Err(ShmError::OsError(_))));
}

#[test]
fn error_display_renders_the_reference_wording() {
    // These strings reach operators: `qualia-init` prints
    // `[init] Failed to create shm: {err}`, and the other runners surface the
    // same text, so the rendered line — not the variant — is the contract.
    assert_eq!(ShmError::OsError(13).to_string(), "OS error 13");
    assert_eq!(
        ShmError::BadMagic.to_string(),
        "bad magic number in shared memory header"
    );
    assert_eq!(
        ShmError::SizeMismatch.to_string(),
        "shared memory size mismatch"
    );
    assert_eq!(
        ShmError::VersionMismatch {
            expected: 2,
            found: 11,
        }
        .to_string(),
        "shared memory version mismatch: expected 2, found 11"
    );
    assert_eq!(
        ShmError::LayoutMismatch.to_string(),
        "shared memory layout contract mismatch"
    );
}

#[test]
fn error_debug_renders_the_reference_spelling() {
    assert_eq!(format!("{:?}", ShmError::OsError(13)), "ShmError::OsError(13)");
    assert_eq!(format!("{:?}", ShmError::BadMagic), "ShmError::BadMagic");
    assert_eq!(
        format!("{:?}", ShmError::SizeMismatch),
        "ShmError::SizeMismatch"
    );
    assert_eq!(
        format!(
            "{:?}",
            ShmError::VersionMismatch {
                expected: 2,
                found: 11,
            }
        ),
        "ShmError::VersionMismatch { expected: 2, found: 11 }"
    );
    assert_eq!(
        format!("{:?}", ShmError::LayoutMismatch),
        "ShmError::LayoutMismatch"
    );
}

#[test]
#[should_panic(expected = "ledger index out of range")]
fn a_ledger_index_past_the_ring_is_rejected_with_the_reference_message() {
    let name = region_name("ledger-bounds");
    let region = ShmRegion::create(&name).expect("create");
    let _ = region.ledger_entry(MAX_LEDGER_ENTRIES);
}

#[cfg(windows)]
#[test]
fn a_name_the_platform_cannot_use_renders_the_operator_line() {
    // A malformed `QUALIA_SHM_NAME` reaches the supervisor as this line, so the
    // whole line — prefix, wording and code — is what an operator reads.
    match ShmRegion::create("") {
        Err(err) => assert_eq!(
            format!("[init] Failed to create shm: {err}"),
            "[init] Failed to create shm: OS error 22"
        ),
        Ok(_) => panic!("an empty name must not map a region"),
    }
}

#[cfg(windows)]
#[test]
fn windows_names_are_normalised_across_separators() {
    let name = region_name("normalise");
    let with_slashes = format!("/qualia/shm/{}", name.trim_start_matches('/'));
    let with_underscores = with_slashes.trim_start_matches('/').replace('/', "_");

    let region = ShmRegion::create(&with_slashes).expect("create");
    let attached = ShmRegion::open(&with_underscores).expect("attach by normalised name");

    region.world_model_mut().last_vision_ns = 11;
    assert_eq!(attached.world_model().last_vision_ns, 11);

    drop(attached);
    drop(region);
}
