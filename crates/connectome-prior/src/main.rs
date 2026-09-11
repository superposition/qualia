//! The prior builder binary: read the Male CNS inputs, write the artifact.
//!
//! One line to stdout on success:
//! `prior: {type_count} types, {edge_count} edges -> {dir}`. Every
//! [`PriorError`] — including `AlreadyBuilt` — exits non-zero, because the
//! ticket's contract is "non-zero exit on `PriorError`".

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use qualia_connectome_prior::{
    build_type_graph, write_prior, Attribution, PriorError, PriorManifest, PriorSource,
};

/// Build the type-level Male CNS connectome prior.
#[derive(Debug, Parser)]
#[command(
    name = "qualia-connectome-prior",
    version,
    about = "Aggregate Male CNS segment edges to a type-level CSR prior"
)]
struct Args {
    /// Segment-level weights, Arrow IPC (Feather): body_pre, body_post, weight.
    #[arg(long)]
    edges: PathBuf,
    /// Body annotations, Arrow IPC (Feather): bodyId, type, superclass, side, dimorphism.
    #[arg(long)]
    annotations: PathBuf,
    /// Output directory for graph.bin, manifest.json and attribution.json.
    #[arg(long)]
    out: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(manifest) => {
            println!(
                "prior: {} types, {} edges -> {}",
                manifest.type_count,
                manifest.edge_count,
                args.out.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("prior: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<PriorManifest, PriorError> {
    let source = PriorSource {
        edges_feather: args.edges.clone(),
        annotations_feather: args.annotations.clone(),
    };
    let graph = build_type_graph(&source)?;
    write_prior(&args.out, &graph, &Attribution::male_cns())
}
