//! `qualia-l2-belief`: the executive belief layer of the body stack.
//!
//! Layer 2 reads the layer beneath it out of the shared arena and is the sole
//! writer of its own slot; the update itself is a GPU kernel, so the compute
//! loop lives in whichever backend the build selected — `qualia-metal` on
//! Apple silicon, `qualia-cuda` on Jetson. This binary carries the part the
//! backend cannot know: which layer it is, what to call it in the log, and the
//! fly coupling the operator asked for. `QUALIA_FLY_MODE` is `off` (the
//! default) or `prior`, and a `prior` names an artifact directory through
//! `QUALIA_FLY_PRIOR_PATH`; the artifact is loaded and verified here so a
//! truncated or edited graph is reported at the front door and the layer runs
//! uncoupled rather than failing to start. `QUALIA_FLY_COUPLING_SCALE` bounds
//! the weight the prior is applied at.
//!
//! Exactly one backend is compiled in. Choosing neither would leave a belief
//! layer with no update rule, and choosing both would run two of them over the
//! same slot, so both misconfigurations are build errors rather than runtime
//! surprises.

use std::path::Path;

use qualia_jepa::prior::CouplingPrior;
#[cfg(feature = "fly-prior")]
use qualia_jepa::prior::{clamp_coupling_scale, COUPLING_SCALE_DEFAULT};

/// Layer index this runner owns, counted from the sensor plane up.
const LAYER: u8 = 2;

/// Name the backend uses in its operator lines and in the arena narration.
const NAME: &str = "l2-belief";

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
            eprintln!("qualia-{NAME}: unknown {FLY_MODE_KEY} {other:?}; expected off, prior or sim");
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
        qualia_cuda::run_layer_with_prior(LAYER, NAME, prior, scale);

        #[cfg(all(feature = "metal", not(feature = "cuda")))]
        qualia_metal::run_layer_with_prior(LAYER, NAME, prior, scale);
    }

    // A verified prior with no coupling compiled in is reported rather than
    // silently dropped: the operator asked for a mode this build cannot run.
    #[cfg(not(feature = "fly-prior"))]
    {
        if prior.is_some() {
            eprintln!("fly prior: disabled (built without the fly-prior feature)");
        }

        #[cfg(all(feature = "cuda", not(feature = "metal")))]
        qualia_cuda::run_layer(LAYER, NAME);

        #[cfg(all(feature = "metal", not(feature = "cuda")))]
        qualia_metal::run_layer(LAYER, NAME);
    }

    #[cfg(all(feature = "cuda", feature = "metal"))]
    compile_error!("cuda and metal features are mutually exclusive");

    #[cfg(all(not(feature = "cuda"), not(feature = "metal")))]
    compile_error!("enable either cuda or metal feature");
}

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
