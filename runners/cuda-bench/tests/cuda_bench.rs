//! Behavioural tests for the `cuda-bench` runner.
//!
//! A runner's whole contract is what an operator observes: the environment it
//! reads, the JSON it prints on stdout, and the exit status it leaves behind.
//! Every test here drives the built binary and asserts exactly those, so no
//! test reaches into the implementation.
//!
//! The device-facing half of a run is a Python child that imports `torch`.
//! Rather than need a GPU, each test puts a stand-in `torch` module first on
//! `PYTHONPATH`: the real interpreter still runs the runner's own program, so
//! the child branch is genuinely exercised, but the device module prints the
//! stdout, stderr and exit code the test chose. That makes every reachable
//! outcome — a completed benchmark, unparsable output, a failed child and no
//! interpreter at all — deterministic and GPU-free.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

/// The binary under test, placed in the target directory by cargo.
const BIN: &str = env!("CARGO_BIN_EXE_qualia-cuda-bench");

/// The stand-in device module. It is the whole of the fake `torch`: write the
/// requested streams, then leave with the requested code, so the test controls
/// what the runner's child reports without a GPU.
const FAKE_TORCH: &str = r#"import os
import sys

sys.stdout.write(os.environ.get("QUALIA_BENCH_STUB_STDOUT", ""))
sys.stdout.flush()
sys.stderr.write(os.environ.get("QUALIA_BENCH_STUB_STDERR", ""))
sys.stderr.flush()
os._exit(int(os.environ.get("QUALIA_BENCH_STUB_CODE", "0")))
"#;

/// A fresh per-test directory under the system temp root, so one test's
/// stand-in device can never be imported by another.
fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "qualia-cuda-bench-{label}-{}-{serial}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

/// Write the stand-in `torch` package into `dir` and return the `PYTHONPATH`
/// entry that shadows any real device binding.
fn fake_device(dir: &Path) -> PathBuf {
    let package = dir.join("torch");
    fs::create_dir_all(&package).expect("create stand-in torch package");
    fs::write(package.join("__init__.py"), FAKE_TORCH).expect("write stand-in torch module");
    dir.to_path_buf()
}

/// The path list that reaches the stand-in device first, spelled for the host.
fn python_path(dir: &Path) -> String {
    let mut parts = vec![dir.to_string_lossy().into_owned()];
    if let Ok(existing) = std::env::var("PYTHONPATH") {
        if !existing.is_empty() {
            parts.push(existing);
        }
    }
    parts.join(if cfg!(windows) { ";" } else { ":" })
}

/// The environment that steers the stand-in device: where to find it, what it
/// prints on each stream, and how it exits.
fn device_env(python_path: &str, stdout: &str, stderr: Option<&str>, code: i32) -> Vec<(String, String)> {
    let mut env = vec![
        ("PYTHONPATH".to_string(), python_path.to_string()),
        ("QUALIA_BENCH_STUB_STDOUT".to_string(), stdout.to_string()),
        ("QUALIA_BENCH_STUB_CODE".to_string(), code.to_string()),
    ];
    if let Some(line) = stderr {
        env.push(("QUALIA_BENCH_STUB_STDERR".to_string(), line.to_string()));
    }
    env
}

/// Run the binary with the two bounding variables cleared, so every case starts
/// from a known state, plus whatever `env` sets.
fn run(env: &[(String, String)]) -> Output {
    let mut command = Command::new(BIN);
    command
        .env_remove("QUALIA_CUDA_BENCH_SHAPE")
        .env_remove("QUALIA_CUDA_BENCH_ITERS");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run the cuda-bench binary")
}

/// Run the binary with `PATH` reduced to `dir`, which holds no interpreter, so
/// the runner cannot reach a child at all.
fn run_without_interpreter(dir: &Path, env: &[(String, String)]) -> Output {
    let mut command = Command::new(BIN);
    command
        .env("PATH", dir.to_string_lossy().as_ref())
        .env_remove("QUALIA_CUDA_BENCH_SHAPE")
        .env_remove("QUALIA_CUDA_BENCH_ITERS");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run the cuda-bench binary")
}

/// The report the binary printed, as a JSON object.
fn report(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not a JSON report: {error}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A child that reports a complete benchmark object: the run completes and each
/// measured value appears in the report under its wire name.
#[test]
fn a_completed_benchmark_is_reported_as_ok() {
    let dir = fake_device(&scratch("ok"));
    let payload = r#"{"device":"stub-gpu","elapsed_s":0.5,"per_iter_ms":0.25,"max_mem_bytes":1048576,"result_mean":3.5}"#;
    let env = device_env(&python_path(&dir), payload, None, 0);

    let output = run(
        &[
            env.clone(),
            vec![
                ("QUALIA_CUDA_BENCH_SHAPE".to_string(), "512".to_string()),
                ("QUALIA_CUDA_BENCH_ITERS".to_string(), "4".to_string()),
            ],
        ]
        .concat(),
    );
    let report = report(&output);

    assert_eq!(output.status.code(), Some(0), "report: {report}");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["reason"], "CUDA benchmark completed successfully");
    assert_eq!(report["shape"], json!([512, 512]));
    assert_eq!(report["iterations"], 4);
    assert_eq!(report["device"], "stub-gpu");
    assert_eq!(report["elapsed_s"], 0.5);
    assert_eq!(report["per_iter_ms"], 0.25);
    assert_eq!(report["max_mem_bytes"], 1048576);
    assert_eq!(report["result_mean"], 3.5);
    assert_eq!(report["stdout"], payload);
    assert_eq!(report["stderr"], Value::Null);
}

/// Output that is not JSON is a failure the operator can see, not a crash: the
/// raw bytes are kept so the misbehaving child can be diagnosed.
#[test]
fn unparsable_benchmark_output_is_reported_as_parse_failed() {
    let dir = fake_device(&scratch("parse"));
    let env = device_env(
        &python_path(&dir),
        "benchmark said nothing recognisable",
        None,
        0,
    );
    let env = [env, vec![("QUALIA_CUDA_BENCH_ITERS".to_string(), "3".to_string())]].concat();

    let output = run(&env);
    let report = report(&output);

    assert_eq!(output.status.code(), Some(1), "report: {report}");
    assert_eq!(report["status"], "parse_failed");
    let reason = report["reason"].as_str().expect("reason is a string");
    assert!(
        reason.starts_with("benchmark output was not valid JSON: "),
        "reason: {reason}"
    );
    assert_eq!(report["stdout"], "benchmark said nothing recognisable");
    assert_eq!(report["iterations"], 3);
    assert_eq!(report["device"], Value::Null);
    assert_eq!(report["elapsed_s"], Value::Null);
}

/// A child that exits non-zero is a failed run: both streams are surfaced, the
/// exit status is non-zero, and no measurement is invented.
#[test]
fn a_failed_benchmark_process_is_reported_as_run_failed() {
    let dir = fake_device(&scratch("run"));
    let env = device_env(
        &python_path(&dir),
        "half a report",
        Some("torch could not open the device"),
        3,
    );

    let output = run(&env);
    let report = report(&output);

    assert_eq!(output.status.code(), Some(1), "report: {report}");
    assert_eq!(report["status"], "run_failed");
    assert_eq!(report["reason"], "python benchmark process failed");
    assert_eq!(report["stdout"], "half a report");
    assert_eq!(report["stderr"], "torch could not open the device");
    assert_eq!(report["device"], Value::Null);
    assert_eq!(report["per_iter_ms"], Value::Null);
}

/// With no interpreter to run the device half, the runner says so and fails,
/// rather than reporting a measurement it never took.
#[test]
fn a_host_without_an_interpreter_is_reported_as_blocked() {
    let dir = scratch("blocked");

    let output = run_without_interpreter(&dir, &[]);
    let report = report(&output);

    assert_eq!(output.status.code(), Some(1), "report: {report}");
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["reason"], "python not found");
    assert_eq!(report["shape"], json!([4096, 4096]));
    assert_eq!(report["iterations"], 10);
    assert_eq!(report["device"], Value::Null);
    assert_eq!(report["elapsed_s"], Value::Null);
    assert_eq!(report["per_iter_ms"], Value::Null);
    assert_eq!(report["max_mem_bytes"], Value::Null);
    assert_eq!(report["result_mean"], Value::Null);
    assert_eq!(report["stdout"], Value::Null);
    assert_eq!(report["stderr"], Value::Null);
}

/// The work is bounded whatever the environment asks for: an override is
/// clamped to the crate's floor and ceiling, and a value that is not a number
/// falls back to the default. The empty `PATH` keeps the run GPU-free, since
/// the bounds are settled before any child could start.
#[test]
fn the_requested_work_is_bounded() {
    let dir = scratch("bounds");
    let shape = |value: &str| vec![("QUALIA_CUDA_BENCH_SHAPE".to_string(), value.to_string())];
    let iters = |value: &str| vec![("QUALIA_CUDA_BENCH_ITERS".to_string(), value.to_string())];

    let defaults = report(&run_without_interpreter(&dir, &[]));
    assert_eq!(defaults["shape"], json!([4096, 4096]));
    assert_eq!(defaults["iterations"], 10);

    let floors = report(&run_without_interpreter(
        &dir,
        &[shape("99"), iters("0")].concat(),
    ));
    assert_eq!(floors["shape"], json!([256, 256]));
    assert_eq!(floors["iterations"], 1);

    let ceilings = report(&run_without_interpreter(
        &dir,
        &[shape("999999"), iters("999999")].concat(),
    ));
    assert_eq!(ceilings["shape"], json!([16384, 16384]));
    assert_eq!(ceilings["iterations"], 200);

    let nonsense = report(&run_without_interpreter(
        &dir,
        &[shape("wide"), iters("")].concat(),
    ));
    assert_eq!(nonsense["shape"], json!([4096, 4096]));
    assert_eq!(nonsense["iterations"], 10);
}
