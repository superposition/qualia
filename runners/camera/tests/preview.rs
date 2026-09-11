//! The operator preview slot: the seqlock the console reads, and the encoding
//! record it decodes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use qualia_camera::{
    publish_camera_preview, PREVIEW_FORMAT_JPEG, PREVIEW_FORMAT_NONE, PREVIEW_FORMAT_PNG,
};
use qualia_shm::ShmRegion;
use qualia_types::CAMERA_PREVIEW_MAX_BYTES;

static NEXT_REGION: AtomicU64 = AtomicU64::new(0);

fn region(tag: &str) -> ShmRegion {
    let name = format!(
        "/qualia_camera_preview_{}_{}_{}",
        std::process::id(),
        tag,
        NEXT_REGION.fetch_add(1, Ordering::Relaxed)
    );
    ShmRegion::create(&name).expect("create region")
}

fn jpeg(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff];
    bytes.extend_from_slice(payload);
    bytes
}

fn png(payload: &[u8]) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn a_published_preview_records_its_encoding_and_geometry() {
    let shm = region("encoding");
    let encoded = jpeg(b"a small jpeg body");
    publish_camera_preview(&shm, &encoded, 640, 480);

    let preview = shm.camera_preview();
    let seq = preview.seq.load(Ordering::Acquire);
    let len = preview.len.load(Ordering::Acquire);
    assert_eq!(seq, 2, "one publish advances the sequence once");
    assert_eq!(preview.format, PREVIEW_FORMAT_JPEG);
    assert_eq!((preview.width, preview.height), (640, 480));
    assert!(preview.timestamp_ns > 0);
    assert_eq!(len, encoded.len());
    assert_eq!(&preview.bytes[..len], &encoded[..]);

    let other = png(b"a png body");
    publish_camera_preview(&shm, &other, 320, 200);
    let seq = preview.seq.load(Ordering::Acquire);
    let len = preview.len.load(Ordering::Acquire);
    assert_eq!(seq, 4);
    assert_eq!(preview.format, PREVIEW_FORMAT_PNG);
    assert_eq!((preview.width, preview.height), (320, 200));
    assert_eq!(len, other.len());
    assert_eq!(&preview.bytes[..len], &other[..]);
}

#[test]
fn an_unknown_or_empty_encoding_clears_the_slot() {
    let shm = region("cleared");
    let preview = shm.camera_preview();
    publish_camera_preview(&shm, &jpeg(b"body"), 100, 100);
    assert!(preview.len.load(Ordering::Acquire) > 0);

    assert_eq!(PREVIEW_FORMAT_NONE, 0);
    publish_camera_preview(&shm, b"GIF89a not a preview this slot carries", 100, 100);
    assert_eq!(preview.len.load(Ordering::Acquire), 0);
    assert_eq!(
        (preview.width, preview.height, preview.format),
        (0, 0, PREVIEW_FORMAT_NONE)
    );
    assert_eq!(preview.timestamp_ns, 0);
    assert_eq!(preview.seq.load(Ordering::Acquire), 4);

    publish_camera_preview(&shm, &jpeg(b"again"), 100, 100);
    assert!(preview.len.load(Ordering::Acquire) > 0);
    publish_camera_preview(&shm, &[], 0, 0);
    assert_eq!(preview.len.load(Ordering::Acquire), 0);
    assert_eq!(
        (preview.width, preview.height, preview.format),
        (0, 0, PREVIEW_FORMAT_NONE)
    );
    assert_eq!(preview.seq.load(Ordering::Acquire), 8);
}

#[test]
fn the_sequence_never_parks_on_an_odd_value() {
    let shm = region("sequence");
    let preview = shm.camera_preview();
    let encoded = jpeg(b"body");

    for publish in 1..=5u64 {
        publish_camera_preview(&shm, &encoded, 16, 16);
        let seq = preview.seq.load(Ordering::Acquire);
        assert_eq!(seq, publish * 2);
        assert_eq!(seq % 2, 0, "a reader may take an even sequence");
    }
}

#[test]
fn readers_never_observe_a_torn_preview() {
    let shm = region("torn");
    let preview = shm.camera_preview();
    let small = jpeg(&[0xaa; 4096]);
    let large = jpeg(&[0x55; 16_384]);
    let stop = AtomicBool::new(false);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut round = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let bytes = if round % 2 == 0 { &small } else { &large };
                publish_camera_preview(&shm, bytes, 64, 48);
                round += 1;
                // Let the reader run between publishes; it still starts reads
                // while a copy is in flight.
                thread::yield_now();
            }
        });

        // Reader protocol exactly as the console and the agent use it: take the
        // sequence, copy, then require the same even sequence still stands.
        let mut accepted = 0u64;
        let mut torn = 0u64;
        let deadline = Instant::now() + Duration::from_secs(5);
        while accepted < 500 && Instant::now() < deadline {
            let before = preview.seq.load(Ordering::Acquire);
            if before == 0 || before % 2 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let len = preview
                .len
                .load(Ordering::Acquire)
                .min(CAMERA_PREVIEW_MAX_BYTES);
            if len == 0 {
                continue;
            }
            // Both encodings carry the same three-byte JPEG prefix, so every
            // byte of the payload must match: a copy that mixed two publishes
            // would show both fills.
            let fill = preview.bytes[3];
            let coherent = preview.bytes[3..len].iter().all(|&byte| byte == fill);
            let after = preview.seq.load(Ordering::Acquire);
            if before != after || after % 2 != 0 {
                // A publish overlapped the read; it is not evidence either way.
                continue;
            }
            accepted += 1;
            if !coherent {
                torn += 1;
            }
        }

        stop.store(true, Ordering::Relaxed);
        assert_eq!(torn, 0, "a coherent read saw two encodings mixed together");
        assert_eq!(accepted, 500, "the reader never caught complete previews");
    });
}
