//! The process contract of `qualia-cuda-smoke`, exercised through the binary.
//!
//! The unit tests drive the report's decision table with injected steps. These
//! run the real process, whatever the host's CUDA state is: one JSON object on
//! stdout, nothing on stderr, and a zero exit status exactly when the report
//! says `ok`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const BINARY: &str = env!("CARGO_BIN_EXE_qualia-cuda-smoke");

/// A directory unique to one test run, cleared first so a stale directory from
/// an earlier run cannot mask a failure.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qualia_cuda_smoke_cli_{}_{}",
        std::process::id(),
        tag
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

/// Runs the binary, optionally pointing `CUDA_PATH` at a stand-in install.
fn run(cuda_path: Option<&Path>) -> Output {
    let mut command = Command::new(BINARY);
    match cuda_path {
        Some(path) => {
            command.env("CUDA_PATH", path);
        }
        None => {
            command.env_remove("CUDA_PATH");
        }
    }
    command.output().expect("run the smoke binary")
}

fn report_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON report: {e}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// A file that exists but is not a CUDA compiler.
fn install_non_compiler(path: &Path) {
    #[cfg(windows)]
    {
        fs::copy(r"C:\Windows\System32\findstr.exe", path).expect("copy the stand-in compiler");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, "#!/bin/sh\nexit 1\n").expect("write the stand-in compiler");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make it runnable");
    }
}

const REPORT_FIELDS: [&str; 6] = [
    "binary_path",
    "compiler",
    "reason",
    "status",
    "stderr",
    "stdout",
];

#[test]
fn the_report_and_the_exit_status_describe_the_host() {
    let output = run(None);
    assert!(
        output.stderr.is_empty(),
        "the runner reports on stdout only: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report = report_of(&output);
    let object = report.as_object().expect("one JSON object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, REPORT_FIELDS, "report fields in {report}");

    let status = report["status"].as_str().expect("status is a string");
    assert!(
        matches!(
            status,
            "ok" | "blocked" | "error" | "compile_failed" | "run_failed"
        ),
        "unknown status {status} in {report}"
    );
    let expected = if status == "ok" { 0 } else { 1 };
    assert_eq!(
        output.status.code(),
        Some(expected),
        "status {status} in {report}, exit {:?}",
        output.status.code()
    );

    if status == "blocked" {
        assert_eq!(report["reason"], "nvcc not found");
    }
    if status == "ok" {
        let printed = report["stdout"]
            .as_str()
            .expect("an ok report carries what the kernel printed");
        assert!(
            printed.contains("[2.0,3.0,4.0,5.0]"),
            "the kernel printed {printed}"
        );
        assert!(
            report["compiler"].as_str().is_some(),
            "an ok report names its compiler"
        );
        let binary = report["binary_path"]
            .as_str()
            .expect("an ok report names its kernel");
        assert!(Path::new(binary).is_file(), "compiled kernel at {binary}");
    }
}

#[test]
fn a_cuda_path_override_is_the_compiler_the_report_names() {
    let dir = scratch("override");
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("create bin directory");
    let stand_in = bin.join(if cfg!(windows) { "nvcc.exe" } else { "nvcc" });
    install_non_compiler(&stand_in);

    let output = run(Some(&dir));
    assert!(
        output.stderr.is_empty(),
        "the runner reports on stdout only: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report = report_of(&output);
    assert_eq!(
        report["compiler"].as_str(),
        stand_in.to_str(),
        "the CUDA_PATH installation is the compiler the report names, in {report}"
    );
    assert_ne!(
        report["status"], "ok",
        "a stand-in cannot pass the smoke: {report}"
    );
    assert_eq!(output.status.code(), Some(1));
}
