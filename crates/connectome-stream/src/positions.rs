//! The positions artifact: one row per CSR node, in node order.
//!
//! Layout, little-endian, no padding:
//!
//! ```text
//! 0   magic "QLCB"          4
//! 4   version               u16
//! 6   reserved              u16
//! 8   neuron_count          u32   = N, the CSR node count
//! 12  id                    u32[N]   released body id per node
//! 12+4N   x                 f32[N]
//! 12+8N   y                 f32[N]
//! 12+12N  z                 f32[N]
//! 12+16N  cell_type         u32[N]   index into `types.txt`
//! 12+20N  end
//! ```
//!
//! Row `k` is CSR node `k`, so a spike frame's id is a row index and needs no
//! lookup table. A row whose `x/y/z` are not finite is a node the released
//! annotations place nowhere: it keeps its id and its cell type, the viewer
//! leaves it out of the cloud, and the manifest reports the placed count
//! beside `neuron_count` rather than inventing a coordinate.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::manifest::{
    hex_digest, ArtifactFile, PositionsManifest, SectionManifest, SourceManifest, TypeTable,
};
use crate::{
    read_header, write_header, Result, StreamError, FORMAT_VERSION, POSITIONS_BIN_FILE,
    POSITIONS_MAGIC, POSITIONS_SCHEMA, POSITIONS_TYPES_FILE,
};

/// Bytes before the first section: magic, version, reserved word, count.
pub const HEADER_BYTES: u64 = 12;

/// One row per CSR node plus the interned cell-type label table.
#[derive(Debug, Clone, PartialEq)]
pub struct Positions {
    /// Released body id, one per node.
    pub ids: Vec<u32>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub z: Vec<f32>,
    /// Index into [`Positions::types`].
    pub cell_type: Vec<u32>,
    /// Interned labels; the line number is the `cell_type` value.
    pub types: Vec<String>,
}

impl Positions {
    /// Rows in the artifact: one per CSR node, placed or not.
    pub fn neuron_count(&self) -> usize {
        self.ids.len()
    }

    /// Rows that carry a finite soma position.
    pub fn placed_count(&self) -> usize {
        (0..self.ids.len()).filter(|row| self.is_placed(*row)).count()
    }

    /// Whether this row has a soma position.
    pub fn is_placed(&self, row: usize) -> bool {
        self.x[row].is_finite() && self.y[row].is_finite() && self.z[row].is_finite()
    }

    /// The soma position of a row, in the dataset's EM voxel units.
    pub fn position(&self, row: usize) -> [f32; 3] {
        [self.x[row], self.y[row], self.z[row]]
    }

    pub fn type_label(&self, cell_type_index: u32) -> Option<&str> {
        self.types.get(cell_type_index as usize).map(String::as_str)
    }

    /// Size of the file this artifact serializes to.
    pub fn byte_len(&self) -> u64 {
        HEADER_BYTES + 20 * self.ids.len() as u64
    }

    /// Checks the invariant the viewer relies on: one row per section, a
    /// unique id per row, every `cell_type` inside the label table.
    pub fn validate(&self) -> Result<()> {
        let count = self.ids.len();
        for (name, len) in [
            ("x", self.x.len()),
            ("y", self.y.len()),
            ("z", self.z.len()),
            ("cell_type", self.cell_type.len()),
        ] {
            if len != count {
                return Err(StreamError::invalid(format!(
                    "positions section {name} has {len} rows, id has {count}"
                )));
            }
        }
        let mut seen = HashSet::with_capacity(count);
        for (row, id) in self.ids.iter().enumerate() {
            if !seen.insert(*id) {
                return Err(StreamError::invalid(format!(
                    "released body id {id} appears twice (rows {} and {row})",
                    row
                )));
            }
        }
        let labels = self.types.len() as u32;
        if let Some(row) = self.cell_type.iter().position(|index| *index >= labels) {
            return Err(StreamError::invalid(format!(
                "row {row} has cell_type {} but the label table holds {labels}",
                self.cell_type[row]
            )));
        }
        Ok(())
    }
}

/// What one call to [`write_positions`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionsWriteReport {
    pub neuron_count: usize,
    pub placed_count: usize,
    pub bin_bytes: u64,
    pub bin_sha256: String,
    pub types_bytes: u64,
    pub types_sha256: String,
    pub type_labels: usize,
    pub sections: Vec<SectionManifest>,
}

impl PositionsWriteReport {
    /// The manifest for the files just written.
    pub fn manifest(&self, source: SourceManifest, coordinate_space: &str) -> PositionsManifest {
        PositionsManifest {
            schema: POSITIONS_SCHEMA.to_string(),
            neuron_count: self.neuron_count as u64,
            placed_count: self.placed_count as u64,
            bin: ArtifactFile {
                file: POSITIONS_BIN_FILE.to_string(),
                bytes: self.bin_bytes,
                sha256: self.bin_sha256.clone(),
            },
            types: TypeTable {
                file: POSITIONS_TYPES_FILE.to_string(),
                bytes: self.types_bytes,
                sha256: self.types_sha256.clone(),
                labels: self.type_labels as u64,
            },
            sections: self.sections.clone(),
            source,
            coordinate_space: coordinate_space.to_string(),
            notes: Vec::new(),
        }
    }
}

/// Writes `positions.bin` and `types.txt`, returning their sizes and digests.
pub fn write_positions(
    bin_path: &Path,
    types_path: &Path,
    positions: &Positions,
) -> Result<PositionsWriteReport> {
    positions.validate()?;

    let mut writer = BufWriter::new(File::create(bin_path).map_err(|error| {
        StreamError::io(&format!("failed to create {}", bin_path.display()), error)
    })?);
    write_header(&mut writer, POSITIONS_MAGIC)?;
    writer
        .write_all(&(positions.ids.len() as u32).to_le_bytes())
        .map_err(|error| StreamError::io("failed to write neuron count", error))?;

    let mut hasher = Sha256::new();
    hasher.update(POSITIONS_MAGIC);
    hasher.update(FORMAT_VERSION.to_le_bytes());
    hasher.update(0u16.to_le_bytes());
    hasher.update((positions.ids.len() as u32).to_le_bytes());

    let mut sections = Vec::with_capacity(5);
    let mut offset = HEADER_BYTES;
    for (name, bytes, min, max) in [
        ("id", section_u32(&positions.ids), None, None),
        (
            "x",
            section_f32(&positions.x),
            finite_min(&positions.x),
            finite_max(&positions.x),
        ),
        (
            "y",
            section_f32(&positions.y),
            finite_min(&positions.y),
            finite_max(&positions.y),
        ),
        (
            "z",
            section_f32(&positions.z),
            finite_min(&positions.z),
            finite_max(&positions.z),
        ),
        (
            "cell_type",
            section_u32(&positions.cell_type),
            None,
            None,
        ),
    ] {
        writer
            .write_all(&bytes)
            .map_err(|error| StreamError::io(&format!("failed to write section {name}"), error))?;
        hasher.update(&bytes);
        sections.push(SectionManifest {
            name: name.to_string(),
            offset,
            bytes: bytes.len() as u64,
            sha256: hex_digest(&bytes),
            min,
            max,
        });
        offset += bytes.len() as u64;
    }
    writer
        .flush()
        .map_err(|error| StreamError::io("failed to flush positions.bin", error))?;
    drop(writer);

    let types_bytes = encode_types(&positions.types);
    std::fs::write(types_path, &types_bytes).map_err(|error| {
        StreamError::io(&format!("failed to write {}", types_path.display()), error)
    })?;

    Ok(PositionsWriteReport {
        neuron_count: positions.neuron_count(),
        placed_count: positions.placed_count(),
        bin_bytes: positions.byte_len(),
        bin_sha256: format!("{:x}", hasher.finalize()),
        types_bytes: types_bytes.len() as u64,
        types_sha256: hex_digest(&types_bytes),
        type_labels: positions.types.len(),
        sections,
    })
}

/// Reads `positions.bin`/`types.txt` from the directory (or the `positions.bin`
/// path) given.
pub fn read_positions(path: &Path) -> Result<Positions> {
    let (bin_path, types_path) = resolve_positions_paths(path)?;
    read_positions_from(&bin_path, &types_path)
}

/// Accepts either the artifact directory or the `positions.bin` inside it.
pub fn resolve_positions_paths(path: &Path) -> Result<(PathBuf, PathBuf)> {
    if path.is_dir() {
        Ok((
            path.join(POSITIONS_BIN_FILE),
            path.join(POSITIONS_TYPES_FILE),
        ))
    } else if path.file_name().is_some_and(|name| name == POSITIONS_BIN_FILE) {
        Ok((path.to_path_buf(), path.with_file_name(POSITIONS_TYPES_FILE)))
    } else if path.exists() {
        Ok((path.to_path_buf(), path.with_file_name(POSITIONS_TYPES_FILE)))
    } else {
        Err(StreamError::invalid(format!(
            "{} is neither a positions artifact directory nor a positions.bin",
            path.display()
        )))
    }
}

/// Reads a binary artifact written by [`write_positions`].
pub fn read_positions_from(bin_path: &Path, types_path: &Path) -> Result<Positions> {
    let bytes = std::fs::read(bin_path).map_err(|error| {
        StreamError::io(&format!("failed to read {}", bin_path.display()), error)
    })?;
    if bytes.len() < HEADER_BYTES as usize {
        return Err(StreamError::invalid(format!(
            "{} holds {} bytes, too few for a positions header",
            bin_path.display(),
            bytes.len()
        )));
    }
    let mut cursor = std::io::Cursor::new(&bytes);
    read_header(&mut cursor, POSITIONS_MAGIC, "positions")?;
    let mut count_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut count_bytes)
        .map_err(|error| StreamError::io("failed to read neuron count", error))?;
    let count = u32::from_le_bytes(count_bytes) as usize;

    let expected = HEADER_BYTES as usize + 20 * count;
    if bytes.len() != expected {
        return Err(StreamError::invalid(format!(
            "{} holds {} bytes, but a {count}-row artifact is {expected}",
            bin_path.display(),
            bytes.len()
        )));
    }

    let mut offset = HEADER_BYTES as usize;
    let ids = read_u32_section(&bytes, &mut offset, count);
    let x = read_f32_section(&bytes, &mut offset, count);
    let y = read_f32_section(&bytes, &mut offset, count);
    let z = read_f32_section(&bytes, &mut offset, count);
    let cell_type = read_u32_section(&bytes, &mut offset, count);

    let types = decode_types(
        &std::fs::read(types_path).map_err(|error| {
            StreamError::io(&format!("failed to read {}", types_path.display()), error)
        })?,
    );

    let positions = Positions {
        ids,
        x,
        y,
        z,
        cell_type,
        types,
    };
    positions.validate()?;
    Ok(positions)
}

fn read_u32_section(bytes: &[u8], offset: &mut usize, count: usize) -> Vec<u32> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[*offset..*offset + 4]);
        out.push(u32::from_le_bytes(raw));
        *offset += 4;
    }
    out
}

fn read_f32_section(bytes: &[u8], offset: &mut usize, count: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[*offset..*offset + 4]);
        out.push(f32::from_le_bytes(raw));
        *offset += 4;
    }
    out
}

fn section_u32(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn section_f32(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn finite_min(values: &[f32]) -> Option<f64> {
    values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .map(f64::from)
        .reduce(f64::min)
}

fn finite_max(values: &[f32]) -> Option<f64> {
    values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .map(f64::from)
        .reduce(f64::max)
}

/// One interned label per line; the line number is the `cell_type` value.
pub fn encode_types(types: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for label in types {
        out.extend_from_slice(label.as_bytes());
        out.push(b'\n');
    }
    out
}

/// Splits a label table on newlines, tolerating CRLF and a missing final
/// newline.
pub fn decode_types(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut labels: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if labels.last().is_some_and(String::is_empty) {
        labels.pop();
    }
    labels
}
