//! `qualia-gates` — the repository's gate layer, in Rust.
//!
//! Each gate is a subcommand that replaces one `scripts/*.py` invocation and
//! keeps its predicates, thresholds, summary line and exit codes:
//!
//! * `qualia-gates provenance` — the clean-room gate every ticket runs (D-001);
//! * `qualia-gates figures` — the figure convention gate (step 38);
//! * `qualia-gates journal` — the journal publish gate (step 37b);
//! * `qualia-gates mission` — step 29's braid-terminal and memory assertions.
//!
//! Exit codes are the scripts': 0 holds, 1 a gate failed, 2 a usage error or a
//! leg that cannot be judged. Nothing here needs an interpreter, so the gates
//! run wherever the stack runs.

mod figures;
mod git;
mod journal;
mod mission;
mod provenance;
mod pytext;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "qualia-gates",
    about = "The repository's gates, in Rust (provenance, figures, journal, mission)",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Gate,
}

#[derive(Subcommand)]
enum Gate {
    /// The clean-room provenance gate (D-001)
    Provenance(provenance::ProvenanceArgs),
    /// The figure convention gate (step 38)
    Figures(figures::FiguresArgs),
    /// The journal publish gate (step 37b)
    Journal(journal::JournalArgs),
    /// Step 29's braid-terminal and memory assertions
    Mission(mission::MissionArgs),
}

fn main() {
    let code = match Cli::parse().command {
        Gate::Provenance(args) => provenance::run(&args),
        Gate::Figures(args) => figures::run(&args),
        Gate::Journal(args) => journal::run(&args),
        Gate::Mission(args) => mission::run(&args),
    };
    std::process::exit(code);
}
