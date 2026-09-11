//! What one poll produced: the braid, the drift, and each panel's reading.
//!
//! [`Sample::degraded`] covers both the console's warm start and every poll
//! that did not answer: the braid comes from the committed
//! `tests/fixtures/braid-state.json`, so the offline console never shows an
//! empty window, and each panel names the reason it has nothing live.

use crate::client::{BraidSnapshot, BraidState, DriftReport};
use crate::views::belief::BeliefView;
use crate::views::evidence::EvidenceView;
use crate::views::telemetry::TelemetryView;
use crate::views::world::WorldView;
use crate::Connection;

/// The region label a fixture-only sample carries.
///
/// The live path reads `QUALIA_SHM_NAME` (see `crate::shm_sample::region_name`);
/// a fixture needs a name to draw, and this is that name, not a default the
/// binary would use.
pub const FIXTURE_SHM_REGION: &str = "qualia";

/// Evidence root used when the fixture is the source.
pub const FIXTURE_EVIDENCE_ROOT: &str = "artifacts/mcap";

/// One coherent poll: everything `render_view` needs except the selected tab.
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
}

impl Sample {
    /// No agent answered: the braid comes from the fixture and every panel
    /// reports why it has nothing live.
    pub fn degraded(fixture: &BraidSnapshot, reason: &str, observed_at_ns: u64) -> Self {
        let region = Some(FIXTURE_SHM_REGION.to_owned());
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
                root: FIXTURE_EVIDENCE_ROOT.to_owned(),
                error: Some(format!("agent unreachable: {reason}")),
                ..EvidenceView::default()
            },
            telemetry: TelemetryView::unattached(format!("agent unreachable: {reason}")),
        }
    }
}
