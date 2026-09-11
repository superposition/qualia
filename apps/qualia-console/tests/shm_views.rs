//! The views against a real shared region.
//!
//! The snapshot tests never open a region, so this is the only cover for the
//! ABI read path: a view must name every runner it claims to show, and it must
//! not turn "nothing published yet" into a frame of zeros — the World view
//! included, whose pose, map and voxel sequence each carry their own publish
//! state.

use std::sync::atomic::{AtomicU64, Ordering};

use egui_kittest::{kittest::Queryable, Harness};
use qualia_console::stack::SensingSet;
use qualia_console::views::telemetry::{SensingRunner, TelemetryView};
use qualia_console::views::world::WorldView;
use qualia_console::{fixture, Connection, ConsoleState, Sample, View, WindowSet};
use qualia_shm::ShmRegion;
use qualia_types::LidarScanSnapshot;

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

/// Unique per test and per run, like the shm crate's own tests: no name is ever
/// reused, so a region left behind by a killed run cannot be attached by
/// mistake.
fn region_name(tag: &str) -> String {
    let index = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
    format!("/qualia_console_test_{}_{}_{}", std::process::id(), tag, index)
}

/// The ABI's sensing slots, in table order, as the console names them.
fn every_slot() -> Vec<&'static str> {
    SensingRunner::ALL
        .iter()
        .map(|slot| slot.runner_name())
        .collect()
}

/// A 720-point lidar scan published into the region's lidar slot.
fn publish_scan(region: &ShmRegion) {
    let scan = LidarScanSnapshot {
        scan_start_ns: 1_000,
        scan_end_ns: 2_000,
        point_count: 720,
        ..LidarScanSnapshot::default()
    };
    region.lidar_scan_mut().publish(&scan).expect("publish scan");
}

#[test]
fn telemetry_names_every_sensing_slot_the_console_reads() {
    let name = region_name("telemetry");
    let region = ShmRegion::create(&name).expect("create region");

    let fresh = TelemetryView::sample(&region, &SensingSet::EverySlot);
    let labels: Vec<&str> = fresh.frames.iter().map(|row| row.runner.as_str()).collect();
    let expected: Vec<&str> = every_slot();
    assert_eq!(
        labels,
        expected,
        "with no stack named, the rows are the ABI's sensing slots, in slot order"
    );
    assert!(
        fresh.frames.iter().all(|row| row.detail.is_none()),
        "a region nobody has written to has no frames to show"
    );

    publish_scan(&region);

    let published = TelemetryView::sample(&region, &SensingSet::EverySlot);
    let lidar = published
        .frames
        .iter()
        .find(|row| row.runner == SensingRunner::Lidar.runner_name())
        .expect("the lidar slot keeps its row");
    assert_eq!(lidar.detail.as_deref(), Some("720 points"));
    assert_eq!(lidar.timestamp_ns, 2_000);
    assert_eq!(
        published
            .frames
            .iter()
            .filter(|row| row.detail.is_none())
            .count(),
        2,
        "the slots that published nothing keep their rows"
    );
}

#[test]
fn telemetry_renders_an_absent_row_for_every_silent_slot() {
    let name = region_name("telemetry_render");
    let region = ShmRegion::create(&name).expect("create region");
    let fixture = fixture();
    let slots = every_slot();

    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.telemetry = TelemetryView::sample(&region, &SensingSet::EverySlot);

    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(View::Telemetry);
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();

    for slot in &slots {
        harness.get_by_label(*slot);
    }
    assert_eq!(
        harness.query_all_by_label("no frame published").count(),
        slots.len(),
        "a silent slot is an absent row, not a missing one"
    );
    assert_eq!(
        harness.query_all_by_label("no telemetry frames").count(),
        0,
        "the slots exist whether or not a runner has published to them"
    );
}

/// A named stack is read, not re-invented: its sensing runners are the rows, in
/// its own order, and a stack that declares none renders the honest empty table
/// — the state `config/stack-manifest.zero-motion.json` shows — rather than
/// borrowing a runner set from the console.
#[test]
fn telemetry_rows_follow_the_named_stack_and_its_order() {
    let name = region_name("telemetry_named");
    let region = ShmRegion::create(&name).expect("create region");
    publish_scan(&region);

    let declared = SensingSet::Declared(vec!["qualia-camera".to_owned(), "qualia-lidar".to_owned()]);
    let view = TelemetryView::sample(&region, &declared);
    let labels: Vec<&str> = view.frames.iter().map(|row| row.runner.as_str()).collect();
    assert_eq!(
        labels,
        vec!["qualia-camera", "qualia-lidar"],
        "the named stack's sensing runners, in the manifest's order"
    );
    assert!(
        view.frames[0].detail.is_none(),
        "a runner the stack names but that has published nothing is absent, not missing"
    );
    assert_eq!(view.frames[1].detail.as_deref(), Some("720 points"));

    let fixture = fixture();
    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.telemetry = view;
    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(View::Telemetry);
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();
    harness.get_by_label("qualia-camera");
    harness.get_by_label("qualia-lidar");
    assert_eq!(harness.query_all_by_label("no frame published").count(), 1);

    let none = TelemetryView::sample(&region, &SensingSet::Declared(Vec::new()));
    assert!(
        none.is_empty(),
        "a stack that declares no sensing runner gets no rows"
    );
    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.telemetry = none;
    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(View::Telemetry);
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();
    harness.get_by_label("no telemetry frames");
    assert_eq!(
        harness.query_all_by_label("no frame published").count(),
        0,
        "the honest empty table has no rows to render"
    );
}

#[test]
fn world_reports_absent_pose_map_and_voxels_until_each_slot_is_published() {
    let name = region_name("world_fresh");
    let region = ShmRegion::create(&name).expect("create region");

    let fresh = WorldView::sample(&region);
    assert!(fresh.pose.is_none(), "a fresh region has no pose");
    assert!(fresh.map.is_none(), "a fresh region has no map");
    assert!(
        fresh.voxel_update_seq.is_none(),
        "a fresh region has no voxel sequence"
    );
    assert!(fresh.is_empty());

    // Publishing the pose must not make the map or the voxels appear.
    {
        let frontend = region.vslam_frontend_mut();
        frontend.pose_x_m = 1.25;
        frontend.pose_z_m = -0.5;
        frontend.yaw_rad = 0.3;
        frontend.pose_confidence = 0.88;
        frontend.timestamp_ns = 5_000;
        frontend.seq.store(2, Ordering::Release);
    }

    let pose_only = WorldView::sample(&region);
    let pose = pose_only.pose.expect("the published pose is read");
    assert_eq!(pose.x_m, 1.25);
    assert!(pose_only.map.is_none(), "the map slot is still unpublished");
    assert!(pose_only.voxel_update_seq.is_none());

    {
        let grid = region.map_grid_mut();
        grid.width = 256;
        grid.height = 256;
        grid.resolution_m = 0.05;
        grid.occupied_cells = 1_234;
        grid.observed_cells = 5_678;
        grid.last_update_ns = 6_000;
        grid.seq.store(2, Ordering::Release);
    }
    region.world_voxels_mut().update_seq.store(7, Ordering::Release);

    let full = WorldView::sample(&region);
    let map = full.map.expect("the published map is read");
    assert_eq!((map.width, map.height, map.seq), (256, 256, 2));
    assert_eq!(full.voxel_update_seq, Some(7));
}

#[test]
fn world_renders_the_absent_arms_for_a_fresh_region() {
    let name = region_name("world_render");
    let region = ShmRegion::create(&name).expect("create region");
    let fixture = fixture();

    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.world = WorldView {
        region: Some(name.clone()),
        ..WorldView::sample(&region)
    };

    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(View::World);
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();

    harness.get_by_label("pose: no fix");
    harness.get_by_label("map: not published");
    harness.get_by_label("voxels: not published");
    assert_eq!(
        harness.query_all_by_label("no world data").count(),
        0,
        "an attached region is not an absent source"
    );
}
