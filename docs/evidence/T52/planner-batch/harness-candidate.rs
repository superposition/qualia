//! T52 planner-batch scratch: write the repo's own parity-fixture candidate.
//!
//! The recipe is the same one T50's `harness-candidate.rs` used (parity.rs's
//! test module): a deterministic `JepaCandidateModel` published through the
//! public `write_candidate_checkpoint` with the synthetic gate block, seed
//! 0xc0ffee. It is a loadable checkpoint, not a trained model.
//!
//! usage: t52-candidate <root-dir> [checkpoint-id]

use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};
use qualia_jepa_model::{
    empty_candidate_manifest, initialize_deterministic, write_candidate_checkpoint, ActionSupport,
    BaselineGate, GroundingGeometry, HeldOutMetrics, JepaCandidateModel,
};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut argv = std::env::args().skip(1);
    let root = argv.next().ok_or("usage: t52-candidate <root-dir> [checkpoint-id]")?;
    let checkpoint_id = argv.next().unwrap_or_else(|| "t52-candidate".to_string());

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
        &checkpoint_id,
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
        write_candidate_checkpoint(&root, &checkpoint_id, &weights, &weights, manifest)?;
    println!("weights {}", weights_path.display());
    println!("manifest {}", manifest_path.display());
    Ok(())
}
