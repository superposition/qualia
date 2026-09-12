//! Build script: optional build-time fatbin for the LIF kernel.
//!
//! A plain `cargo build -p qualia-connectome-cns` needs no CUDA toolkit: the
//! kernel is embedded as source and compiled by NVRTC when the runner starts.
//! Setting `CUDAARCHS=87-real` asks `nvcc` for real device code instead, which
//! is what the Orin wants when the toolkit's PTX ISA is newer than the driver
//! understands. Without `CUDAARCHS` the script writes an empty placeholder so
//! the `include_bytes!` site stays valid.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The one kernel this crate carries.
const KERNEL: &str = "cns_lif";

fn main() {
    println!("cargo:rerun-if-env-changed=CUDAARCHS");
    println!("cargo:rerun-if-env-changed=NVCC");
    println!("cargo:rerun-if-env-changed=NVCC_CCBIN");

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let kernels = manifest_dir.join("../../kernels");
    println!("cargo:rerun-if-changed={}", kernels.join(format!("{KERNEL}.cu")).display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let architectures = parse_architectures(&env::var("CUDAARCHS").unwrap_or_default());
    let output = out_dir.join(format!("{KERNEL}.fatbin"));
    if architectures.is_empty() {
        fs::write(&output, []).expect("write the fatbin placeholder");
    } else {
        compile(&kernels, &architectures, &output);
    }
}

/// One `-gencode` pair: the front-end architecture and the code it emits.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArchitectureTarget {
    compute: String,
    code: String,
    label: String,
}

fn parse_architectures(raw: &str) -> Vec<ArchitectureTarget> {
    let mut targets = Vec::new();
    for entry in raw.split([';', ',']).map(str::trim).filter(|e| !e.is_empty()) {
        let (version, kind) = match entry.split_once('-') {
            Some((version, kind)) => (version.trim(), kind.trim()),
            None => (entry, "both"),
        };
        let capability = normalized(version, entry);
        let real = ArchitectureTarget {
            compute: format!("compute_{capability}"),
            code: format!("sm_{capability}"),
            label: format!("sm_{capability}"),
        };
        let virtual_target = ArchitectureTarget {
            compute: format!("compute_{capability}"),
            code: format!("compute_{capability}"),
            label: format!("compute_{capability}"),
        };
        match kind {
            "real" => targets.push(real),
            "virtual" => targets.push(virtual_target),
            "both" => {
                targets.push(real);
                targets.push(virtual_target);
            }
            other => panic!("CUDAARCHS entry {entry:?} has unknown kind {other:?}"),
        }
    }
    targets
}

fn normalized(version: &str, entry: &str) -> String {
    let digits: String = version.chars().filter(char::is_ascii_digit).collect();
    assert!(
        !digits.is_empty(),
        "CUDAARCHS entry {entry:?} names no compute capability"
    );
    digits
}

fn compile(kernels: &Path, architectures: &[ArchitectureTarget], output: &Path) {
    let source = kernels.join(format!("{KERNEL}.cu"));
    let nvcc = env::var("NVCC").unwrap_or_else(|_| "nvcc".to_string());
    let mut command = Command::new(&nvcc);
    command.arg("-fatbin");
    for target in architectures {
        command
            .arg("-gencode")
            .arg(format!("arch={},code={}", target.compute, target.code));
    }
    if let Some(ccbin) = ccbin_directory() {
        command.arg("-ccbin").arg(ccbin);
    }
    command.arg("-o").arg(output).arg(&source);

    let result = command
        .output()
        .unwrap_or_else(|error| panic!("cannot run {nvcc:?}: {error}"));
    if !result.status.success() {
        panic!(
            "{nvcc} failed for {} ({}):\n{}",
            source.display(),
            result.status,
            String::from_utf8_lossy(&result.stderr)
        );
    }
    println!(
        "cargo:warning=qualia-connectome-cns: {} -> {} ({} bytes)",
        source.display(),
        output.display(),
        fs::metadata(output).map(|m| m.len()).unwrap_or(0)
    );
}

/// The host compiler directory to hand nvcc, if one is known.
fn ccbin_directory() -> Option<PathBuf> {
    if let Ok(explicit) = env::var("NVCC_CCBIN") {
        if !explicit.trim().is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    if !cfg!(windows) {
        return None;
    }
    let program_files = env::var_os("ProgramFiles")?;
    let visual_studio = PathBuf::from(program_files).join("Microsoft Visual Studio");
    for edition in ["2022", "2019", "2017"] {
        for product in ["BuildTools", "Community", "Professional", "Enterprise", "Preview"] {
            let tools = visual_studio.join(edition).join(product).join("VC/Tools/MSVC");
            let Ok(versions) = fs::read_dir(&tools) else {
                continue;
            };
            let mut candidates: Vec<PathBuf> = versions
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("bin/Hostx64/x64/cl.exe"))
                .filter(|cl| cl.is_file())
                .collect();
            candidates.sort();
            if let Some(cl) = candidates.pop() {
                return cl.parent().map(Path::to_path_buf);
            }
        }
    }
    None
}
