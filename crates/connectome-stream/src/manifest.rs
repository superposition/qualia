//! The manifest beside a positions artifact, and the check that the artifact
//! matches it.
//!
//! The manifest is the only text file in either format, and it stays small: it
//! carries the counts, the source table's digest and counts, and one entry per
//! binary section with its byte range and digest. The artifact's writer
//! (`crates/connectome-cns`) owns this file; this reader is deliberately
//! tolerant, because the check matters more than the spelling:
//!
//! - every field of [`PositionsManifest`] reads with a default, so a manifest
//!   carrying more (or less) than this crate knows about still reads;
//! - [`verify_positions`] looks the counts and the section digests up under
//!   either spelling — this crate's `sections[]`/`bin`/`types`, or the
//!   importer's `positions_sections{}`/`types_sha256` — and reports only real
//!   disagreements, counting how many sections it was able to check.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::positions::{read_positions, resolve_positions_paths, HEADER_BYTES};
use crate::{Result, StreamError, POSITIONS_MANIFEST_FILE};

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn file_digest(path: &Path) -> Result<(u64, String)> {
    let bytes = std::fs::read(path)
        .map_err(|error| StreamError::io(&format!("failed to read {}", path.display()), error))?;
    Ok((bytes.len() as u64, hex_digest(&bytes)))
}

/// One binary section: where it sits, what it hashes to, the range it spans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionManifest {
    pub name: String,
    pub offset: u64,
    pub bytes: u64,
    pub sha256: String,
    /// Smallest finite value, or `null` when the section has none (the id
    /// section, or coordinates that are all unplaced).
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

/// One file of the artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ArtifactFile {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The interned label table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TypeTable {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
    pub labels: u64,
}

/// The released table the artifact was extracted from, with its counts, so the
/// coverage can be read off the artifact without re-reading the dataset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SourceManifest {
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
    /// Bodies in the annotation table.
    pub bodies: u64,
    /// Bodies carrying a soma position.
    pub positioned: u64,
    /// Distinct cell-type labels in the table.
    pub type_labels: u64,
}

/// `manifest.json` as this crate writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PositionsManifest {
    #[serde(default)]
    pub schema: String,
    /// Rows in `positions.bin`: one per CSR node.
    #[serde(default)]
    pub neuron_count: u64,
    /// Rows carrying a finite soma position.
    #[serde(default)]
    pub placed_count: u64,
    #[serde(default)]
    pub bin: ArtifactFile,
    #[serde(default)]
    pub types: TypeTable,
    #[serde(default)]
    pub sections: Vec<SectionManifest>,
    #[serde(default)]
    pub source: SourceManifest,
    /// The units and frame the coordinates are in, in words.
    #[serde(default)]
    pub coordinate_space: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl PositionsManifest {
    /// Whether the manifest claims a schema at all.
    pub fn has_schema(&self) -> bool {
        !self.schema.is_empty()
    }
}

/// Writes the manifest, pretty printed, with a final newline.
pub fn write_manifest(path: &Path, manifest: &PositionsManifest) -> Result<()> {
    let mut text = serde_json::to_string_pretty(manifest)
        .map_err(|error| StreamError::json("failed to encode the manifest", error))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| {
        StreamError::io(&format!("failed to write {}", path.display()), error)
    })
}

/// Reads a manifest into this crate's shape. Unknown fields are ignored and
/// absent ones default, so a manifest written by another crate still reads.
pub fn read_manifest(path: &Path) -> Result<PositionsManifest> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        StreamError::io(&format!("failed to read {}", path.display()), error)
    })?;
    serde_json::from_str(&text)
        .map_err(|error| StreamError::json(&format!("failed to parse {}", path.display()), error))
}

/// Reads the raw manifest document, for fields this crate does not model.
pub fn read_manifest_document(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        StreamError::io(&format!("failed to read {}", path.display()), error)
    })?;
    serde_json::from_str(&text)
        .map_err(|error| StreamError::json(&format!("failed to parse {}", path.display()), error))
}

fn json_u64(document: &Value, path: &[&str]) -> Option<u64> {
    let mut cursor = document;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_u64()
}

fn json_str(document: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = document;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().map(str::to_string)
}

/// The per-section claims, under either spelling: `sections[{name,offset,bytes,
/// sha256}]` or `positions_sections{name:{offset,len,sha256}}`.
fn manifest_sections(document: &Value) -> Vec<(String, Option<u64>, Option<u64>, String)> {
    let mut out = Vec::new();
    if let Some(sections) = document.get("sections").and_then(Value::as_array) {
        for section in sections {
            let name = json_str(section, &["name"]).unwrap_or_default();
            out.push((
                name,
                json_u64(section, &["offset"]),
                json_u64(section, &["bytes"]).or_else(|| json_u64(section, &["len"])),
                json_str(section, &["sha256"]).unwrap_or_default(),
            ));
        }
        return out;
    }
    if let Some(sections) = document.get("positions_sections").and_then(Value::as_object) {
        for (name, section) in sections {
            out.push((
                name.clone(),
                json_u64(section, &["offset"]),
                json_u64(section, &["bytes"]).or_else(|| json_u64(section, &["len"])),
                json_str(section, &["sha256"]).unwrap_or_default(),
            ));
        }
        out.sort_by(|left, right| left.1.cmp(&right.1));
    }
    out
}

/// What a verification pass found.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionsVerification {
    pub manifest_present: bool,
    /// Whatever the manifest calls its schema.
    pub schema: String,
    pub neuron_count: usize,
    pub placed_count: usize,
    pub unplaced_count: usize,
    pub type_labels: usize,
    pub bin_bytes: u64,
    pub bin_sha256: String,
    pub types_bytes: u64,
    pub types_sha256: String,
    /// Re-read section digests, in file order.
    pub section_sha256: Vec<(String, String)>,
    /// How many of those the manifest also carried a digest for.
    pub sections_checked: usize,
    /// Source-table counts recorded in the manifest, when it carries them.
    pub source_bodies: u64,
    pub source_positioned: u64,
    pub source_type_labels: u64,
    /// Everything that disagreed; empty means the artifact and its manifest
    /// agree.
    pub mismatches: Vec<String>,
}

impl PositionsVerification {
    pub fn is_ok(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// Re-reads an artifact and checks it against the manifest beside it.
///
/// Checks: the header count against the section sizes, every section digest and
/// byte range the manifest carries, the type table's digest and label count,
/// the file digests it carries, and the placed count against the source table's
/// positioned count.
pub fn verify_positions(path: &Path) -> Result<PositionsVerification> {
    let (bin_path, types_path) = resolve_positions_paths(path)?;
    let positions = read_positions(&bin_path)?;
    let (bin_bytes, bin_sha256) = file_digest(&bin_path)?;
    let (types_bytes, types_sha256) = file_digest(&types_path)?;

    let raw = std::fs::read(&bin_path)
        .map_err(|error| StreamError::io(&format!("failed to read {}", bin_path.display()), error))?;
    let count = positions.neuron_count();
    let section_bytes = count as u64 * 4;
    let mut section_sha256 = Vec::with_capacity(5);
    let mut offset = HEADER_BYTES;
    for name in ["id", "x", "y", "z", "cell_type"] {
        let end = offset + section_bytes;
        section_sha256.push((
            name.to_string(),
            hex_digest(&raw[offset as usize..end as usize]),
        ));
        offset = end;
    }

    let mut manifest_present = false;
    let mut schema = String::new();
    let mut mismatches = Vec::new();
    let mut sections_checked = 0usize;
    let mut source_bodies = 0;
    let mut source_positioned = 0;
    let mut source_type_labels = 0;

    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let manifest_path: PathBuf = directory.join(POSITIONS_MANIFEST_FILE);
    if manifest_path.exists() {
        manifest_present = true;
        let document = read_manifest_document(&manifest_path)?;
        schema = json_str(&document, &["schema"]).unwrap_or_default();
        if schema.is_empty() {
            mismatches.push("the manifest names no schema".to_string());
        }

        if let Some(declared) = json_u64(&document, &["neuron_count"]) {
            if declared as usize != count {
                mismatches.push(format!(
                    "manifest neuron_count is {declared} but the file holds {count} rows"
                ));
            }
        }
        if let Some(declared) = json_u64(&document, &["positioned_count"])
            .or_else(|| json_u64(&document, &["placed_count"]))
        {
            source_positioned = declared;
            if declared as usize != positions.placed_count() {
                mismatches.push(format!(
                    "manifest positioned_count is {declared} but {} rows are placed",
                    positions.placed_count()
                ));
            }
        }
        if let Some(declared) = json_u64(&document, &["type_count"])
            .or_else(|| json_u64(&document, &["types", "labels"]))
        {
            source_type_labels = declared;
            if declared as usize != positions.types.len() {
                mismatches.push(format!(
                    "manifest type_count is {declared} but types.txt holds {}",
                    positions.types.len()
                ));
            }
        }
        if let Some(declared) = json_u64(&document, &["sources", "body_annotations", "rows"])
            .or_else(|| json_u64(&document, &["source", "bodies"]))
        {
            source_bodies = declared;
        }

        let bin_digest = json_str(&document, &["bin", "sha256"])
            .or_else(|| json_str(&document, &["positions_sha256"]));
        if let Some(declared) = bin_digest {
            if declared != bin_sha256 {
                mismatches.push(format!(
                    "positions.bin sha256 is {bin_sha256}, the manifest says {declared}"
                ));
            }
        }
        if let Some(declared) = json_u64(&document, &["bin", "bytes"]) {
            if declared != bin_bytes {
                mismatches.push(format!(
                    "positions.bin is {bin_bytes} bytes, the manifest says {declared}"
                ));
            }
        }

        let types_digest = json_str(&document, &["types", "sha256"])
            .or_else(|| json_str(&document, &["types_sha256"]));
        if let Some(declared) = types_digest {
            if declared != types_sha256 {
                mismatches.push(format!(
                    "types.txt sha256 is {types_sha256}, the manifest says {declared}"
                ));
            }
        }

        for (name, declared_offset, declared_bytes, declared_digest) in
            manifest_sections(&document)
        {
            let computed = section_sha256
                .iter()
                .enumerate()
                .find(|(_, (section_name, _))| *section_name == name)
                .map(|(index, (_, digest))| {
                    (HEADER_BYTES + index as u64 * section_bytes, digest.clone())
                });
            let Some((expected_offset, computed_digest)) = computed else {
                mismatches.push(format!(
                    "the manifest names a section {name} that the artifact does not have"
                ));
                continue;
            };
            if let Some(declared) = declared_offset {
                if declared != expected_offset {
                    mismatches.push(format!(
                        "section {name} starts at {expected_offset}, the manifest says {declared}"
                    ));
                }
            }
            if let Some(declared) = declared_bytes {
                if declared != section_bytes {
                    mismatches.push(format!(
                        "section {name} is {section_bytes} bytes, the manifest says {declared}"
                    ));
                }
            }
            if !declared_digest.is_empty() {
                sections_checked += 1;
                if declared_digest != computed_digest {
                    mismatches.push(format!(
                        "section {name} sha256 is {computed_digest}, the manifest says {declared_digest}"
                    ));
                }
            }
        }
    } else {
        mismatches.push(format!("no {POSITIONS_MANIFEST_FILE} beside the artifact"));
    }

    Ok(PositionsVerification {
        manifest_present,
        schema,
        neuron_count: count,
        placed_count: positions.placed_count(),
        unplaced_count: count - positions.placed_count(),
        type_labels: positions.types.len(),
        bin_bytes,
        bin_sha256,
        types_bytes,
        types_sha256,
        section_sha256,
        sections_checked,
        source_bodies,
        source_positioned,
        source_type_labels,
        mismatches,
    })
}
