//! Telemetry view: the sensing runners' latest frames.
//!
//! Traceability (`docs/frontend-lessons.md`, source 1): the panel reads the
//! `#[repr(C)]` ABI directly with no IPC layer, and staleness is derived from
//! the frame's own timestamp so the redraw is driven by the data.
//!
//! The runner set is named ([`SENSING_RUNNERS`]) rather than being whatever the
//! region happens to hold, so a runner that has published nothing keeps a row
//! saying so instead of shrinking the table.

use std::sync::atomic::Ordering;

use egui::Ui;
use qualia_shm::ShmRegion;

use crate::{format_age_ms, ConsoleState};

/// The sensing runners this view shows, in table order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensingRunner {
    Lidar,
    Camera,
    Vslam,
}

/// Every runner the console expects to hear from.
pub const SENSING_RUNNERS: [SensingRunner; 3] = [
    SensingRunner::Lidar,
    SensingRunner::Camera,
    SensingRunner::Vslam,
];

impl SensingRunner {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lidar => "lidar",
            Self::Camera => "camera",
            Self::Vslam => "vslam",
        }
    }

    /// The newest frame this runner has published, or `None` when it has
    /// published none.
    ///
    /// Every slot's seqlock sequence reads zero until its first publish, which
    /// is what separates "no frame yet" from a frame whose own numbers are zero.
    pub fn newest(self, region: &ShmRegion) -> Option<FrameReading> {
        match self {
            Self::Lidar => {
                let scan = region.lidar_scan().snapshot(4).ok()?;
                (scan.seq != 0).then(|| {
                    FrameReading::published(
                        self,
                        format!("{} points", scan.point_count),
                        scan.scan_end_ns,
                    )
                })
            }
            Self::Camera => {
                let frame = region.camera_frame().snapshot(4).ok()?;
                (frame.seq != 0).then(|| {
                    FrameReading::published(
                        self,
                        format!(
                            "{}x{} luma {:.2}±{:.2}",
                            frame.source_width,
                            frame.source_height,
                            frame.luminance_mean,
                            frame.luminance_stddev
                        ),
                        frame.timestamp_ns,
                    )
                })
            }
            Self::Vslam => {
                let frontend = region.vslam_frontend();
                (frontend.seq.load(Ordering::Acquire) != 0).then(|| {
                    FrameReading::published(
                        self,
                        format!(
                            "{} features, {} keyframes, confidence {:.2}",
                            frontend.feature_count,
                            frontend.keyframe_count,
                            frontend.tracking_confidence
                        ),
                        frontend.timestamp_ns,
                    )
                })
            }
        }
    }
}

/// One row of the telemetry table: the runner, and its newest frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReading {
    pub runner: &'static str,
    /// The frame's summary, or `None` when the runner has published none.
    pub detail: Option<String>,
    pub timestamp_ns: u64,
}

impl FrameReading {
    /// A frame the runner has published.
    pub fn published(runner: SensingRunner, detail: impl Into<String>, timestamp_ns: u64) -> Self {
        Self {
            runner: runner.label(),
            detail: Some(detail.into()),
            timestamp_ns,
        }
    }

    /// A runner that has published nothing.
    pub fn absent(runner: SensingRunner) -> Self {
        Self {
            runner: runner.label(),
            detail: None,
            timestamp_ns: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TelemetryView {
    /// One row per [`SENSING_RUNNERS`] entry, in the same order.
    pub frames: Vec<FrameReading>,
    pub error: Option<String>,
}

impl TelemetryView {
    pub fn unattached(error: impl Into<String>) -> Self {
        Self {
            frames: Vec::new(),
            error: Some(error.into()),
        }
    }

    /// Copy the newest frame each sensing runner has published.
    pub fn sample(region: &ShmRegion) -> Self {
        Self {
            frames: SENSING_RUNNERS
                .iter()
                .map(|runner| {
                    runner
                        .newest(region)
                        .unwrap_or_else(|| FrameReading::absent(*runner))
                })
                .collect(),
            error: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    let view = &state.telemetry;
    ui.heading("Telemetry");

    if let Some(error) = &view.error {
        ui.colored_label(
            egui::Color32::from_rgb(224, 160, 138),
            format!("telemetry source error: {error}"),
        );
    }

    if view.is_empty() {
        ui.label("no telemetry frames");
        return;
    }

    egui::Grid::new("telemetry_frames")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.label("runner");
            ui.label("newest frame");
            ui.label("age");
            ui.end_row();

            for frame in &view.frames {
                ui.label(frame.runner);
                match &frame.detail {
                    Some(detail) => {
                        ui.label(detail.as_str());
                        ui.label(format!(
                            "{} ms",
                            format_age_ms(state.observed_at_ns, frame.timestamp_ns)
                        ));
                    }
                    None => {
                        ui.label("no frame published");
                        ui.label("—");
                    }
                }
                ui.end_row();
            }
        });
}
