//! The `qualia-camera` process: attach to the arena, then feed it frames.
//!
//! The supervisor spawns this binary with the camera's environment. Everything
//! testable lives in the library; what is left here is the process boundary —
//! choosing the stream or the polling loop, and logging what the arena holds so
//! a field failure is attributable from the supervisor's child log alone.
//!
//! Exit codes: 1 cannot attach to shared memory.

use std::thread;
use std::time::{Duration, Instant};

use qualia_camera::{
    capture_once, frame_quality, ingest_snapshot_bytes, init_camera_frame, init_camera_preview,
    CameraConfig, CaptureOutcome, MjpegFrameReader, SnapshotSource, LOG_EVERY_FRAMES,
};
use qualia_shm::ShmRegion;

/// Seqlock attempts for the readback line; a torn read is retried, not fatal.
const SNAPSHOT_ATTEMPTS: usize = 8;
/// Pause before reconnecting an MJPEG stream that ended or refused to open.
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

fn main() {
    let config = CameraConfig::from_env();

    let shm = match ShmRegion::open(&config.shm_name) {
        Ok(shm) => shm,
        Err(error) => {
            eprintln!(
                "qualia-camera: failed to open shm '{}': {error}",
                config.shm_name
            );
            std::process::exit(1);
        }
    };

    init_camera_frame(shm.camera_frame_mut());
    init_camera_preview(shm.camera_preview_mut());

    let poll_ms = config.poll.as_millis();
    if let Some(url) = &config.stream_url {
        println!(
            "qualia-camera: consuming live MJPEG stream {url}; publishing at most every {poll_ms}ms"
        );
        run_http_mjpeg_stream(&shm, url, config.http_timeout, config.poll);
    }

    match &config.source {
        SnapshotSource::File(path) => {
            println!("qualia-camera: polling snapshot path {path} every {poll_ms}ms")
        }
        SnapshotSource::Http { url, .. } => println!(
            "qualia-camera: polling live snapshot {url} every {poll_ms}ms; frame history is not retained"
        ),
    }

    let mut watermark_ns = 0u128;
    let mut last_log_seq = 0u64;
    let mut failures = 0u64;
    loop {
        match capture_once(&shm, &config.source, watermark_ns) {
            CaptureOutcome::Published { seq, mtime_ns } => {
                failures = 0;
                watermark_ns = mtime_ns;
                if seq == 2 || seq >= last_log_seq.saturating_add(LOG_EVERY_FRAMES) {
                    log_camera_frame(&shm, seq);
                    last_log_seq = seq;
                }
            }
            CaptureOutcome::Unchanged => {}
            CaptureOutcome::Unavailable(reason) | CaptureOutcome::Corrupt(reason) => {
                failures = failures.saturating_add(1);
                if failures == 1 || failures % 20 == 0 {
                    eprintln!("qualia-camera: live snapshot unavailable (failure {failures}): {reason}");
                }
            }
        }
        thread::sleep(config.poll);
    }
}

/// Consumes an MJPEG stream for the life of the process.
///
/// A stream that refuses to open, answers with the wrong content type, or ends
/// is retried forever: the runner outlives the camera, and the arena keeps the
/// last frame it published until a new one arrives.
fn run_http_mjpeg_stream(
    shm: &ShmRegion,
    url: &str,
    timeout: Duration,
    min_interval: Duration,
) -> ! {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout.max(Duration::from_secs(4)))
        .build();
    let mut last_published = None::<Instant>;
    let mut last_log_seq = 0u64;
    let mut failures = 0u64;

    loop {
        let response = match agent
            .get(url)
            .set("Accept", "multipart/x-mixed-replace")
            .set("Cache-Control", "no-cache")
            .call()
        {
            Ok(response) => response,
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures == 1 || failures % 20 == 0 {
                    eprintln!(
                        "qualia-camera: MJPEG stream unavailable (failure {failures}): GET {url}: {error}"
                    );
                }
                thread::sleep(RECONNECT_DELAY);
                continue;
            }
        };

        let content_type = response.header("content-type").unwrap_or_default();
        if !content_type
            .to_ascii_lowercase()
            .contains("multipart/x-mixed-replace")
        {
            failures = failures.saturating_add(1);
            eprintln!(
                "qualia-camera: MJPEG stream returned unsupported content type {content_type:?}"
            );
            thread::sleep(RECONNECT_DELAY);
            continue;
        }

        let mut frames = MjpegFrameReader::new(response.into_reader());
        loop {
            let bytes = match frames.next_frame() {
                Ok(bytes) => bytes,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures % 20 == 0 {
                        eprintln!("qualia-camera: MJPEG stream ended (failure {failures}): {error}");
                    }
                    break;
                }
            };
            if last_published.is_some_and(|instant: Instant| instant.elapsed() < min_interval) {
                continue;
            }
            match ingest_snapshot_bytes(shm, &bytes) {
                Ok(seq) => {
                    failures = 0;
                    last_published = Some(Instant::now());
                    if seq == 2 || seq >= last_log_seq.saturating_add(LOG_EVERY_FRAMES) {
                        log_camera_frame(shm, seq);
                        last_log_seq = seq;
                    }
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures % 20 == 0 {
                        eprintln!(
                            "qualia-camera: MJPEG frame rejected (failure {failures}): {error}"
                        );
                    }
                }
            }
        }
        thread::sleep(RECONNECT_DELAY);
    }
}

/// Reads the just-published frame back out of the arena and prints its summary,
/// so the log is evidence about the bytes readers will see rather than about
/// the buffer the runner happened to hold.
fn log_camera_frame(shm: &ShmRegion, seq: u64) {
    match shm.camera_frame().snapshot(SNAPSHOT_ATTEMPTS) {
        Ok(frame) => println!(
            "qualia-camera: frame_seq={seq} src={}x{} thumb={}x{} luma_mean={:.3} luma_std={:.3} quality={}",
            frame.source_width,
            frame.source_height,
            frame.thumb_width,
            frame.thumb_height,
            frame.luminance_mean,
            frame.luminance_stddev,
            frame_quality(frame.luminance_mean, frame.luminance_stddev).label(),
        ),
        Err(error) => eprintln!("qualia-camera: frame {seq} readback failed: {error}"),
    }
}
