//! T52 planner-batch scratch: batched runtime step against the single-row step.
//!
//! Runs the batched primitive `predict_latent_steps` on `--rows` deterministic
//! rows and the single-row `predict_latent_step` on each of the same rows, then
//! reports whether every published value is bit-identical. On CUDA this is the
//! device half of the ticket's "the same emitted values" claim.
//!
//! usage: t52-steps <candidate|checkpoint-dir> [--backend cuda] [--rows 64]

use qualia_jepa_model::device_for_backend;
use qualia_jepa_model::runtime::CoherentJepaRuntime;
use serde_json::json;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let checkpoint = args.next().ok_or("usage: t52-steps <checkpoint-dir> [--backend cuda] [--rows 64]")?;
    let mut backend = "cuda".to_string();
    let mut rows = 64_usize;
    while let Some(token) = args.next() {
        match token.as_str() {
            "--backend" => backend = args.next().ok_or("--backend requires a value")?,
            "--rows" => {
                rows = args
                    .next()
                    .ok_or("--rows requires a value")?
                    .parse()
                    .map_err(|error| format!("--rows: {error}"))?
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let device = device_for_backend(&backend)?;
    let runtime = CoherentJepaRuntime::from_checkpoint(&checkpoint, device)?;

    let core = qualia_jepa::CORE_DIM;
    let action_width = qualia_jepa::ACTION_DIM;
    let latents: Vec<f32> = (0..rows * core)
        .map(|index| ((index % 89) as f32 - 44.0) / 44.0)
        .collect();
    let actions: Vec<f32> = (0..rows * action_width)
        .map(|index| ((index % 5) as f32 - 2.0) * 0.1)
        .collect();
    let deltas: Vec<f32> = (0..rows).map(|row| 0.1 + 0.001 * row as f32).collect();

    let batched_started = Instant::now();
    let batched = runtime.predict_latent_steps(&latents, &actions, &deltas)?;
    let batched_ms = batched_started.elapsed().as_secs_f64() * 1000.0;
    let single_started = Instant::now();
    let mut bit_identical = batched.len() == rows;
    let mut mismatched_rows = 0_usize;
    let mut head_mismatches = [0_usize; 3];
    let mut worst_bits = 0_u32;
    let mut worst_ulp = 0_u32;
    for row in 0..rows {
        let latent = &latents[row * core..(row + 1) * core];
        let action: [f32; qualia_jepa::ACTION_DIM] = actions
            [row * action_width..(row + 1) * action_width]
            .try_into()
            .map_err(|_| "action row width")?;
        let single = runtime.predict_latent_step(latent, action, deltas[row])?;
        let heads: [(&[f32], &[f32]); 3] = [
            (&batched[row].mean, &single.mean),
            (&batched[row].log_variance, &single.log_variance),
            (&batched[row].occupancy_logits, &single.occupancy_logits),
        ];
        let mut row_identical = true;
        for (index, (left_head, right_head)) in heads.into_iter().enumerate() {
            for (left, right) in left_head.iter().zip(right_head.iter()) {
                if left.to_bits() != right.to_bits() {
                    row_identical = false;
                    head_mismatches[index] += 1;
                    worst_bits = worst_bits.max(left.to_bits().abs_diff(right.to_bits()));
                    worst_ulp = worst_ulp.max(ulp_distance(*left, *right));
                }
            }
        }
        if !row_identical {
            mismatched_rows += 1;
            bit_identical = false;
        }
    }
    let single_ms = single_started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        json!({
            "mode": "steps",
            "backend": backend,
            "checkpoint_id": runtime.checkpoint_id(),
            "rows": rows,
            "bit_identical": bit_identical,
            "mismatched_rows": mismatched_rows,
            "mean_mismatched_values": head_mismatches[0],
            "log_variance_mismatched_values": head_mismatches[1],
            "occupancy_logits_mismatched_values": head_mismatches[2],
            "worst_raw_bit_delta": worst_bits,
            "worst_ulp": worst_ulp,
            "batched_step_ms": batched_ms,
            "single_step_ms": single_ms,
        })
    );
    Ok(())
}

/// Distance between two finite floats in units of their own last place, or a
/// large sentinel when either side is non-finite or the signs differ.
fn ulp_distance(left: f32, right: f32) -> u32 {
    if !left.is_finite() || !right.is_finite() || left.signum() != right.signum() {
        return u32::MAX;
    }
    left.to_bits().abs_diff(right.to_bits())
}
