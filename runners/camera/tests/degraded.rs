//! What the runner does when the camera is not delivering: a source that is
//! missing, unchanged, oversized, corrupt or answering with an HTTP error must
//! never cost the stack the last readable frame.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::{ImageBuffer, ImageFormat, Luma};
use qualia_camera::{
    capture_once, ingest_snapshot_bytes, CaptureOutcome, IngestError, SnapshotSource,
    MAX_SNAPSHOT_BYTES, PREVIEW_FORMAT_NONE,
};
use qualia_shm::ShmRegion;
use qualia_types::{CameraFrameSnapshot, CAMERA_PREVIEW_MAX_BYTES};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

const TIMEOUT: Duration = Duration::from_secs(5);

fn region(tag: &str) -> ShmRegion {
    let name = format!(
        "/qualia_camera_degraded_{}_{}_{}",
        std::process::id(),
        tag,
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    );
    ShmRegion::create(&name).expect("create region")
}

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "qualia_camera_{}_{}_{}.png",
        std::process::id(),
        tag,
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn snapshot_bytes(width: u32, height: u32, brightness: u8) -> Vec<u8> {
    let image: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_fn(width, height, |_, _| Luma([brightness]));
    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode snapshot");
    bytes
}

fn read_frame(shm: &ShmRegion) -> CameraFrameSnapshot {
    shm.camera_frame().snapshot(64).expect("coherent frame")
}

/// Serves `responses` on a loopback port, one connection each, then exits.
/// Returns the URL clients should use.
fn serve(responses: Vec<(u16, &'static str, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let address = listener.local_addr().expect("local address");
    std::thread::spawn(move || {
        for (status, content_type, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).expect("write head");
            stream.write_all(&body).expect("write body");
            let _ = stream.flush();
        }
    });
    format!("http://{address}/snapshot.jpg")
}

#[test]
fn a_missing_snapshot_file_captures_nothing() {
    let shm = region("missing");
    let missing = temp_path("missing");
    let source = SnapshotSource::file(missing.to_string_lossy());

    let outcome = capture_once(&shm, &source, 0);
    let CaptureOutcome::Unavailable(reason) = outcome else {
        panic!("a missing file must be unavailable: {outcome:?}");
    };
    assert!(
        reason.starts_with("stat snapshot: "),
        "the operator line carries the reference payload: {reason}"
    );
    assert_eq!(read_frame(&shm).seq, 0, "nothing was published");
}

#[test]
fn a_new_file_publishes_once_and_an_unchanged_file_is_left_alone() {
    let shm = region("mtime");
    let path = temp_path("mtime");
    std::fs::write(&path, snapshot_bytes(64, 48, 200)).expect("write snapshot");
    let source = SnapshotSource::file(path.to_string_lossy());

    let CaptureOutcome::Published { seq, mtime_ns } = capture_once(&shm, &source, 0) else {
        panic!("the first poll must publish");
    };
    assert_eq!(seq, 2);
    assert!(mtime_ns > 0, "a file source carries its modification time");

    assert_eq!(
        capture_once(&shm, &source, mtime_ns),
        CaptureOutcome::Unchanged,
        "the same modification time means no new frame"
    );
    assert_eq!(read_frame(&shm).seq, 2, "and no second publish");
}

#[test]
fn a_corrupt_snapshot_leaves_the_last_good_frame_readable() {
    let shm = region("corrupt");
    let path = temp_path("corrupt");
    let png = snapshot_bytes(64, 48, 90);
    std::fs::write(&path, &png).expect("write snapshot");
    let source = SnapshotSource::file(path.to_string_lossy());

    let CaptureOutcome::Published { .. } = capture_once(&shm, &source, 0) else {
        panic!("the first poll must publish");
    };
    let good = read_frame(&shm);

    // The file is rewritten with something that is not an image at all. The
    // watermark is deliberately ignored so the read is not skipped.
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nthis is not a picture").expect("rewrite snapshot");
    let outcome = capture_once(&shm, &source, 0);
    let CaptureOutcome::Corrupt(reason) = outcome else {
        panic!("undecodable bytes must be corrupt: {outcome:?}");
    };
    assert!(
        reason.starts_with("decode snapshot: "),
        "the operator line carries the reference payload: {reason}"
    );

    let frame = read_frame(&shm);
    assert_eq!(frame.seq, good.seq, "the last good frame is untouched");
    assert_eq!(frame.timestamp_ns, good.timestamp_ns);
    assert_eq!(frame.luminance_mean, good.luminance_mean);
    assert!(frame.valid);

    let preview = shm.camera_preview();
    let len = preview.len.load(Ordering::Acquire);
    assert_eq!(len, png.len(), "the last good preview is untouched");
    assert_eq!(&preview.bytes[..len], &png[..]);
}

#[test]
fn a_snapshot_file_over_the_size_cap_is_never_published() {
    let shm = region("oversize");
    let path = temp_path("oversize");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .expect("create snapshot");
    file.set_len(MAX_SNAPSHOT_BYTES + 1).expect("grow snapshot");
    drop(file);
    let source = SnapshotSource::file(path.to_string_lossy());

    let outcome = capture_once(&shm, &source, 0);
    assert!(
        matches!(outcome, CaptureOutcome::Unavailable(_)),
        "{outcome:?}"
    );
    assert_eq!(read_frame(&shm).seq, 0);
}

#[test]
fn an_http_snapshot_publishes_the_body() {
    let shm = region("http");
    let body = snapshot_bytes(64, 48, 30);
    let url = serve(vec![(200, "image/png", body)]);
    let source = SnapshotSource::http(url, TIMEOUT);

    let CaptureOutcome::Published { seq, .. } = capture_once(&shm, &source, 0) else {
        panic!("an HTTP snapshot must publish");
    };
    assert_eq!(seq, 2);
    let frame = read_frame(&shm);
    assert!(frame.valid);
    assert_eq!((frame.source_width, frame.source_height), (64, 48));
}

#[test]
fn an_http_error_page_is_unavailable() {
    let shm = region("http-error");
    let url = serve(vec![(503, "text/plain", b"camera warming up".to_vec())]);
    let source = SnapshotSource::http(url.clone(), TIMEOUT);

    let outcome = capture_once(&shm, &source, 0);
    let CaptureOutcome::Unavailable(reason) = outcome else {
        panic!("an HTTP error page must be unavailable: {outcome:?}");
    };
    assert!(
        reason.starts_with(&format!("GET {url}: ")),
        "the operator line carries the reference payload: {reason}"
    );
    assert_eq!(read_frame(&shm).seq, 0);
}

#[test]
fn an_empty_http_snapshot_is_unavailable() {
    let shm = region("http-empty");
    let url = serve(vec![(200, "image/jpeg", Vec::new())]);
    let source = SnapshotSource::http(url, TIMEOUT);

    let outcome = capture_once(&shm, &source, 0);
    let CaptureOutcome::Unavailable(reason) = outcome else {
        panic!("an empty body must be unavailable: {outcome:?}");
    };
    assert_eq!(reason, "snapshot response was empty");
}

#[test]
fn an_http_snapshot_over_the_size_cap_is_unavailable() {
    let shm = region("http-oversize");
    let url = serve(vec![(
        200,
        "image/png",
        vec![0u8; MAX_SNAPSHOT_BYTES as usize + 1],
    )]);
    let source = SnapshotSource::http(url, TIMEOUT);

    let outcome = capture_once(&shm, &source, 0);
    let CaptureOutcome::Unavailable(reason) = outcome else {
        panic!("an oversized body must be unavailable: {outcome:?}");
    };
    assert_eq!(reason, format!("snapshot exceeds {MAX_SNAPSHOT_BYTES} byte limit"));
}

/// The MJPEG loop prints `IngestError`'s `Display` straight into the operator
/// log, so both prefixes there are the reference's operator wording.
#[test]
fn ingest_error_display_carries_the_reference_prefixes() {
    assert_eq!(
        IngestError::Decode("not an image".to_string()).to_string(),
        "decode snapshot: not an image"
    );
    assert_eq!(
        IngestError::Publish("arena busy".to_string()).to_string(),
        "publish camera snapshot: arena busy"
    );
}

#[test]
fn a_preview_over_the_slot_is_dropped_while_the_thumbnail_survives() {
    let shm = region("preview-oversize");
    // Per-pixel noise compresses badly, so this JPEG lands over the preview
    // slot but inside the snapshot cap. The noise is confined to the upper
    // half of the luma range, so the downscaled thumbnail must average out
    // around that band: the frame the perception stack reads is published even
    // though the operator preview is dropped.
    let image: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_fn(1400, 1050, |x, y| {
        let hash = (x as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        Luma([((hash >> 33) % 64) as u8 + 128])
    });
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut std::io::Cursor::new(&mut jpeg), 95)
        .encode_image(&image)
        .expect("encode jpeg");
    assert!(jpeg.len() > CAMERA_PREVIEW_MAX_BYTES, "{}", jpeg.len());
    assert!(jpeg.len() as u64 <= MAX_SNAPSHOT_BYTES, "{}", jpeg.len());

    let seq = ingest_snapshot_bytes(&shm, &jpeg).expect("ingest snapshot");
    assert_eq!(seq, 2);

    let frame = read_frame(&shm);
    assert!(frame.valid, "the thumbnail is still published");
    assert_eq!((frame.source_width, frame.source_height), (1400, 1050));
    assert!(
        (0.59..=0.66).contains(&frame.luminance_mean),
        "the thumbnail still tracks the snapshot's exposure: {}",
        frame.luminance_mean
    );

    let preview = shm.camera_preview();
    assert_eq!(preview.seq.load(Ordering::Acquire), 2, "the seqlock closed");
    assert_eq!(preview.len.load(Ordering::Acquire), 0);
    assert_eq!(
        (preview.width, preview.height, preview.format),
        (0, 0, PREVIEW_FORMAT_NONE)
    );
    assert_eq!(preview.timestamp_ns, 0);
}
