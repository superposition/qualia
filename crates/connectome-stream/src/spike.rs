//! The per-tick spike stream.
//!
//! Layout, little-endian, no padding. One header, then frames to EOF:
//!
//! ```text
//! magic "QLSP"     4
//! version          u16
//! reserved         u16
//! --- repeated ---
//! tick             u64
//! t_ns             u64   wall clock at the tick, unix nanoseconds
//! count            u32
//! ids              u32[count]   node ids, strictly ascending
//! ```
//!
//! A file and a socket carry the same bytes: the viewer is handed a
//! [`std::io::Read`] either way, so a recorded run and a live run are the same
//! code path. The id a frame carries is a CSR node index, which is also the
//! row index in the positions artifact.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use crate::{
    read_exact_or_eof, read_header, write_header, Result, StreamError, MAX_FRAME_IDS,
    SPIKE_MAGIC,
};

/// One tick's firing set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpikeFrame {
    /// Tick index, ascending across the stream.
    pub tick: u64,
    /// Wall clock at the tick, unix nanoseconds.
    pub t_ns: u64,
    /// Firing node ids, strictly ascending.
    pub ids: Vec<u32>,
}

impl SpikeFrame {
    /// Builds a frame, checking the ordering the format promises.
    pub fn new(tick: u64, t_ns: u64, ids: Vec<u32>) -> Result<Self> {
        let frame = Self { tick, t_ns, ids };
        frame.validate()?;
        Ok(frame)
    }

    /// The frame's own invariant: ids strictly ascending, and a count a reader
    /// is willing to allocate for.
    pub fn validate(&self) -> Result<()> {
        if self.ids.len() as u64 > u64::from(MAX_FRAME_IDS) {
            return Err(StreamError::invalid(format!(
                "tick {} fires {} ids, over the {MAX_FRAME_IDS} ceiling",
                self.tick,
                self.ids.len()
            )));
        }
        if let Some(pair) = self.ids.windows(2).find(|pair| pair[0] >= pair[1]) {
            return Err(StreamError::invalid(format!(
                "tick {} fires ids out of order: {} then {}",
                self.tick, pair[0], pair[1]
            )));
        }
        Ok(())
    }
}

/// Writes the header then one frame per tick.
pub struct SpikeWriter<W: Write> {
    inner: W,
}

impl<W: Write> SpikeWriter<W> {
    /// Writes the stream header. On a socket this is the once-per-connection
    /// handshake; on a file it is the start of the run.
    pub fn new(mut inner: W) -> Result<Self> {
        write_header(&mut inner, SPIKE_MAGIC)?;
        Ok(Self { inner })
    }

    pub fn write_frame(&mut self, frame: &SpikeFrame) -> Result<()> {
        frame.validate()?;
        let mut head = [0u8; 20];
        head[..8].copy_from_slice(&frame.tick.to_le_bytes());
        head[8..16].copy_from_slice(&frame.t_ns.to_le_bytes());
        head[16..20].copy_from_slice(&(frame.ids.len() as u32).to_le_bytes());
        self.inner
            .write_all(&head)
            .map_err(|error| StreamError::io(&format!("failed to write tick {}", frame.tick), error))?;
        let mut ids = Vec::with_capacity(frame.ids.len() * 4);
        for id in &frame.ids {
            ids.extend_from_slice(&id.to_le_bytes());
        }
        self.inner.write_all(&ids).map_err(|error| {
            StreamError::io(&format!("failed to write tick {} ids", frame.tick), error)
        })
    }

    pub fn flush(&mut self) -> Result<()> {
        self.inner
            .flush()
            .map_err(|error| StreamError::io("failed to flush the spike stream", error))
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

/// Reads frames until the stream ends.
pub struct SpikeReader<R: std::io::Read> {
    inner: R,
    last_tick: Option<u64>,
    frames: u64,
}

impl<R: std::io::Read> SpikeReader<R> {
    /// Reads and checks the stream header.
    pub fn new(mut inner: R) -> Result<Self> {
        read_header(&mut inner, SPIKE_MAGIC, "spike stream")?;
        Ok(Self {
            inner,
            last_tick: None,
            frames: 0,
        })
    }

    /// Frames read so far.
    pub fn frames_read(&self) -> u64 {
        self.frames
    }

    /// The next frame, or `None` at a clean end of stream. A frame that stops
    /// halfway is an error, not an end: a truncated run must not look like a
    /// shorter run.
    pub fn next_frame(&mut self) -> Result<Option<SpikeFrame>> {
        let mut head = [0u8; 20];
        if !read_exact_or_eof(&mut self.inner, &mut head, "spike frame header")? {
            return Ok(None);
        }
        let tick = u64::from_le_bytes(head[..8].try_into().expect("8 bytes"));
        let t_ns = u64::from_le_bytes(head[8..16].try_into().expect("8 bytes"));
        let count = u32::from_le_bytes(head[16..20].try_into().expect("4 bytes"));
        if count > MAX_FRAME_IDS {
            return Err(StreamError::invalid(format!(
                "tick {tick} declares {count} ids, over the {MAX_FRAME_IDS} ceiling"
            )));
        }
        if let Some(previous) = self.last_tick {
            if tick <= previous {
                return Err(StreamError::invalid(format!(
                    "tick {tick} follows tick {previous}; the stream must ascend"
                )));
            }
        }

        let mut raw = vec![0u8; count as usize * 4];
        if !read_exact_or_eof(&mut self.inner, &mut raw, &format!("tick {tick} ids"))? {
            return Err(StreamError::invalid(format!(
                "truncated tick {tick} ids: 0 of {} bytes",
                raw.len()
            )));
        }
        let ids: Vec<u32> = raw
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4 bytes")))
            .collect();

        let frame = SpikeFrame { tick, t_ns, ids };
        frame.validate()?;
        self.last_tick = Some(tick);
        self.frames += 1;
        Ok(Some(frame))
    }
}

/// What one recorded run wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpikeRunWrite {
    pub frames: u64,
    pub ids: u64,
    pub bytes: u64,
}

/// Writes a whole run to one file.
pub fn write_spike_run(path: &Path, frames: &[SpikeFrame]) -> Result<SpikeRunWrite> {
    let file = File::create(path)
        .map_err(|error| StreamError::io(&format!("failed to create {}", path.display()), error))?;
    let mut writer = SpikeWriter::new(BufWriter::new(file))?;
    let mut ids = 0u64;
    for frame in frames {
        writer.write_frame(frame)?;
        ids += frame.ids.len() as u64;
    }
    writer.flush()?;
    let bytes = std::fs::metadata(path)
        .map_err(|error| StreamError::io(&format!("failed to stat {}", path.display()), error))?
        .len();
    Ok(SpikeRunWrite {
        frames: frames.len() as u64,
        ids,
        bytes,
    })
}

/// Reads a whole recorded run, for tests and inspection.
pub fn read_spike_run(path: &Path) -> Result<Vec<SpikeFrame>> {
    let file = File::open(path)
        .map_err(|error| StreamError::io(&format!("failed to open {}", path.display()), error))?;
    let mut reader = SpikeReader::new(BufReader::new(file))?;
    let mut frames = Vec::new();
    while let Some(frame) = reader.next_frame()? {
        frames.push(frame);
    }
    Ok(frames)
}
