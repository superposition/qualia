//! Cross-lane scratch helper: write the parity-fixture candidate checkpoint the board run needs.
//!
//! `qualia-jepa-parity` loads the CPU reference runtime out of a checkpoint *before* it consults
//! `--target`, so reaching its compile-time backend gate needs a directory that
//! `CoherentJepaRuntime::from_checkpoint` accepts. No promotion-passing candidate exists in the tree
//! or on the board, so this writes the same fixture the crate's own test module uses
//! (`crates/jepa-model/src/parity.rs:433-486`): a deterministic `JepaCandidateModel` built through
//! the public API, published by the public writer `write_candidate_checkpoint` with the synthetic
//! gate block that test uses. It is not a trained model.
//!
//! It is the same program as `docs/evidence/T50/model-eval/harness-candidate.rs`, which is committed
//! for the same reason; the two were cross-checked by output size on 2026-09-11 and produce the same
//! 15 432 280-byte `weights.safetensors` and 1 471-byte `manifest.json`.
//!
//! usage: zz-fixture-checkpoint <output-root>            # writes <output-root>/cross-lane-fixture
//!
//! To build it: copy this file to `crates/jepa-model/src/bin/zz-fixture-checkpoint.rs` in the tree
//! being cross-built. It is a binary of that package rather than a crate of its own so that a single
//! `cargo zigbuild ... -p qualia-jepa-model --bins` produces it alongside the four real binaries,
//! with the same target and the same `+fp16` flag. It is not part of the package's committed
//! targets, so a build of a clean `git archive` produces four binaries, not five.

use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};
use qualia_jepa_model::{
    empty_candidate_manifest, initialize_deterministic, write_candidate_checkpoint, ActionSupport,
    BaselineGate, GroundingGeometry, HeldOutMetrics, JepaCandidateModel,
};

/// The directory name the cross-lane board transcript uses.
const CHECKPOINT_ID: &str = "cross-lane-fixture";

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::env::args()
        .nth(1)
        .ok_or("usage: zz-fixture-checkpoint <output-root>")?;

    let weights = VarMap::new();
    JepaCandidateModel::load(VarBuilder::from_varmap(&weights, DType::F32, &Device::Cpu))?;
    initialize_deterministic(&weights, 0xc0ffee)?;

    let baseline_gate = BaselineGate {
        dataset_digest: "d".repeat(64),
        valid_transitions: 50_000,
        sessions: 12,
        conditions: 3,
        environments: 3,
        constant: HeldOutMetrics { transition_nll: 2.0, rollout_error: 2.0 },
        flat_mlp: HeldOutMetrics { transition_nll: 1.5, rollout_error: 1.5 },
        tiny_cnn: HeldOutMetrics { transition_nll: 1.0, rollout_error: 1.0 },
    };
    let action_support = ActionSupport {
        sample_count: 50_000,
        min_left: -1.0,
        max_left: 1.0,
        min_right: -1.0,
        max_right: 1.0,
        min_effective_forward: -1.0,
        max_effective_forward: 1.0,
        min_effective_turn: -1.0,
        max_effective_turn: 1.0,
        min_speed_scale: 0.0,
        max_speed_scale: 1.0,
        min_delta_seconds: 0.05,
        max_delta_seconds: 0.5,
    };
    let manifest = empty_candidate_manifest(
        CHECKPOINT_ID,
        &"d".repeat(64),
        0xc0ffee,
        "cpu",
        baseline_gate,
        action_support,
        GroundingGeometry {
            width: qualia_jepa::GROUNDING_WIDTH,
            height: qualia_jepa::GROUNDING_HEIGHT,
            resolution_m: 0.05,
        },
    );

    let (weights_path, manifest_path) =
        write_candidate_checkpoint(&root, CHECKPOINT_ID, &weights, &weights, manifest)?;
    println!("checkpoint_id={CHECKPOINT_ID}");
    println!("dir={}", std::path::Path::new(&root).join(CHECKPOINT_ID).display());
    println!("weights={}", weights_path.display());
    println!("manifest={}", manifest_path.display());
    Ok(())
}
