//! T52 planner-batch scratch: drive the proposal step at the shape T50 measured.
//!
//! Calls the same public function `qualia-jepa-plan-eval propose` calls —
//! `evaluate_rollout_proposals` — on the same `CoherentJepaRuntime`, because the
//! binary's provenance gate stops it before the step (see the T50 model-eval
//! README). Prints the step time and, with `--output`, the whole proposal JSON
//! so a before/after run can be diffed bit for bit.
//!
//! usage: t52-probe propose --checkpoint <dir> --request <json> [--backend cuda] [--output <json>]

use qualia_jepa_model::device_for_backend;
use qualia_jepa_model::planner::{evaluate_rollout_proposals, RolloutEvaluationRequest};
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
    let mode = args.next().ok_or("usage: t52-probe propose ...")?;
    let mut checkpoint = None;
    let mut request = None;
    let mut backend = "cuda".to_string();
    let mut output = None;
    while let Some(token) = args.next() {
        match token.as_str() {
            "--checkpoint" => checkpoint = Some(flag(&mut args, "--checkpoint")?),
            "--request" => request = Some(flag(&mut args, "--request")?),
            "--backend" => backend = flag(&mut args, "--backend")?,
            "--output" => output = Some(flag(&mut args, "--output")?),
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    if mode != "propose" {
        return Err(format!("unknown mode {mode}").into());
    }
    let checkpoint = checkpoint.ok_or("--checkpoint is required")?;
    let request = request.ok_or("--request is required")?;

    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&request)?)?;
    let rollout: RolloutEvaluationRequest = serde_json::from_value(raw["rollout"].clone())?;
    let device = device_for_backend(&backend)?;
    let load_started = Instant::now();
    let runtime = CoherentJepaRuntime::from_checkpoint(&checkpoint, device)?;
    let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
    let steps_per_candidate = rollout.candidates.first().map(|row| row.steps.len()).unwrap_or(0);
    let started = Instant::now();
    let proposal = evaluate_rollout_proposals(&runtime, &rollout, true)?;
    let step_ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Some(path) = output {
        let rendered = serde_json::to_vec(&proposal)?;
        std::fs::write(path, &rendered)?;
    }
    println!(
        "{}",
        json!({
            "mode": "propose",
            "backend": backend,
            "checkpoint_id": runtime.checkpoint_id(),
            "candidates": rollout.candidates.len(),
            "steps_per_candidate": steps_per_candidate,
            "predict_calls": rollout.candidates.len() * steps_per_candidate,
            "load_ms": load_ms,
            "step_ms": step_ms,
            "selected_candidate_id": proposal.selected_candidate_id,
            "direct_planner_write": proposal.direct_planner_write,
        })
    );
    Ok(())
}
