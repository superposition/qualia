//! The Male CNS connectome tools: import, verify, bench and run.
//!
//! One binary, five verbs:
//!
//! * `import` reads the released Feather tables and writes the artifact.
//! * `verify` loads the artifact back and reports what the digests checked.
//! * `bench` steps the network as fast as it can, on the CPU or the device,
//!   and prints synapses/s and ticks/s.
//! * `loop` closes the loop: a real camera frame stream drives the
//!   photoreceptors, the descending and motor populations are read out as a
//!   steering command, the firing sets are recorded as a `QLSP` stream and the
//!   per-tick trace is written beside it.
//! * `replay` reads a recorded firing stream back and summarises it.
//!
//! Nothing here is trained and nothing here is synthetic: the encoder's grid
//! and gain, and the read-out rule, are fixed constants stated in the README,
//! and the frames come from a camera.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use qualia_connectome_cns::{
    import, read_error, read_spikes, step_cpu, Artifact, CnsError, LifParams, LifState, NodeMeta,
    Source, SpikeFrame, SpikeWriter,
};

/// Build, verify and run the Male CNS connectome artifact.
#[derive(Debug, Parser)]
#[command(
    name = "qualia-connectome-cns",
    version,
    about = "Import the Male CNS release and step it as a spiking network"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read the released tables and write the artifact.
    Import {
        /// `connectome-weights-*.feather`.
        #[arg(long)]
        weights: PathBuf,
        /// `body-annotations-*.feather`.
        #[arg(long)]
        annotations: PathBuf,
        /// `body-neurotransmitters-*.feather`.
        #[arg(long)]
        neurotransmitters: PathBuf,
        /// Output artifact directory.
        #[arg(long)]
        out: PathBuf,
    },
    /// Load the artifact back and verify every section digest.
    Verify {
        /// Artifact directory.
        #[arg(long)]
        artifact: PathBuf,
    },
    /// Step the network and measure the rate.
    Bench {
        /// Artifact directory.
        #[arg(long)]
        artifact: PathBuf,
        /// Ticks to step.
        #[arg(long, default_value_t = 600)]
        ticks: u64,
        /// Ticks to ignore before timing.
        #[arg(long, default_value_t = 100)]
        warmup: u64,
        /// `cpu` or `gpu`.
        #[arg(long, default_value = "gpu")]
        device: String,
    },
    /// Close the loop over a real frame stream and record the run.
    Loop {
        /// Artifact directory.
        #[arg(long)]
        artifact: PathBuf,
        /// Raw 8-bit grayscale frames, back to back, `width * height` each.
        #[arg(long, conflicts_with = "camera")]
        frames: Option<PathBuf>,
        /// Live JPEG camera endpoint, fetched once per tick — the robot's
        /// leash serves `/camera/snapshot`.
        #[arg(long, conflicts_with = "frames")]
        camera: Option<String>,
        /// Frame width in pixels (raw `--frames` only).
        #[arg(long, default_value_t = 640)]
        width: usize,
        /// Frame height in pixels (raw `--frames` only).
        #[arg(long, default_value_t = 480)]
        height: usize,
        /// Ticks to run; each tick consumes the next frame.
        #[arg(long, default_value_t = 600)]
        ticks: u64,
        /// Recorded firing stream to write.
        #[arg(long)]
        spikes: PathBuf,
        /// Per-tick trace to write.
        #[arg(long)]
        trace: PathBuf,
        /// `cpu` or `gpu`.
        #[arg(long, default_value = "gpu")]
        device: String,
        /// Name of the session the frames came from, recorded in the trace.
        #[arg(long, default_value = "unattributed")]
        session: String,
    },
    /// Replay a recorded firing stream and print its summary.
    Replay {
        /// `spikes.bin` to read.
        #[arg(long)]
        spikes: PathBuf,
    },
}

/// Columns of the encoder's luminance grid.
const GRID_COLUMNS: usize = 8;

/// Current per unit of luminance injected into a photoreceptor. Fixed, not
/// trained.
const SENSORY_GAIN: f32 = 0.5;

/// Dead band on the read-out: a command needs this much rate difference.
const DECISION_MARGIN: f32 = 0.001;

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("connectome-cns: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &Command) -> Result<(), CnsError> {
    match command {
        Command::Import {
            weights,
            annotations,
            neurotransmitters,
            out,
        } => {
            let started = Instant::now();
            let source = Source {
                weights: weights.clone(),
                annotations: annotations.clone(),
                neurotransmitters: neurotransmitters.clone(),
            };
            let report = import(&source, out)?;
            let manifest = &report.manifest;
            println!("cns-import: neurons {}", manifest.neuron_count);
            println!("cns-import: edges {}", manifest.edge_count);
            println!("cns-import: synapses {}", manifest.synapse_count);
            println!(
                "cns-import: sign +{} -{} ?{}",
                manifest.excitatory_synapses,
                manifest.inhibitory_synapses,
                manifest.unknown_synapses
            );
            println!(
                "cns-import: source rows {} source synapses {}",
                report.source_rows, report.source_synapses
            );
            println!(
                "cns-import: mapped rows {} dropped pre {} dropped post {} folded {}",
                report.mapped_rows, report.dropped_pre, report.dropped_post, report.folded_rows
            );
            println!(
                "cns-import: distinct bodies on the pre end {}",
                report.distinct_pre_bodies
            );
            println!(
                "cns-import: positioned {} types {}",
                manifest.positioned_count, manifest.type_count
            );
            println!("cns-import: -> {} in {:?}", out.display(), started.elapsed());
            Ok(())
        }
        Command::Verify { artifact } => {
            let loaded = Artifact::load(artifact)?;
            let graph = &loaded.graph;
            println!(
                "cns-verify: neurons {} edges {} synapses {}",
                graph.neuron_count(),
                graph.edge_count(),
                graph.synapse_count()
            );
            let (excitatory, inhibitory, unknown) = graph.sign_totals();
            println!("cns-verify: sign +{excitatory} -{inhibitory} ?{unknown}");
            println!(
                "cns-verify: positioned {} types {} nodes {}",
                loaded.manifest.positioned_count,
                loaded.types.len(),
                loaded.nodes.len()
            );
            let bytes = fs::metadata(artifact.join("weights.bin"))
                .map(|meta| meta.len())
                .unwrap_or(0);
            println!("cns-verify: weights.bin {bytes} bytes, every section digest verified");
            Ok(())
        }
        Command::Bench {
            artifact,
            ticks,
            warmup,
            device,
        } => bench(artifact, *ticks, *warmup, device),
        Command::Loop {
            artifact,
            frames,
            camera,
            width,
            height,
            ticks,
            spikes,
            trace,
            device,
            session,
        } => close_loop(
            artifact,
            frames.as_deref(),
            camera.as_deref(),
            *width,
            *height,
            *ticks,
            spikes,
            trace,
            device,
            session,
        ),
        Command::Replay { spikes } => {
            let frames = read_spikes(spikes)?;
            let total: u64 = frames.iter().map(|frame| frame.ids.len() as u64).sum();
            println!("cns-replay: frames {}", frames.len());
            println!("cns-replay: spikes {total}");
            if let Some(first) = frames.first() {
                println!(
                    "cns-replay: first tick {} {} ids",
                    first.tick,
                    first.ids.len()
                );
            }
            if let Some(last) = frames.last() {
                println!(
                    "cns-replay: last tick {} {} ids wall {} ns",
                    last.tick,
                    last.ids.len(),
                    last.t_ns
                );
            }
            Ok(())
        }
    }
}

fn bench(artifact: &Path, ticks: u64, warmup: u64, device: &str) -> Result<(), CnsError> {
    let loaded = Artifact::load(artifact)?;
    let incoming = loaded.incoming();
    let params = LifParams::default();
    let edges = incoming.edge_count() as u64;
    println!(
        "cns-bench: {} neurons, {} edges, {} synapses, device {device}",
        incoming.neuron_count(),
        edges,
        loaded.graph.synapse_count()
    );

    match device {
        "cpu" => {
            let mut state = LifState::new(incoming.neuron_count());
            for _ in 0..warmup {
                step_cpu(&incoming, &params, &mut state, &[]);
            }
            let started = Instant::now();
            for _ in 0..ticks {
                step_cpu(&incoming, &params, &mut state, &[]);
            }
            let elapsed = started.elapsed().as_secs_f64();
            report(ticks, edges, elapsed, "cpu", state.total_spikes);
            Ok(())
        }
        "gpu" => {
            #[cfg(feature = "cuda")]
            {
                let mut lif =
                    qualia_connectome_cns::gpu::LifDevice::new(&incoming).map_err(CnsError::Artifact)?;
                for _ in 0..warmup {
                    lif.step(&params).map_err(CnsError::Artifact)?;
                }
                lif.synchronize().map_err(CnsError::Artifact)?;
                let started = Instant::now();
                for _ in 0..ticks {
                    lif.step(&params).map_err(CnsError::Artifact)?;
                }
                lif.synchronize().map_err(CnsError::Artifact)?;
                let elapsed = started.elapsed().as_secs_f64();
                let fired = lif.fired().map_err(CnsError::Artifact)?;
                println!("cns-bench: resident device bytes {}", lif.bytes_resident());
                report(ticks, edges, elapsed, lif.device_name(), fired.len() as u64);
                Ok(())
            }
            #[cfg(not(feature = "cuda"))]
            {
                let _ = (incoming, params, ticks, warmup, edges);
                Err(CnsError::Artifact(
                    "this build has no `cuda` feature; rebuild with --features cuda".into(),
                ))
            }
        }
        other => Err(CnsError::Artifact(format!(
            "unknown device `{other}`; use cpu or gpu"
        ))),
    }
}

fn report(ticks: u64, edges: u64, elapsed: f64, device: &str, spikes: u64) {
    let per_tick = elapsed / ticks as f64;
    println!("cns-bench: {ticks} ticks in {elapsed:.3} s on {device}");
    println!("cns-bench: {:.3} ms/tick", per_tick * 1e3);
    println!("cns-bench: {:.1} ticks/s", 1.0 / per_tick);
    println!("cns-bench: {:.1} M synapses/s", edges as f64 / per_tick / 1e6);
    println!("cns-bench: {spikes} spikes on the last tick");
}

/// One frame's drive: the mean luminance of each grid column.
fn column_luminance(frame: &[u8], width: usize, height: usize) -> Vec<f32> {
    let mut columns = vec![0f32; GRID_COLUMNS];
    let mut counts = vec![0f32; GRID_COLUMNS];
    for y in 0..height {
        for x in 0..width {
            let column = x * GRID_COLUMNS / width.max(1);
            columns[column] += f32::from(frame[y * width + x]);
            counts[column] += 1.0;
        }
    }
    for column in 0..GRID_COLUMNS {
        if counts[column] > 0.0 {
            columns[column] = columns[column] / counts[column] / 255.0;
        }
    }
    columns
}

/// Where the loop's frames come from: a raw file, or the leash's camera.
enum FrameSource {
    /// `width * height` bytes per frame, back to back.
    Raw {
        bytes: Vec<u8>,
        width: usize,
        height: usize,
        available: usize,
    },
    /// One JPEG per tick from the leash's HTTP surface.
    Camera {
        agent: ureq::Agent,
        url: String,
        width: usize,
        height: usize,
    },
}

impl FrameSource {
    /// A raw 8-bit grayscale stream.
    fn raw(path: &Path, width: usize, height: usize) -> Result<Self, CnsError> {
        let bytes = fs::read(path).map_err(|error| read_error(path, error))?;
        let frame_size = width * height;
        let available = bytes.len() / frame_size.max(1);
        if available == 0 {
            return Err(CnsError::Read(format!(
                "{}: no complete {width}x{height} frame in {} bytes",
                path.display(),
                bytes.len()
            )));
        }
        Ok(Self::Raw {
            bytes,
            width,
            height,
            available,
        })
    }

    /// The leash's JPEG snapshot endpoint.
    fn camera(url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(5))
            .build();
        Self::Camera {
            agent,
            url: url.to_string(),
            width: 0,
            height: 0,
        }
    }

    /// Grab the frame for `tick`, as 8-bit luma.
    fn frame(&mut self, tick: u64) -> Result<Vec<u8>, CnsError> {
        match self {
            Self::Raw {
                bytes,
                width: w,
                height: h,
                available,
            } => {
                let frame_size = *w * *h;
                let offset = (tick as usize % *available) * frame_size;
                Ok(bytes[offset..offset + frame_size].to_vec())
            }
            Self::Camera {
                agent,
                url,
                width,
                height,
            } => {
                let response = agent
                    .get(url)
                    .call()
                    .map_err(|error| CnsError::Read(format!("camera {url}: {error}")))?;
                let mut jpeg = Vec::new();
                response
                    .into_reader()
                    .read_to_end(&mut jpeg)
                    .map_err(|error| CnsError::Read(format!("camera {url}: {error}")))?;
                let image = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
                    .map_err(|error| CnsError::Read(format!("camera {url}: decode: {error}")))?
                    .to_luma8();
                *width = image.width() as usize;
                *height = image.height() as usize;
                Ok(image.into_raw())
            }
        }
    }

    /// The frame the source will hand out next, for reporting.
    fn describe(&self) -> String {
        match self {
            Self::Raw {
                width,
                height,
                available,
                ..
            } => format!("{available} frames of {width}x{height}"),
            Self::Camera { url, .. } => format!("live JPEG camera at {url}"),
        }
    }
}

/// The encoder: put each visual-stage cell's soma on a grid column.
///
/// The input population is the release's optic-lobe intrinsic cells
/// (`superclass = ol_intrinsic`: lamina, medulla and lobula intrinsic neurons,
/// 89,403 of which 81,055 carry a soma) together with the photoreceptors
/// themselves (`R1-R6`, `R7*`, `R8*` — the cells that transduce light; only 28
/// of the 6,091 carry a released soma).
///
/// The photoreceptors are **histaminergic, and 100% of their 74,557 outgoing
/// edges in this artifact are inhibitory** — measured, not assumed — so feeding
/// them luminance plus a plain LIF (no rebound, no NMDA) silences the lamina
/// instead of driving it. The optic-lobe intrinsic population is the stage that
/// carries the drive onward: 79.8% of its 10,866,800 outgoing edges are
/// excitatory. Both populations are fed; the intrinsic cells are what propagates.
///
/// A cell's column is its soma's position along the eye axis, normalised over
/// the input population and quantised to [`GRID_COLUMNS`]; a cell with no
/// released soma takes the column of the nearest positioned cell in the same
/// population, by index. Its current is that column's mean luminance times
/// [`SENSORY_GAIN`]. The axis, the grid and the gain are fixed here and stated
/// in the README; nothing is fitted.
fn encoder(nodes: &[NodeMeta], neurons: &[qualia_connectome_cns::Neuron]) -> (Vec<u32>, Vec<usize>) {
    let mut targets: Vec<(u32, Option<f32>)> = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let is_photoreceptor = node.type_name == "R1-R6"
            || node.type_name.starts_with("R7")
            || node.type_name.starts_with("R8");
        let is_optic_intrinsic = node.superclass == "ol_intrinsic";
        if !is_photoreceptor && !is_optic_intrinsic {
            continue;
        }
        targets.push((index as u32, neurons[index].soma.map(|soma| soma[1])));
    }
    let (min, max) = targets
        .iter()
        .filter_map(|(_, axis)| *axis)
        .fold((f32::MAX, f32::MIN), |acc, axis| {
            (acc.0.min(axis), acc.1.max(axis))
        });
    let span = (max - min).max(1.0);
    // Cells without a released soma borrow the previous positioned cell's
    // column, which keeps the column counts proportional to the positioned
    // population rather than to the release's indexing.
    let mut last = GRID_COLUMNS / 2;
    let indices: Vec<u32> = targets.iter().map(|(index, _)| *index).collect();
    let columns: Vec<usize> = targets
        .iter()
        .map(|(_, axis)| {
            if let Some(axis) = axis {
                let column = (((axis - min) / span) * GRID_COLUMNS as f32) as usize;
                last = column.min(GRID_COLUMNS - 1);
            }
            last
        })
        .collect();
    (indices, columns)
}

/// The decoder: the descending and motor pool split by side.
///
/// Output neurons are the released `descending_neuron` and `vnc_motor`
/// superclasses — the cells that carry the CNS's instructions to the body.
/// Their side is the release's `somaSide`. The command is the sign of the
/// right-minus-left firing rate against [`DECISION_MARGIN`]; the throttle is
/// the total output rate. Nothing is fitted: the rule is stated here and in
/// the README.
fn decoder(nodes: &[NodeMeta]) -> (Vec<bool>, Vec<bool>) {
    let mut left = vec![false; nodes.len()];
    let mut right = vec![false; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        let is_output = node.superclass == "descending_neuron" || node.superclass == "vnc_motor";
        if !is_output {
            continue;
        }
        match node.side.as_str() {
            "L" => left[index] = true,
            "R" => right[index] = true,
            _ => {}
        }
    }
    (left, right)
}

#[allow(clippy::too_many_arguments)]
fn close_loop(
    artifact: &Path,
    frames_path: Option<&Path>,
    camera_url: Option<&str>,
    width: usize,
    height: usize,
    ticks: u64,
    spikes: &Path,
    trace: &Path,
    device: &str,
    session: &str,
) -> Result<(), CnsError> {
    let loaded = Artifact::load(artifact)?;
    let incoming = loaded.incoming();
    let params = LifParams::default();
    let mut source = match (frames_path, camera_url) {
        (Some(path), _) => FrameSource::raw(path, width, height)?,
        (None, Some(url)) => FrameSource::camera(url),
        (None, None) => {
            return Err(CnsError::Artifact(
                "loop needs either --frames <raw file> or --camera <jpeg url>".into(),
            ))
        }
    };
    let (input_indices, input_columns) = encoder(&loaded.nodes, &loaded.neurons);
    let (left_index, right_index) = decoder(&loaded.nodes);
    let left_count = left_index.iter().filter(|flag| **flag).count();
    let right_count = right_index.iter().filter(|flag| **flag).count();
    println!("cns-loop: session {session}; input {}", source.describe());
    println!(
        "cns-loop: encoder drives {} visual-stage cells (ol_intrinsic + R1-R6/R7/R8) over {GRID_COLUMNS} columns, gain {SENSORY_GAIN}",
        input_indices.len()
    );
    println!(
        "cns-loop: decoder reads {} descending/motor neurons ({left_count} L, {right_count} R)",
        left_count + right_count
    );

    let started = Instant::now();
    let mut frames = Vec::with_capacity(ticks as usize);
    let mut luminance = Vec::with_capacity(ticks as usize);
    let mut external = vec![0f32; incoming.neuron_count()];
    let mut width = width;
    let mut height = height;

    match device {
        "cpu" => {
            let mut state = LifState::new(incoming.neuron_count());
            for tick in 0..ticks {
                let frame = source.frame(tick)?;
                if let FrameSource::Camera { width: w, height: h, .. } = &source {
                    width = *w;
                    height = *h;
                }
                let columns = column_luminance(&frame, width, height);
                drive(&mut external, &input_indices, &input_columns, &columns);
                step_cpu(&incoming, &params, &mut state, &external);
                frames.push(SpikeFrame {
                    tick,
                    t_ns: started.elapsed().as_nanos() as u64,
                    ids: state.fired.clone(),
                });
                luminance.push(columns.iter().sum::<f32>() / GRID_COLUMNS as f32);
            }
        }
        "gpu" => {
            #[cfg(feature = "cuda")]
            {
                let mut lif = qualia_connectome_cns::gpu::LifDevice::new(&incoming)
                    .map_err(CnsError::Artifact)?;
                println!("cns-loop: device {}", lif.device_name());
                for tick in 0..ticks {
                    let frame = source.frame(tick)?;
                    if let FrameSource::Camera { width: w, height: h, .. } = &source {
                        width = *w;
                        height = *h;
                    }
                    let columns = column_luminance(&frame, width, height);
                    drive(&mut external, &input_indices, &input_columns, &columns);
                    lif.set_external(&external).map_err(CnsError::Artifact)?;
                    lif.step(&params).map_err(CnsError::Artifact)?;
                    let fired = lif.fired().map_err(CnsError::Artifact)?;
                    frames.push(SpikeFrame {
                        tick,
                        t_ns: started.elapsed().as_nanos() as u64,
                        ids: fired,
                    });
                    luminance.push(columns.iter().sum::<f32>() / GRID_COLUMNS as f32);
                }
                lif.synchronize().map_err(CnsError::Artifact)?;
            }
            #[cfg(not(feature = "cuda"))]
            {
                let _ = (incoming, params, ticks);
                return Err(CnsError::Artifact(
                    "this build has no `cuda` feature; rebuild with --features cuda".into(),
                ));
            }
        }
        other => {
            return Err(CnsError::Artifact(format!(
                "unknown device `{other}`; use cpu or gpu"
            )))
        }
    }
    let elapsed = started.elapsed().as_secs_f64();

    let mut writer = SpikeWriter::create(spikes)?;
    let mut trace_text = format!(
        "# session {session}; frames {width}x{height}; input {}\n",
        source.describe()
    );
    trace_text.push_str("tick,luminance,fired_input,rate_l,rate_r,command,throttle\n");
    let mut total_spikes = 0u64;
    let mut commands = 0u64;
    let mut input_set = vec![false; loaded.nodes.len()];
    for index in &input_indices {
        input_set[*index as usize] = true;
    }
    for frame in &frames {
        let fired_left = frame.ids.iter().filter(|id| left_index[**id as usize]).count();
        let fired_right = frame.ids.iter().filter(|id| right_index[**id as usize]).count();
        let fired_input = frame.ids.iter().filter(|id| input_set[**id as usize]).count();
        let rate_l = fired_left as f32 / left_count.max(1) as f32;
        let rate_r = fired_right as f32 / right_count.max(1) as f32;
        let command = if (rate_r - rate_l).abs() < DECISION_MARGIN {
            0
        } else if rate_r > rate_l {
            1
        } else {
            -1
        };
        let throttle = (rate_l + rate_r).min(1.0);
        if command != 0 {
            commands += 1;
        }
        total_spikes += frame.ids.len() as u64;
        trace_text.push_str(&format!(
            "{},{:.4},{fired_input},{rate_l:.5},{rate_r:.5},{command},{throttle:.5}\n",
            frame.tick, luminance[frame.tick as usize],
        ));
        writer.write_frame(frame)?;
    }
    writer.finish()?;
    fs::write(trace, trace_text).map_err(|error| qualia_connectome_cns::read_error(trace, error))?;

    println!(
        "cns-loop: {ticks} ticks in {elapsed:.3} s ({:.1} ticks/s), {total_spikes} spikes, {commands} non-hold commands",
        ticks as f64 / elapsed
    );
    println!("cns-loop: {} -> {}", spikes.display(), trace.display());
    Ok(())
}

/// Set each photoreceptor's external current from its column's luminance.
fn drive(external: &mut [f32], indices: &[u32], columns: &[usize], luminance: &[f32]) {
    external.fill(0.0);
    for (neuron, column) in indices.iter().zip(columns) {
        external[*neuron as usize] += luminance[*column] * SENSORY_GAIN;
    }
}
