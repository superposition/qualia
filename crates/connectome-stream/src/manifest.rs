//! The manifest beside a positions artifact, and the check that the artifact
//! matches it.
//!
//! The manifest is the only text file in either format, and it stays small: it
//! carries the counts, the source table's digest and counts, and one entry per
//! binary section with its byte range, digest and value range. Every field
//! reads with a default, so the importer can carry more than this crate knows
//! about and an older manifest still verifies.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::positions::{read_positions, resolve_positions_paths, HEADER_BYTES};
use crate::{
    Result, StreamError, POSITIONS_BIN_FILE, POSITIONS_MANIFEST_FILE, POSITIONS_SCHEMA,
    POSITIONS_TYPES_FILE,
};

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

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionsManifest {
    pub schema: String,
    /// Rows in `positions.bin`: one per CSR node.
    pub neuron_count: u64,
    /// Rows carrying a finite soma position.
    #[serde(default)]
    pub placed_count: u64,
    pub bin: ArtifactFile,
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

/// Writes the manifest, pretty printed, with a final newline.
pub fn write_manifest(path: &Path, manifest: &PositionsManifest) -> Result<()> {
    let mut text = serde_json::to_string_pretty(manifest)
        .map_err(|error| StreamError::json("failed to encode the manifest", error))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| {
        StreamError::io(&format!("failed to write {}", path.display()), error)
    })
}

/// Reads a manifest. Unknown fields are ignored and absent ones default, so a
/// manifest written by another crate still reads.
pub fn read_manifest(path: &Path) -> Result<PositionsManifest> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        StreamError::io(&format!("failed to read {}", path.display()), error)
    })?;
    serde_json::from_str(&text)
        .map_err(|error| StreamError::json(&format!("failed to parse {}", path.display()), error))
}

/// What a verification pass found.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionsVerification {
    pub manifest_present: bool,
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
/// byte range against the manifest, the file digests, the placed count against
/// the source table's positioned count, and the label count against the
/// source's distinct-type count.
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
        let manifest = read_manifest(&manifest_path)?;
        schema = manifest.schema.clone();
        if manifest.schema != POSITIONS_SCHEMA {
            mismatches.push(format!(
                "manifest schema is {} not {POSITIONS_SCHEMA}",
                manifest.schema
            ));
        }
        if manifest.neuron_count as usize != count {
            mismatches.push(format!(
                "manifest neuron_count is {} but the file holds {count} rows",
                manifest.neuron_count
            ));
        }
        if manifest.placed_count != 0 && manifest.placed_count as usize != positions.placed_count() {
            mismatches.push(format!(
                "manifest placed_count is {} but {} rows are placed",
                manifest.placed_count,
                positions.placed_count()
            ));
        }
        if manifest.bin.file != POSITIONS_BIN_FILE {
            mismatches.push(format!(
                "manifest bin file is {} not {POSITIONS_BIN_FILE}",
                manifest.bin.file
            ));
        }
        if !manifest.bin.sha256.is_empty() && manifest.bin.sha256 != bin_sha256 {
            mismatches.push(format!(
                "positions.bin sha256 is {bin_sha256}, the manifest says {}",
                manifest.bin.sha256
            ));
        }
        if manifest.bin.bytes != 0 && manifest.bin.bytes != bin_bytes {
            mismatches.push(format!(
                "positions.bin is {bin_bytes} bytes, the manifest says {}",
                manifest.bin.bytes
            ));
        }
        if manifest.types.file != POSITIONS_TYPES_FILE {
            mismatches.push(format!(
                "manifest type table is {} not {POSITIONS_TYPES_FILE}",
                manifest.types.file
            ));
        }
        if !manifest.types.sha256.is_empty() && manifest.types.sha256 != types_sha256 {
            mismatches.push(format!(
                "types.txt sha256 is {types_sha256}, the manifest says {}",
                manifest.types.sha256
            ));
        }
        if manifest.types.labels != 0 && manifest.types.labels as usize != positions.types.len() {
            mismatches.push(format!(
                "manifest labels is {} but types.txt holds {}",
                manifest.types.labels,
                positions.types.len()
            ));
        }
        if !manifest.sections.is_empty() {
            let expected_offsets: Vec<(String, u64, u64)> = section_sha256
                .iter()
                .enumerate()
                .map(|(index, (name, _))| {
                    let start = HEADER_BYTES + index as u64 * section_bytes;
                    (name.clone(), start, section_bytes)
                })
                .collect();
            if manifest.sections.len() != expected_offsets.len() {
                mismatches.push(format!(
                    "manifest lists {} sections, the artifact has {}",
                    manifest.sections.len(),
                    expected_offsets.len()
                ));
            }
            for (section, (name, start, len)) in
                manifest.sections.iter().zip(expected_offsets.iter())
            {
                if section.name != *name || section.offset != *start || section.bytes != *len {
                    mismatches.push(format!(
                        "section {name} sits at {start}+{len}, the manifest says {} at {}+{}",
                        section.name, section.offset, section.bytes
                    ));
                }
                let computed = section_sha256
                    .iter()
                    .find(|(section_name, _)| *section_name == section.name)
                    .map(|(_, digest)| digest.clone())
                    .unwrap_or_default();
                if !section.sha256.is_empty() && section.sha256 != computed {
                    mismatches.push(format!(
                        "section {} sha256 is {computed}, the manifest says {}",
                        section.name, section.sha256
                    ));
                }
            }
        }
        source_bodies = manifest.source.bodies;
        source_positioned = manifest.source.positioned;
        source_type_labels = manifest.source.type_labels;
        if source_positioned != 0 && source_positioned as usize != positions.placed_count() {
            mismatches.push(format!(
                "source table positions {source_positioned} bodies, the artifact places {}",
                positions.placed_count()
            ));
        }
        if source_type_labels != 0 && source_type_labels as usize != positions.types.len() {
            mismatches.push(format!(
                "source table holds {source_type_labels} type labels, the artifact's table holds {}",
                positions.types.len()
            ));
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
        source_bodies,
        source_positioned,
        source_type_labels,
        mismatches,
    })
}
