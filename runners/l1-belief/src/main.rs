//! `qualia-l1-belief`: the process a stack spawns to run belief layer 1.
//!
//! This crate is deliberately only the process boundary. The belief loop, the
//! shared-memory publication and every operator line it prints belong to the
//! compute backend — `qualia-cuda` on the Orin Nano, `qualia-metal` on Apple
//! silicon — and the backend is chosen at build time rather than probed at
//! run time, so a deployment cannot silently fall back to the wrong one.
//!
//! What this binary owns is the hand-off: which arena slot it claims, which
//! name the backend logs and panics under, and the fly coupling the operator
//! asked for. `QUALIA_FLY_MODE` is `off` (the default) or `prior`, and a
//! `prior` names an artifact directory through `QUALIA_FLY_PRIOR_PATH`. The
//! artifact is loaded and verified here, before the layer is entered, so a
//! truncated or edited graph is reported at the front door and the layer runs
//! uncoupled instead of refusing to start — the prior is optional
//! infrastructure and must never take a belief layer down with it. The
//! supervisor's stack manifest spawns every belief layer with both keys, and
//! `QUALIA_FLY_COUPLING_SCALE` bounds the weight the prior is applied at.

use std::path::Path;

use qualia_jepa::prior::CouplingPrior;
#[cfg(feature = "fly-prior")]
use qualia_jepa::prior::{clamp_coupling_scale, COUPLING_SCALE_DEFAULT};

/// Arena slot this layer owns.
///
/// `qualia-shm` orders slots from the bottom of the stack upward, and the
/// health tap flattens them in that order; the supervisor's manifest lists the
/// runners in the same order, so this value is the layer's identity, not a
/// tuning knob.
const LAYER_ID: u8 = 1;

/// The layer's operator-facing name.
///
/// The backend prefixes its log lines with `qualia-<name>`, which makes this
/// the string an operator greps for in the child log, and it is also the last
/// word of the refusal printed on a host with no compute device.
const LAYER_NAME: &str = "l1-belief";

/// Environment key selecting the fly coupling.
const FLY_MODE_KEY: &str = "QUALIA_FLY_MODE";

/// Environment key naming the verified prior artifact directory.
const FLY_PRIOR_PATH_KEY: &str = "QUALIA_FLY_PRIOR_PATH";

/// Environment key bounding how hard the prior is applied.
///
/// The agent steps this dial from mission outcomes and writes it into the
/// stack manifest the supervisor hands down, so a layer couples at the
/// bounded weight the agent last chose. An unset key, or one that is not a
/// number, is the identity — the prior's own normalised weight, unchanged. A
/// reading outside the coupling's bounds is applied at the bound, so a
/// hand-edited manifest cannot drive the coupling to zero or to infinity.
#[cfg(feature = "fly-prior")]
const FLY_COUPLING_SCALE_KEY: &str = "QUALIA_FLY_COUPLING_SCALE";

/// Which fly coupling the process was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlyMode {
    /// No coupling. The default, and the value the stack manifest ships.
    Off,
    /// Couple every belief tick through the verified connectome prior.
    Prior,
    /// Observe the circuit simulator; wired where the `sim` feature lives.
    Sim,
}

/// Reads `QUALIA_FLY_MODE`, refusing a value this runner does not implement.
///
/// An unset or empty key is `off`, so a stack configured before the key
/// existed still starts. Any other value is an operator error and stops the
/// process before it enters the layer: coupling to a mode nobody asked for
/// would be worse than not starting.
fn fly_mode_from_env(name: &str) -> FlyMode {
    let raw = std::env::var(FLY_MODE_KEY).unwrap_or_default();
    match raw.trim() {
        "" | "off" => FlyMode::Off,
        "prior" => FlyMode::Prior,
        "sim" => FlyMode::Sim,
        other => {
            eprintln!("qualia-{name}: unknown {FLY_MODE_KEY} {other:?}; expected off, prior or sim");
            std::process::exit(2);
        }
    }
}

/// Loads the prior `QUALIA_FLY_PRIOR_PATH` names, or reports why it cannot.
///
/// `None` means the layer runs uncoupled. The caller logs the accepted prior
/// only through its effect on the published belief, so nothing is printed on
/// success; every way of failing to produce a verified prior prints exactly
/// one `fly prior: disabled` line naming the reason.
fn load_fly_prior() -> Option<CouplingPrior> {
    let raw = std::env::var(FLY_PRIOR_PATH_KEY).unwrap_or_default();
    let path = raw.trim();
    if path.is_empty() {
        eprintln!("fly prior: disabled ({FLY_PRIOR_PATH_KEY} is not set)");
        return None;
    }
    match CouplingPrior::load(Path::new(path)) {
        Ok(prior) => Some(prior),
        Err(error) => {
            eprintln!("fly prior: disabled ({error})");
            None
        }
    }
}

/// Reads `QUALIA_FLY_COUPLING_SCALE`, bounded to the range a coupling may be
/// applied at.
#[cfg(feature = "fly-prior")]
fn fly_coupling_scale_from_env() -> f32 {
    std::env::var(FLY_COUPLING_SCALE_KEY)
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .map_or(COUPLING_SCALE_DEFAULT, clamp_coupling_scale)
}

fn main() {
    let mode = fly_mode_from_env(LAYER_NAME);
    let prior = if mode == FlyMode::Prior {
        load_fly_prior()
    } else {
        None
    };

    #[cfg(feature = "fly-prior")]
    {
        let scale = fly_coupling_scale_from_env();

        #[cfg(all(feature = "cuda", not(feature = "metal")))]
        qualia_cuda::run_layer_with_prior(LAYER_ID, LAYER_NAME, prior, scale);

        #[cfg(all(feature = "metal", not(feature = "cuda")))]
        qualia_metal::run_layer_with_prior(LAYER_ID, LAYER_NAME, prior, scale);
    }

    // A verified prior with no coupling compiled in is reported rather than
    // silently dropped: the operator asked for a mode this build cannot run.
    #[cfg(not(feature = "fly-prior"))]
    {
        if prior.is_some() {
            eprintln!("fly prior: disabled (built without the fly-prior feature)");
        }

        #[cfg(all(feature = "cuda", not(feature = "metal")))]
        qualia_cuda::run_layer(LAYER_ID, LAYER_NAME);

        #[cfg(all(feature = "metal", not(feature = "cuda")))]
        qualia_metal::run_layer(LAYER_ID, LAYER_NAME);
    }

    // Both backends compiled in would make the choice of device implicit in
    // `cfg` order, and neither leaves the process with nothing to drive.
    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");

    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}
