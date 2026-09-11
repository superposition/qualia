//! The evidence directory, read when it changes and named when it cannot be.
//!
//! `EvidenceScan` exists because inventorying a sealed segment reads the whole
//! file; these tests pin what an operator observes — the view follows the
//! directory it was pointed at, and a segment that cannot be read is reported
//! rather than dropped.

use std::fs;

use qualia_console::views::evidence::EvidenceScan;

#[test]
fn the_scan_follows_partials_as_they_appear_and_vanish() {
    let directory = tempfile::tempdir().expect("temp dir");
    let mut scan = EvidenceScan::default();

    let empty = scan.refresh(Some(directory.path()), Vec::new());
    assert!(empty.segments.is_empty());
    assert!(empty.quarantined.is_empty());
    assert!(empty.error.is_none());

    let partial = directory.path().join("run.mcap.partial");
    fs::write(&partial, b"the writer died mid-segment").expect("write partial");
    let listed = scan.refresh(Some(directory.path()), Vec::new());
    assert_eq!(listed.quarantined, vec![partial.display().to_string()]);

    fs::remove_file(&partial).expect("remove partial");
    let gone = scan.refresh(Some(directory.path()), Vec::new());
    assert!(
        gone.quarantined.is_empty(),
        "a vanished partial must not linger"
    );
}

#[test]
fn an_unreadable_segment_is_named_and_not_kept() {
    let directory = tempfile::tempdir().expect("temp dir");
    let mut scan = EvidenceScan::default();

    let segment = directory.path().join("broken.mcap");
    fs::write(&segment, b"not an mcap").expect("write segment");
    let failed = scan.refresh(Some(directory.path()), Vec::new());
    assert!(failed.segments.is_empty());
    let error = failed.error.expect("the unreadable segment is named");
    assert!(error.contains("broken.mcap"), "error was {error}");

    fs::remove_file(&segment).expect("remove segment");
    let cleared = scan.refresh(Some(directory.path()), Vec::new());
    assert!(
        cleared.error.is_none(),
        "the reason must not outlive the file"
    );
}

#[test]
fn an_unset_root_names_the_variable() {
    let view = EvidenceScan::default().refresh(None, Vec::new());
    assert_eq!(view.error.as_deref(), Some("QUALIA_EVIDENCE_DIR is unset"));
    assert!(view.root.is_empty());
}
