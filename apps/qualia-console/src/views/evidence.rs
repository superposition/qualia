//! Evidence view: sealed MCAP segments, quarantined partials, ledger rows.
//!
//! Traceability (`docs/frontend-lessons.md`, sources 2 and 3): the empty case
//! is written first and named ("no sealed segments"), because an operator
//! seeing nothing must be able to tell "no evidence yet" from "cannot read
//! evidence". The segment summary is `qualia-mcap`'s own inventory type, so the
//! console never invents a second reading of a sealed file.
//!
//! Inventorying a segment reads the whole file, so it happens when a segment is
//! sealed or changes, not on every poll: [`EvidenceScan`] keeps the directory's
//! fingerprint beside the readings it produced.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use egui::Ui;
use qualia_mcap::ChannelInventory;
use qualia_shm::ShmRegion;

use crate::ConsoleState;

/// A sealed segment as the console shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentReading {
    pub path: String,
    pub byte_length: u64,
    pub channels: Vec<ChannelInventory>,
}

/// One belief-ledger row, copied out of the region.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerRow {
    pub seq: u64,
    pub layer: u8,
    pub event: String,
    pub vfe: f32,
    pub residual_norm: f32,
    pub timestamp_ns: u64,
}

impl LedgerRow {
    pub fn from_entry(entry: &qualia_types::LedgerEntry) -> Self {
        Self {
            seq: entry.seq,
            layer: entry.layer,
            event: format!("{:?}", entry.event),
            vfe: entry.vfe,
            residual_norm: entry.residual_norm,
            timestamp_ns: entry.timestamp_ns,
        }
    }

    /// Every ledger row the region has appended, oldest first.
    pub fn sample(region: &ShmRegion) -> Vec<Self> {
        let sequence = region.ledger_seq();
        let capacity = qualia_shm::MAX_LEDGER_ENTRIES as u64;
        let occupied = sequence.min(capacity);
        let first = sequence.saturating_sub(occupied);
        (0..occupied)
            .map(|offset| Self::from_entry(region.ledger_entry(((first + offset) % capacity) as usize)))
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvidenceView {
    /// The directory scanned for `*.mcap`, from `QUALIA_EVIDENCE_DIR` or
    /// [`DEFAULT_EVIDENCE_ROOT`].
    pub root: String,
    pub segments: Vec<SegmentReading>,
    pub quarantined: Vec<String>,
    pub ledger: Vec<LedgerRow>,
    pub error: Option<String>,
}

impl EvidenceView {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.quarantined.is_empty() && self.ledger.is_empty()
    }
}

/// The evidence root the console scans when `QUALIA_EVIDENCE_DIR` is unset.
///
/// No manifest, runner or ticket names an evidence directory yet
/// (`runners/arena-recorder` is a stub), so this is the console's own documented
/// convenience: the fixture root and the live default are the same one literal.
pub const DEFAULT_EVIDENCE_ROOT: &str = "artifacts/mcap";

/// The evidence directory, from the environment or [`DEFAULT_EVIDENCE_ROOT`].
pub fn evidence_root() -> PathBuf {
    std::env::var_os("QUALIA_EVIDENCE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EVIDENCE_ROOT))
}

/// One candidate file, with the size and modification time its inventory
/// depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    byte_length: u64,
    modified_ns: u64,
}

/// The evidence directory across polls.
///
/// Held by the poller thread and never shared. Each call answers with a view;
/// the directory is only walked when its candidates' names, sizes or
/// modification times differ from the last walk, so a multi-hundred-megabyte
/// segment is read when it is written rather than on every tick.
#[derive(Debug, Default)]
pub struct EvidenceScan {
    /// The fingerprint `segments` and `quarantined` were read from, or `None`
    /// before the first scan or after a directory that could not be listed.
    fingerprint: Option<Vec<FileStamp>>,
    scanned: bool,
    segments: Vec<SegmentReading>,
    quarantined: Vec<String>,
    error: Option<String>,
}

impl EvidenceScan {
    /// The evidence view for `root`, inventorying only what changed.
    pub fn refresh(&mut self, root: &Path, ledger: Vec<LedgerRow>) -> EvidenceView {
        match candidate_stamps(root) {
            Ok(stamps) => {
                if !self.scanned || self.fingerprint.as_ref() != Some(&stamps) {
                    let (segments, quarantined, error) = inventory(&stamps);
                    self.segments = segments;
                    self.quarantined = quarantined;
                    self.error = error;
                    self.fingerprint = Some(stamps);
                    self.scanned = true;
                }
            }
            Err(error) => {
                // A directory that cannot be listed is a named state, not a
                // stale one: the readings go and the reason stays.
                self.segments.clear();
                self.quarantined.clear();
                self.error = Some(format!("{}: {error}", root.display()));
                self.fingerprint = None;
                self.scanned = true;
            }
        }

        EvidenceView {
            root: root.display().to_string(),
            segments: self.segments.clone(),
            quarantined: self.quarantined.clone(),
            ledger,
            error: self.error.clone(),
        }
    }
}

/// Every `*.mcap` and `*.mcap.partial` under `root`, sorted by path.
fn candidate_stamps(root: &Path) -> std::io::Result<Vec<FileStamp>> {
    let mut stamps = Vec::new();

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !(name.ends_with(".mcap") || name.ends_with(".mcap.partial")) {
            continue;
        }
        let metadata = entry.metadata()?;
        stamps.push(FileStamp {
            byte_length: metadata.len(),
            modified_ns: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_nanos() as u64)
                .unwrap_or(0),
            path,
        });
    }

    stamps.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(stamps)
}

/// Summarize the candidates: sealed segments through `qualia-mcap`'s inventory,
/// partials as the quarantined list. A segment that cannot be read is named and
/// omitted; nothing here writes, moves or repairs evidence.
fn inventory(stamps: &[FileStamp]) -> (Vec<SegmentReading>, Vec<String>, Option<String>) {
    let mut segments = Vec::new();
    let mut quarantined = Vec::new();
    let mut errors = Vec::new();

    for stamp in stamps {
        match stamp
            .path
            .extension()
            .and_then(|extension| extension.to_str())
        {
            Some("mcap") => match inventory_segment(stamp) {
                Ok(segment) => segments.push(segment),
                Err(error) => errors.push(format!("{}: {error}", stamp.path.display())),
            },
            Some("partial") => quarantined.push(stamp.path.display().to_string()),
            _ => {}
        }
    }

    (
        segments,
        quarantined,
        (!errors.is_empty()).then(|| errors.join("; ")),
    )
}

fn inventory_segment(stamp: &FileStamp) -> Result<SegmentReading, String> {
    let channels = qualia_mcap::inventory(&stamp.path).map_err(|error| error.to_string())?;
    Ok(SegmentReading {
        path: stamp.path.display().to_string(),
        byte_length: stamp.byte_length,
        channels,
    })
}

pub fn render(ui: &mut Ui, state: &ConsoleState) {
    let view = &state.evidence;
    ui.heading("Evidence");

    ui.label(format!("evidence root: {}", view.root));

    if let Some(error) = &view.error {
        ui.colored_label(
            egui::Color32::from_rgb(224, 160, 138),
            format!("evidence read error: {error}"),
        );
    }

    if view.segments.is_empty() {
        ui.label("no sealed segments");
    } else {
        for segment in &view.segments {
            ui.label(format!("sealed segment: {}", segment.path));
            ui.label(format!("segment bytes: {}", segment.byte_length));
            for channel in &segment.channels {
                ui.label(format!(
                    "channel {}: {} messages",
                    channel.topic, channel.message_count
                ));
            }
        }
    }

    for partial in &view.quarantined {
        ui.label(format!("quarantined partial: {partial}"));
    }

    ui.separator();
    if view.ledger.is_empty() {
        ui.label("ledger empty");
    } else {
        ui.label(format!("ledger entries: {}", view.ledger.len()));
        if let Some(newest) = view.ledger.last() {
            ui.label(format!(
                "ledger newest: seq {} layer {} {} vfe {:.4}",
                newest.seq, newest.layer, newest.event, newest.vfe
            ));
        }
    }
}
