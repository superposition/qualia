//! `qualia-vslam` binary: attach to the arena and run the front end.
//!
//! The process owns no files and prints three lines of state — the accelerator
//! it found, the region it attached to, and every thirtieth tracked frame — so
//! the supervisor's log is the only place an operator needs to look when the
//! front end stops publishing.

use qualia_shm::ShmRegion;
use qualia_vslam::{Frontend, FrontendConfig};

/// Name of the accelerator this build will actually use, if any.
#[cfg(feature = "cuda")]
fn cuda_device_name() -> Option<String> {
    cudarc::driver::CudaContext::new(0)
        .ok()
        .and_then(|context| context.name().ok())
}

/// The CPU-only build reports no accelerator rather than failing.
#[cfg(not(feature = "cuda"))]
fn cuda_device_name() -> Option<String> {
    None
}

fn main() {
    let config = FrontendConfig::from_env();

    let shm = match ShmRegion::open(&config.shm_name) {
        Ok(shm) => shm,
        Err(err) => {
            eprintln!(
                "qualia-vslam: failed to open shm '{}': {err}",
                config.shm_name
            );
            std::process::exit(1);
        }
    };

    let mut frontend = Frontend::new(config.clone());
    frontend.init_state(&shm);

    match cuda_device_name() {
        Some(name) => println!("qualia-vslam: cuda device ready: {name}"),
        None => println!("qualia-vslam: cuda path unavailable at build/runtime, using CPU frontend"),
    }
    println!(
        "qualia-vslam: consuming camera thumbnails from shm={}, local odom pose publish={} force={}",
        config.shm_name, config.publish_pose, config.force_pose
    );

    frontend.run(&shm)
}
