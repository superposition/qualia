//! Reframing an MJPEG response: the runner must find whole JPEG frames in a
//! multipart stream, however the socket chooses to deliver the bytes.

use std::io::{Cursor, Read};

use qualia_camera::{MjpegFrameReader, MAX_SNAPSHOT_BYTES};

const FIRST: &[u8] = b"\xff\xd8first frame\xff\xd9";
const SECOND: &[u8] = b"\xff\xd8second frame\xff\xd9";

fn multipart() -> Vec<u8> {
    let mut body = Vec::new();
    for frame in [FIRST, SECOND] {
        body.extend_from_slice(b"--leashframe\r\nContent-Type: image/jpeg\r\n\r\n");
        body.extend_from_slice(frame);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--leashframe--\r\n");
    body
}

/// A reader that never hands out more than `chunk` bytes at a time, the way a
/// socket under load does.
struct Trickle {
    inner: Cursor<Vec<u8>>,
    chunk: usize,
}

impl Read for Trickle {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let limit = buffer.len().min(self.chunk);
        self.inner.read(&mut buffer[..limit])
    }
}

#[test]
fn consecutive_frames_are_cut_out_of_a_multipart_body() {
    let mut reader = MjpegFrameReader::new(Cursor::new(multipart()));

    assert_eq!(reader.next_frame().expect("first frame"), FIRST);
    assert_eq!(reader.next_frame().expect("second frame"), SECOND);
    assert!(reader.next_frame().is_err(), "the stream has no third frame");
}

#[test]
fn frames_split_across_reads_are_reassembled() {
    for chunk in [1usize, 3, 7, 64] {
        let mut reader = MjpegFrameReader::new(Trickle {
            inner: Cursor::new(multipart()),
            chunk,
        });
        assert_eq!(
            reader.next_frame().expect("first frame"),
            FIRST,
            "chunk size {chunk}"
        );
        assert_eq!(
            reader.next_frame().expect("second frame"),
            SECOND,
            "chunk size {chunk}"
        );
    }
}

#[test]
fn bytes_before_the_first_frame_are_discarded() {
    let mut body = b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace\r\n\r\n".to_vec();
    body.extend_from_slice(FIRST);
    let mut reader = MjpegFrameReader::new(Cursor::new(body));

    assert_eq!(reader.next_frame().expect("first frame"), FIRST);
}

#[test]
fn a_stream_that_stops_mid_frame_ends_with_an_error() {
    let mut reader = MjpegFrameReader::new(Cursor::new(b"\xff\xd8only half a frame".to_vec()));
    assert!(reader.next_frame().is_err());

    let mut reader = MjpegFrameReader::new(Cursor::new(b"no jpeg here at all".to_vec()));
    assert!(reader.next_frame().is_err());

    let mut reader = MjpegFrameReader::new(Cursor::new(Vec::new()));
    let error = reader.next_frame().expect_err("an empty stream has no frame");
    assert_eq!(error, "MJPEG stream closed before the next complete frame");
}

#[test]
fn a_frame_beyond_the_snapshot_cap_is_an_error() {
    let mut body = vec![0xff, 0xd8];
    body.resize(MAX_SNAPSHOT_BYTES as usize + 2, 0x00);
    let mut reader = MjpegFrameReader::new(Cursor::new(body));

    let error = reader
        .next_frame()
        .expect_err("the frame never ends");
    assert_eq!(
        error,
        format!("MJPEG frame exceeds {MAX_SNAPSHOT_BYTES} byte limit")
    );
}
