//! Test state, built by hand from the committed fixture.
//!
//! `docs/frontend-lessons.md` (source 3) keeps fixtures in the test tree and out
//! of the shipped binary: the console's live path reads the shared region and
//! the agent, so the readings below are scaffolding, not a fallback the binary
//! can reach. Nothing here opens a socket or touches the region.

use qualia_console::sample::FIXTURE_SHM_REGION;
use qualia_console::views::belief::{BeliefReading, BeliefView};
use qualia_console::views::brain::matrices::{MatrixReading, MATRIX_CELLS};
use qualia_console::views::brain::{BrainView, CloudReading, FiringReading, PlanPose, SceneToggles};
use qualia_console::views::coach::{coach_url, CoachView};
use qualia_console::views::evidence::{
    EvidenceView, LedgerRow, SegmentReading, DEFAULT_EVIDENCE_ROOT,
};
use qualia_console::views::stats::StatsView;
use qualia_console::views::telemetry::{FrameReading, SensingRunner, TelemetryView};
use qualia_console::views::world::{MapReading, PoseReading, WorldView};
use qualia_console::{BraidSnapshot, Connection, Sample};

/// The fixture's own clock plus `offset_ms` milliseconds.
pub fn ms_after_promotion(fixture: &BraidSnapshot, offset_ms: u64) -> u64 {
    fixture.braid.last_promotion_ns + offset_ms * 1_000_000
}

/// A whole healthy poll derived from the committed fixture, with one frame per
/// sensing runner and one sealed segment.
pub fn healthy(fixture: &BraidSnapshot, observed_at_ns: u64) -> Sample {
    let braid = fixture.braid.clone();
    let base_ns = braid.last_promotion_ns;

    let belief = BeliefView {
        region: Some(FIXTURE_SHM_REGION.to_owned()),
        readings: (0..qualia_types::NUM_LAYERS)
            .map(|layer| BeliefReading {
                layer: layer as u8,
                vfe: 0.08 + 0.02 * layer as f32,
                residual_norm: 0.03 + 0.01 * layer as f32,
                compression: (1 + layer % 4) as u8,
                cycle_us: 800 + 10 * layer as u32,
                timestamp_ns: base_ns + layer as u64 * 1_000_000,
            })
            .collect(),
        error: None,
    };

    let world = WorldView {
        region: Some(FIXTURE_SHM_REGION.to_owned()),
        pose: Some(PoseReading {
            x_m: 1.2,
            z_m: 0.4,
            yaw_rad: 0.05,
            confidence: 0.9,
            timestamp_ns: base_ns + 2_000_000,
        }),
        map: Some(MapReading {
            width: qualia_types::MAP_GRID_W as u32,
            height: qualia_types::MAP_GRID_H as u32,
            resolution_m: 0.05,
            occupied_cells: 412,
            observed_cells: 9_100,
            seq: 34,
            last_update_ns: base_ns + 3_000_000,
        }),
        voxel_update_seq: Some(56),
        error: None,
    };

    let telemetry = TelemetryView {
        frames: vec![
            FrameReading::published(
                SensingRunner::Lidar.runner_name(),
                "720 points",
                base_ns + 3_000_000,
            ),
            FrameReading::published(
                SensingRunner::Camera.runner_name(),
                "640x480 luma 0.42±0.11",
                base_ns + 2_500_000,
            ),
            FrameReading::published(
                SensingRunner::Vslam.runner_name(),
                "180 features, 12 keyframes, confidence 0.88",
                base_ns + 2_200_000,
            ),
        ],
        error: None,
    };

    let evidence = EvidenceView {
        root: DEFAULT_EVIDENCE_ROOT.to_owned(),
        segments: vec![SegmentReading {
            path: format!("{DEFAULT_EVIDENCE_ROOT}/{}.mcap", braid.session_id),
            byte_length: 4_194_304,
            channels: vec![
                qualia_mcap::ChannelInventory {
                    topic: qualia_mcap::TOPIC_LIDAR.to_owned(),
                    schema: qualia_mcap::JSON_SCHEMA_NAME.to_owned(),
                    message_count: 1_200,
                    min_log_time_ns: base_ns,
                    max_log_time_ns: base_ns + 60_000_000_000,
                },
                qualia_mcap::ChannelInventory {
                    topic: qualia_mcap::TOPIC_BELIEF.to_owned(),
                    schema: qualia_mcap::JSON_SCHEMA_NAME.to_owned(),
                    message_count: 240,
                    min_log_time_ns: base_ns,
                    max_log_time_ns: base_ns + 60_000_000_000,
                },
            ],
        }],
        quarantined: Vec::new(),
        ledger: (0..8)
            .map(|row| LedgerRow {
                seq: 40 + row,
                layer: (row % qualia_types::NUM_LAYERS as u64) as u8,
                event: "Confirm".to_owned(),
                vfe: 0.05 + 0.01 * row as f32,
                residual_norm: 0.02,
                timestamp_ns: base_ns + row * 1_000_000,
            })
            .collect(),
        error: None,
    };

    let mut brain = BrainView {
        region: Some(FIXTURE_SHM_REGION.to_owned()),
        error: None,
        firing: Some(FiringReading {
            sim_id: "qualia.fly-circuit.rate.v1".to_owned(),
            type_count: 5,
            sim_step: 128,
            producer_epoch: 3,
            timestamp_ns: base_ns + 1_500_000,
            flags: 0,
            rates: vec![0.90, 0.55, 0.72, 0.20, 0.45],
        }),
        layers: (0..qualia_types::NUM_LAYERS)
            .map(|layer| MatrixReading {
                layer: layer as u8,
                weight: (0..MATRIX_CELLS)
                    .map(|cell| ((cell + layer) % 11) as f32 / 11.0)
                    .collect(),
                belief: (0..MATRIX_CELLS)
                    .map(|cell| ((cell * 3 + layer) % 7) as f32 / 14.0 - 0.25)
                    .collect(),
                vfe: 0.08 + 0.02 * layer as f32,
                residual_norm: 0.03 + 0.01 * layer as f32,
                timestamp_ns: base_ns + layer as u64 * 1_000_000,
            })
            .collect(),
        cloud: CloudReading {
            points: (0..48)
                .map(|index| {
                    let angle = index as f32 * 0.13;
                    [angle.cos() * 0.8, 0.0, angle.sin() * 0.8]
                })
                .collect(),
            voxels: (0..96)
                .map(|index| {
                    [
                        ((index % 12) as f32 - 6.0) * 0.08,
                        ((index / 12) as f32 - 4.0) * 0.08,
                        ((index % 7) as f32 - 3.0) * 0.08,
                    ]
                })
                .collect(),
            voxel_total: 128,
            lidar_timestamp_ns: base_ns + 3_000_000,
            voxel_seq: Some(56),
            floor_seq: Some(12),
            pose: Some(PlanPose {
                x_m: 1.2,
                z_m: 0.4,
                yaw_rad: 0.05,
            }),
        },
        markers: Vec::new(),
        history: Vec::new(),
        coupling_scale: Some(1.0),
        camera: Default::default(),
        toggles: SceneToggles::default(),
        selected_layer: 0,
        counts: Default::default(),
    };
    brain.record_braid(&braid);

    Sample {
        observed_at_ns,
        connection: Connection::Live,
        braid,
        drift: fixture.drift.clone(),
        belief,
        world,
        evidence,
        telemetry,
        brain,
        // No stats region is opened in tests: the HUD draws nothing, which is
        // what an absent region means, and no test asserts on HUD panels.
        stats: StatsView::unattached(None, "test: no stats region".to_owned()),
        // No broker runs in tests: the Coach panel shows its named degrade
        // line, which is exactly what the window shows without one.
        coach: CoachView::Unreachable {
            url: coach_url(),
            reason: "test: no broker".to_owned(),
        },
    }
}
