//! Builder tests for the connectome prior.
//!
//! The fixtures are hand-authored CSV because a human has to read them; the
//! builder reads Arrow IPC. These tests therefore synthesise Feather inputs
//! from the CSV on every run, and the CSV stays the source of truth for the
//! oracle the emitted CSR is compared against.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use arrow::array::{ArrayRef, Int64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use qualia_connectome_prior::{
    build_type_graph, build_type_graph_from_rows, build_type_graph_reporting, write_prior,
    Attribution, BodyAnnotation, PriorError, PriorSource, SegmentEdge, PRIOR_SCHEMA,
};
use sha2::{Digest, Sha256};

const MANIFEST_FILE: &str = "manifest.json";
const GRAPH_FILE: &str = "graph.bin";
const ATTRIBUTION_FILE: &str = "attribution.json";

/// One row of the hand-authored annotation fixture.
struct Annotation(i64, String, String, String, String);

/// The hand-authored fixtures, decoded from CSV.
struct Fixture {
    edges: Vec<(String, String, u64)>,
    annotations: Vec<Annotation>,
}

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn read_csv(path: &Path) -> Vec<Vec<String>> {
    let mut reader = csv::Reader::from_path(path).expect("fixture opens as CSV");
    reader
        .records()
        .map(|record| {
            record
                .expect("fixture row parses")
                .iter()
                .map(str::to_string)
                .collect()
        })
        .collect()
}

fn fixture() -> Fixture {
    let dir = fixture_dir();
    let annotations = read_csv(&dir.join("mini-body-annotations.csv"))
        .into_iter()
        .map(|row| {
            Annotation(
                row[0].parse().expect("body id is an integer"),
                row[1].clone(),
                row[2].clone(),
                row[3].clone(),
                row[4].clone(),
            )
        })
        .collect();
    let edges = read_csv(&dir.join("mini-type-edges.csv"))
        .into_iter()
        .map(|row| {
            (
                row[0].clone(),
                row[1].clone(),
                row[2].parse().expect("weight is an integer"),
            )
        })
        .collect();
    Fixture { edges, annotations }
}

fn first_body_id_by_type(fixture: &Fixture) -> HashMap<&str, i64> {
    let mut map = HashMap::new();
    for annotation in &fixture.annotations {
        map.entry(annotation.1.as_str()).or_insert(annotation.0);
    }
    map
}

fn write_feather(path: &Path, fields: Vec<Field>, columns: Vec<ArrayRef>) {
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), columns).expect("batch matches schema");
    let file = fs::File::create(path).expect("feather file created");
    let mut writer = FileWriter::try_new(file, &schema).expect("feather writer created");
    writer.write(&batch).expect("batch written");
    writer.finish().expect("feather finished");
}

/// Synthesise the Arrow IPC inputs the builder reads from the CSV fixtures.
///
/// `extra` appends raw `(body_pre, body_post, weight)` rows, so a test can
/// inject a body id that no annotation covers.
fn synth_inputs(dir: &Path, fixture: &Fixture, extra: &[(i64, i64, u64)]) -> PriorSource {
    let by_type = first_body_id_by_type(fixture);
    let mut body_pre = Vec::new();
    let mut body_post = Vec::new();
    let mut weight = Vec::new();
    for (pre, post, value) in &fixture.edges {
        body_pre.push(*by_type.get(pre.as_str()).expect("pre type is annotated"));
        body_post.push(*by_type.get(post.as_str()).expect("post type is annotated"));
        weight.push(*value as u32);
    }
    for (pre, post, value) in extra {
        body_pre.push(*pre);
        body_post.push(*post);
        weight.push(*value as u32);
    }

    let edges_feather = dir.join("weights.feather");
    write_feather(
        &edges_feather,
        vec![
            Field::new("body_pre", DataType::Int64, false),
            Field::new("body_post", DataType::Int64, false),
            Field::new("weight", DataType::UInt32, false),
        ],
        vec![
            Arc::new(Int64Array::from(body_pre)),
            Arc::new(Int64Array::from(body_post)),
            Arc::new(UInt32Array::from(weight)),
        ],
    );

    let annotations_feather = dir.join("annotations.feather");
    write_feather(
        &annotations_feather,
        vec![
            Field::new("bodyId", DataType::Int64, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("superclass", DataType::Utf8, false),
            Field::new("side", DataType::Utf8, false),
            Field::new("dimorphism", DataType::Utf8, false),
        ],
        vec![
            Arc::new(Int64Array::from(
                fixture
                    .annotations
                    .iter()
                    .map(|annotation| annotation.0)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                fixture
                    .annotations
                    .iter()
                    .map(|annotation| annotation.1.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                fixture
                    .annotations
                    .iter()
                    .map(|annotation| annotation.2.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                fixture
                    .annotations
                    .iter()
                    .map(|annotation| annotation.3.clone())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                fixture
                    .annotations
                    .iter()
                    .map(|annotation| annotation.4.clone())
                    .collect::<Vec<_>>(),
            )),
        ],
    );

    PriorSource {
        edges_feather,
        annotations_feather,
    }
}

/// Aggregate the CSV rows independently of the crate: the oracle.
fn oracle(edges: &[(String, String, u64)]) -> (Vec<String>, Vec<u64>, Vec<u32>, Vec<u32>) {
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for (pre, post, _) in edges {
        names.insert(pre.as_str());
        names.insert(post.as_str());
    }
    let types: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    let index_of: HashMap<&str, u32> = names
        .iter()
        .enumerate()
        .map(|(index, name)| (*name, index as u32))
        .collect();

    let mut sums: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for (pre, post, weight) in edges {
        *sums
            .entry((index_of[pre.as_str()], index_of[post.as_str()]))
            .or_insert(0) += weight;
    }

    let mut rowptr = vec![0u64; types.len() + 1];
    let mut cols = Vec::with_capacity(sums.len());
    let mut weights = Vec::with_capacity(sums.len());
    for ((pre, post), weight) in sums {
        cols.push(post);
        weights.push(weight as u32);
        rowptr[pre as usize + 1] += 1;
    }
    for index in 0..types.len() {
        rowptr[index + 1] += rowptr[index];
    }
    (types, rowptr, cols, weights)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in Sha256::digest(bytes) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Name, mtime, length and bytes of each artifact file, so a rewrite is visible
/// even when the clock is too coarse to move the mtime.
fn file_state(dir: &Path) -> Vec<(String, Option<SystemTime>, u64, Vec<u8>)> {
    let mut state: Vec<_> = [MANIFEST_FILE, GRAPH_FILE, ATTRIBUTION_FILE]
        .iter()
        .map(|name| {
            let path = dir.join(name);
            let metadata = fs::metadata(&path).expect("artifact file exists");
            (
                (*name).to_string(),
                metadata.modified().ok(),
                metadata.len(),
                fs::read(&path).expect("artifact file reads"),
            )
        })
        .collect();
    state.sort();
    state
}

#[test]
fn builds_csr_from_fixture() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = synth_inputs(temp.path(), &fixture, &[]);
    let graph = build_type_graph(&source).expect("fixture builds");

    assert!(graph.is_well_formed());

    // The literal expectation: 12 fixture rows, 5 types, 9 type pairs.
    assert_eq!(
        graph.types,
        vec!["DNp01", "EPG", "KCg-m", "OA-VPM3", "PEN_a"]
    );
    assert_eq!(graph.rowptr, vec![0, 2, 4, 5, 7, 9]);
    assert_eq!(graph.cols, vec![1, 2, 2, 4, 0, 0, 2, 2, 3]);
    assert_eq!(graph.weights, vec![3, 2, 8, 25, 13, 6, 11, 12, 12]);

    // And the same answer derived from the CSV alone.
    let (types, rowptr, cols, weights) = oracle(&fixture.edges);
    assert_eq!(&graph.types, &types);
    assert_eq!(&graph.rowptr, &rowptr);
    assert_eq!(&graph.cols, &cols);
    assert_eq!(&graph.weights, &weights);

    // The shared row seam the Arrow path and any other reader go through.
    let by_type = first_body_id_by_type(&fixture);
    let rows: Vec<SegmentEdge> = fixture
        .edges
        .iter()
        .map(|(pre, post, weight)| SegmentEdge {
            body_pre: by_type[pre.as_str()].to_string(),
            body_post: by_type[post.as_str()].to_string(),
            weight: *weight,
        })
        .collect();
    let annotations: Vec<BodyAnnotation> = fixture
        .annotations
        .iter()
        .map(|annotation| BodyAnnotation {
            body_id: annotation.0.to_string(),
            type_name: annotation.1.clone(),
            superclass: annotation.2.clone(),
            side: annotation.3.clone(),
            dimorphism: annotation.4.clone(),
        })
        .collect();
    let (row_graph, skipped, total) = build_type_graph_from_rows(&rows, &annotations);
    assert_eq!(skipped, 0);
    assert_eq!(total, 12);
    assert_eq!(row_graph, graph);
}

#[test]
fn emits_attribution_and_manifest() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = synth_inputs(temp.path(), &fixture, &[]);
    let graph = build_type_graph(&source).expect("fixture builds");
    let out = temp.path().join("prior");

    let manifest = write_prior(&out, &graph, &Attribution::male_cns()).expect("prior written");

    let manifest_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join(MANIFEST_FILE)).expect("manifest reads"))
            .expect("manifest is JSON");
    assert_eq!(manifest_json["schema"], PRIOR_SCHEMA);
    assert_eq!(manifest_json["type_count"], 5);
    assert_eq!(manifest_json["edge_count"], 9);
    assert_eq!(manifest.schema, PRIOR_SCHEMA);
    assert_eq!(manifest.type_count, 5);
    assert_eq!(manifest.edge_count, 9);
    assert!(manifest.created_at_ms > 0);

    let attribution_text =
        fs::read_to_string(out.join(ATTRIBUTION_FILE)).expect("attribution reads");
    let attribution_json: serde_json::Value =
        serde_json::from_str(&attribution_text).expect("attribution is JSON");
    assert_eq!(attribution_json["dataset"], "male-cns:v1.0");
    assert_eq!(attribution_json["licence"], "CC-BY-4.0");
    assert_eq!(attribution_json["url"], "https://male-cns.janelia.org");
    assert_eq!(attribution_json["citation"], "Berg et al. 2026, Cell");
    assert_eq!(manifest_json["attribution"], attribution_json);
    let decoded: Attribution =
        serde_json::from_str(&attribution_text).expect("attribution decodes");
    assert_eq!(decoded, Attribution::male_cns());
    assert_eq!(manifest.attribution, Attribution::male_cns());

    // graph.bin is rowptr then cols then weights, little-endian, in that order.
    let graph_bin = fs::read(out.join(GRAPH_FILE)).expect("graph reads");
    let rowptr_len = graph.rowptr.len() * 8;
    let cols_len = graph.cols.len() * 4;
    assert_eq!(graph_bin.len(), rowptr_len + cols_len + graph.weights.len() * 4);
    assert_eq!(
        &graph_bin[..rowptr_len],
        graph
            .rowptr
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>()
            .as_slice()
    );
    assert_eq!(
        &graph_bin[rowptr_len..rowptr_len + cols_len],
        graph
            .cols
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>()
            .as_slice()
    );

    // Every digest is over the bytes it names.
    assert_eq!(manifest_json["source_sha256"], sha256_hex(&graph_bin));
    assert_eq!(
        manifest_json["rowptr_sha256"],
        sha256_hex(&graph_bin[..rowptr_len])
    );
    assert_eq!(
        manifest_json["cols_sha256"],
        sha256_hex(&graph_bin[rowptr_len..rowptr_len + cols_len])
    );
    assert_eq!(
        manifest_json["weights_sha256"],
        sha256_hex(&graph_bin[rowptr_len + cols_len..])
    );
}

#[test]
fn rerun_is_idempotent() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = synth_inputs(temp.path(), &fixture, &[]);
    let graph = build_type_graph(&source).expect("fixture builds");
    let out = temp.path().join("prior");

    write_prior(&out, &graph, &Attribution::male_cns()).expect("first write succeeds");
    let before = file_state(&out);

    let error = write_prior(&out, &graph, &Attribution::male_cns()).expect_err("second write refused");
    assert!(matches!(error, PriorError::AlreadyBuilt));
    assert_eq!(file_state(&out), before);
}

#[test]
fn skips_unmapped_rows_and_warns() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("tempdir");
    // Body 99999 is in no annotation, so its row cannot be mapped.
    let source = synth_inputs(temp.path(), &fixture, &[(99999, 10001, 7)]);

    let (graph, warning) = build_type_graph_reporting(&source).expect("unmapped rows are not fatal");
    assert_eq!(graph.cols.len(), 9);
    assert_eq!(
        warning,
        Some(PriorError::UnmappedRows {
            skipped: 1,
            total: 13
        })
    );
    let text = warning.expect("warning present").to_string();
    assert!(text.contains('1') && text.contains("13"), "{text}");

    // The plain entry point is equally non-fatal.
    let graph = build_type_graph(&source).expect("unmapped rows are not fatal");
    assert_eq!(graph.cols.len(), 9);

    // A fully mapped build carries no warning at all.
    let clean = synth_inputs(temp.path(), &fixture, &[]);
    let (_, warning) = build_type_graph_reporting(&clean).expect("mapped fixture builds");
    assert_eq!(warning, None);
}

#[test]
fn missing_input_is_a_read_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = PriorSource {
        edges_feather: temp.path().join("absent-weights.feather"),
        annotations_feather: temp.path().join("absent-annotations.feather"),
    };
    assert!(matches!(build_type_graph(&source), Err(PriorError::Read(_))));
}

#[test]
fn missing_column_is_a_read_error() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = synth_inputs(temp.path(), &fixture, &[]);
    let incomplete = temp.path().join("weights-without-weight.feather");
    write_feather(
        &incomplete,
        vec![
            Field::new("body_pre", DataType::Int64, false),
            Field::new("body_post", DataType::Int64, false),
        ],
        vec![
            Arc::new(Int64Array::from(vec![10001_i64, 10004])),
            Arc::new(Int64Array::from(vec![10004_i64, 10007])),
        ],
    );
    let broken = PriorSource {
        edges_feather: incomplete,
        annotations_feather: source.annotations_feather,
    };
    assert!(matches!(build_type_graph(&broken), Err(PriorError::Read(_))));
}

#[test]
fn binary_rerun_is_idempotent() {
    let fixture = fixture();
    let temp = tempfile::tempdir().expect("tempdir");
    let source = synth_inputs(temp.path(), &fixture, &[]);
    let out = temp.path().join("prior-bin");
    let binary = env!("CARGO_BIN_EXE_qualia-connectome-prior");

    let first = Command::new(binary)
        .arg("--edges")
        .arg(&source.edges_feather)
        .arg("--annotations")
        .arg(&source.annotations_feather)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("binary runs");
    assert!(first.status.success(), "{first:?}");
    assert_eq!(
        String::from_utf8_lossy(&first.stdout).trim(),
        format!("prior: 5 types, 9 edges -> {}", out.display())
    );

    let before = file_state(&out);
    let second = Command::new(binary)
        .arg("--edges")
        .arg(&source.edges_feather)
        .arg("--annotations")
        .arg(&source.annotations_feather)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("binary runs");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already built"));
    assert_eq!(file_state(&out), before);
}

