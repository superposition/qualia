//! `qualia-cuda-smoke`: prove that this machine's CUDA toolchain and device
//! really work, end to end.
//!
//! The runner looks for `nvcc`, compiles a four-element kernel, executes what
//! the compiler produced, and prints a one-line JSON report on stdout. The exit
//! status is zero exactly when the report's `status` is `ok`, so a supervisor
//! can read the proof from the report and the verdict from the status.

use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// The report `status` when the kernel compiled and executed.
const STATUS_OK: &str = "ok";
/// The report `status` when the host has no compiler to try.
const STATUS_BLOCKED: &str = "blocked";
/// The report `status` when the runner could not carry out a step.
const STATUS_ERROR: &str = "error";
/// The report `status` when `nvcc` refused the source.
const STATUS_COMPILE_FAILED: &str = "compile_failed";
/// The report `status` when the compiled kernel failed on the device.
const STATUS_RUN_FAILED: &str = "run_failed";

/// Name of the source the runner writes into its scratch directory.
const SOURCE_FILE: &str = "smoke.cu";
/// Stem of the compiled kernel, suffixed with `.exe` on Windows.
const BINARY_STEM: &str = "smoke";

/// The JSON report printed on stdout; the field names are the wire contract.
#[derive(Serialize)]
struct Report {
    status: String,
    reason: String,
    compiler: Option<String>,
    binary_path: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
}

/// Produces the scratch directory a smoke is built in.
type ScratchDir = dyn Fn() -> io::Result<PathBuf>;

/// Compiles `source` into `exe` inside `workdir` using `nvcc`.
type Compile = dyn Fn(&Path, &Path, &Path, &Path) -> io::Result<Output>;

/// The `nvcc` install location the runner falls back to on Windows.
#[cfg(windows)]
const WINDOWS_DEFAULT_NVCC: &str =
    r"C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8\bin\nvcc.exe";

/// The Visual Studio developer command bootstrap `nvcc` needs on Windows.
#[cfg(windows)]
const WINDOWS_VS_DEV_CMD: &str =
    r"C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat";

fn main() {
    let report = smoke();
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("the report serialises")
    );
    if report.status != STATUS_OK {
        std::process::exit(1);
    }
}

/// Runs the smoke with the compiler this host offers.
fn smoke() -> Report {
    smoke_with(find_nvcc(), &make_scratch_dir, &compile_smoke)
}

/// The smoke's decision table. The steps that touch the machine are injected so
/// every branch is reachable in a test without a CUDA toolchain.
fn smoke_with(compiler: Option<PathBuf>, scratch: &ScratchDir, compile: &Compile) -> Report {
    let nvcc = match compiler {
        Some(path) => path,
        None => return Report::new(STATUS_BLOCKED, "nvcc not found".to_string()),
    };
    let compiler_text = nvcc.display().to_string();

    let workdir = match scratch() {
        Ok(dir) => dir,
        Err(e) => {
            return Report::new(STATUS_ERROR, format!("failed to create temp dir: {e}"))
                .with_compiler(&compiler_text);
        }
    };
    let source = workdir.join(SOURCE_FILE);
    let exe = workdir.join(binary_name(BINARY_STEM));
    let binary_text = exe.display().to_string();

    if let Err(e) = fs::write(&source, smoke_source()) {
        return Report::new(STATUS_ERROR, format!("failed to write source: {e}"))
            .with_compiler(&compiler_text)
            .with_binary(&binary_text);
    }

    let compilation = match compile(&nvcc, &source, &exe, &workdir) {
        Ok(output) => output,
        Err(e) => {
            return Report::new(STATUS_ERROR, format!("failed to invoke compiler: {e}"))
                .with_compiler(&compiler_text)
                .with_binary(&binary_text);
        }
    };
    if !compilation.status.success() {
        return Report::new(STATUS_COMPILE_FAILED, "nvcc compile step failed".to_string())
            .with_compiler(&compiler_text)
            .with_binary(&binary_text)
            .with_output(&compilation);
    }

    match Command::new(&exe).output() {
        Ok(output) => {
            let report = if output.status.success() {
                Report::new(
                    STATUS_OK,
                    "CUDA smoke kernel compiled and executed successfully".to_string(),
                )
            } else {
                Report::new(
                    STATUS_RUN_FAILED,
                    "compiled smoke binary failed at runtime".to_string(),
                )
            };
            report
                .with_compiler(&compiler_text)
                .with_binary(&binary_text)
                .with_output(&output)
        }
        Err(e) => Report::new(STATUS_ERROR, format!("failed to launch smoke binary: {e}"))
            .with_compiler(&compiler_text)
            .with_binary(&binary_text),
    }
}

impl Report {
    /// The report of a status reached before any step ran.
    fn new(status: &str, reason: String) -> Self {
        Report {
            status: status.to_string(),
            reason,
            compiler: None,
            binary_path: None,
            stdout: None,
            stderr: None,
        }
    }

    fn with_compiler(mut self, compiler: &str) -> Self {
        self.compiler = Some(compiler.to_string());
        self
    }

    fn with_binary(mut self, binary: &str) -> Self {
        self.binary_path = Some(binary.to_string());
        self
    }

    fn with_output(mut self, output: &Output) -> Self {
        self.stdout = Some(trimmed(&output.stdout));
        self.stderr = Some(trimmed(&output.stderr));
        self
    }
}

/// The first `nvcc` this host offers: the `CUDA_PATH` installation, the
/// standard Windows install location, then whatever the PATH resolves.
fn find_nvcc() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("CUDA_PATH") {
        let candidate = PathBuf::from(root).join("bin").join(binary_name("nvcc"));
        if candidate.exists() {
            return Some(candidate);
        }
    }

    if let Some(installed) = standard_install() {
        return Some(installed);
    }

    let locator = if cfg!(windows) { "where.exe" } else { "which" };
    let located = Command::new(locator).arg("nvcc").output().ok()?;
    if !located.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&located.stdout);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(PathBuf::from(first))
    }
}

/// The toolkit location a Windows host installs by default.
#[cfg(windows)]
fn standard_install() -> Option<PathBuf> {
    let path = Path::new(WINDOWS_DEFAULT_NVCC);
    path.exists().then(|| path.to_path_buf())
}

/// No host-wide install location outside Windows.
#[cfg(not(windows))]
fn standard_install() -> Option<PathBuf> {
    None
}

/// A fresh directory under the system temp root. The compiled kernel stays in
/// it so the report can name the artifact it ran.
fn make_scratch_dir() -> io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = std::env::temp_dir().join(format!("qualia_cuda_smoke_{stamp}"));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The kernel executable's name on this platform.
fn binary_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Compiles the smoke source.
#[cfg(windows)]
fn compile_smoke(nvcc: &Path, source: &Path, exe: &Path, workdir: &Path) -> io::Result<Output> {
    let bootstrap = visual_studio_bootstrap()
        .ok_or_else(|| io::Error::other("Visual Studio developer command bootstrap not found"))?;

    // `nvcc` drives the MSVC toolchain, which is only on PATH after the
    // bootstrap script has run, so the compile goes through a batch file.
    let body = [
        "@echo off".to_string(),
        format!(
            "call \"{}\" -arch=x64 -host_arch=x64 >nul",
            bootstrap.display()
        ),
        "where.exe cl".to_string(),
        format!(
            "\"{}\" \"{}\" -o \"{}\"",
            nvcc.display(),
            source.display(),
            exe.display()
        ),
    ]
    .join("\r\n")
        + "\r\n";
    let script = workdir.join("build_smoke.bat");
    fs::write(&script, body)?;
    Command::new("cmd").arg("/c").arg(&script).output()
}

/// Compiles the smoke source.
#[cfg(not(windows))]
fn compile_smoke(nvcc: &Path, source: &Path, exe: &Path, _workdir: &Path) -> io::Result<Output> {
    Command::new(nvcc).arg(source).arg("-o").arg(exe).output()
}

/// The Visual Studio developer command bootstrap, when it is installed.
#[cfg(windows)]
fn visual_studio_bootstrap() -> Option<PathBuf> {
    let path = Path::new(WINDOWS_VS_DEV_CMD);
    path.exists().then(|| path.to_path_buf())
}

fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// The kernel the smoke compiles: each of four threads bumps its own slot, so a
/// working device prints one line with the slots incremented by one.
fn smoke_source() -> &'static str {
    r#"#include <cstdio>
#include <cuda_runtime.h>

__global__ void increment(float *values) {
    const int slot = threadIdx.x;
    if (slot < 4) {
        values[slot] += 1.0f;
    }
}

int main() {
    float host[4] = {1.0f, 2.0f, 3.0f, 4.0f};
    float *device = nullptr;
    if (cudaMalloc(&device, sizeof(host)) != cudaSuccess) {
        return 1;
    }
    if (cudaMemcpy(device, host, sizeof(host), cudaMemcpyHostToDevice) != cudaSuccess) {
        cudaFree(device);
        return 2;
    }
    increment<<<1, 4>>>(device);
    if (cudaDeviceSynchronize() != cudaSuccess) {
        cudaFree(device);
        return 3;
    }
    if (cudaMemcpy(host, device, sizeof(host), cudaMemcpyDeviceToHost) != cudaSuccess) {
        cudaFree(device);
        return 4;
    }
    cudaFree(device);
    std::printf("[%.1f,%.1f,%.1f,%.1f]\n", host[0], host[1], host[2], host[3]);
    return 0;
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to one test, cleared first so a stale
    /// directory from an earlier run cannot mask a failure.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qualia_cuda_smoke_test_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test scratch directory");
        dir
    }

    /// Path a test uses as the discovered compiler.
    fn fake_nvcc(dir: &Path) -> PathBuf {
        dir.join(binary_name("nvcc"))
    }

    /// A real subprocess carrying the exit status and streams a compiler would
    /// have produced, so the report is driven by a genuine `Output`.
    fn stub_output(dir: &Path, code: i32, out: &str, err: &str) -> Output {
        #[cfg(windows)]
        {
            let mut body = String::from("@echo off\r\n");
            if !out.is_empty() {
                body.push_str(&format!("echo {out}\r\n"));
            }
            if !err.is_empty() {
                body.push_str(&format!(">&2 echo {err}\r\n"));
            }
            body.push_str(&format!("exit /b {code}\r\n"));
            let script = dir.join("stub.bat");
            fs::write(&script, body).expect("write stub script");
            Command::new("cmd")
                .arg("/c")
                .arg(&script)
                .output()
                .expect("run stub script")
        }
        #[cfg(not(windows))]
        {
            let mut body = String::from("#!/bin/sh\n");
            if !out.is_empty() {
                body.push_str(&format!("printf '%s\\n' '{out}'\n"));
            }
            if !err.is_empty() {
                body.push_str(&format!("printf '%s\\n' '{err}' >&2\n"));
            }
            body.push_str(&format!("exit {code}\n"));
            let script = dir.join("stub.sh");
            fs::write(&script, body).expect("write stub script");
            Command::new("sh")
                .arg(&script)
                .output()
                .expect("run stub script")
        }
    }

    /// What the compile step leaves behind for the run step.
    #[derive(Clone, Copy)]
    enum Installed {
        /// Exits zero, so the run step should report `ok`.
        Succeeds,
        /// Exits non-zero with a message on stderr, so the run step should
        /// report `run_failed`.
        Fails,
    }

    #[cfg(windows)]
    fn install_program(exe: &Path, kind: Installed) -> io::Result<()> {
        let source = match kind {
            Installed::Succeeds => std::env::var_os("ComSpec").map(PathBuf::from),
            Installed::Fails => Some(PathBuf::from(r"C:\Windows\System32\findstr.exe")),
        };
        let source = source.ok_or_else(|| io::Error::other("system program unavailable"))?;
        fs::copy(source, exe)?;
        Ok(())
    }

    #[cfg(unix)]
    fn install_program(exe: &Path, kind: Installed) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let body = match kind {
            Installed::Succeeds => "printf '[2.0,3.0,4.0,5.0]\\n'\n",
            Installed::Fails => "printf 'device fault\\n' >&2\nexit 7\n",
        };
        fs::write(exe, format!("#!/bin/sh\n{body}"))?;
        fs::set_permissions(exe, fs::Permissions::from_mode(0o755))
    }

    /// What a run of the installed program observes, taken by running it here.
    fn observed(exe: &Path) -> (bool, String, String) {
        let output = Command::new(exe).output().expect("run installed program");
        (
            output.status.success(),
            trimmed(&output.stdout),
            trimmed(&output.stderr),
        )
    }

    #[test]
    fn a_host_without_a_compiler_is_blocked_and_does_no_work() {
        let compile = |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            panic!("a blocked smoke must not compile")
        };
        let scratch_dir = || -> io::Result<PathBuf> { panic!("a blocked smoke must not make scratch") };

        let report = smoke_with(None, &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_BLOCKED);
        assert_eq!(report.reason, "nvcc not found");
        assert_eq!(report.compiler, None);
        assert_eq!(report.binary_path, None);
        assert_eq!(report.stdout, None);
        assert_eq!(report.stderr, None);
    }

    #[test]
    fn a_scratch_directory_that_cannot_be_made_is_an_error() {
        let dir = scratch("no_scratch");
        let nvcc = fake_nvcc(&dir);
        let compile = |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            panic!("the compiler must not run without scratch space")
        };
        let scratch_dir = || -> io::Result<PathBuf> { Err(io::Error::other("no scratch space")) };

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_ERROR);
        assert_eq!(report.reason, "failed to create temp dir: no scratch space");
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        assert_eq!(report.binary_path, None);
        assert_eq!(report.stdout, None);
        assert_eq!(report.stderr, None);
    }

    #[test]
    fn a_source_that_cannot_be_written_is_an_error() {
        let dir = scratch("unwritable_source");
        // A directory where the source belongs makes the write fail.
        fs::create_dir(dir.join(SOURCE_FILE)).expect("block the source path");
        let nvcc = fake_nvcc(&dir);
        let compile = |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            panic!("the compiler must not run when the source cannot be written")
        };
        let scratch_dir = fixed_scratch(&dir);

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_ERROR);
        assert!(
            report.reason.starts_with("failed to write source: "),
            "unexpected reason: {}",
            report.reason
        );
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        assert_eq!(report.binary_path.as_deref(), binary_path(&dir).to_str());
        assert_eq!(report.stdout, None);
        assert_eq!(report.stderr, None);
    }

    #[test]
    fn a_compiler_that_will_not_start_is_an_error() {
        let dir = scratch("compiler_spawn");
        let nvcc = fake_nvcc(&dir);
        let compile = |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            Err(io::Error::other("compiler vanished"))
        };
        let scratch_dir = fixed_scratch(&dir);

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_ERROR);
        assert_eq!(report.reason, "failed to invoke compiler: compiler vanished");
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        assert_eq!(report.binary_path.as_deref(), binary_path(&dir).to_str());
        assert_eq!(report.stdout, None);
        assert_eq!(report.stderr, None);
    }

    #[test]
    fn a_refused_source_publishes_the_compiler_streams() {
        let dir = scratch("compile_failed");
        let nvcc = fake_nvcc(&dir);
        let compile_dir = dir.clone();
        let compile = move |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            Ok(stub_output(&compile_dir, 3, "compile out", "compile err"))
        };
        let scratch_dir = fixed_scratch(&dir);

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_COMPILE_FAILED);
        assert_eq!(report.reason, "nvcc compile step failed");
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        assert_eq!(report.binary_path.as_deref(), binary_path(&dir).to_str());
        assert_eq!(report.stdout.as_deref(), Some("compile out"));
        assert_eq!(report.stderr.as_deref(), Some("compile err"));
        assert!(
            !dir.join(binary_name(BINARY_STEM)).exists(),
            "a refused compile must leave nothing to run"
        );
    }

    #[test]
    fn a_kernel_that_compiles_and_runs_is_ok() {
        let dir = scratch("ok");
        let nvcc = fake_nvcc(&dir);
        let compile_dir = dir.clone();
        let compile = move |_: &Path, _: &Path, exe: &Path, _: &Path| -> io::Result<Output> {
            install_program(exe, Installed::Succeeds)?;
            Ok(stub_output(&compile_dir, 0, "", ""))
        };
        let scratch_dir = fixed_scratch(&dir);

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_OK);
        assert_eq!(
            report.reason,
            "CUDA smoke kernel compiled and executed successfully"
        );
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        assert_eq!(report.binary_path.as_deref(), binary_path(&dir).to_str());
        let (success, out, err) = observed(&dir.join(binary_name(BINARY_STEM)));
        assert!(success, "the installed program is expected to succeed");
        assert_eq!(report.stdout.as_deref(), Some(out.as_str()));
        assert_eq!(report.stderr.as_deref(), Some(err.as_str()));
    }

    #[test]
    fn a_kernel_that_fails_on_the_device_is_reported_with_its_streams() {
        let dir = scratch("run_failed");
        let nvcc = fake_nvcc(&dir);
        let compile_dir = dir.clone();
        let compile = move |_: &Path, _: &Path, exe: &Path, _: &Path| -> io::Result<Output> {
            install_program(exe, Installed::Fails)?;
            Ok(stub_output(&compile_dir, 0, "", ""))
        };
        let scratch_dir = fixed_scratch(&dir);

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_RUN_FAILED);
        assert_eq!(report.reason, "compiled smoke binary failed at runtime");
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        let (success, out, err) = observed(&dir.join(binary_name(BINARY_STEM)));
        assert!(!success, "the installed program is expected to fail");
        assert_eq!(report.stdout.as_deref(), Some(out.as_str()));
        assert_eq!(report.stderr.as_deref(), Some(err.as_str()));
    }

    #[test]
    fn a_compile_that_leaves_no_binary_is_an_error() {
        let dir = scratch("no_binary");
        let nvcc = fake_nvcc(&dir);
        let compile_dir = dir.clone();
        let compile = move |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            Ok(stub_output(&compile_dir, 0, "", ""))
        };
        let scratch_dir = fixed_scratch(&dir);

        let report = smoke_with(Some(nvcc.clone()), &scratch_dir, &compile);

        assert_eq!(report.status, STATUS_ERROR);
        assert!(
            report.reason.starts_with("failed to launch smoke binary: "),
            "unexpected reason: {}",
            report.reason
        );
        assert_eq!(report.compiler.as_deref(), nvcc.to_str());
        assert_eq!(report.stdout, None);
        assert_eq!(report.stderr, None);
    }

    #[test]
    fn the_report_is_the_six_field_wire_object() {
        let compile = |_: &Path, _: &Path, _: &Path, _: &Path| -> io::Result<Output> {
            panic!("a blocked smoke must not compile")
        };
        let scratch_dir = || -> io::Result<PathBuf> { panic!("a blocked smoke must not make scratch") };

        let report = smoke_with(None, &scratch_dir, &compile);
        let value = serde_json::to_value(&report).expect("report serialises");
        let object = value.as_object().expect("report is a JSON object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["binary_path", "compiler", "reason", "status", "stderr", "stdout"]
        );
        assert_eq!(object["status"], "blocked");
        assert!(
            object["compiler"].is_null(),
            "an absent compiler is null, never omitted"
        );

        let pretty = serde_json::to_string_pretty(&report).expect("pretty report");
        assert!(pretty.contains('\n'), "the report is pretty-printed");
        let round_trip: serde_json::Value = serde_json::from_str(&pretty).expect("report reparses");
        assert_eq!(round_trip, value);
    }

    /// A scratch factory that hands out a directory made by the test.
    fn fixed_scratch(dir: &Path) -> impl Fn() -> io::Result<PathBuf> {
        let dir = dir.to_path_buf();
        move || -> io::Result<PathBuf> { Ok(dir.clone()) }
    }

    /// Path the report is expected to name as the compiled kernel.
    fn binary_path(dir: &Path) -> PathBuf {
        dir.join(binary_name(BINARY_STEM))
    }
}
