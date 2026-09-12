//! What one poll produced: the braid, the drift, and each panel's reading.
//!
//! [`Sample::degraded`] covers both the console's warm start and every poll
//! that did not answer: the braid comes from the committed
//! `tests/fixtures/braid-state.json`, so the offline console never shows an
//! empty window, and each panel names the reason it has nothing live.

use crate::client::{BraidSnapshot, BraidState, DriftReport};
use crate::views::belief::BeliefView;
use crate::views::brain::BrainView;
use crate::views::coach::CoachView;
use crate::views::evidence::{EvidenceView, DEFAULT_EVIDENCE_ROOT};
use crate::views::stats::StatsView;
use crate::views::telemetry::TelemetryView;
use crate::views::world::WorldView;
use crate::Connection;

/// The region label a fixture-only sample carries.
///
/// The live path reads `QUALIA_SHM_NAME` (see `crate::shm_sample::region_name`);
/// a fixture needs a name to draw, and this is that name, not a default the
/// binary would use.
pub const FIXTURE_SHM_REGION: &str = "qualia";

/// One coherent poll: everything `render_view` needs except which panels are
/// open.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub observed_at_ns: u64,
    pub connection: Connection,
    pub braid: BraidState,
    pub drift: Option<DriftReport>,
    pub belief: BeliefView,
    pub world: WorldView,
    pub evidence: EvidenceView,
    pub telemetry: TelemetryView,
    pub brain: BrainView,
    /// The runner telemetry frames the HUD draws. The poller fills this from
    /// the stats region even when the agent is unreachable, so the operator's
    /// panels stay live without a braid.
    pub stats: StatsView,
    /// The mission broker's decision stream (T63), read from its own loopback
    /// status surface; a broker that is not running is a named line, never an
    /// invented decision.
    pub coach: CoachView,
}

impl Sample {
    /// No agent answered: the braid comes from the fixture and every panel
    /// reports why it has nothing live.
    pub fn degraded(fixture: &BraidSnapshot, reason: &str, observed_at_ns: u64) -> Self {
        let region = Some(FIXTURE_SHM_REGION.to_owned());
        let mut brain = BrainView::unattached(
            region.clone(),
            format!("agent unreachable: {reason}"),
        );
        brain.record_braid(&fixture.braid);
        brain.observe_coupling_scale(observed_at_ns);
        Self {
            observed_at_ns,
            connection: Connection::Unreachable {
                reason: reason.to_owned(),
            },
            braid: fixture.braid.clone(),
            drift: fixture.drift.clone(),
            belief: BeliefView::unattached(region.clone(), format!("agent unreachable: {reason}")),
            world: WorldView::unattached(region, format!("agent unreachable: {reason}")),
            evidence: EvidenceView {
                root: DEFAULT_EVIDENCE_ROOT.to_owned(),
                error: Some(format!("agent unreachable: {reason}")),
                ..EvidenceView::default()
            },
            telemetry: TelemetryView::unattached(format!("agent unreachable: {reason}")),
            brain,
            stats: StatsView::unattached(
                Some(FIXTURE_SHM_REGION.to_owned()),
                format!("agent unreachable: {reason}"),
            ),
            coach: CoachView::Unreachable {
                url: crate::views::coach::coach_url(),
                reason: "not polled".to_string(),
            },
        }
    }
}
