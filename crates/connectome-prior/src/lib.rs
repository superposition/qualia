//! Type-level connectome prior. See README.md.
//!
//! Segment-level edges from the Male CNS weights file are aggregated to type
//! level through the body-annotation table and emitted as CSR with `u32` type
//! indices. The builder reads Arrow IPC (Feather) and writes a verified
//! artifact: `graph.bin` (rowptr, then cols, then weights, little-endian), a
//! `manifest.json` carrying per-section SHA-256 digests, and an
//! `attribution.json` holding the CC-BY attribution required by `NOTICE`.
//!
//! `edge_count` is the number of *type-level* edges in the CSR, i.e.
//! `cols.len()`: the segment rows that aggregate into one type pair are counted
//! once. That is the only meaning `write_prior` can reconstruct, because it is
//! handed the graph and nothing else.
//!
//! Nothing in this crate runs in the motor path, and nothing here carries
//! dynamics: it produces a graph, and only a graph.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use arrow::array::{
    Array, Float32Array, Float64Array, Int32Array, Int64Array, UInt32Array, UInt64Array,
};
use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::{FileReader, StreamReader};
use arrow::record_batch::RecordBatch;

/// Schema literal carried by every emitted `manifest.json`.
pub const PRIOR_SCHEMA: &str = "qualia.connectome-prior.v1";


/// A failed prior build.
///
/// `UnmappedRows` is the one variant a successful build can still carry: rows
/// whose body id is absent from the annotation table are dropped, counted, and
/// reported as a warning, never as a hard failure.
#[derive(Debug, PartialEq, Eq)]
pub enum PriorError {
    /// A Feather input was missing, unreadable, not Arrow IPC, or lacked a
    /// required column.
    Read(String),
    /// `skipped` of `total` segment rows named a body id with no annotation.
    UnmappedRows { skipped: u64, total: u64 },
    /// The target directory already holds a prior built from the same graph.
    AlreadyBuilt,
}

impl Display for PriorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(detail) => write!(formatter, "read: {detail}"),
            Self::UnmappedRows { skipped, total } => write!(
                formatter,
                "unmapped rows: {skipped} of {total} segment rows skipped"
            ),
            Self::AlreadyBuilt => {
                write!(formatter, "prior already built from the same graph")
            }
        }
    }
}

impl Error for PriorError {}

/// Where a prior build reads its inputs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorSource {
    /// Segment-level weights, Arrow IPC: `body_pre`, `body_post`, `weight`.
    pub edges_feather: PathBuf,
    /// Body annotations, Arrow IPC: `bodyId`, `type`, `superclass`, `side`,
    /// `dimorphism`.
    pub annotations_feather: PathBuf,
}

/// The type-level graph in compressed sparse row form.
///
/// `rowptr` has one more entry than `types`; row `i` spans
/// `rowptr[i]..rowptr[i + 1]` of `cols` and `weights`, and `cols[j]` is the
/// post-synaptic type index of edge `j`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeGraph {
    /// Type names, sorted, in CSR row order.
    pub types: Vec<String>,
    /// Row offsets, length `types.len() + 1`.
    pub rowptr: Vec<u64>,
    /// Post-synaptic type index per edge.
    pub cols: Vec<u32>,
    /// Aggregated weight per edge.
    pub weights: Vec<u32>,
}

impl TypeGraph {
    /// Number of type-level edges.
    pub fn edge_count(&self) -> u64 {
        self.cols.len() as u64
    }

    /// Whether the CSR invariants hold.
    pub fn is_well_formed(&self) -> bool {
        self.rowptr.len() == self.types.len() + 1
            && self.cols.len() == self.weights.len()
            && self.rowptr.first().copied() == Some(0)
            && self.rowptr.last().copied() == Some(self.cols.len() as u64)
            && self.rowptr.windows(2).all(|pair| pair[0] <= pair[1])
            && self.cols.iter().all(|column| (*column as usize) < self.types.len())
    }
}

/// One segment-level edge as read from the weights file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentEdge {
    /// Pre-synaptic body id.
    pub body_pre: String,
    /// Post-synaptic body id.
    pub body_post: String,
    /// Segment weight.
    pub weight: u64,
}

/// One body annotation as read from the annotations file.
///
/// `type_name` alone drives the aggregation; the remaining columns are part of
/// the source contract and are validated on read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyAnnotation {
    /// Body id this annotation describes.
    pub body_id: String,
    /// Type-level cell type.
    pub type_name: String,
    /// Coarse superclass.
    pub superclass: String,
    /// Left/right side, where the dataset records one.
    pub side: String,
    /// `male-specific`, `unisex`, or the dataset's other labels.
    pub dimorphism: String,
}

/// Build the type-level graph from the Arrow IPC inputs.
///
/// Rows naming a body id with no annotation are skipped and reported through
/// `eprintln!` as [`PriorError::UnmappedRows`]; they are never a hard failure.
/// Use [`build_type_graph_reporting`] to receive the warning instead.
pub fn build_type_graph(src: &PriorSource) -> Result<TypeGraph, PriorError> {
    let (graph, warning) = build_type_graph_reporting(src)?;
    if let Some(warning) = warning {
        eprintln!("connectome-prior: warning: {warning}");
    }
    Ok(graph)
}

/// Build the type-level graph and return any warning alongside it.
///
/// The second element is `Some(PriorError::UnmappedRows { .. })` exactly when
/// at least one segment row named an unannotated body id.
pub fn build_type_graph_reporting(
    src: &PriorSource,
) -> Result<(TypeGraph, Option<PriorError>), PriorError> {
    let edges = read_segment_edges(&src.edges_feather)?;
    let annotations = read_body_annotations(&src.annotations_feather)?;
    let (graph, skipped, total) = build_type_graph_from_rows(&edges, &annotations);
    let warning = (skipped > 0).then_some(PriorError::UnmappedRows { skipped, total });
    Ok((graph, warning))
}

/// Aggregate already-decoded rows to type level.
///
/// This is the seam the Arrow IPC path and any other reader share: both
/// endpoints of every segment edge are mapped through `annotations`, weights
/// are summed per `(pre_type, post_type)` pair, the pairs are sorted by
/// `(pre_type, post_type)`, and the result is emitted as CSR. Returns the graph
/// and the `(skipped, total)` row counts.
pub fn build_type_graph_from_rows(
    edges: &[SegmentEdge],
    annotations: &[BodyAnnotation],
) -> (TypeGraph, u64, u64) {
    let mut type_names: BTreeSet<&str> = BTreeSet::new();
    for annotation in annotations {
        type_names.insert(annotation.type_name.as_str());
    }
    let types: Vec<String> = type_names.iter().map(|name| (*name).to_string()).collect();
    let index_of: HashMap<&str, u32> = type_names
        .iter()
        .enumerate()
        .map(|(index, name)| (*name, index as u32))
        .collect();

    let mut body_type: HashMap<&str, u32> = HashMap::with_capacity(annotations.len());
    for annotation in annotations {
        if let Some(index) = index_of.get(annotation.type_name.as_str()) {
            body_type.insert(annotation.body_id.as_str(), *index);
        }
    }

    let mut sums: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    let mut skipped = 0u64;
    for edge in edges {
        match (
            body_type.get(edge.body_pre.as_str()),
            body_type.get(edge.body_post.as_str()),
        ) {
            (Some(pre), Some(post)) => {
                *sums.entry((*pre, *post)).or_insert(0) += edge.weight;
            }
            _ => skipped += 1,
        }
    }

    let mut rowptr = vec![0u64; types.len() + 1];
    let mut cols = Vec::with_capacity(sums.len());
    let mut weights = Vec::with_capacity(sums.len());
    for ((pre, post), weight) in sums {
        cols.push(post);
        weights.push(weight.min(u64::from(u32::MAX)) as u32);
        rowptr[pre as usize + 1] += 1;
    }
    for index in 0..types.len() {
        rowptr[index + 1] += rowptr[index];
    }

    (
        TypeGraph {
            types,
            rowptr,
            cols,
            weights,
        },
        skipped,
        edges.len() as u64,
    )
}

fn read_error(path: &Path, error: std::io::Error) -> PriorError {
    PriorError::Read(format!("{}: {error}", path.display()))
}

fn decode_error(path: &Path, error: arrow::error::ArrowError) -> PriorError {
    PriorError::Read(format!("{}: {error}", path.display()))
}

/// Read the weights file into segment edges, requiring all three columns.
fn read_segment_edges(path: &Path) -> Result<Vec<SegmentEdge>, PriorError> {
    let table = FeatherTable::read(path)?;
    let body_pre = table.strings("body_pre")?;
    let body_post = table.strings("body_post")?;
    let weight = table.unsigned("weight")?;
    if body_pre.len() != body_post.len() || body_pre.len() != weight.len() {
        return Err(PriorError::Read(format!(
            "{}: columns disagree on row count ({} pre, {} post, {} weight)",
            path.display(),
            body_pre.len(),
            body_post.len(),
            weight.len()
        )));
    }
    Ok(body_pre
        .into_iter()
        .zip(body_post)
        .zip(weight)
        .map(|((body_pre, body_post), weight)| SegmentEdge {
            body_pre,
            body_post,
            weight,
        })
        .collect())
}

/// Read the annotations file, requiring all five columns.
fn read_body_annotations(path: &Path) -> Result<Vec<BodyAnnotation>, PriorError> {
    let table = FeatherTable::read(path)?;
    let body_id = table.strings("bodyId")?;
    let type_name = table.strings("type")?;
    let superclass = table.strings("superclass")?;
    let side = table.strings("side")?;
    let dimorphism = table.strings("dimorphism")?;
    if [body_id.len(), type_name.len(), superclass.len(), side.len(), dimorphism.len()]
        .windows(2)
        .any(|pair| pair[0] != pair[1])
    {
        return Err(PriorError::Read(format!(
            "{}: annotation columns disagree on row count",
            path.display()
        )));
    }
    Ok((0..body_id.len())
        .map(|row| BodyAnnotation {
            body_id: body_id[row].clone(),
            type_name: type_name[row].clone(),
            superclass: superclass[row].clone(),
            side: side[row].clone(),
            dimorphism: dimorphism[row].clone(),
        })
        .collect())
}

/// An Arrow IPC table, read as either the file or the stream framing.
struct FeatherTable {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
}

impl FeatherTable {
    fn read(path: &Path) -> Result<Self, PriorError> {
        let open = || fs::File::open(path).map_err(|error| read_error(path, error));
        if let Ok(reader) = FileReader::try_new(open()?, None) {
            let schema = reader.schema();
            let batches = reader
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| decode_error(path, error))?;
            return Ok(Self { schema, batches });
        }
        let reader = StreamReader::try_new(open()?, None)
            .map_err(|error| PriorError::Read(format!("{}: not Arrow IPC: {error}", path.display())))?;
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| decode_error(path, error))?;
        Ok(Self { schema, batches })
    }

    /// The column index, or a `Read` error naming the missing column.
    fn require(&self, name: &str) -> Result<usize, PriorError> {
        self.schema
            .index_of(name)
            .map_err(|_| PriorError::Read(format!("missing column `{name}`")))
    }

    fn strings(&self, name: &str) -> Result<Vec<String>, PriorError> {
        let index = self.require(name)?;
        let mut out = Vec::new();
        for batch in &self.batches {
            let column = batch.column(index).as_ref();
            for row in 0..column.len() {
                out.push(cell_string(column, row)?);
            }
        }
        Ok(out)
    }

    fn unsigned(&self, name: &str) -> Result<Vec<u64>, PriorError> {
        let index = self.require(name)?;
        let mut out = Vec::new();
        for batch in &self.batches {
            let column = batch.column(index).as_ref();
            for row in 0..column.len() {
                out.push(cell_u64(column, row)?);
            }
        }
        Ok(out)
    }
}

fn cell_string(array: &dyn Array, row: usize) -> Result<String, PriorError> {
    if array.is_null(row) {
        return Ok(String::new());
    }
    arrow::util::display::array_value_to_string(array, row)
        .map_err(|error| PriorError::Read(format!("value at row {row}: {error}")))
}

fn cell_u64(array: &dyn Array, row: usize) -> Result<u64, PriorError> {
    if array.is_null(row) {
        return Ok(0);
    }
    macro_rules! integral {
        ($ty:ty) => {
            if let Some(values) = array.as_any().downcast_ref::<$ty>() {
                return Ok(values.value(row).max(0) as u64);
            }
        };
    }
    macro_rules! float {
        ($ty:ty) => {
            if let Some(values) = array.as_any().downcast_ref::<$ty>() {
                let value = f64::from(values.value(row));
                return Ok(if value > 0.0 { value.round() as u64 } else { 0 });
            }
        };
    }
    integral!(Int64Array);
    integral!(Int32Array);
    integral!(UInt64Array);
    integral!(UInt32Array);
    float!(Float64Array);
    float!(Float32Array);
    let text = cell_string(array, row)?;
    text.trim()
        .parse::<u64>()
        .map_err(|_| PriorError::Read(format!("weight column is not numeric: `{text}`")))
}
