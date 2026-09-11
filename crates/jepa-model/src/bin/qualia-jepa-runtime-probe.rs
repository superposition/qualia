//! Latency and finiteness probe for the frozen runtime fixture.
//!
//! One JSON object goes to stdout. No checkpoint is read: the fixture is a
//! pure function of the seed, so this doubles as an ABI and profiler smoke
//! test for the tiled runtime.

use qualia_jepa::OBS_DIM;
use qualia_jepa_model::device_for_backend;
use qualia_jepa_model::runtime::{CoherentJepaRuntime, RuntimeInput, RuntimeOutput, RUNTIME_ID};
use serde::Serialize;
use std::env;

const SCHEMA_VERSION: &str = "qualia.jepa-runtime-probe.v1";

#[derive(Serialize)]
struct ProbeReport {
    schema_version: &'static str,
    runtime_id: &'static str,
    backend: String,
    iterations: usize,
    warmup_iterations: usize,
    fixture_seed: u64,
    synchronized_latency_p50_us: u64,
    synchronized_latency_p95_us: u64,
    synchronized_latency_max_us: u64,
    outputs_finite: bool,
    output_dimensions: [usize; 5],
}

struct Options {
    backend: String,
    iterations: usize,
    warmup_iterations: usize,
    fixture_seed: u64,
}

enum Command {
    Help,
    Measure(Options),
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let options = match parse_command_line()? {
        Command::Help => {
            println!("qualia-jepa-runtime-probe --backend cpu|metal|cuda [--iterations N] [--warmup N] [--seed N]");
            return Ok(());
        }
        Command::Measure(options) => options,
    };
    if options.iterations == 0 {
        return Err("--iterations must be positive".into());
    }
    let report = measure(&options)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_command_line() -> Result<Command, Box<dyn std::error::Error + Send + Sync>> {
    let mut options = Options {
        backend: "cpu".to_string(),
        iterations: 10,
        warmup_iterations: 2,
        fixture_seed: 0xc0ffee,
    };
    let mut argv = env::args().skip(1);
    loop {
        let Some(flag) = argv.next() else {
            return Ok(Command::Measure(options));
        };
        match flag.as_str() {
            "--backend" => options.backend = value_of(&mut argv, "--backend requires a value")?,
            "--iterations" => {
                options.iterations = value_of(&mut argv, "--iterations requires a value")?.parse()?
            }
            "--warmup" => {
                options.warmup_iterations =
                    value_of(&mut argv, "--warmup requires a value")?.parse()?
            }
            "--seed" => {
                options.fixture_seed = value_of(&mut argv, "--seed requires a value")?.parse()?
            }
            "--help" | "-h" => return Ok(Command::Help),
            unknown => return Err(format!("unknown argument {}", unknown).into()),
        }
    }
}

fn value_of(
    argv: &mut impl Iterator<Item = String>,
    missing: &'static str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    match argv.next() {
        Some(value) => Ok(value),
        None => Err(missing.into()),
    }
}

fn measure(options: &Options) -> Result<ProbeReport, Box<dyn std::error::Error + Send + Sync>> {
    let device = device_for_backend(&options.backend)?;
    let runtime = CoherentJepaRuntime::frozen_fixture(device, options.fixture_seed)?;
    let input = fixture_input();
    let mut latencies: Vec<u64> = Vec::with_capacity(options.iterations);
    let mut outputs_finite = true;
    let mut output_dimensions = [0usize; 5];
    for _ in 0..options.warmup_iterations {
        outputs_finite &= runtime.infer(&input)?.is_finite();
    }
    for _ in 0..options.iterations {
        let output = runtime.infer(&input)?;
        outputs_finite &= output.is_finite();
        latencies.push(output.synchronized_latency_us);
        output_dimensions = widths(&output);
    }
    latencies.sort_unstable();
    Ok(ProbeReport {
        schema_version: SCHEMA_VERSION,
        runtime_id: RUNTIME_ID,
        backend: options.backend.clone(),
        iterations: options.iterations,
        warmup_iterations: options.warmup_iterations,
        fixture_seed: options.fixture_seed,
        synchronized_latency_p50_us: percentile(&latencies, 50),
        synchronized_latency_p95_us: percentile(&latencies, 95),
        synchronized_latency_max_us: latencies.last().copied().unwrap_or(0),
        outputs_finite,
        output_dimensions,
    })
}

fn fixture_input() -> RuntimeInput {
    RuntimeInput {
        observation: (0..OBS_DIM).map(observation_value).collect(),
        applied_action: [0.2, -0.1, 0.3, 0.05],
        delta_seconds: 0.125,
    }
}

fn observation_value(index: usize) -> f32 {
    ((index * 17 % 251) as f32 - 125.0) / 125.0
}

fn widths(output: &RuntimeOutput) -> [usize; 5] {
    [
        output.latent.len(),
        output.predicted_mean.len(),
        output.predicted_log_variance.len(),
        output.evidence.len(),
        output.occupancy_logits.len(),
    ]
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    let Some(last) = values.len().checked_sub(1) else {
        return 0;
    };
    let scaled = last * percentile;
    values[((scaled + 99) / 100).min(last)]
}
