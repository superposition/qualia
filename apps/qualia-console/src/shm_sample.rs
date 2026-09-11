//! Attach to the shared region and copy each panel's data out of it.
//!
//! One attach per poll, three copies, no `unsafe` in this crate: everything
//! that touches the mapping lives in `qualia-shm`.

use qualia_shm::ShmRegion;

use crate::views::belief::BeliefView;
use crate::views::evidence::LedgerRow;
use crate::views::telemetry::TelemetryView;
use crate::views::world::WorldView;

/// The region name, from the environment, never a source literal.
pub fn region_name() -> Option<String> {
    std::env::var("QUALIA_SHM_NAME")
        .ok()
        .filter(|name| !name.is_empty())
}

/// The three panel readings plus the ledger, or one reason they are absent.
#[derive(Debug, Clone, PartialEq)]
pub struct ShmSample {
    pub belief: BeliefView,
    pub world: WorldView,
    pub telemetry: TelemetryView,
    pub ledger: Vec<LedgerRow>,
    pub error: Option<String>,
}

fn unavailable(region: Option<String>, reason: String) -> ShmSample {
    ShmSample {
        belief: BeliefView::unattached(region.clone(), reason.clone()),
        world: WorldView::unattached(region, reason.clone()),
        telemetry: TelemetryView::unattached(reason.clone()),
        ledger: Vec::new(),
        error: Some(reason),
    }
}

/// Attach to `region_name` and read the belief, world, telemetry and ledger.
pub fn sample(region_name: Option<&str>) -> ShmSample {
    let Some(name) = region_name else {
        return unavailable(None, "QUALIA_SHM_NAME is unset".to_owned());
    };

    match ShmRegion::open(name) {
        Ok(region) => ShmSample {
            belief: BeliefView {
                region: Some(name.to_owned()),
                ..BeliefView::sample(&region)
            },
            world: WorldView {
                region: Some(name.to_owned()),
                ..WorldView::sample(&region)
            },
            telemetry: TelemetryView::sample(&region),
            ledger: LedgerRow::sample(&region),
            error: None,
        },
        Err(error) => unavailable(Some(name.to_owned()), error.to_string()),
    }
}
