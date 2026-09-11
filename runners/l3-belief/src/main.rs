//! The `qualia-l3-belief` process: hand belief layer 3 to the compute backend.
//!
//! Layer 3 is `belief_visual` in `qualia.json` — shared-memory slot 3, 100 Hz,
//! predicting `visual_features` — but the belief loop is not in this crate.
//! `qualia-metal` owns it on macOS and `qualia-cuda` owns it on a CUDA host;
//! each owns the contents of the layer slot it writes and the operator lines
//! the layer emits, and both are budgeted as the same layer, so exactly one of
//! them is compiled in.
//!
//! What the binary owns is the identity it hands over — the slot the backend
//! writes and the name it reports under — plus the fly coupling the operator
//! asked for. `QUALIA_FLY_MODE` is `off` (the default) or `prior`, and a
//! `prior` names an artifact directory through `QUALIA_FLY_PRIOR_PATH`; the
//! artifact is loaded and verified here, so a truncated or edited graph is
//! reported before the layer starts and the layer runs uncoupled instead of
//! failing to start. `QUALIA_FLY_COUPLING_SCALE` bounds the weight the prior is
//! applied at. Both backends on, or neither, is refused at compile time
//! rather than resolved by a preference the operator never sees.

use std::path::Path;

use qualia_jepa::prior::CouplingPrior;
#[cfg(feature = "fly-prior")]
use qualia_jepa::prior::{clamp_coupling_scale, COUPLING_SCALE_DEFAULT};

/// The shared-memory layer slot this runner drives: `belief_visual`, id 3.
const LAYER_ID: u8 = 3;

/// The layer's name, as the stack manifest spawns it and the backend logs it.
const LAYER_NAME: &str = "l3-belief";

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
/// process before it enters the layer.
fn fly_mode_from_env() -> FlyMode {
    let raw = std::env::var(FLY_MODE_KEY).unwrap_or_default();
    match raw.trim() {
        "" | "off" => FlyMode::Off,
        "prior" => FlyMode::Prior,
        "sim" => FlyMode::Sim,
        other => {
            eprintln!(
                "qualia-{LAYER_NAME}: unknown {FLY_MODE_KEY} {other:?}; expected off, prior or sim"
            );
            std::process::exit(2);
        }
    }
}

/// Loads the prior `QUALIA_FLY_PRIOR_PATH` names, or reports why it cannot.
///
/// `None` means the layer runs uncoupled. Nothing is printed on success; every
/// way of failing to produce a verified prior prints one line naming why.
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
    let mode = fly_mode_from_env();
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
}

#[cfg(all(feature = "cuda", feature = "metal"))]
compile_error!("cuda and metal features are mutually exclusive");

#[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
compile_error!("enable either cuda or metal feature");

#[cfg(all(test, feature = "fly-prior"))]
mod coupling_scale {
    use super::{fly_coupling_scale_from_env, FLY_COUPLING_SCALE_KEY};
    use qualia_jepa::prior::{COUPLING_SCALE_CEILING, COUPLING_SCALE_DEFAULT, COUPLING_SCALE_FLOOR};

    /// The dial the operator hands down is read and bounded, so a hand-edited
    /// manifest cannot drive this layer's coupling to zero or to infinity.
    #[test]
    fn the_dial_is_read_at_the_coupling_bounds() {
        for (raw, expected) in [
            ("0", COUPLING_SCALE_FLOOR),
            ("inf", COUPLING_SCALE_CEILING),
            ("nan", COUPLING_SCALE_DEFAULT),
            ("2.5", 2.5),
            ("not-a-number", COUPLING_SCALE_DEFAULT),
        ] {
            std::env::set_var(FLY_COUPLING_SCALE_KEY, raw);
            assert_eq!(fly_coupling_scale_from_env(), expected, "dial {raw:?}");
        }

        std::env::remove_var(FLY_COUPLING_SCALE_KEY);
        assert_eq!(
            fly_coupling_scale_from_env(),
            COUPLING_SCALE_DEFAULT,
            "an unset dial is the identity"
        );
    }
}
