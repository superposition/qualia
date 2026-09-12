//! The released male-CNS connectome in the scene: the real point cloud and the
//! firing set of the tick that is showing.
//!
//! This is the panel's own reading of the artifact the importer writes and the
//! runner feeds (`crates/connectome-stream`): `positions.bin` is one row per CSR
//! node, 139,662 of them placed, and `spikes.bin` carries one frame per tick
//! with the sorted firing node ids. Nothing here invents a spike and nothing
//! here writes: the cloud is read once, the stream is read on its own thread,
//! and the panel paints what the last frame said.
//!
//! Two environment keys configure it, and the panel says which one is missing
//! rather than drawing an empty brain:
//!
//! - `QUALIA_CONNECTOME_DIR` — the artifact directory holding `positions.bin`
//!   and `types.txt`; unset, the artifact committed under
//!   `assets/brain/connectome-cns/` is read, so an offline console needs no
//!   environment at all;
//! - `QUALIA_CONNECTOME_SPIKES` — the spike stream: a `spikes.bin` path (played
//!   back on the stream's own clock) or `tcp://host:port` for the runner on the
//!   board over the USB link.
//!
//! The frame cost is bounded like every other layer: the cloud draws a fixed
//! decimated subset and reports what it dropped, and the firing set is capped.
//! The reading thread is beside the poller's, never inside the paint.

use std::io::Read;
use std::collections::VecDeque;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use egui::Color32;
use qualia_connectome_stream::{read_positions, SpikeReader};

use super::scene;

/// The artifact directory holding `positions.bin` and `types.txt`.
pub const ARTIFACT_DIR_ENV: &str = "QUALIA_CONNECTOME_DIR";
/// The spike stream: a file path, or `tcp://host:port`.
pub const STREAM_ENV: &str = "QUALIA_CONNECTOME_SPIKES";
/// How long a live source keeps retrying after a disconnect.
const LIVE_RETRY: Duration = Duration::from_millis(500);

/// One placed node, already in the scene's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudPoint {
    /// CSR node index: what a spike frame carries.
    pub node: u32,
    /// Scene-frame position, centred and scaled into the unit layout.
    pub position: [f32; 3],
    /// The cell type's colour, from the same golden-ratio rule the Rerun
    /// projection uses, so the two surfaces agree.
    pub colour: Color32,
}

/// The cloud, read once from the artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectomeCloud {
    /// Placed nodes, ascending by node index.
    pub points: Vec<CloudPoint>,
    /// The decimated subset the cloud layer draws, indices into `points`.
    pub draw: Vec<usize>,
    /// Rows in the artifact: CSR nodes, placed or not.
    pub node_count: u32,
    /// Rows carrying a soma position.
    pub placed: usize,
    /// Interned cell-type labels.
    pub types: usize,
    /// The artifact directory this was read from.
    pub directory: String,
    /// Where the coordinates came from, from the manifest.
    pub source: String,
}

impl ConnectomeCloud {
    /// The point a firing id indexes: row `k` of the artifact is node `k`.
    pub fn point_of(&self, node: u32) -> Option<&CloudPoint> {
        self.points
            .binary_search_by_key(&node, |point| point.node)
            .ok()
            .map(|index| &self.points[index])
    }

    /// Placed nodes the scene could not draw, because the layer is capped.
    pub fn dropped(&self) -> usize {
        self.points.len().saturating_sub(self.draw.len())
    }
}

/// What the panel knows about the cloud before it has one.
#[derive(Debug, Clone, PartialEq)]
pub enum CloudState {
    /// `QUALIA_CONNECTOME_DIR` is not set.
    Unset,
    Ready(Arc<ConnectomeCloud>),
    Failed(String),
}

/// One tick as the panel reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpikeActivity {
    pub received: Instant,
    pub tick: u64,
    pub count: usize,
}

/// Latest frame plus a bounded history of distinct received model frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConnectomeFrame {
    pub tick: u64,
    pub t_ns: u64,
    /// The tick's firing node ids, ascending.
    pub firing: Vec<u32>,
    /// Frames read so far.
    pub ticks_read: u64,
    /// Local ingestion time of the last distinct producer frame.
    pub last_advanced: Option<Instant>,
    pub activity: VecDeque<SpikeActivity>,
    /// The source reached its end (a recorded run) rather than failing.
    pub finished: bool,
    /// The source could not be read.
    pub error: Option<String>,
    /// Frames per second the panel has seen arrive.
    pub rate_hz: f32,
    /// The source string, for the panel's own line.
    pub source: String,
}

impl ConnectomeFrame {
    pub fn firing_count(&self) -> usize {
        self.firing.len()
    }
}

/// Reads the stream on its own thread and keeps the newest tick where the panel
/// can take it without waiting.
pub struct ConnectomeStream {
    latest: Arc<Mutex<ConnectomeFrame>>,
    stop: Arc<AtomicBool>,
    source: String,
}

impl ConnectomeStream {
    /// Opens `source`: a `spikes.bin` path, or `tcp://host:port`.
    pub fn open(source: &str) -> Self {
        let latest = Arc::new(Mutex::new(ConnectomeFrame {
            source: source.to_string(),
            ..ConnectomeFrame::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_latest = Arc::clone(&latest);
        let thread_stop = Arc::clone(&stop);
        let thread_source = source.to_string();
        let _ = std::thread::Builder::new()
            .name("connectome-spikes".to_string())
            .spawn(move || pump(&thread_source, thread_latest, thread_stop));
        Self {
            latest,
            stop,
            source: source.to_string(),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// The newest tick. Small copies only: the panel calls this every frame.
    pub fn frame(&self) -> ConnectomeFrame {
        match self.latest.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl Drop for ConnectomeStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// The spike stream, opened once from the environment.
static STREAM: LazyLock<Option<ConnectomeStream>> = LazyLock::new(|| {
    std::env::var(STREAM_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| ConnectomeStream::open(&value))
});

pub fn stream() -> Option<&'static ConnectomeStream> {
    STREAM.as_ref()
}

/// The newest tick, or an empty reading when no stream is configured.
pub fn frame() -> ConnectomeFrame {
    match stream() {
        Some(stream) => stream.frame(),
        None => ConnectomeFrame {
            source: format!("{STREAM_ENV} is not set"),
            ..ConnectomeFrame::default()
        },
    }
}

/// The cloud, read once from the artifact on the first frame that wants it.
///
/// `positions.bin` is 3.3 MB and parsing it takes tens of milliseconds, once;
/// doing it here rather than on a thread means the panel's first frame already
/// has the brain in it, and there is no loading state to race.
static CLOUD: LazyLock<CloudState> = LazyLock::new(|| match artifact_dir() {
    Some(directory) => match load_cloud(&directory) {
        Ok(cloud) => CloudState::Ready(Arc::new(cloud)),
        Err(error) => CloudState::Failed(error),
    },
    None => CloudState::Unset,
});

pub fn cloud() -> CloudState {
    CLOUD.clone()
}

/// `QUALIA_CONNECTOME_DIR`, if the deployment named one; otherwise the artifact
/// committed under `assets/brain/connectome-cns/`, so an offline console draws
/// the real cloud with no environment at all.
pub fn artifact_dir() -> Option<PathBuf> {
    if let Some(named) = std::env::var(ARTIFACT_DIR_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(named));
    }
    let committed = PathBuf::from(COMMITTED_DIR);
    committed.is_dir().then_some(committed)
}

/// The artifact committed in this repository. The console is built and run from
/// the tree (see `apps/qualia-console/README.md`), so the manifest and the label
/// table beside the binary are the ones the panel reads.
const COMMITTED_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/brain/connectome-cns"
);

/// Reads the artifact and puts it in the scene's frame.
///
/// The scene's layout is the unit frame the floor grid and the prior use, so the
/// cloud is centred on its own bounding box and scaled by one factor — the
/// longest axis fits `[-1, 1]` — and the dataset's dorsoventral axis (`z`) is
/// the scene's `y`, so the brain floats upright rather than on its back.
pub fn load_cloud(directory: &Path) -> Result<ConnectomeCloud, String> {
    let positions =
        read_positions(directory).map_err(|error| format!("reading {}: {error}", directory.display()))?;

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for row in 0..positions.neuron_count() {
        if !positions.is_placed(row) {
            continue;
        }
        let [x, y, z] = positions.position(row);
        for (axis, value) in [x, z, y].into_iter().enumerate() {
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    if !min[0].is_finite() {
        return Err("the artifact places no node".to_string());
    }
    let centre = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let extent = (0..3).fold(0.0_f32, |span, axis| span.max(max[axis] - min[axis]));
    let scale = if extent > 0.0 { 2.0 / extent } else { 1.0 };

    let mut points = Vec::with_capacity(positions.placed_count());
    for row in 0..positions.neuron_count() {
        if !positions.is_placed(row) {
            continue;
        }
        let [x, y, z] = positions.position(row);
        points.push(CloudPoint {
            node: row as u32,
            position: [
                (x - centre[0]) * scale,
                (z - centre[1]) * scale,
                (y - centre[2]) * scale,
            ],
            colour: cell_type_colour(positions.cell_type[row]),
        });
    }

    let stride = points.len().div_ceil(scene::MAX_CLOUD_DRAWN).max(1);
    let draw = (0..points.len()).step_by(stride).collect();

    let source = manifest_source(directory);
    Ok(ConnectomeCloud {
        points,
        draw,
        node_count: positions.neuron_count() as u32,
        placed: positions.placed_count(),
        types: positions.types.len(),
        // The committed artifact is named by its tree path, not by where this
        // checkout happens to live, so a panel rendered on another host reads
        // the same.
        directory: if directory == Path::new(COMMITTED_DIR) {
            "committed assets/brain/connectome-cns".to_string()
        } else {
            directory.display().to_string()
        },
        source,
    })
}

/// The annotation table the artifact came from, when the manifest says.
fn manifest_source(directory: &Path) -> String {
    let manifest = directory.join(qualia_connectome_stream::POSITIONS_MANIFEST_FILE);
    let Ok(document) = qualia_connectome_stream::read_manifest_document(&manifest) else {
        return "unrecorded".to_string();
    };
    document
        .get("sources")
        .and_then(|sources| sources.get("body_annotations"))
        .and_then(|table| table.get("file"))
        .and_then(|file| file.as_str())
        .map(str::to_string)
        .or_else(|| {
            document
                .get("source")
                .and_then(|source| source.get("file"))
                .and_then(|file| file.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unrecorded".to_string())
}

/// The cell-type colour: a golden-ratio walk around the hue circle, the same
/// rule `crates/rerun-bridge` paints with, so a type reads the same in both
/// surfaces.
pub fn cell_type_colour(index: u32) -> Color32 {
    let hue = (f64::from(index) * 0.618_033_988_749_895).fract();
    let (red, green, blue) = hsv(hue, 0.55, 0.62);
    Color32::from_rgb(red, green, blue)
}

/// The colour a firing node is painted: brighter than any type colour.
pub fn firing_colour() -> Color32 {
    Color32::from_rgb(255, 236, 96)
}

fn hsv(hue: f64, saturation: f64, value: f64) -> (u8, u8, u8) {
    let sector = hue * 6.0;
    let index = sector.floor();
    let fraction = sector - index;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - saturation * fraction);
    let t = value * (1.0 - saturation * (1.0 - fraction));
    let (red, green, blue) = match index as i64 % 6 {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    };
    let channel = |component: f64| (component.clamp(0.0, 1.0) * 255.0).round() as u8;
    (channel(red), channel(green), channel(blue))
}

/// Reads frames and keeps the newest where the panel can take it.
///
/// A recorded run is played back on its own clock, so the panel shows the run as
/// it happened; a live socket is read as it arrives. A disconnect reconnects
/// while the panel is up.
fn pump(source: &str, latest: Arc<Mutex<ConnectomeFrame>>, stop: Arc<AtomicBool>) {
    let recorded = !source.starts_with("tcp://");
    let mut origin_ns: Option<u64> = None;
    let mut started = Instant::now();
    let mut first_arrival: Option<Instant> = None;
    let mut connection_frames = 0u64;
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let reader: Box<dyn Read + Send> = if recorded {
            match std::fs::File::open(source) {
                Ok(file) => Box::new(std::io::BufReader::new(file)),
                Err(error) => return fail(&latest, format!("opening {source}: {error}")),
            }
        } else {
            let address = source.trim_start_matches("tcp://");
            match TcpStream::connect(address) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    Box::new(stream)
                }
                Err(error) => {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    fail(&latest, format!("connecting to {address}: {error}; retrying"));
                    std::thread::sleep(LIVE_RETRY);
                    continue;
                }
            }
        };

        let mut reader = match SpikeReader::new(reader) {
            Ok(reader) => reader,
            Err(error) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                fail(&latest, format!("reading the spike stream header: {error}"));
                if recorded { return; }
                std::thread::sleep(LIVE_RETRY);
                continue;
            }
        };

        loop {
            if stop.load(Ordering::Acquire) {
                return;
            }
            match reader.next_frame() {
                Ok(Some(frame)) => {
                    if recorded {
                        let origin = *origin_ns.get_or_insert(frame.t_ns);
                        let target = Duration::from_nanos(frame.t_ns.saturating_sub(origin));
                        if let Some(wait) = target.checked_sub(started.elapsed()) {
                            std::thread::sleep(wait);
                        }
                    }
                    let arrival = *first_arrival.get_or_insert_with(Instant::now);
                    connection_frames += 1;
                    if let Ok(mut guard) = latest.lock() {
                        if guard.ticks_read == 0 || (guard.tick, guard.t_ns) != (frame.tick, frame.t_ns) {
                            let received = Instant::now();
                            if frame.tick < guard.tick || frame.t_ns < guard.t_ns { guard.activity.clear(); }
                            guard.activity.retain(|sample| received.duration_since(sample.received) < Duration::from_secs(10));
                            if guard.activity.len() >= 512 { guard.activity.pop_front(); }
                            guard.activity.push_back(SpikeActivity {received, tick:frame.tick, count:frame.ids.len()});
                            guard.last_advanced = Some(received);
                        }
                        let read = guard.ticks_read + 1;
                        guard.tick = frame.tick;
                        guard.t_ns = frame.t_ns;
                        guard.firing = frame.ids;
                        guard.ticks_read = read;
                        guard.rate_hz =
                            connection_frames.saturating_sub(1) as f32 / arrival.elapsed().as_secs_f32().max(f32::EPSILON);
                        guard.finished = false;
                        guard.error = None;
                    }
                }
                Ok(None) => {
                    if let Ok(mut guard) = latest.lock() {
                        guard.finished = recorded;
                        if !recorded { guard.error = Some("Live spike socket closed; reconnecting".into()); }
                    }
                    if recorded { return; }
                    std::thread::sleep(LIVE_RETRY);
                    break;
                }
                Err(error) => {
                    if let Ok(mut guard) = latest.lock() {
                        guard.error = Some(format!("reading the spike stream: {error}"));
                    }
                    if recorded {
                        return;
                    }
                    std::thread::sleep(LIVE_RETRY);
                    break;
                }
            }
        }
        if recorded {
            return;
        }
        // A reconnect restarts the clock for the rate.
        origin_ns = None;
        started = Instant::now();
        first_arrival = None;
        connection_frames = 0;
    }
}

fn fail(latest: &Arc<Mutex<ConnectomeFrame>>, message: String) {
    if let Ok(mut guard) = latest.lock() {
        guard.error = Some(message);
    }
}
