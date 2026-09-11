//! The views against a real shared region.
//!
//! The snapshot tests never open a region, so this is the only cover for the
//! ABI read path: a view must name every runner it claims to show, and it must
//! not turn "nothing published yet" into a frame of zeros — the World view
//! included, whose pose, map and voxel sequence each carry their own publish
//! state.

use std::sync::atomic::{AtomicU64, Ordering};

use egui_kittest::{kittest::Queryable, Harness};
use qualia_console::views::telemetry::TelemetryView;
use qualia_console::views::world::WorldView;
use qualia_console::{fixture, stack, Connection, ConsoleState, Sample, View, WindowSet};
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

/// The runner names a manifest declaring the three sensing runners carries.
fn sensing_runner_names() -> Vec<String> {
    ["qualia-lidar", "qualia-camera", "qualia-vslam"]
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

#[test]
fn telemetry_names_every_runner_and_only_reports_published_frames() {
    let name = region_name("telemetry");
    let region = ShmRegion::create(&name).expect("create region");
    let runners = sensing_runner_names();

    let fresh = TelemetryView::sample(&region, &runners);
    assert_eq!(fresh.frames.len(), runners.len());
    assert!(
        fresh.frames.iter().all(|row| row.detail.is_none()),
        "a region nobody has written to has no frames to show"
    );
    assert_eq!(
        fresh.frames[0].runner, "qualia-lidar",
        "the row carries the name the manifest declares, not the slot's own"
    );

    let scan = LidarScanSnapshot {
        scan_start_ns: 1_000,
        scan_end_ns: 2_000,
        point_count: 720,
        ..LidarScanSnapshot::default()
    };
    region.lidar_scan_mut().publish(&scan).expect("publish scan");

    let published = TelemetryView::sample(&region, &runners);
    let lidar = published
        .frames
        .iter()
        .find(|row| row.runner == "qualia-lidar")
        .expect("lidar keeps its row");
    assert_eq!(lidar.detail.as_deref(), Some("720 points"));
    assert_eq!(lidar.timestamp_ns, 2_000);
    assert_eq!(
        published
            .frames
            .iter()
            .filter(|row| row.detail.is_none())
            .count(),
        2,
        "the runners that published nothing keep their rows"
    );
}

#[test]
fn telemetry_renders_an_absent_row_for_every_silent_runner() {
    let name = region_name("telemetry_render");
    let region = ShmRegion::create(&name).expect("create region");
    let fixture = fixture();
    let runners = sensing_runner_names();

    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.telemetry = TelemetryView::sample(&region, &runners);

    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(View::Telemetry);
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();

    for runner in &runners {
        harness.get_by_label(runner.as_str());
    }
    assert_eq!(
        harness.query_all_by_label("no frame published").count(),
        runners.len(),
        "a silent runner is an absent row, not a missing one"
    );
}

/// The shipped configuration, end to end: the rows come from the manifest
/// compiled into the binary. Before `config/stack-manifest.default.json` named
/// the sensing runners this rendered `no telemetry frames` while a published
/// scan sat in the region the console was attached to.
#[test]
fn telemetry_renders_the_rows_the_shipped_default_manifest_declares() {
    let name = region_name("telemetry_default");
    let region = ShmRegion::create(&name).expect("create region");
    let runners = stack::sensing_runner_names(stack::DEFAULT_MANIFEST)
        .expect("the compiled-in default manifest parses");

    let scan = LidarScanSnapshot {
        scan_start_ns: 1_000,
        scan_end_ns: 2_000,
        point_count: 720,
        ..LidarScanSnapshot::default()
    };
    region.lidar_scan_mut().publish(&scan).expect("publish scan");

    let fixture = fixture();
    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.telemetry = TelemetryView::sample(&region, &runners);
    assert_eq!(
        sample.telemetry.frames.len(),
        3,
        "the shipped default must name the three sensing runners the body stack runs"
    );

    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.windows = WindowSet::only(View::Telemetry);
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();

    for runner in &runners {
        harness.get_by_label(runner.as_str());
    }
    harness.get_by_label("720 points");
    assert_eq!(
        harness.query_all_by_label("no telemetry frames").count(),
        0,
        "the shipped default must not render the empty table"
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
