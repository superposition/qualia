//! The `cuda-bench` runner: one bounded CUDA matmul benchmark, reported as JSON.
//!
//! The process asks the host's Python interpreter for a PyTorch measurement of
//! a single square matrix multiply on the default CUDA device, then prints what
//! it observed as one JSON object on stdout. Every device-facing step happens
//! in the child, so this crate stays a thin reporter with no CUDA dependency of
//! its own; a host without the interpreter or the device still produces a
//! report that says so.
//!
//! The work is bounded by two environment variables, so no caller can ask for
//! an open-ended run:
//!
//! * `QUALIA_CUDA_BENCH_SHAPE` — the side of the square operands, clamped to
//!   `256..=16384`; default `4096`.
//! * `QUALIA_CUDA_BENCH_ITERS` — the timed iterations, clamped to `1..=200`;
//!   default `10`.
//!
//! `status` is `ok` on a completed measurement and otherwise names what went
//! wrong (`blocked` with no interpreter, `parse_failed` when the child's stdout
//! is not JSON, `run_failed` when the child exits non-zero, `error` when the
//! interpreter cannot be launched). Any status but `ok` exits non-zero, so a
//! supervisor can gate on the process.

use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

const SHAPE_ENV: &str = "QUALIA_CUDA_BENCH_SHAPE";
const ITERS_ENV: &str = "QUALIA_CUDA_BENCH_ITERS";

const SHAPE_DEFAULT: u32 = 4096;
const SHAPE_MIN: u32 = 256;
const SHAPE_MAX: u32 = 16384;

const ITERS_DEFAULT: u32 = 10;
const ITERS_MIN: u32 = 1;
const ITERS_MAX: u32 = 200;

/// Warm-up matmuls issued before the timed ones, so the first measurement is
/// not paying for allocation, cuBLAS handle creation or JIT.
const WARMUP: u32 = 5;

const STATUS_OK: &str = "ok";
const STATUS_BLOCKED: &str = "blocked";
const STATUS_PARSE_FAILED: &str = "parse_failed";
const STATUS_RUN_FAILED: &str = "run_failed";
const STATUS_ERROR: &str = "error";

const REASON_OK: &str = "CUDA benchmark completed successfully";
const REASON_NO_PYTHON: &str = "python not found";
const REASON_RUN_FAILED: &str = "python benchmark process failed";

/// The report, printed as pretty JSON in this field order. `None` marks a value
/// the run never produced; it is emitted as JSON `null` rather than omitted, so
/// every report has the same keys.
#[derive(Serialize)]
struct Report {
    status: String,
    reason: String,
    device: Option<String>,
    shape: [u32; 2],
    iterations: u32,
    elapsed_s: Option<f64>,
    per_iter_ms: Option<f64>,
    max_mem_bytes: Option<u64>,
    result_mean: Option<f64>,
    stdout: Option<String>,
    stderr: Option<String>,
}

impl Report {
    /// A report carrying everything the runner knows before the child has said
    /// anything: the outcome, why, and the work that was requested.
    fn bare(status: &str, reason: String, shape: u32, iterations: u32) -> Self {
        Report {
            status: status.to_string(),
            reason,
            device: None,
            shape: [shape, shape],
            iterations,
            elapsed_s: None,
            per_iter_ms: None,
            max_mem_bytes: None,
            result_mean: None,
            stdout: None,
            stderr: None,
        }
    }

    /// Attach the child's streams, so a failure can be diagnosed from the
    /// report alone.
    fn with_streams(mut self, stdout: Option<String>, stderr: Option<String>) -> Self {
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }
}

fn main() {
    let shape = bounded(SHAPE_ENV, SHAPE_DEFAULT, SHAPE_MIN, SHAPE_MAX);
    let iterations = bounded(ITERS_ENV, ITERS_DEFAULT, ITERS_MIN, ITERS_MAX);

    let report = benchmark(shape, iterations);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("the report serializes")
    );

    if report.status != STATUS_OK {
        std::process::exit(1);
    }
}

/// The value `name` asks for, or `default` when it is unset or not a number;
/// the result never leaves `min..=max`.
fn bounded(name: &str, default: u32, min: u32, max: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

/// Locate the interpreter, run the device half in it, and turn its exit and
/// streams into a report.
fn benchmark(shape: u32, iterations: u32) -> Report {
    let python = match locate_interpreter() {
        Some(path) => path,
        None => return Report::bare(STATUS_BLOCKED, REASON_NO_PYTHON.to_string(), shape, iterations),
    };

    let script = torch_benchmark(shape, iterations);
    match Command::new(&python).args(["-c", &script]).output() {
        Ok(output) if output.status.success() => completed(shape, iterations, &output),
        Ok(output) => Report::bare(
            STATUS_RUN_FAILED,
            REASON_RUN_FAILED.to_string(),
            shape,
            iterations,
        )
        .with_streams(Some(trimmed(&output.stdout)), Some(trimmed(&output.stderr))),
        Err(error) => Report::bare(
            STATUS_ERROR,
            format!("failed to launch python benchmark: {error}"),
            shape,
            iterations,
        ),
    }
}

/// The child finished successfully, so its stdout must be the benchmark's JSON
/// object; each measured value is copied into the report under its wire name.
fn completed(shape: u32, iterations: u32, output: &Output) -> Report {
    let stdout = trimmed(&output.stdout);
    let stderr = trimmed(&output.stderr);
    let stderr = if stderr.is_empty() { None } else { Some(stderr) };

    match serde_json::from_str::<Value>(&stdout) {
        Ok(value) => Report {
            status: STATUS_OK.to_string(),
            reason: REASON_OK.to_string(),
            device: value.get("device").and_then(Value::as_str).map(str::to_string),
            shape: [shape, shape],
            iterations,
            elapsed_s: value.get("elapsed_s").and_then(Value::as_f64),
            per_iter_ms: value.get("per_iter_ms").and_then(Value::as_f64),
            max_mem_bytes: value.get("max_mem_bytes").and_then(Value::as_u64),
            result_mean: value.get("result_mean").and_then(Value::as_f64),
            stdout: Some(stdout),
            stderr,
        },
        Err(error) => Report::bare(
            STATUS_PARSE_FAILED,
            format!("benchmark output was not valid JSON: {error}"),
            shape,
            iterations,
        )
        .with_streams(Some(stdout), stderr),
    }
}

/// The first `python` (Windows) or `python3` (elsewhere) the platform's own
/// locator reports, or `None` when the locator cannot run or finds nothing.
fn locate_interpreter() -> Option<PathBuf> {
    let (program, wanted) = if cfg!(target_os = "windows") {
        ("where.exe", "python")
    } else {
        ("which", "python3")
    };

    let listing = Command::new(program).arg(wanted).output().ok()?;
    if !listing.status.success() {
        return None;
    }

    let listing_stdout = String::from_utf8_lossy(&listing.stdout);
    let first = listing_stdout.lines().next().map(str::trim).unwrap_or("");
    if first.is_empty() {
        None
    } else {
        Some(PathBuf::from(first))
    }
}

/// A stream as the report carries it: lossy UTF-8, with the child's trailing
/// newline removed.
fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// The program the child runs: check the device, warm it up, time `iterations`
/// matmuls, and print the measurements as one JSON object. Those keys are what
/// the report above reads, so they are part of the contract.
fn torch_benchmark(shape: u32, iterations: u32) -> String {
    format!(
        r#"import json, time
import torch

if not torch.cuda.is_available():
    raise SystemExit("torch CUDA unavailable")

torch.cuda.empty_cache()
torch.cuda.reset_peak_memory_stats()
left = torch.randn(({shape}, {shape}), device="cuda")
right = torch.randn(({shape}, {shape}), device="cuda")

for _ in range({warmup}):
    product = left @ right
    torch.cuda.synchronize()

started = time.perf_counter()
for _ in range({iterations}):
    product = left @ right
    torch.cuda.synchronize()
elapsed = time.perf_counter() - started

print(json.dumps({{
    "device": torch.cuda.get_device_name(0),
    "elapsed_s": round(elapsed, 6),
    "per_iter_ms": round(elapsed * 1000.0 / {iterations}, 4),
    "max_mem_bytes": int(torch.cuda.max_memory_allocated()),
    "result_mean": float(product.mean().item()),
}}))
"#,
        warmup = WARMUP,
    )
}
