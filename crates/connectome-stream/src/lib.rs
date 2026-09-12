//! Binary framing shared by the Male CNS importer and the desktop viewer.
//!
//! Two artifacts live here:
//!
//! - the **positions artifact** `crates/connectome-cns` writes: one row per
//!   CSR node, carrying the node's released body id, its soma position and its
//!   interned cell type;
//! - the **spike stream** the connectome runner writes: one frame per tick,
//!   carrying the tick index, the wall clock and the sorted firing node ids.
//!
//! Both are little-endian and self-describing: a four-byte magic, a `u16`
//! version, a `u16` reserved word, then fixed-stride sections or frames. The
//! spike framing is identical for a file and a socket, so a recorded run and a
//! live run differ only in the reader the viewer is handed. The one text file
//! is the small manifest beside a positions artifact.
//!
//! Nothing here runs the network or draws anything: the crate moves bytes the
//! importer wrote and the viewer reads, and it does not depend on Rerun.

mod manifest;
mod positions;
mod spike;

pub use manifest::{
    read_manifest, read_manifest_document, verify_positions, write_manifest, ArtifactFile,
    PositionsManifest, PositionsVerification, SectionManifest, SourceManifest, TypeTable,
};
pub use positions::{
    read_positions, read_positions_from, resolve_positions_paths, write_positions, Positions,
    PositionsWriteReport, HEADER_BYTES,
};
pub use spike::{read_spike_run, write_spike_run, SpikeFrame, SpikeReader, SpikeRunWrite, SpikeWriter};

/// Magic of the positions artifact.
pub const POSITIONS_MAGIC: [u8; 4] = *b"QLCB";
/// Magic of the spike stream.
pub const SPIKE_MAGIC: [u8; 4] = *b"QLSP";
/// Version written into both headers.
pub const FORMAT_VERSION: u16 = 1;
/// Schema string in `manifest.json`.
pub const POSITIONS_SCHEMA: &str = "qualia.connectome.positions.v1";
/// The three files a positions artifact directory holds.
pub const POSITIONS_BIN_FILE: &str = "positions.bin";
pub const POSITIONS_TYPES_FILE: &str = "types.txt";
pub const POSITIONS_MANIFEST_FILE: &str = "manifest.json";
/// Conventional name of a recorded spike stream, beside the artifact.
pub const SPIKES_BIN_FILE: &str = "spikes.bin";
/// Ceiling on one frame's firing count: a corrupt length must not turn into a
/// wild allocation.
pub const MAX_FRAME_IDS: u32 = 1 << 26;

/// A framing failure, with what was being read or written folded in.
#[derive(Debug)]
pub struct StreamError {
    detail: String,
}

impl StreamError {
    pub(crate) fn io(context: &str, error: std::io::Error) -> Self {
        Self {
            detail: format!("{context}: {error}"),
        }
    }

    pub(crate) fn json(context: &str, error: serde_json::Error) -> Self {
        Self {
            detail: format!("{context}: {error}"),
        }
    }

    /// A stream that does not satisfy the format: bad magic, a non-ascending
    /// firing set, a truncated frame.
    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for StreamError {}

pub type Result<T, E = StreamError> = std::result::Result<T, E>;

/// Writes `magic | version u16 | reserved u16`.
pub(crate) fn write_header<W: std::io::Write>(inner: &mut W, magic: [u8; 4]) -> Result<()> {
    inner
        .write_all(&magic)
        .map_err(|error| StreamError::io("failed to write stream magic", error))?;
    inner
        .write_all(&FORMAT_VERSION.to_le_bytes())
        .map_err(|error| StreamError::io("failed to write stream version", error))?;
    inner
        .write_all(&0u16.to_le_bytes())
        .map_err(|error| StreamError::io("failed to write stream reserved word", error))
}

/// Reads and checks `magic | version | reserved`.
pub(crate) fn read_header<R: std::io::Read>(
    inner: &mut R,
    magic: [u8; 4],
    what: &str,
) -> Result<()> {
    let mut header = [0u8; 8];
    inner
        .read_exact(&mut header)
        .map_err(|error| StreamError::io(&format!("failed to read the {what} header"), error))?;
    if header[..4] != magic {
        return Err(StreamError::invalid(format!(
            "{what} magic is {:02x?}, expected {:02x?}",
            &header[..4],
            magic
        )));
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != FORMAT_VERSION {
        return Err(StreamError::invalid(format!(
            "{what} version is {version}, this reader speaks {FORMAT_VERSION}"
        )));
    }
    Ok(())
}

/// Reads exactly `buffer.len()` bytes, or reports EOF when the first byte is
/// already past the end. A partial read is a truncated record, never a
/// successful end of stream.
pub(crate) fn read_exact_or_eof<R: std::io::Read>(
    inner: &mut R,
    buffer: &mut [u8],
    what: &str,
) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match inner.read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(StreamError::invalid(format!(
                    "truncated {what}: {} of {} bytes",
                    filled,
                    buffer.len()
                )))
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(StreamError::io(&format!("failed to read {what}"), error)),
        }
    }
    Ok(true)
}
