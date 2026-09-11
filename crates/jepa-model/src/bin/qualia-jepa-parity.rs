//! Backend parity gate for a single immutable checkpoint.
//!
//! A report is published only as a brand-new file: an occupied destination is
//! refused, so no earlier artifact can be silently swapped out.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use qualia_jepa_model::parity::{compare_checkpoint_backends, ParityThresholds};
use serde::Serialize;

const USAGE: &str = "qualia-jepa-parity --checkpoint <candidate-dir> --target metal|cuda --output <new-report.json>";

struct Selection {
    checkpoint: Option<PathBuf>,
    target: Option<String>,
    output: Option<PathBuf>,
}

enum Invocation {
    Help,
    Compare(Selection),
}

fn main() {
    match execute() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("qualia-jepa-parity: {}", error);
            std::process::exit(1);
        }
    }
}

fn execute() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let selection = match scan_arguments()? {
        Invocation::Help => {
            println!("{}", USAGE);
            return Ok(());
        }
        Invocation::Compare(selection) => selection,
    };
    let checkpoint = selection.checkpoint.ok_or("--checkpoint is required")?;
    let target = selection.target.ok_or("--target is required")?;
    if target != "metal" && target != "cuda" {
        return Err("--target must be metal or cuda".into());
    }
    let destination = selection.output.ok_or("--output is required")?;
    let report = compare_checkpoint_backends(checkpoint, &target, ParityThresholds::default())?;
    publish(&destination, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.passes {
        Ok(())
    } else {
        Err("frozen CPU/accelerator parity gates failed".into())
    }
}

fn scan_arguments() -> Result<Invocation, Box<dyn std::error::Error + Send + Sync>> {
    let mut selection = Selection {
        checkpoint: None,
        target: None,
        output: None,
    };
    let mut argv = env::args().skip(1);
    loop {
        let Some(flag) = argv.next() else {
            return Ok(Invocation::Compare(selection));
        };
        match flag.as_str() {
            "--checkpoint" => selection.checkpoint = argv.next().map(PathBuf::from),
            "--target" => selection.target = argv.next(),
            "--output" => selection.output = argv.next().map(PathBuf::from),
            "--help" | "-h" => return Ok(Invocation::Help),
            unknown => return Err(format!("unknown argument {}", unknown).into()),
        }
    }
}

/// Publish `value` at a path that must not exist yet, via staging plus rename.
fn publish(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    if !directory.is_dir() || path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "output parent must exist and output must be new",
        ));
    }
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid output"))?;
    let staging = directory.join(format!(".{}.partial", stem));
    let mut handle = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    serde_json::to_writer_pretty(&mut handle, value)?;
    handle.write_all(b"\n")?;
    handle.sync_all()?;
    fs::rename(&staging, path)?;
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    Ok(())
}
