//! Stats view: the runner telemetry frames the HUD draws.
//!
//! Every producer publishes one fixed-width frame per runner
//! ([`qualia_types::RunnerStats`]) into the stats region beside the data arena
//! (`crates/shm::StatsRegion`). This view is the console's read of that region:
//! one [`StatsRow`] per published frame, and the reason named when the region
//! cannot be attached. It draws nothing on its own — [`crate::hud`] owns the
//! floating panels and the layout — so the sampled numbers stay separable from
//! the windows that show them.
//!
//! The rate is the one the **producer** measured, not one the console guessed:
//! `rate_milli_hz` is the writer's own one-second window over its ticks, so a
//! panel that reads at 1 Hz still shows a 30 Hz camera as 30 Hz. The history
//! kept here is the console's own — the last [`HISTORY_LEN`] samples of each
//! runner's rate — and it is what the panels' sparklines draw.

use std::collections::{BTreeMap, VecDeque};

use qualia_shm::StatsRegion;
use qualia_types::RunnerStatsSnapshot;

use crate::stack;

/// Rate samples kept per runner for the sparkline. At the console's 4 Hz poll
/// this is the last half-minute.
pub const HISTORY_LEN: usize = 120;

/// One runner's telemetry frame, as the console reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsRow {
    pub runner: String,
    pub seq: u64,
    /// The rate the producer measured, in hertz.
    pub rate_hz: f64,
    pub bytes_per_sec: u64,
    pub backlog: u32,
    pub ticks: u64,
    pub errors: u64,
    /// The frame's own update time, for the age the panel shows.
    pub published_at_ns: u64,
    /// The values the producer last emitted, labelled, with unset ones dropped.
    pub values: Vec<(String, f32)>,
    pub publishing: bool,
}

impl StatsRow {
    fn from_frame(frame: &RunnerStatsSnapshot) -> Self {
        let values = frame
            .values
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                let label = frame.label(index);
                (!label.is_empty()).then(|| (label.to_owned(), *value))
            })
            .collect();
        Self {
            runner: frame.runner.clone(),
            seq: frame.seq,
            rate_hz: frame.rate_hz(),
            bytes_per_sec: frame.bytes_per_sec,
            backlog: frame.backlog,
            ticks: frame.ticks,
            errors: frame.errors,
            published_at_ns: frame.published_at_ns,
            values,
            publishing: frame.is_publishing(),
        }
    }
}

/// The stats region as this poll found it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsView {
    /// The region the console tried to attach, so a panel can name it.
    pub region: Option<String>,
    /// Why there are no frames: an unattached region, or none published yet.
    pub error: Option<String>,
    pub rows: Vec<StatsRow>,
    /// Runners the stack knows about but that published no frame: the gaps the
    /// HUD menu names rather than drawing an empty panel.
    pub gaps: Vec<String>,
}

impl StatsView {
    /// No region was looked for yet (the console's first frame).
    pub fn pending() -> Self {
        Self {
            region: None,
            error: Some("waiting for the first poll".to_owned()),
            rows: Vec::new(),
            gaps: Vec::new(),
        }
    }

    /// The region could not be attached; nothing is drawn.
    pub fn unattached(region: Option<String>, reason: String) -> Self {
        Self {
            region,
            error: Some(reason),
            rows: Vec::new(),
            gaps: Vec::new(),
        }
    }

    /// Read every published frame out of the stats region beside `data_region`.
    pub fn sample(data_region: &str) -> Self {
        let name = qualia_shm::stats_region_name_from_env(data_region);
        let expected = stack::load_runner_names();
        match StatsRegion::open(&name) {
            Ok(region) => {
                let mut rows: Vec<StatsRow> =
                    region.poll(4).iter().map(StatsRow::from_frame).collect();
                // A producer that restarts claims a fresh slot and leaves the old
                // frame behind, so the newest frame per runner is the one shown;
                // the panels come out in runner order, not claim order.
                rows.sort_by(|left, right| {
                    left.runner
                        .cmp(&right.runner)
                        .then(right.published_at_ns.cmp(&left.published_at_ns))
                });
                rows.dedup_by(|later, kept| later.runner == kept.runner);
                let gaps = expected
                    .iter()
                    .filter(|runner| !rows.iter().any(|row| row.runner == **runner))
                    .cloned()
                    .collect();
                Self {
                    region: Some(name),
                    error: None,
                    rows,
                    gaps,
                }
            }
            Err(error) => {
                // The region is created by `qualia-init` beside the arena (or by
                // the first producer that starts alone), so an absent region is
                // a stack that is not up rather than a console fault.
                let mut view = Self::unattached(Some(name), format!("stats region: {error}"));
                view.gaps = expected;
                view
            }
        }
    }

    /// Whether any runner published a frame.
    pub fn has_frames(&self) -> bool {
        !self.rows.is_empty()
    }

    /// The row for `runner`, if it published.
    pub fn row(&self, runner: &str) -> Option<&StatsRow> {
        self.rows.iter().find(|row| row.runner == runner)
    }
}

/// The console's own history of the producers' rates, one series per runner.
///
/// Held outside [`StatsView`] because a poll replaces that view whole; this is
/// what survives a poll, the way the Brain view keeps its camera and markers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatsHistory {
    series: BTreeMap<String, VecDeque<f32>>,
}

impl StatsHistory {
    /// Record this poll's rates and drop every series whose runner is gone.
    pub fn record(&mut self, view: &StatsView) {
        let mut live: Vec<&str> = Vec::with_capacity(view.rows.len());
        for row in &view.rows {
            live.push(row.runner.as_str());
            let series = self.series.entry(row.runner.clone()).or_default();
            series.push_back(row.rate_hz as f32);
            while series.len() > HISTORY_LEN {
                series.pop_front();
            }
        }
        self.series.retain(|runner, _| live.contains(&runner.as_str()));
    }

    /// The rate samples for `runner`, oldest first.
    pub fn get(&self, runner: &str) -> Option<&VecDeque<f32>> {
        self.series.get(runner)
    }

    /// Whether `runner` has any history to draw.
    pub fn has(&self, runner: &str) -> bool {
        self.series
            .get(runner)
            .is_some_and(|series| !series.is_empty())
    }
}
