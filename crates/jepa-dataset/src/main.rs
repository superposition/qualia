//! `qualia-jepa-dataset`: build an immutable, audited JEPA dataset manifest.
//!
//! Reads a catalog of sealed session evidence, builds the transition manifest,
//! writes it under the output directory, and then enforces the promotion gate
//! so a catalog that cannot train is a non-zero exit rather than a silent one.

use qualia_jepa_dataset::{
    build_manifest, validate_dataset_promotion_gate, write_immutable_manifest, DatasetConfig,
    SessionEvidence,
};
use serde::Deserialize;
use std::path::PathBuf;

const DEFAULT_OUTPUT_DIR: &str = "artifacts/jepa/datasets";

#[derive(Deserialize)]
struct DatasetCatalog {
    #[serde(default)]
    config: Option<DatasetConfig>,
    sources: Vec<SessionEvidence>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qualia-jepa-dataset: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args_os().skip(1);
    let mut catalog = None;
    let mut output = PathBuf::from(DEFAULT_OUTPUT_DIR);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--catalog") => catalog = args.next().map(PathBuf::from),
            Some("--output-dir") => {
                output = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--output-dir requires a path")?;
            }
            Some("--help" | "-h") => {
                println!(
                    "usage: qualia-jepa-dataset --catalog <catalog.json> [--output-dir <dir>]"
                );
                return Ok(());
            }
            _ => return Err("unknown or incomplete argument".into()),
        }
    }
    let catalog_path = catalog.ok_or("--catalog is required")?;
    let catalog: DatasetCatalog = serde_json::from_slice(&std::fs::read(catalog_path)?)?;
    let manifest = build_manifest(catalog.sources, catalog.config.unwrap_or_default())?;
    let path = write_immutable_manifest(output, &manifest)?;
    println!(
        "{}\nvalid={} candidates={} sessions={} environments={} conditions={}",
        path.display(),
        manifest.audit.valid_transitions,
        manifest.audit.candidate_transitions,
        manifest.audit.sessions,
        manifest.audit.environments,
        manifest.audit.conditions.len(),
    );
    validate_dataset_promotion_gate(&manifest)?;
    Ok(())
}
