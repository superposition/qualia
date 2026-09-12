//! `qualia-connectome-view` — the desktop viewer entry point for the floating
//! brain.
//!
//! ```text
//! # replay a recorded run into a file, then open it:
//! qualia-connectome-view view --positions <artifact-dir> --spikes spikes.bin --out brain.rrd
//! rerun brain.rrd
//!
//! # live: a Rerun viewer serving on 9876, the runner's stream on the USB link:
//! rerun --serve-grpc                                   # once, on the desktop
//! qualia-connectome-view view --positions <artifact-dir> --live 192.168.55.1:7777 \
//!     --attach rerun+http://127.0.0.1:9876/proxy --speed 1
//! ```

use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand};
use qualia_connectome_view::{
    connect_live, load_cloud, run, verify, ViewOptions, ViewerSink,
};

#[derive(Parser)]
#[command(
    name = "qualia-connectome-view",
    about = "The floating brain: the connectome as a Rerun point cloud with each tick's firing set highlighted"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the cloud and a spike stream in a Rerun viewer.
    View(Box<ViewArgs>),
    /// Check a positions artifact against its manifest and print its counts.
    Verify {
        /// Positions artifact: its directory, or `positions.bin` inside it.
        #[arg(long)]
        positions: PathBuf,
    },
}

#[derive(clap::Args)]
#[command(group(ArgGroup::new("source").required(true).args(["spikes", "live"])))]
#[command(group(ArgGroup::new("sink").required(true).args(["out", "attach"])))]
struct ViewArgs {
    /// Positions artifact: its directory, or `positions.bin` inside it.
    #[arg(long)]
    positions: PathBuf,
    /// Recorded spike stream, `QLSP` frames.
    #[arg(long)]
    spikes: Option<PathBuf>,
    /// Live spike stream socket, `host:port`.
    #[arg(long)]
    live: Option<String>,
    /// Write the recording here; open it afterwards with `rerun <file>`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Attach to a running Rerun viewer or server, e.g.
    /// `rerun+http://127.0.0.1:9876/proxy`.
    #[arg(long)]
    attach: Option<String>,
    /// Stop after this many ticks.
    #[arg(long)]
    max_ticks: Option<u64>,
    /// Playback speed against the stream's own clock: `1` is real time, `0` is
    /// as fast as it reads.
    #[arg(long, default_value_t = 1.0)]
    speed: f64,
    /// Do not send the Brain blueprint.
    #[arg(long)]
    no_blueprint: bool,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::View(args) => view(*args),
        Command::Verify { positions } => verify_command(&positions),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn view(args: ViewArgs) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_cloud(&args.positions)?;
    println!(
        "artifact: nodes={} placed={} unplaced={} cell_types={} source={} space={} positions_sha256={}",
        loaded.node_count,
        loaded.neurons.len(),
        loaded.unplaced_count(),
        loaded.cell_types.len(),
        loaded.source,
        loaded.coordinate_space,
        loaded.bin_sha256
    );

    let sink = match (&args.out, &args.attach) {
        (Some(path), _) => ViewerSink::Save(path.clone()),
        (_, Some(url)) => ViewerSink::Attach(url.clone()),
        _ => unreachable!("clap requires one of --out or --attach"),
    };
    let options = ViewOptions {
        spikes: args.spikes.clone(),
        live: args.live.clone(),
        sink,
        max_ticks: args.max_ticks,
        speed: args.speed,
        blueprint: !args.no_blueprint,
    };

    let stats = match (&options.spikes, &options.live) {
        (Some(path), _) => {
            let file = std::fs::File::open(path)
                .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
            run(&loaded, std::io::BufReader::new(file), &options)?
        }
        (_, Some(address)) => run(&loaded, connect_live(address)?, &options)?,
        _ => unreachable!("clap requires one of --spikes or --live"),
    };
    println!("{}", stats.summary(&loaded));
    Ok(())
}

fn verify_command(positions: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let report = verify(positions)?;
    println!(
        "schema={} manifest={} nodes={} placed={} unplaced={} cell_types={}",
        if report.schema.is_empty() {
            "unknown"
        } else {
            &report.schema
        },
        if report.manifest_present { "present" } else { "absent" },
        report.neuron_count,
        report.placed_count,
        report.unplaced_count,
        report.type_labels
    );
    println!(
        "positions.bin: {} bytes sha256={}",
        report.bin_bytes, report.bin_sha256
    );
    println!(
        "types.txt: {} bytes sha256={}",
        report.types_bytes, report.types_sha256
    );
    for (name, digest) in &report.section_sha256 {
        println!("section {name}: sha256={digest}");
    }
    println!(
        "sections checked against the manifest: {}/{}",
        report.sections_checked,
        report.section_sha256.len()
    );
    if report.source_bodies != 0 {
        println!(
            "source table: {} bodies, {} positioned, {} type labels",
            report.source_bodies, report.source_positioned, report.source_type_labels
        );
    }
    if report.is_ok() {
        println!("artifact: OK");
        Ok(())
    } else {
        for line in &report.mismatches {
            println!("mismatch: {line}");
        }
        Err("the artifact and its manifest disagree".into())
    }
}
