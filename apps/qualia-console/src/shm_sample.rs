//! Attach to the shared region and copy each panel's data out of it.
//!
//! One attach per poll, three copies, no `unsafe` in this crate: everything
//! that touches the mapping lives in `qualia-shm`.

use qualia_shm::ShmRegion;

use crate::stack::SensingSet;
use crate::views::belief::BeliefView;
use crate::views::brain::BrainView;
use crate::views::evidence::LedgerRow;
use crate::views::telemetry::TelemetryView;
use crate::views::world::WorldView;

/// The arena every region-opening runner and the default manifest name
/// (`config/stack-manifest.default.json` `shared_memory.name`;
/// `runners/lidar/src/lib.rs` `DEFAULT_SHM_NAME`).
pub const DEFAULT_SHM_NAME: &str = "/qualia_body";

/// The region name, from the environment, defaulting to the stack's arena.
pub fn region_name() -> String {
    std::env::var("QUALIA_SHM_NAME")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_SHM_NAME.to_owned())
}

/// The four panel readings plus the ledger, or one reason they are absent.
#[derive(Debug, Clone, PartialEq)]
pub struct ShmSample {
    pub belief: BeliefView,
    pub world: WorldView,
    pub telemetry: TelemetryView,
    pub brain: BrainView,
    pub ledger: Vec<LedgerRow>,
    pub error: Option<String>,
}

fn unavailable(region: Option<String>, reason: String) -> ShmSample {
    ShmSample {
        belief: BeliefView::unattached(region.clone(), reason.clone()),
        world: WorldView::unattached(region.clone(), reason.clone()),
        telemetry: TelemetryView::unattached(reason.clone()),
        brain: BrainView::unattached(region, reason.clone()),
        ledger: Vec::new(),
        error: Some(reason),
    }
}

/// Attach to `region` and read the belief, world, brain, telemetry and ledger.
///
/// `sensing` is the sensing row set the stack declares, or the reason the
/// manifest naming it could not be read; the telemetry rows come from that set,
/// and a manifest that cannot be read is named without blanking the other
/// panels.
pub fn sample(region: &str, sensing: &Result<SensingSet, String>) -> ShmSample {
    match ShmRegion::open(region) {
        Ok(region_handle) => ShmSample {
            belief: BeliefView {
                region: Some(region.to_owned()),
                ..BeliefView::sample(&region_handle)
            },
            world: WorldView {
                region: Some(region.to_owned()),
                ..WorldView::sample(&region_handle)
            },
            brain: BrainView {
                region: Some(region.to_owned()),
                ..BrainView::sample(&region_handle)
            },
            telemetry: match sensing {
                Ok(set) => TelemetryView::sample(&region_handle, set),
                Err(reason) => TelemetryView::unattached(format!("stack manifest: {reason}")),
            },
            ledger: LedgerRow::sample(&region_handle),
            error: None,
        },
        Err(error) => unavailable(Some(region.to_owned()), error.to_string()),
    }
}
