//! The `qualia-health` process: attach to the arena and stream report frames.
//!
//! Everything testable lives in the library; this binary is the process
//! boundary the supervisor spawns. It logs the region it was told to open
//! before trying, so a failed startup is attributable from the supervisor's
//! child log alone.

use qualia_shm::ShmRegion;

fn main() {
    let shm_name = qualia_health::shm_name();
    eprintln!("qualia-health: opening shm '{shm_name}'");

    let shm = ShmRegion::open(&shm_name).expect("Failed to open shm");
    let mut stdout = std::io::stdout().lock();
    qualia_health::stream(&shm, &mut stdout);
}
