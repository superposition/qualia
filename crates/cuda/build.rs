//! Build script: optional build-time fatbins for the CUDA kernels.
//!
//! A plain `cargo build -p qualia-cuda --features cuda` needs no CUDA toolkit.
//! The kernels under `kernels/` are embedded as source and compiled by NVRTC
//! when the backend starts, so the same sources reach the device on whatever
//! toolkit the target carries.
//!
//! Setting `CUDAARCHS` asks for real device code instead. Every kernel is then
//! compiled by `nvcc` into one fatbin carrying each requested architecture, and
//! the backend loads that image in preference to NVRTC. This is what a target
//! whose driver predates the toolkit's PTX ISA version needs: NVRTC emits only
//! PTX, and a PTX ISA version newer than the driver understands cannot be JITed,
//! while a real cubin for the device's own `sm_` loads as-is.
//!
//! The variable's spelling is the CUDA CMake one, and the Makefile's
//! `cuda-fatbin` targets set it:
//!
//! ```text
//! CUDAARCHS=87-real;89-real   sm_87 and sm_89 cubins (Orin, 4090)
//! CUDAARCHS=87-virtual        compute_87 PTX only
//! CUDAARCHS=87                both the real and the virtual target
//! ```
//!
//! `NVCC` names the compiler (default `nvcc`); `NVCC_CCBIN` is an optional
//! `-ccbin` directory, which nvcc needs on Windows to find `cl.exe` when the
//! Visual Studio environment is not already loaded. Without `CUDAARCHS` the
//! script writes empty placeholders so the `include_bytes!` sites in the crate
//! stay valid and the run-time path is NVRTC.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every kernel file the runtime loads, by module name.
const KERNELS: [&str; 8] = [
    "action_score",
    "belief_couple",
    "belief_update",
    "cognition_update",
    "costmap_stats",
    "evidence_layout",
    "perception_voxel",
    "smoke",
];

fn main() {
    println!("cargo:rerun-if-env-changed=CUDAARCHS");
    println!("cargo:rerun-if-env-changed=NVCC");
    println!("cargo:rerun-if-env-changed=NVCC_CCBIN");

    // The build script's working directory is the package, not the workspace
    // root; the kernels live at the root (D-006).
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let kernels = manifest_dir.join("../../kernels");
    for kernel in KERNELS {
        println!(
            "cargo:rerun-if-changed={}",
            kernels.join(format!("{kernel}.cu")).display()
        );
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let architectures = parse_architectures(&env::var("CUDAARCHS").unwrap_or_default());
    let fatbin = env::var_os("CARGO_FEATURE_CUDA").is_some() && !architectures.is_empty();

    for kernel in KERNELS {
        let output = out_dir.join(format!("{kernel}.fatbin"));
        if fatbin {
            compile(&kernels, kernel, &architectures, &output);
        } else {
            // The empty file is the signal the run-time code reads: no fatbin.
            fs::write(&output, []).expect("write the fatbin placeholder");
        }
    }

    if fatbin {
        println!(
            "cargo:warning=qualia-cuda: embedded fatbins for {}",
            architectures
                .iter()
                .map(|target| target.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// One `-gencode` pair: the front-end architecture and the code it emits.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArchitectureTarget {
    compute: String,
    code: String,
    label: String,
}

/// Parses `CUDAARCHS` (`87-real;89-real`) into nvcc `-gencode` pairs.
///
/// An entry that cannot be understood is a hard error: silently dropping a
/// requested architecture would ship a binary that cannot load on the device
/// the caller asked for.
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

/// `87` and `8.7` both name compute capability 8.7.
fn normalized(version: &str, entry: &str) -> String {
    let digits: String = version.chars().filter(char::is_ascii_digit).collect();
    assert!(
        !digits.is_empty(),
        "CUDAARCHS entry {entry:?} names no compute capability"
    );
    digits
}

/// Compiles one kernel into `output`, which the crate embeds.
fn compile(kernels: &Path, kernel: &str, architectures: &[ArchitectureTarget], output: &Path) {
    let source = kernels.join(format!("{kernel}.cu"));
    let nvcc = env::var("NVCC").unwrap_or_else(|_| "nvcc".to_string());
    let mut command = Command::new(&nvcc);
    command.arg("-fatbin");
    for target in architectures {
        command.arg("-gencode").arg(format!("arch={},code={}", target.compute, target.code));
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
        "cargo:warning=qualia-cuda: {} -> {} ({} bytes)",
        source.display(),
        output.display(),
        fs::metadata(output).map(|m| m.len()).unwrap_or(0)
    );
}

/// The host compiler directory to hand nvcc, if one is known.
///
/// On Windows nvcc cannot find `cl.exe` unless the Visual Studio environment is
/// already loaded, so the standard installation is searched for it; on other
/// hosts nvcc finds the system C++ compiler on its own.
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
