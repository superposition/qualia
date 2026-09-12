//! T50 close-out scratch: drive the two `qualia-jepa-plan-eval` step functions
//! directly, at the shapes the binary's own argv carries.
//!
//! `qualia-jepa-plan-eval propose` refuses before its step on this host: it calls
//! `verify_provenance`, which requires a training report that clears
//! `TrainingReport::validate_for_promotion`, and no artifact this host can publish
//! does. This probe therefore calls the same two public functions the binary calls —
//! `evaluate_rollout_proposals` (planner.rs:229) and `compare_offline_planners`
//! (planner.rs:715) — with the same `CoherentJepaRuntime` and the same request file,
//! so the step's cost is measured even where the binary's gate stops it.
//!
//! usage: t50close-probe <propose|compare> --checkpoint <dir> --request <json> [--backend cuda]

use qualia_jepa_model::device_for_backend;
use qualia_jepa_model::planner::{
    compare_offline_planners, evaluate_rollout_proposals, OfflinePlannerComparisonConfig,
    OfflinePlannerComparisonProvenance, OfflinePlannerDecision, RolloutEvaluationRequest,
};
use qualia_jepa_model::runtime::CoherentJepaRuntime;
use serde_json::json;
use std::time::Instant;

fn flag(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    match args.next() {
        Some(value) => Ok(value),
        None => Err(format!("{name} requires a value")),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("usage: t50close-probe <propose|compare> ...")?;
    let mut checkpoint = None;
    let mut request = None;
    let mut backend = "cuda".to_string();
    while let Some(token) = args.next() {
        match token.as_str() {
            "--checkpoint" => checkpoint = Some(flag(&mut args, "--checkpoint")?),
            "--request" => request = Some(flag(&mut args, "--request")?),
            "--backend" => backend = flag(&mut args, "--backend")?,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let checkpoint = checkpoint.ok_or("--checkpoint is required")?;
    let request = request.ok_or("--request is required")?;

    match mode.as_str() {
        "propose" => {
            let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&request)?)?;
            let rollout: RolloutEvaluationRequest = serde_json::from_value(raw["rollout"].clone())?;
            let device = device_for_backend(&backend)?;
            let load_started = Instant::now();
            let runtime = CoherentJepaRuntime::from_checkpoint(&checkpoint, device)?;
            let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
            let started = Instant::now();
            let proposal = evaluate_rollout_proposals(&runtime, &rollout, true)?;
            let step_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{}",
                json!({
                    "mode": "propose",
                    "backend": backend,
                    "checkpoint_id": runtime.checkpoint_id(),
                    "candidates": rollout.candidates.len(),
                    "steps_per_candidate": rollout.candidates.first().map(|row| row.steps.len()).unwrap_or(0),
                    "predict_calls": rollout.candidates.len() * rollout.candidates.first().map(|row| row.steps.len()).unwrap_or(0),
                    "load_ms": load_ms,
                    "step_ms": step_ms,
                    "selected_candidate_id": proposal.selected_candidate_id,
                    "direct_planner_write": proposal.direct_planner_write,
                })
            );
        }
        "compare" => {
            let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&request)?)?;
            let provenance: OfflinePlannerComparisonProvenance =
                serde_json::from_value(raw["provenance"].clone())?;
            let config: OfflinePlannerComparisonConfig =
                serde_json::from_value(raw["config"].clone())?;
            let decisions: Vec<OfflinePlannerDecision> =
                serde_json::from_value(raw["decisions"].clone())?;
            let input_sha256 = "0".repeat(64);
            let started = Instant::now();
            let report =
                compare_offline_planners(&decisions, &config, &provenance, &input_sha256)?;
            let step_ms = started.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{}",
                json!({
                    "mode": "compare",
                    "decisions": decisions.len(),
                    "step_ms": step_ms,
                    "report": serde_json::to_value(&report)?,
                })
            );
        }
        other => return Err(format!("unknown mode {other}").into()),
    }
    Ok(())
}
