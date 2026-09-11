//! What one captured snapshot does to the shared thumbnail slot: geometry,
//! luma statistics and the exposure classification the runner logs.

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

use image::codecs::jpeg::JpegEncoder;
use image::{ImageBuffer, ImageFormat, Luma, Rgb, RgbImage};
use qualia_camera::{
    frame_quality, ingest_snapshot_bytes, init_camera_frame, init_camera_preview, FrameQuality,
};
use qualia_shm::ShmRegion;
use qualia_types::{CameraFrameSnapshot, CAMERA_THUMB_H, CAMERA_THUMB_PIXELS, CAMERA_THUMB_W};

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region(tag: &str) -> ShmRegion {
    let name = format!(
        "/qualia_camera_frame_{}_{}_{}",
        std::process::id(),
        tag,
        NEXT_REGION.fetch_add(1, Ordering::Relaxed)
    );
    ShmRegion::create(&name).expect("create region")
}

fn encode_png(image: &ImageBuffer<Luma<u8>, Vec<u8>>) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode snapshot");
    bytes
}

/// A luma snapshot whose brightness is decided per pixel.
fn luma_snapshot(width: u32, height: u32, mut pixel: impl FnMut(u32, u32) -> u8) -> Vec<u8> {
    encode_png(&ImageBuffer::from_fn(width, height, |x, y| {
        Luma([pixel(x, y)])
    }))
}

fn read_frame(shm: &ShmRegion) -> CameraFrameSnapshot {
    shm.camera_frame().snapshot(64).expect("coherent frame")
}

fn thumbnail_row(frame: &CameraFrameSnapshot, row: usize) -> &[u8] {
    let width = CAMERA_THUMB_W;
    &frame.thumbnail_luma[row * width..(row + 1) * width]
}

#[test]
fn a_snapshot_is_decoded_into_the_shared_thumbnail() {
    let shm = region("decode");
    // A left/right split survives a 128x96 -> 64x48 downscale unchanged, so the
    // thumbnail's own pixels say what the decoder did with the source.
    let png = luma_snapshot(128, 96, |x, _| if x < 64 { 0 } else { 255 });

    let seq = ingest_snapshot_bytes(&shm, &png).expect("ingest snapshot");
    assert_eq!(seq, 2, "one publish advances the sequence once");

    let frame = read_frame(&shm);
    assert_eq!(frame.seq, seq);
    assert!(frame.valid, "a decoded snapshot is a valid frame");
    assert_eq!((frame.source_width, frame.source_height), (128, 96));
    assert_eq!(
        (frame.thumb_width, frame.thumb_height),
        (CAMERA_THUMB_W as u32, CAMERA_THUMB_H as u32)
    );
    assert_eq!(frame.thumbnail_luma.len(), CAMERA_THUMB_PIXELS);
    assert!(frame.timestamp_ns > 0, "the frame is stamped with wall time");

    for row in 0..CAMERA_THUMB_H {
        let row = thumbnail_row(&frame, row);
        assert!(
            row[..24].iter().all(|&px| px == 0),
            "the dark half stays dark: {row:?}"
        );
        assert!(
            row[40..].iter().all(|&px| px == 255),
            "the bright half stays bright: {row:?}"
        );
    }

    assert!(
        (frame.luminance_mean - 0.5).abs() < 0.02,
        "mean {}",
        frame.luminance_mean
    );
    assert!(
        (frame.luminance_stddev - 0.5).abs() < 0.02,
        "stddev {}",
        frame.luminance_stddev
    );
}

#[test]
fn luma_statistics_track_the_thumbnail_pixels() {
    let shm = region("stats");
    let png = luma_snapshot(CAMERA_THUMB_W as u32, CAMERA_THUMB_H as u32, |_, _| 128);

    ingest_snapshot_bytes(&shm, &png).expect("ingest snapshot");
    let frame = read_frame(&shm);

    let expected = 128.0 / 255.0;
    assert!(
        (frame.luminance_mean - expected).abs() < 1e-4,
        "mean {}",
        frame.luminance_mean
    );
    assert!(
        frame.luminance_stddev < 1e-4,
        "a flat snapshot has no spread: {}",
        frame.luminance_stddev
    );
    assert!(frame
        .thumbnail_luma
        .iter()
        .all(|&px| px == frame.thumbnail_luma[0]));
}

#[test]
fn a_jpeg_snapshot_decodes_to_the_same_geometry() {
    let shm = region("jpeg");
    let image: RgbImage =
        ImageBuffer::from_fn(160, 120, |x, _| if x < 80 { Rgb([0, 0, 0]) } else { Rgb([255, 255, 255]) });
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut Cursor::new(&mut jpeg), 90)
        .encode_image(&image)
        .expect("encode jpeg");

    let seq = ingest_snapshot_bytes(&shm, &jpeg).expect("ingest jpeg");
    let frame = read_frame(&shm);

    assert_eq!(frame.seq, seq);
    assert!(frame.valid);
    assert_eq!((frame.source_width, frame.source_height), (160, 120));
    assert!(
        (0.44..=0.56).contains(&frame.luminance_mean),
        "mean {}",
        frame.luminance_mean
    );
    assert!(
        (0.44..=0.56).contains(&frame.luminance_stddev),
        "stddev {}",
        frame.luminance_stddev
    );
}

#[test]
fn bytes_that_do_not_decode_publish_nothing() {
    let shm = region("garbage");
    let png = luma_snapshot(32, 32, |_, _| 200);

    assert!(ingest_snapshot_bytes(&shm, b"this is not an image").is_err());
    assert!(ingest_snapshot_bytes(&shm, &png[..16]).is_err(), "a truncated snapshot");
    assert!(ingest_snapshot_bytes(&shm, &[]).is_err(), "an empty snapshot");

    let frame = read_frame(&shm);
    assert_eq!(frame.seq, 0, "a rejected snapshot publishes no frame");
    assert!(!frame.valid);
    assert_eq!(
        shm.camera_preview().seq.load(Ordering::Acquire),
        0,
        "a rejected snapshot publishes no preview"
    );
    assert_eq!(shm.camera_preview().len.load(Ordering::Acquire), 0);
}

#[test]
fn initialising_a_slot_clears_whatever_it_held() {
    let shm = region("init");
    let png = luma_snapshot(64, 48, |_, _| 255);
    ingest_snapshot_bytes(&shm, &png).expect("ingest snapshot");
    assert!(read_frame(&shm).valid);

    init_camera_frame(shm.camera_frame_mut());
    init_camera_preview(shm.camera_preview_mut());

    let frame = read_frame(&shm);
    assert_eq!(frame.seq, 0);
    assert!(!frame.valid);
    assert_eq!((frame.source_width, frame.source_height), (0, 0));
    assert_eq!(
        (frame.thumb_width, frame.thumb_height),
        (CAMERA_THUMB_W as u32, CAMERA_THUMB_H as u32)
    );
    assert_eq!((frame.luminance_mean, frame.luminance_stddev), (0.0, 0.0));
    assert_eq!(frame.thumbnail_luma, [0; CAMERA_THUMB_PIXELS]);

    let preview = shm.camera_preview();
    assert_eq!(preview.seq.load(Ordering::Acquire), 0);
    assert_eq!(preview.len.load(Ordering::Acquire), 0);
    assert_eq!((preview.width, preview.height, preview.format), (0, 0, 0));
    assert_eq!(preview.timestamp_ns, 0);
}

#[test]
fn exposure_classification_prefers_the_extreme_over_the_flatness() {
    assert_eq!(frame_quality(0.95, 0.10), FrameQuality::Blown);
    assert_eq!(frame_quality(0.05, 0.10), FrameQuality::Dark);
    assert_eq!(frame_quality(0.50, 0.02), FrameQuality::Flat);
    assert_eq!(frame_quality(0.50, 0.10), FrameQuality::Usable);

    // The windows are inclusive at their edges.
    assert_eq!(frame_quality(0.90, 0.10), FrameQuality::Blown);
    assert_eq!(frame_quality(0.08, 0.10), FrameQuality::Dark);
    assert_eq!(frame_quality(0.50, 0.04), FrameQuality::Flat);
    assert_eq!(frame_quality(0.899, 0.10), FrameQuality::Usable);
    assert_eq!(frame_quality(0.081, 0.10), FrameQuality::Usable);
    assert_eq!(frame_quality(0.50, 0.041), FrameQuality::Usable);

    // A blown frame is reported as blown even when it is also flat.
    assert_eq!(frame_quality(0.99, 0.0), FrameQuality::Blown);
    assert_eq!(frame_quality(0.0, 0.0), FrameQuality::Dark);
}
