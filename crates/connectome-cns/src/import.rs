//! Reading the released Male CNS tables and mapping them to the artifact.
//!
//! The release's flat tables are *segment*-level, not neuron-level: the
//! annotation table names 166,700 neurons, while `body-stats` describes
//! 88,384,522 segments and the weights table spans them. The importer therefore
//! maps through the annotation table exactly as `connectome-prior` does: a
//! weights row becomes an edge only when *both* its ends are released neurons.
//! Everything else is counted and reported, never silently dropped.
//!
//! The published totals the artifact must reproduce are recorded from the
//! source itself: the weights table's row count and the sum of its `weight`
//! column, which is the release's own postsynaptic-site total (it equals the
//! sum of `post` in `body-stats`, which is how the two tables cross-check).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use arrow::array::{Array, Float64Array, Int32Array, Int64Array, UInt32Array, UInt64Array};
use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::{FileReader, StreamReader};
use arrow::record_batch::RecordBatch;

use crate::{
    file_digest, sign_of, write_artifact, CnsError, CnsGraph, Manifest, NodeMeta, Neuron, SourceFile,
};

/// Where an import reads its inputs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `connectome-weights-*.feather`: `body_pre`, `body_post`, `weight`.
    pub weights: std::path::PathBuf,
    /// `body-annotations-*.feather`: `bodyId`, `type`, `superclass`,
    /// `somaLocation`.
    pub annotations: std::path::PathBuf,
    /// `body-neurotransmitters-*.feather`: `body`, `consensus_nt`,
    /// `predicted_nt`.
    pub neurotransmitters: std::path::PathBuf,
}

/// What one import consumed and produced.
#[derive(Debug, Clone)]
pub struct ImportReport {
    /// The manifest written.
    pub manifest: Manifest,
    /// Rows read from the weights table.
    pub source_rows: u64,
    /// Synapses the weights table reports over all rows.
    pub source_synapses: u64,
    /// Rows whose pre end is not a released neuron.
    pub dropped_pre: u64,
    /// Rows whose post end is not a released neuron.
    pub dropped_post: u64,
    /// Rows whose pre and post ends are both released neurons.
    pub mapped_rows: u64,
    /// Duplicate `(pre, post)` rows folded into one edge.
    pub folded_rows: u64,
    /// Neuron bodies the annotation table names.
    pub neurons: u64,
    /// Distinct presynaptic bodies in the weights table.
    pub distinct_pre_bodies: u64,
}

/// Import the released tables and write the artifact into `out`.
pub fn import(src: &Source, out: &Path) -> Result<ImportReport, CnsError> {
    let annotation_table = FeatherTable::read(&src.annotations)?;
    let neuron_rows = read_neurons(&annotation_table)?;

    let transmitter_table = FeatherTable::read(&src.neurotransmitters)?;
    let transmitters = read_transmitters(&transmitter_table)?;

    let mut neuron_index: HashMap<u64, u32> = HashMap::with_capacity(neuron_rows.len());
    let mut neurons: Vec<Neuron> = Vec::with_capacity(neuron_rows.len());
    let mut types: BTreeSet<String> = BTreeSet::new();
    for row in &neuron_rows {
        types.insert(row.type_name.clone());
    }
    let types: Vec<String> = types.into_iter().collect();
    let mut type_index: HashMap<&str, u32> = HashMap::with_capacity(types.len());
    for (index, name) in types.iter().enumerate() {
        type_index.insert(name.as_str(), index as u32);
    }
    for row in &neuron_rows {
        let index = neurons.len() as u32;
        neuron_index.insert(row.body_id, index);
        neurons.push(Neuron {
            body_id: row.body_id,
            type_index: type_index[row.type_name.as_str()],
            soma: row.soma,
        });
    }

    let mut nodes: Vec<NodeMeta> = Vec::with_capacity(neurons.len());
    for row in &neuron_rows {
        nodes.push(NodeMeta {
            body_id: row.body_id,
            type_name: row.type_name.clone(),
            superclass: row.superclass.clone(),
            side: row.side.clone(),
            instance: row.instance.clone(),
        });
    }

    // The weights table is 1.0 GB and 152M rows: it is streamed, never held.
    let mut sums: HashMap<(u32, u32), u32> = HashMap::new();
    let mut source_rows = 0u64;
    let mut source_synapses = 0u64;
    let mut dropped_pre = 0u64;
    let mut dropped_post = 0u64;
    let mut mapped_rows = 0u64;
    let mut distinct_pre: HashSet<u64> = HashSet::new();
    for_each_batch(&src.weights, |batch, index| {
        let pre = batch.column(index("body_pre")?);
        let post = batch.column(index("body_post")?);
        let weight = batch.column(index("weight")?);
        // The release writes all three columns as int64 with no nulls, so the
        // row loop can walk value slices instead of doing three typed
        // downcasts per cell: 152M rows is 456M cells.
        let (pre, post, weight) = match (
            int64_values(pre),
            int64_values(post),
            int64_values(weight),
        ) {
            (Some(pre), Some(post), Some(weight)) => (pre, post, weight),
            _ => {
                for row in 0..batch.num_rows() {
                    let pre_id = cell_u64(pre, row)?;
                    let post_id = cell_u64(post, row)?;
                    let synapses = cell_u64(weight, row)?;
                    count_row(
                        pre_id,
                        post_id,
                        synapses,
                        &neuron_index,
                        &mut sums,
                        &mut distinct_pre,
                        &mut source_rows,
                        &mut source_synapses,
                        &mut dropped_pre,
                        &mut dropped_post,
                        &mut mapped_rows,
                    );
                }
                return Ok(());
            }
        };
        for row in 0..pre.len() {
            count_row(
                pre[row].max(0) as u64,
                post[row].max(0) as u64,
                weight[row].max(0) as u64,
                &neuron_index,
                &mut sums,
                &mut distinct_pre,
                &mut source_rows,
                &mut source_synapses,
                &mut dropped_pre,
                &mut dropped_post,
                &mut mapped_rows,
            );
        }
        Ok(())
    })?;
    let folded_rows = mapped_rows - sums.len() as u64;

    // CSR: rows are the neurons in ascending bodyid order, which is the order
    // `neurons` is already in because the annotation table is read in id order.
    let mut rowptr = vec![0u64; neurons.len() + 1];
    let mut edges: Vec<((u32, u32), u32)> = sums.into_iter().collect();
    edges.sort_unstable_by_key(|((pre, post), _)| (*pre, *post));
    let mut cols = Vec::with_capacity(edges.len());
    let mut weight = Vec::with_capacity(edges.len());
    for ((pre, post), synapses) in edges {
        cols.push(post);
        weight.push(synapses.min(u32::from(u16::MAX)) as u16);
        rowptr[pre as usize + 1] += 1;
    }
    for index in 0..neurons.len() {
        rowptr[index + 1] += rowptr[index];
    }
    let sign: Vec<i8> = {
        let mut per_neuron = vec![0i8; neurons.len()];
        for (index, neuron) in neurons.iter().enumerate() {
            per_neuron[index] = transmitters
                .get(&neuron.body_id)
                .map_or(0, |label| sign_of(label));
        }
        let mut out = Vec::with_capacity(cols.len());
        let mut pre = 0usize;
        for edge in 0..cols.len() {
            while rowptr[pre + 1] as usize <= edge {
                pre += 1;
            }
            out.push(per_neuron[pre]);
        }
        out
    };

    let graph = CnsGraph {
        neuron_ids: neurons.iter().map(|neuron| neuron.body_id).collect(),
        rowptr,
        cols,
        sign,
        weight,
    };
    if !graph.is_well_formed() {
        return Err(CnsError::Malformed("imported CSR is not well formed".into()));
    }
    let presynaptic_neurons = graph.rowptr.windows(2).filter(|pair| pair[0] != pair[1]).count() as u64;

    let mut sources = BTreeMap::new();
    for (name, path, rows) in [
        (
            "connectome_weights",
            &src.weights,
            source_rows,
        ),
        (
            "body_annotations",
            &src.annotations,
            annotation_table.rows(),
        ),
        (
            "body_neurotransmitters",
            &src.neurotransmitters,
            transmitter_table.rows(),
        ),
    ] {
        let (bytes, sha256) = file_digest(path)?;
        sources.insert(
            name.to_string(),
            SourceFile {
                file: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                bytes,
                sha256,
                rows,
            },
        );
    }

    let manifest = write_artifact(
        out,
        &graph,
        &neurons,
        &nodes,
        &types,
        sources,
        source_rows,
        source_synapses,
        // Rows that are not an edge, a fold, or nothing: exactly the rows
        // where at least one end is not a released neuron. `dropped_pre` and
        // `dropped_post` are the per-end diagnostics reported alongside.
        source_rows - mapped_rows,
        folded_rows,
        presynaptic_neurons,
    )?;

    Ok(ImportReport {
        manifest,
        source_rows,
        source_synapses,
        dropped_pre,
        dropped_post,
        mapped_rows,
        folded_rows,
        neurons: neurons.len() as u64,
        distinct_pre_bodies: distinct_pre.len() as u64,
    })
}

/// One row of the annotation table that names a neuron.
#[derive(Debug, Clone, PartialEq)]
struct NeuronRow {
    body_id: u64,
    type_name: String,
    superclass: String,
    side: String,
    instance: String,
    soma: Option<[f32; 3]>,
}

/// Read the annotation table, keeping the rows that name a neuron.
///
/// The release marks a neuron by carrying a `superclass`; rows without one are
/// glia, trachea and unnamed fragments, and they are not nodes of the
/// connectome.
fn read_neurons(table: &FeatherTable) -> Result<Vec<NeuronRow>, CnsError> {
    let body_id = table.index("bodyId")?;
    let type_name = table.index("type")?;
    let superclass = table.index("superclass")?;
    let side = table.index("somaSide")?;
    let instance = table.index("instance")?;
    let soma = table.index("somaLocation")?;
    let mut out = Vec::new();
    for batch in &table.batches {
        let ids = batch.column(body_id);
        let names = batch.column(type_name);
        let classes = batch.column(superclass);
        let sides = batch.column(side);
        let instances = batch.column(instance);
        let somas = batch.column(soma);
        for row in 0..batch.num_rows() {
            if classes.is_null(row) {
                continue;
            }
            out.push(NeuronRow {
                body_id: cell_u64(ids, row)?,
                type_name: cell_string(names, row)?,
                superclass: cell_string(classes, row)?,
                side: cell_string(sides, row)?,
                instance: cell_string(instances, row)?,
                soma: cell_soma(somas, row)?,
            });
        }
    }
    out.sort_unstable_by_key(|row| row.body_id);
    out.dedup_by_key(|row| row.body_id);
    Ok(out)
}

/// Read the neurotransmitter table into `body id → label`.
///
/// `consensus_nt` wins when the release reports one; otherwise the per-neuron
/// `predicted_nt` is used. The label is passed through verbatim, so the sign
/// mapping in [`crate::sign_of`] is the only place polarity is decided.
fn read_transmitters(table: &FeatherTable) -> Result<HashMap<u64, String>, CnsError> {
    let body = table.index("body")?;
    let consensus = table.index("consensus_nt")?;
    let predicted = table.index("predicted_nt")?;
    let mut out = HashMap::new();
    for batch in &table.batches {
        let bodies = batch.column(body);
        let consensus = batch.column(consensus);
        let predicted = batch.column(predicted);
        for row in 0..batch.num_rows() {
            let id = cell_u64(bodies, row)?;
            let label = if !consensus.is_null(row) && !cell_string(consensus, row)?.is_empty() {
                cell_string(consensus, row)?
            } else if !predicted.is_null(row) {
                cell_string(predicted, row)?
            } else {
                String::new()
            };
            out.insert(id, label);
        }
    }
    Ok(out)
}

fn cell_soma(array: &dyn Array, row: usize) -> Result<Option<[f32; 3]>, CnsError> {
    if array.is_null(row) {
        return Ok(None);
    }
    let text = arrow::util::display::array_value_to_string(array, row)
        .map_err(|error| CnsError::Read(format!("somaLocation at row {row}: {error}")))?;
    let trimmed = text.trim().trim_start_matches('[').trim_end_matches(']');
    let mut values = [0f32; 3];
    let mut count = 0;
    for part in trimmed.split(',') {
        if count == 3 {
            break;
        }
        values[count] = part.trim().parse::<f32>().unwrap_or(f32::NAN);
        count += 1;
    }
    if count == 3 && values.iter().all(|value| value.is_finite()) {
        Ok(Some(values))
    } else {
        Ok(None)
    }
}

fn cell_string(array: &dyn Array, row: usize) -> Result<String, CnsError> {
    if array.is_null(row) {
        return Ok(String::new());
    }
    arrow::util::display::array_value_to_string(array, row)
        .map_err(|error| CnsError::Read(format!("value at row {row}: {error}")))
}

fn cell_u64(array: &dyn Array, row: usize) -> Result<u64, CnsError> {
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
    let text = cell_string(array, row)?;
    text.trim()
        .parse::<u64>()
        .map_err(|_| CnsError::Read(format!("not numeric: `{text}`")))
}

/// An Arrow IPC table, read as either the file or the stream framing.
struct FeatherTable {
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
}

impl FeatherTable {
    fn read(path: &Path) -> Result<Self, CnsError> {
        Self::open(path)
    }

    fn open(path: &Path) -> Result<Self, CnsError> {
        let open = || std::fs::File::open(path).map_err(|error| crate::read_error(path, error));
        let file_error = match FileReader::try_new(open()?, None) {
            Ok(reader) => {
                let schema = reader.schema();
                let batches = reader
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| decode_error(path, error))?;
                return Ok(Self { schema, batches });
            }
            Err(error) => error,
        };
        let reader = StreamReader::try_new(open()?, None).map_err(|error| {
            CnsError::Read(format!(
                "{}: not Arrow IPC: {error} (file framing: {file_error})",
                path.display()
            ))
        })?;
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| decode_error(path, error))?;
        Ok(Self { schema, batches })
    }

    fn index(&self, name: &str) -> Result<usize, CnsError> {
        self.schema
            .index_of(name)
            .map_err(|_| CnsError::Read(format!("missing column `{name}`")))
    }

    fn rows(&self) -> u64 {
        self.batches.iter().map(|batch| batch.num_rows() as u64).sum()
    }
}

fn decode_error(path: &Path, error: arrow::error::ArrowError) -> CnsError {
    CnsError::Read(format!("{}: {error}", path.display()))
}

/// The raw `i64` values of a column when it is a non-null `int64` array.
fn int64_values(array: &dyn Array) -> Option<&[i64]> {
    if array.null_count() != 0 {
        return None;
    }
    array
        .as_any()
        .downcast_ref::<Int64Array>()
        .map(|values| &values.values()[..])
}

/// Fold one weights row into the mapped edge set and the running totals.
#[allow(clippy::too_many_arguments)]
fn count_row(
    pre_id: u64,
    post_id: u64,
    synapses: u64,
    neuron_index: &HashMap<u64, u32>,
    sums: &mut HashMap<(u32, u32), u32>,
    distinct_pre: &mut HashSet<u64>,
    source_rows: &mut u64,
    source_synapses: &mut u64,
    dropped_pre: &mut u64,
    dropped_post: &mut u64,
    mapped_rows: &mut u64,
) {
    *source_rows += 1;
    *source_synapses += synapses;
    distinct_pre.insert(pre_id);
    match (neuron_index.get(&pre_id), neuron_index.get(&post_id)) {
        (Some(from), Some(to)) => {
            *mapped_rows += 1;
            let entry = sums.entry((*from, *to)).or_insert(0);
            *entry = entry.saturating_add(synapses.min(u64::from(u32::MAX)) as u32);
        }
        (None, Some(_)) => *dropped_pre += 1,
        (Some(_), None) => *dropped_post += 1,
        (None, None) => {
            *dropped_pre += 1;
            *dropped_post += 1;
        }
    }
}

/// Stream an Arrow IPC table, calling `visit` once per record batch.
///
/// `visit` receives the batch and a column-index lookup; returning an error
/// stops the walk. Nothing larger than one batch is ever resident, which is
/// what lets the 152M-row weights table be read on a 3.6 GB board.
fn for_each_batch<F>(path: &Path, mut visit: F) -> Result<(), CnsError>
where
    F: FnMut(&RecordBatch, &dyn Fn(&str) -> Result<usize, CnsError>) -> Result<(), CnsError>,
{
    let open = || std::fs::File::open(path).map_err(|error| crate::read_error(path, error));
    let lookup = |schema: &SchemaRef| {
        let schema = schema.clone();
        move |name: &str| {
            schema
                .index_of(name)
                .map_err(|_| CnsError::Read(format!("missing column `{name}`")))
        }
    };
    if let Ok(reader) = FileReader::try_new(open()?, None) {
        let index = lookup(&reader.schema());
        for batch in reader {
            let batch = batch.map_err(|error| decode_error(path, error))?;
            visit(&batch, &index)?;
        }
        return Ok(());
    }
    let reader = StreamReader::try_new(open()?, None)
        .map_err(|error| CnsError::Read(format!("{}: not Arrow IPC: {error}", path.display())))?;
    let index = lookup(&reader.schema());
    for batch in reader {
        let batch = batch.map_err(|error| decode_error(path, error))?;
        visit(&batch, &index)?;
    }
    Ok(())
}
