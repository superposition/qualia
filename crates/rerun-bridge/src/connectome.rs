//! The connectome projection: the brain as a point cloud with the firing set
//! of the current tick highlighted.
//!
//! The input is what the connectome importer and runner produce: one placed
//! soma position and cell type per CSR node, and, per tick, the sorted firing
//! node ids. Nothing here is per-tick state of its own: the cloud is static,
//! the firing set is logged on the tick timeline, so scrubbing the timeline
//! replays the activity.
//!
//! Entity paths are stable and public:
//!
//! - `qualia/connectome/brain/points` — every placed node, one point each,
//!   coloured by cell type (logged once, static);
//! - `qualia/connectome/brain/firing` — the firing set of the current tick,
//!   larger and hotter than the cloud;
//! - `qualia/connectome/brain/summary` — the cloud's counts and palette rule;
//! - `qualia/connectome/stream/summary` — the tick, its wall clock and counts;
//! - `qualia/connectome/stream/firing_count` — firing count per tick, the
//!   curve the time-series view plots.
//!
//! The projection never carries control authority: it reads the artifact and
//! the stream and writes a recording.

use rerun::Color;

/// Stable entity paths for the connectome projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectomeEntityTaxonomy;

impl ConnectomeEntityTaxonomy {
    pub const ROOT: &'static str = "qualia/connectome";
    pub const BRAIN_ROOT: &'static str = "qualia/connectome/brain";
    pub const STREAM_ROOT: &'static str = "qualia/connectome/stream";

    pub fn root() -> &'static str {
        Self::ROOT
    }

    pub fn brain_root() -> &'static str {
        Self::BRAIN_ROOT
    }

    pub fn stream_root() -> &'static str {
        Self::STREAM_ROOT
    }

    /// The whole cloud, logged once and static.
    pub fn points() -> &'static str {
        "qualia/connectome/brain/points"
    }

    /// The firing set of the current tick.
    pub fn firing() -> &'static str {
        "qualia/connectome/brain/firing"
    }

    /// The cloud's own summary.
    pub fn cloud_summary() -> &'static str {
        "qualia/connectome/brain/summary"
    }

    /// The current tick's summary.
    pub fn stream_summary() -> &'static str {
        "qualia/connectome/stream/summary"
    }

    /// Firing count per tick, on the tick timeline.
    pub fn firing_count() -> &'static str {
        "qualia/connectome/stream/firing_count"
    }
}

/// Cloud radius in UI points. Negative radii are UI points in Rerun, which
/// keeps the points visible at every zoom of a 130 000-voxel-wide brain.
pub const CLOUD_POINT_RADIUS: f32 = -1.0;

/// Firing radius in UI points: large enough that a spike reads as a spike.
pub const FIRING_POINT_RADIUS: f32 = -3.5;

/// The firing colour: brighter than any palette entry, so a firing node does
/// not blend into its cell type.
pub fn firing_color() -> Color {
    Color::from_rgb(255, 236, 96)
}

/// A golden-ratio walk around the hue circle: a type keeps one colour across
/// runs and a table of eleven thousand labels needs no palette in the
/// recording. Saturation and value stay well below the firing colour, so a
/// firing node pops out of the cloud.
pub fn cell_type_color(index: u32) -> Color {
    let hue = (f64::from(index) * 0.618_033_988_749_895).fract();
    hsv_to_rgb(hue, 0.55, 0.62)
}

fn hsv_to_rgb(hue: f64, saturation: f64, value: f64) -> Color {
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
    Color::from_rgb(channel(red), channel(green), channel(blue))
}

/// One node of the cloud.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnectomeNeuron {
    /// CSR node index: the id a spike frame carries, and the row index in the
    /// positions artifact.
    pub node: u32,
    /// Soma position, in the dataset's EM voxel units.
    pub position: [f32; 3],
    /// Index into [`ConnectomeCloud::cell_types`].
    pub cell_type: u32,
}

/// The cloud as the artifact holds it.
pub struct ConnectomeCloud<'a> {
    /// The placed nodes, ascending by node: a node the annotations place
    /// nowhere is left out rather than drawn at a made-up coordinate.
    pub neurons: &'a [ConnectomeNeuron],
    /// Interned cell-type labels; the index is `ConnectomeNeuron::cell_type`.
    pub cell_types: &'a [String],
    /// Rows in the artifact: CSR nodes, placed or not.
    pub node_count: u32,
    /// The released table the artifact was extracted from, for the summary.
    pub source: &'a str,
    /// The units and frame the coordinates are in, in words.
    pub coordinate_space: &'a str,
}

/// One tick's slice.
pub struct ConnectomeProjection<'a> {
    pub cloud: &'a ConnectomeCloud<'a>,
    /// Firing node ids for this tick, ascending.
    pub firing: &'a [u32],
    /// Wall clock at the tick, unix nanoseconds.
    pub wall_clock_ns: u64,
}

/// The payload written at one connectome entity path.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectomeProjectionContent {
    Document(String),
    /// One point per entry, one colour per entry, one radius for all of them
    /// (UI points when negative).
    Points3D {
        positions: Vec<[f32; 3]>,
        colors: Vec<Color>,
        radius: f32,
    },
    Scalars(f64),
}

/// One entity path plus what to write there.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectomeProjectionRecord {
    pub entity_path: String,
    pub content: ConnectomeProjectionContent,
}

fn document_record(entity_path: &str, markdown: String) -> ConnectomeProjectionRecord {
    ConnectomeProjectionRecord {
        entity_path: entity_path.to_string(),
        content: ConnectomeProjectionContent::Document(markdown),
    }
}

fn points_record(
    entity_path: &str,
    positions: Vec<[f32; 3]>,
    colors: Vec<Color>,
    radius: f32,
) -> ConnectomeProjectionRecord {
    ConnectomeProjectionRecord {
        entity_path: entity_path.to_string(),
        content: ConnectomeProjectionContent::Points3D {
            positions,
            colors,
            radius,
        },
    }
}

/// The static cloud: one point per placed node, its colour from the cell-type
/// table, plus the cloud's summary. Logged once per run; the firing set is
/// what moves.
pub fn collect_connectome_cloud_records(
    cloud: &ConnectomeCloud<'_>,
) -> Vec<ConnectomeProjectionRecord> {
    let mut positions = Vec::with_capacity(cloud.neurons.len());
    let mut colors = Vec::with_capacity(cloud.neurons.len());
    for neuron in cloud.neurons {
        positions.push(neuron.position);
        colors.push(cell_type_color(neuron.cell_type));
    }
    let placed = cloud.neurons.len();
    let nodes = cloud.node_count as usize;
    vec![
        points_record(
            ConnectomeEntityTaxonomy::points(),
            positions,
            colors,
            CLOUD_POINT_RADIUS,
        ),
        document_record(
            ConnectomeEntityTaxonomy::cloud_summary(),
            format!(
                "# Connectome cloud\n\n\
                 - nodes: {nodes}\n\
                 - placed: {placed}\n\
                 - unplaced: {}\n\
                 - cell types: {}\n\
                 - colour: golden-ratio hue walk over the cell-type index, \
                 saturation 0.62, value 0.78\n\
                 - radius: {CLOUD_POINT_RADIUS} UI points\n\
                 - positions: {}\n\
                 - source: {}\n",
                nodes.saturating_sub(placed),
                cloud.cell_types.len(),
                cloud.coordinate_space,
                cloud.source
            ),
        ),
    ]
}

/// One tick: the firing set as points, its count on the timeline, and the
/// tick's summary line.
pub fn collect_connectome_tick_records(
    tick: u64,
    projection: &ConnectomeProjection<'_>,
) -> Vec<ConnectomeProjectionRecord> {
    let cloud = projection.cloud;
    let mut positions = Vec::with_capacity(projection.firing.len());
    for node in projection.firing {
        if let Some(position) = node_position(cloud, *node) {
            positions.push(position);
        }
    }
    let drawn = positions.len();
    let firing = projection.firing.len();
    vec![
        points_record(
            ConnectomeEntityTaxonomy::firing(),
            positions,
            vec![firing_color(); drawn],
            FIRING_POINT_RADIUS,
        ),
        ConnectomeProjectionRecord {
            entity_path: ConnectomeEntityTaxonomy::firing_count().to_string(),
            content: ConnectomeProjectionContent::Scalars(firing as f64),
        },
        document_record(
            ConnectomeEntityTaxonomy::stream_summary(),
            format!(
                "# Tick {tick}\n\n\
                 - wall clock: {} ns unix\n\
                 - firing: {firing}\n\
                 - drawn: {drawn}\n\
                 - firing nodes with no soma position: {}\n\
                 - nodes: {}\n",
                projection.wall_clock_ns,
                firing - drawn,
                cloud.node_count
            ),
        ),
    ]
}

/// The placed position of a node, by the node order the artifact guarantees.
fn node_position(cloud: &ConnectomeCloud<'_>, node: u32) -> Option<[f32; 3]> {
    cloud
        .neurons
        .binary_search_by_key(&node, |neuron| neuron.node)
        .ok()
        .map(|index| cloud.neurons[index].position)
}
