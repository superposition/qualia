//! The desktop viewer entry point for the connectome: the floating brain with
//! the current tick's firing set highlighted.
//!
//! Two ways in, one projection:
//!
//! - **replay** — a recorded spike stream from a file, paced by the stream's
//!   own wall clock, so a run can be watched again or scrubbed afterwards;
//! - **live** — the same frames off a socket: the connectome runner on the
//!   Jetson over the USB link, or any other producer speaking the same framing.
//!
//! Both hand the same reader the same bytes and both log through
//! `qualia-rerun-bridge`, so the two paths differ only in where the stream
//! comes from. The projection has no control authority: it reads an artifact
//! and a stream and writes a recording.
//!
//! The cloud is the released dataset's own positions, one point per CSR node
//! the annotations place; the firing set is the runner's own per-tick output.
//! Nothing here invents a spike.

use std::fmt::{Display, Formatter};
use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qualia_connectome_stream::{
    read_manifest, read_positions, resolve_positions_paths, verify_positions, Positions,
    PositionsVerification, SpikeReader, POSITIONS_MANIFEST_FILE,
};
use qualia_rerun_bridge::{
    ConnectomeCloud, ConnectomeNeuron, ConnectomeProjection, QualiaRerunBridge,
    RerunBridgeConfig, RerunSinkConfig,
};

/// A viewer failure: an artifact or stream that does not read, or a recording
/// that does not open.
#[derive(Debug)]
pub struct ViewError {
    detail: String,
}

impl ViewError {
    fn new(context: &str, error: impl Display) -> Self {
        Self {
            detail: format!("{context}: {error}"),
        }
    }
}

impl Display for ViewError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for ViewError {}

pub type Result<T, E = ViewError> = std::result::Result<T, E>;

/// Where the recording goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerSink {
    /// Stream into a file, for the viewer to open afterwards.
    Save(PathBuf),
    /// Stream into a running Rerun viewer or server, over gRPC.
    Attach(String),
    /// Keep the recording in memory.
    Buffered,
}

/// One viewer run.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewOptions {
    /// Recorded spike stream, or `None` when the stream arrives on a socket.
    pub spikes: Option<PathBuf>,
    /// Live stream socket, `host:port`.
    pub live: Option<String>,
    pub sink: ViewerSink,
    /// Stop after this many ticks.
    pub max_ticks: Option<u64>,
    /// Playback speed against the stream's own clock: `1.0` is real time, `0.0`
    /// is as fast as the machine reads.
    pub speed: f64,
    /// Send the Brain blueprint before the first tick.
    pub blueprint: bool,
}

/// What one run did.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewStats {
    pub ticks: u64,
    pub first_tick: u64,
    pub last_tick: u64,
    /// Wall clock of the first and last tick, unix nanoseconds, so the stream's
    /// own rate can be stated beside the logging rate.
    pub first_t_ns: u64,
    pub last_t_ns: u64,
    pub firing_points: u64,
    pub cloud_points: u64,
    pub elapsed: Duration,
}

impl ViewStats {
    /// Points written to the recording: the cloud once, then every tick's
    /// highlighted set.
    pub fn points_logged(&self) -> u64 {
        self.cloud_points + self.firing_points
    }

    pub fn points_per_second(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            0.0
        } else {
            self.points_logged() as f64 / seconds
        }
    }

    pub fn ticks_per_second(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            0.0
        } else {
            self.ticks as f64 / seconds
        }
    }

    /// Wall-clock span the stream itself covers.
    pub fn stream_span(&self) -> Duration {
        Duration::from_nanos(self.last_t_ns.saturating_sub(self.first_t_ns))
    }

    /// The stream's own tick rate.
    pub fn stream_hz(&self) -> f64 {
        let seconds = self.stream_span().as_secs_f64();
        if seconds <= 0.0 {
            0.0
        } else {
            self.ticks as f64 / seconds
        }
    }

    /// The line the evidence quotes.
    pub fn summary(&self, cloud: &LoadedCloud) -> String {
        format!(
            "ticks={} tick_range={}..{} firing_points={} cloud_points={} points_logged={} \
             elapsed_s={:.3} points_per_s={:.0} ticks_per_s={:.1} stream_span_s={:.3} stream_hz={:.1} \
             artifact={{nodes={} placed={} types={} sha256={}}}",
            self.ticks,
            self.first_tick,
            self.last_tick,
            self.firing_points,
            self.cloud_points,
            self.points_logged(),
            self.elapsed.as_secs_f64(),
            self.points_per_second(),
            self.ticks_per_second(),
            self.stream_span().as_secs_f64(),
            self.stream_hz(),
            cloud.node_count,
            cloud.neurons.len(),
            cloud.cell_types.len(),
            cloud.bin_sha256
        )
    }
}

/// The positions artifact as the projection wants it.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedCloud {
    /// The placed nodes, ascending by node index.
    pub neurons: Vec<ConnectomeNeuron>,
    pub cell_types: Vec<String>,
    /// Rows in the artifact: CSR nodes, placed or not.
    pub node_count: u32,
    pub source: String,
    pub coordinate_space: String,
    pub bin_sha256: String,
}

impl LoadedCloud {
    /// The borrow the bridge's projection takes.
    pub fn cloud(&self) -> ConnectomeCloud<'_> {
        ConnectomeCloud {
            neurons: &self.neurons,
            cell_types: &self.cell_types,
            node_count: self.node_count,
            source: &self.source,
            coordinate_space: &self.coordinate_space,
        }
    }

    /// Nodes the annotations place nowhere.
    pub fn unplaced_count(&self) -> usize {
        self.node_count as usize - self.neurons.len()
    }
}

/// Reads a positions artifact, keeping the placed rows and the label table.
///
/// A node whose `x/y/z` are not finite is a node the released annotations place
/// nowhere: it is left out of the cloud rather than drawn at a made-up
/// coordinate, and its row index still is its CSR node index.
pub fn load_cloud(path: &Path) -> Result<LoadedCloud> {
    let (bin_path, _) = resolve_positions_paths(path).map_err(|error| {
        ViewError::new("failed to resolve the positions artifact", error)
    })?;
    let positions: Positions = read_positions(path)
        .map_err(|error| ViewError::new("failed to read the positions artifact", error))?;

    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let manifest_path = directory.join(POSITIONS_MANIFEST_FILE);
    let (source, coordinate_space) = if manifest_path.exists() {
        match read_manifest(&manifest_path) {
            Ok(manifest) => (manifest.source.file, manifest.coordinate_space),
            Err(error) => {
                eprintln!("warning: {manifest_path:?} did not read: {error}");
                (String::new(), String::new())
            }
        }
    } else {
        (String::new(), String::new())
    };

    let mut neurons = Vec::with_capacity(positions.placed_count());
    let node_count = positions.neuron_count() as u32;
    for row in 0..positions.neuron_count() {
        if !positions.is_placed(row) {
            continue;
        }
        neurons.push(ConnectomeNeuron {
            node: row as u32,
            position: positions.position(row),
            cell_type: positions.cell_type[row],
        });
    }

    Ok(LoadedCloud {
        neurons,
        cell_types: positions.types,
        node_count,
        source: if source.is_empty() {
            "unrecorded".to_string()
        } else {
            source
        },
        coordinate_space: if coordinate_space.is_empty() {
            "dataset EM voxel units".to_string()
        } else {
            coordinate_space
        },
        bin_sha256: sha256_file(&bin_path)?,
    })
}

/// Checks an artifact against the manifest beside it.
pub fn verify(path: &Path) -> Result<PositionsVerification> {
    verify_positions(path).map_err(|error| ViewError::new("failed to verify the artifact", error))
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)
        .map_err(|error| ViewError::new(&format!("failed to read {}", path.display()), error))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Opens the live stream socket. `TCP_NODELAY` keeps a tick from waiting for
/// the next one in a Nagle buffer.
pub fn connect_live(address: &str) -> Result<TcpStream> {
    let stream = TcpStream::connect(address)
        .map_err(|error| ViewError::new(&format!("failed to connect to {address}"), error))?;
    stream
        .set_nodelay(true)
        .map_err(|error| ViewError::new("failed to set TCP_NODELAY on the live stream", error))?;
    Ok(stream)
}

/// Runs one viewer session: opens the sink, logs the cloud once, then logs
/// every tick the stream carries.
pub fn run<R: Read>(
    loaded: &LoadedCloud,
    stream: R,
    options: &ViewOptions,
) -> Result<ViewStats> {
    let bridge = QualiaRerunBridge::new(&RerunBridgeConfig {
        application_id: "qualia-connectome".to_string(),
        sink: match &options.sink {
            ViewerSink::Save(path) => RerunSinkConfig::Save(path.clone()),
            ViewerSink::Attach(url) => RerunSinkConfig::Connect(url.clone()),
            ViewerSink::Buffered => RerunSinkConfig::Buffered,
        },
    })
    .map_err(|error| ViewError::new("failed to open the recording", error))?;

    let cloud = loaded.cloud();
    if options.blueprint {
        bridge
            .send_default_connectome_blueprint()
            .map_err(|error| ViewError::new("failed to send the Brain blueprint", error))?;
    }
    bridge
        .log_connectome_cloud(&cloud)
        .map_err(|error| ViewError::new("failed to log the point cloud", error))?;

    let mut reader = SpikeReader::new(stream)
        .map_err(|error| ViewError::new("failed to read the spike stream header", error))?;
    let started = Instant::now();
    let mut origin_ns = None;
    let mut ticks = 0u64;
    let mut first_tick = 0u64;
    let mut last_tick = 0u64;
    let mut last_t_ns = 0u64;
    let mut firing_points = 0u64;

    while let Some(frame) = reader
        .next_frame()
        .map_err(|error| ViewError::new("failed to read a spike frame", error))?
    {
        if options.max_ticks.is_some_and(|max| ticks >= max) {
            break;
        }
        let origin = *origin_ns.get_or_insert(frame.t_ns);
        if options.speed > 0.0 {
            let target = Duration::from_nanos(
                ((frame.t_ns.saturating_sub(origin)) as f64 / options.speed) as u64,
            );
            if let Some(wait) = target.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }

        let projection = ConnectomeProjection {
            cloud: &cloud,
            firing: &frame.ids,
            wall_clock_ns: frame.t_ns,
        };
        bridge
            .log_connectome_tick(frame.tick, &projection)
            .map_err(|error| {
                ViewError::new(&format!("failed to log tick {}", frame.tick), error)
            })?;

        if ticks == 0 {
            first_tick = frame.tick;
        }
        last_tick = frame.tick;
        last_t_ns = frame.t_ns;
        ticks += 1;
        firing_points += frame.ids.len() as u64;
    }

    Ok(ViewStats {
        ticks,
        first_tick,
        last_tick,
        first_t_ns: origin_ns.unwrap_or(0),
        last_t_ns,
        firing_points,
        cloud_points: loaded.neurons.len() as u64,
        elapsed: started.elapsed(),
    })
}
