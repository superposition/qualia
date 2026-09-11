//! Coupling tests for the loaded connectome prior.
//!
//! With `fly-prior` off `couple_prior` is the zero no-op the interface fixes;
//! with it on the prior's verified graph scales the mapped slots by normalised
//! in-strength, times the dial the runner hands over. The arithmetic is
//! host-side data (D-001), but the symbol hangs off a layer's context, so each
//! test opens one and reports and returns where no CUDA device is present,
//! exactly as the other device tests here do.

#![cfg(feature = "cuda")]

use qualia_cuda::{couple_prior, default_params, CudaContext, STATE_DIM};
use qualia_jepa::prior::{CouplingPrior, COUPLING_SCALE_DEFAULT};

/// Three types, four edges; in-strength is 4 for type 0, 8 for type 1 and 2 for
/// type 2, so type 1 is the graph's strongest type.
fn prior() -> CouplingPrior {
    CouplingPrior {
        type_count: 3,
        rowptr: vec![0, 1, 3, 4],
        cols: vec![1, 1, 2, 0],
        weights: vec![3, 5, 2, 4],
    }
}

fn context() -> Option<CudaContext> {
    match CudaContext::new(&default_params(1)) {
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("skipping coupling test: {error}");
            None
        }
    }
}

/// Without the feature nothing is coupled, whatever the mapping says.
#[cfg(not(feature = "fly-prior"))]
#[test]
fn coupling_is_noop_without_feature() {
    let Some(context) = context() else {
        return;
    };
    let prior = prior();

    assert_eq!(
        couple_prior(&context, &prior, &[], COUPLING_SCALE_DEFAULT).expect("empty mapping"),
        0.0
    );
    assert_eq!(
        couple_prior(&context, &prior, &[(1, 0), (0, 1)], COUPLING_SCALE_DEFAULT)
            .expect("mapped slots"),
        0.0
    );
    assert_eq!(
        couple_prior(&context, &prior, &[(9, STATE_DIM)], COUPLING_SCALE_DEFAULT)
            .expect("unmappable slots"),
        0.0
    );
}

/// With the feature on the total is what `CouplingPrior::couple` applies.
#[cfg(feature = "fly-prior")]
#[test]
fn coupling_applies_the_prior_by_type_strength() {
    let Some(context) = context() else {
        return;
    };
    let prior = prior();

    // The strongest type couples at unit weight, half and a quarter the
    // strength at half and a quarter the weight.
    assert_eq!(
        couple_prior(&context, &prior, &[(1, 0)], COUPLING_SCALE_DEFAULT).expect("strongest type"),
        1.0
    );
    assert_eq!(
        couple_prior(&context, &prior, &[(0, 0)], COUPLING_SCALE_DEFAULT).expect("half strength"),
        0.5
    );
    assert_eq!(
        couple_prior(&context, &prior, &[(2, 0)], COUPLING_SCALE_DEFAULT)
            .expect("quarter strength"),
        0.25
    );
    assert_eq!(
        couple_prior(
            &context,
            &prior,
            &[(1, 0), (0, 1), (2, 2)],
            COUPLING_SCALE_DEFAULT
        )
        .expect("three slots"),
        1.75
    );
    // The slot a type feeds does not change the weight it carries.
    assert_eq!(
        couple_prior(&context, &prior, &[(1, 7)], COUPLING_SCALE_DEFAULT).expect("a distant slot"),
        1.0
    );
    // An empty mapping is the no-prior state and applies nothing.
    assert_eq!(
        couple_prior(&context, &prior, &[], COUPLING_SCALE_DEFAULT).expect("empty mapping"),
        0.0
    );
    // The dial the runner hands over is what the total is multiplied by
    // (T30, #46): the same graph at twice the dial couples twice as hard.
    assert_eq!(
        couple_prior(&context, &prior, &[(1, 0)], 2.0).expect("doubled dial"),
        2.0
    );
}

/// A mapping the graph or the layer cannot satisfy is refused, not skipped.
#[cfg(feature = "fly-prior")]
#[test]
fn coupling_refuses_an_unmappable_slot() {
    let Some(context) = context() else {
        return;
    };
    let prior = prior();

    assert_eq!(
        couple_prior(&context, &prior, &[(3, 0)], COUPLING_SCALE_DEFAULT).unwrap_err(),
        qualia_cuda::CudaError::UnknownType(3)
    );
    assert_eq!(
        couple_prior(&context, &prior, &[(1, STATE_DIM)], COUPLING_SCALE_DEFAULT).unwrap_err(),
        qualia_cuda::CudaError::SlotOutOfRange(STATE_DIM)
    );
}
