//! The recorded firing stream: magic `QLSP`, one frame per tick.
//!
//! A frame is `tick u64 | t_ns u64 | count u32 | ids u32[count]`, with the ids
//! sorted ascending. The header is 12 bytes, so a frame starts 8-aligned and
//! the whole file is a sequential read or an mmap walk — the viewer replays it
//! and the runner appends to it with the same code.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use crate::{CnsError, BASE_HEADER_LEN, FORMAT_VERSION, SPIKES_MAGIC};

/// One tick's firing set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpikeFrame {
    /// Tick index.
    pub tick: u64,
    /// Monotonic wall clock, nanoseconds.
    pub t_ns: u64,
    /// Firing node ids, ascending.
    pub ids: Vec<u32>,
}

/// Append-only writer for a `QLSP` stream.
pub struct SpikeWriter {
    inner: BufWriter<File>,
}

impl SpikeWriter {
    /// Create (or truncate) a stream at `path` and write its header.
    pub fn create(path: &Path) -> Result<Self, CnsError> {
        let file = File::create(path).map_err(|error| crate::read_error(path, error))?;
        let mut writer = Self {
            inner: BufWriter::new(file),
        };
        writer
            .inner
            .write_all(&SPIKES_MAGIC)
            .and_then(|()| writer.inner.write_all(&FORMAT_VERSION.to_le_bytes()))
            .and_then(|()| writer.inner.write_all(&0u16.to_le_bytes()))
            .map_err(|error| crate::read_error(path, error))?;
        Ok(writer)
    }

    /// Append one frame.
    pub fn write_frame(&mut self, frame: &SpikeFrame) -> Result<(), CnsError> {
        let mut bytes = Vec::with_capacity(20 + 4 * frame.ids.len());
        bytes.extend_from_slice(&frame.tick.to_le_bytes());
        bytes.extend_from_slice(&frame.t_ns.to_le_bytes());
        bytes.extend_from_slice(&(frame.ids.len() as u32).to_le_bytes());
        for id in &frame.ids {
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        self.inner
            .write_all(&bytes)
            .map_err(|error| CnsError::Artifact(format!("spikes: {error}")))
    }

    /// Flush buffered frames to the file.
    pub fn finish(mut self) -> Result<(), CnsError> {
        self.inner
            .flush()
            .map_err(|error| CnsError::Artifact(format!("spikes: {error}")))
    }
}

/// Read a whole `QLSP` stream into frames.
///
/// Frames are validated as they are read: the ids must be ascending, so a
/// truncated or reordered stream is refused rather than replayed wrong.
pub fn read_spikes(path: &Path) -> Result<Vec<SpikeFrame>, CnsError> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| crate::read_error(path, error))?;
    decode_spikes(&bytes)
}

/// Decode a `QLSP` stream from memory.
pub fn decode_spikes(bytes: &[u8]) -> Result<Vec<SpikeFrame>, CnsError> {
    if bytes.len() < BASE_HEADER_LEN {
        return Err(CnsError::Artifact(format!(
            "spikes: {} bytes is shorter than the header",
            bytes.len()
        )));
    }
    if bytes[0..4] != SPIKES_MAGIC {
        return Err(CnsError::Artifact(format!(
            "spikes: bad magic {:02x?}",
            &bytes[0..4]
        )));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return Err(CnsError::Artifact(format!(
            "spikes: format version {version}, this build writes {FORMAT_VERSION}"
        )));
    }
    let mut cursor = BASE_HEADER_LEN;
    let mut frames = Vec::new();
    while cursor < bytes.len() {
        let head = bytes
            .get(cursor..cursor + 20)
            .ok_or_else(|| CnsError::Malformed("spikes: truncated frame header".to_string()))?;
        let tick = u64::from_le_bytes(head[0..8].try_into().expect("8 bytes"));
        let t_ns = u64::from_le_bytes(head[8..16].try_into().expect("8 bytes"));
        let count = u32::from_le_bytes(head[16..20].try_into().expect("4 bytes")) as usize;
        cursor += 20;
        let span = count
            .checked_mul(4)
            .ok_or_else(|| CnsError::Malformed("spikes: frame overflows".to_string()))?;
        let ids = bytes
            .get(cursor..cursor + span)
            .ok_or_else(|| CnsError::Malformed("spikes: truncated frame body".to_string()))?;
        let ids: Vec<u32> = ids
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4 bytes")))
            .collect();
        if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(CnsError::Malformed(format!(
                "spikes: frame {tick} ids are not strictly ascending"
            )));
        }
        cursor += span;
        frames.push(SpikeFrame { tick, t_ns, ids });
    }
    Ok(frames)
}

/// Write frames from an existing stream to a new path, one `QLSP` file.
pub fn write_spikes(path: &Path, frames: &[SpikeFrame]) -> Result<(), CnsError> {
    let mut writer = SpikeWriter::create(path)?;
    for frame in frames {
        writer.write_frame(frame)?;
    }
    writer.finish()
}

/// Convenience for a caller that holds a plain `File`.
impl SpikeWriter {
    /// Wrap an already-open file positioned at the start of a stream.
    pub fn from_file(file: File) -> Self {
        Self {
            inner: BufWriter::new(file),
        }
    }

    /// Open a path for appending frames after the header.
    pub fn open(path: &Path) -> Result<Self, CnsError> {
        let file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|error| crate::read_error(path, error))?;
        Ok(Self::from_file(file))
    }
}
