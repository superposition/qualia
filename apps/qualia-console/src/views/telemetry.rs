//! Telemetry view: the sensing runners' latest frames.
//!
//! Traceability (`docs/frontend-lessons.md`, source 1): the panel reads the
//! `#[repr(C)]` ABI directly with no IPC layer, and staleness is derived from
//! the frame's own timestamp so the redraw is driven by the data.
//!
//! The runner set comes from the stack manifest ([`crate::stack`]) rather than
//! a list here (`docs/frontend-lessons.md` source 1 counts two hard-coded
//! runner lists beside `QUALIA_STACK_MANIFEST` as a mistake): the rows are
//! exactly the manifest's sensing runners, in manifest order, and a runner
//! that has published nothing keeps its row saying so instead of shrinking the
//! table.

use std::sync::atomic::Ordering;

use egui::Ui;
use qualia_shm::ShmRegion;

use crate::{theme, ConsoleState};

/// Column widths of the frame table, on the 8 px rhythm and sized to the
/// panel's default width.
const COL_RUNNER: f32 = 200.0;
const COL_FRAME: f32 = 360.0;
const COL_AGE: f32 = 72.0;

/// The sensing slots the region's ABI defines, and the runner crate that
/// publishes each (`runners/*/Cargo.toml`).
///
/// This is slot knowledge, not a stack definition: which of these runners a
/// deployment runs is what the manifest declares ([`crate::stack`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensingRunner {
    Lidar,
    Camera,
    Vslam,
}

impl SensingRunner {
    /// The runner crate that publishes this slot.
    pub const fn runner_name(self) -> &'static str {
        match self {
            Self::Lidar => "qualia-lidar",
            Self::Camera => "qualia-camera",
            Self::Vslam => "qualia-vslam",
        }
    }

    /// The sensing slot whose publishing runner is `name`, if any.
    pub fn for_runner_name(name: &str) -> Option<Self> {
        match name {
            "qualia-lidar" => Some(Self::Lidar),
            "qualia-camera" => Some(Self::Camera),
            "qualia-vslam" => Some(Self::Vslam),
            _ => None,
        }
    }

    /// The newest frame this runner has published, or `None` when it has
    /// published none.
    ///
    /// Every slot's seqlock sequence reads zero until its first publish, which
    /// is what separates "no frame yet" from a frame whose own numbers are zero.
    pub fn newest(self, region: &ShmRegion, runner: impl Into<String>) -> Option<FrameReading> {
        match self {
            Self::Lidar => {
                let scan = region.lidar_scan().snapshot(4).ok()?;
                (scan.seq != 0).then(|| {
                    FrameReading::published(
                        runner,
                        format!("{} points", scan.point_count),
                        scan.scan_end_ns,
                    )
                })
            }
            Self::Camera => {
                let frame = region.camera_frame().snapshot(4).ok()?;
                (frame.seq != 0).then(|| {
                    FrameReading::published(
                        runner,
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
                        runner,
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
    /// The runner name the stack manifest declares.
    pub runner: String,
    /// The frame's summary, or `None` when the runner has published none.
    pub detail: Option<String>,
    pub timestamp_ns: u64,
}

impl FrameReading {
    /// A frame the runner has published.
    pub fn published(
        runner: impl Into<String>,
        detail: impl Into<String>,
        timestamp_ns: u64,
    ) -> Self {
        Self {
            runner: runner.into(),
            detail: Some(detail.into()),
            timestamp_ns,
        }
    }

    /// A runner that has published nothing.
    pub fn absent(runner: impl Into<String>) -> Self {
        Self {
            runner: runner.into(),
            detail: None,
            timestamp_ns: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TelemetryView {
    /// One row per sensing runner the manifest declares, in manifest order.
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

    /// Copy the newest frame each of `runner_names` has published.
    ///
    /// `runner_names` is the stack manifest's runner list, so the table shows
    /// the runners this stack declares and nothing else; a declared name that
    /// is not a sensing slot keeps no row.
    pub fn sample(region: &ShmRegion, runner_names: &[String]) -> Self {
        Self {
            frames: runner_names
                .iter()
                .filter_map(|name| {
                    let runner = SensingRunner::for_runner_name(name)?;
                    Some(
                        runner
                            .newest(region, name.clone())
                            .unwrap_or_else(|| FrameReading::absent(name.clone())),
                    )
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

    if let Some(error) = &view.error {
        theme::error_banner(ui, &format!("telemetry source error: {error}"));
    }

    if view.is_empty() {
        theme::state_line(ui, "no telemetry frames", theme::TEXT_SECOND);
        return;
    }

    theme::header(
        ui,
        &[
            ("runner", COL_RUNNER, false),
            ("newest frame", COL_FRAME, false),
            ("age", COL_AGE, true),
        ],
    );

    for frame in &view.frames {
        match &frame.detail {
            Some(detail) => {
                let age = theme::age(state.observed_at_ns, frame.timestamp_ns);
                theme::cells(
                    ui,
                    &[
                        (frame.runner.as_str(), COL_RUNNER, theme::TEXT, false),
                        (detail.as_str(), COL_FRAME, theme::TEXT_SECOND, false),
                        (age.as_str(), COL_AGE, theme::TEXT, true),
                    ],
                );
            }
            None => theme::cells(
                ui,
                &[
                    (frame.runner.as_str(), COL_RUNNER, theme::TEXT, false),
                    ("no frame published", COL_FRAME, theme::MUTED, false),
                    (theme::DASH, COL_AGE, theme::MUTED, true),
                ],
            ),
        }
    }
}
