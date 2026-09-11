//! The views against a real shared region.
//!
//! The snapshot tests never open a region, so this is the only cover for the
//! ABI read path: a view must name every runner it claims to show, and it must
//! not turn "nothing published yet" into a frame of zeros.

use std::sync::atomic::{AtomicU64, Ordering};

use egui_kittest::{kittest::Queryable, Harness};
use qualia_console::views::telemetry::{TelemetryView, SENSING_RUNNERS};
use qualia_console::{fixture, Connection, ConsoleState, Sample, View};
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

#[test]
fn telemetry_names_every_runner_and_only_reports_published_frames() {
    let name = region_name("telemetry");
    let region = ShmRegion::create(&name).expect("create region");

    let fresh = TelemetryView::sample(&region);
    assert_eq!(fresh.frames.len(), SENSING_RUNNERS.len());
    assert!(
        fresh.frames.iter().all(|row| row.detail.is_none()),
        "a region nobody has written to has no frames to show"
    );

    let scan = LidarScanSnapshot {
        scan_start_ns: 1_000,
        scan_end_ns: 2_000,
        point_count: 720,
        ..LidarScanSnapshot::default()
    };
    region.lidar_scan_mut().publish(&scan).expect("publish scan");

    let published = TelemetryView::sample(&region);
    let lidar = published
        .frames
        .iter()
        .find(|row| row.runner == "lidar")
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

    let mut sample = Sample::degraded(&fixture, "not used here", fixture.braid.last_promotion_ns);
    sample.connection = Connection::Live;
    sample.telemetry = TelemetryView::sample(&region);

    let mut state = ConsoleState::from_sample(sample, "http://127.0.0.1:8080");
    state.view = View::Telemetry;
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(1280.0, 820.0))
        .wgpu()
        .build_ui_state(|ui, state| qualia_console::render_view(ui, state), state);
    harness.run();

    for runner in SENSING_RUNNERS {
        harness.get_by_label(runner.label());
    }
    assert_eq!(
        harness.query_all_by_label("no frame published").count(),
        SENSING_RUNNERS.len(),
        "a silent runner is an absent row, not a missing one"
    );
}
