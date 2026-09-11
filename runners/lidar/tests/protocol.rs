//! Wire-format and scan-assembly behaviour: the LD-style frame the device
//! emits, the resynchronising extractor, the rotation assembler, and the
//! startup deadline.

use std::collections::VecDeque;
use std::time::Duration;

use qualia_lidar::{
    crc8, extract_packet, idle_action, parse_packet, DevicePoint, IdleAction, ScanAssembler,
    FRAME_HEADER, FRAME_SIZE, FRAME_VER_LEN, POINTS_PER_PACKET,
};

/// Assembles a device frame the way the hardware does: header, speed, start
/// angle, twelve `(range_mm, signal)` returns, end angle, timestamp, CRC
/// over everything but the trailing byte.
fn frame(start_cdeg: u16, end_cdeg: u16, device_time_ms: u16, readings: &[(u16, u8)]) -> [u8; FRAME_SIZE] {
    let mut raw = [0u8; FRAME_SIZE];
    raw[0] = FRAME_HEADER;
    raw[1] = FRAME_VER_LEN;
    raw[2..4].copy_from_slice(&600u16.to_le_bytes());
    raw[4..6].copy_from_slice(&start_cdeg.to_le_bytes());
    raw[42..44].copy_from_slice(&end_cdeg.to_le_bytes());
    raw[44..46].copy_from_slice(&device_time_ms.to_le_bytes());
    for (index, (distance, signal)) in readings.iter().enumerate() {
        let base = 6 + index * 3;
        raw[base..base + 2].copy_from_slice(&distance.to_le_bytes());
        raw[base + 2] = *signal;
    }
    let last = FRAME_SIZE - 1;
    raw[last] = crc8(&raw[..last]);
    raw
}

fn reading() -> Vec<(u16, u8)> {
    (0..POINTS_PER_PACKET)
        .map(|index| (1000 + index as u16, 10 + index as u8))
        .collect()
}

fn point(bearing_deg: f32) -> DevicePoint {
    DevicePoint {
        bearing_deg,
        range_mm: 1000,
        signal: 5,
    }
}

#[test]
fn the_crc_is_the_protocol_polynomial() {
    assert_eq!(crc8(&[]), 0);
    // Derived from the device's CRC-8 (polynomial 0x4D, MSB-first) over the
    // header bytes every frame opens with.
    assert_eq!(crc8(&[FRAME_HEADER, FRAME_VER_LEN]), 0xD8);
}

#[test]
fn one_packet_becomes_twelve_interpolated_points() {
    let raw = frame(0, 1100, 4242, &reading());
    let packet = parse_packet(&raw);

    assert_eq!(packet.spin_rate_dps, 600);
    assert_eq!(packet.sweep_start_cdeg, 0);
    assert_eq!(packet.sweep_end_cdeg, 1100);
    assert_eq!(packet.device_time_ms, 4242);

    for (index, point) in packet.points.iter().enumerate() {
        assert!(
            (point.bearing_deg - index as f32).abs() < 1e-5,
            "point {index} angle {}",
            point.bearing_deg
        );
        assert_eq!(point.range_mm, 1000 + index as u16);
        assert_eq!(point.signal, 10 + index as u8);
    }
}

#[test]
fn angles_sweep_through_the_wrap_point() {
    // 359.00 -> 10.00 centidegree start/end: an eleven degree sweep that
    // crosses zero, which is exactly where the assembler cuts a rotation.
    let raw = frame(35_900, 1_000, 0, &[(500, 4); POINTS_PER_PACKET]);
    let packet = parse_packet(&raw);

    assert!((packet.points[0].bearing_deg - 359.0).abs() < 1e-3);
    assert!(packet.points[1].bearing_deg.abs() < 1e-3);
    assert!((packet.points[2].bearing_deg - 1.0).abs() < 1e-3);
    assert!((packet.points[11].bearing_deg - 10.0).abs() < 1e-3);
}

#[test]
fn extraction_waits_for_a_whole_frame_before_consuming() {
    let raw = frame(0, 1100, 9, &[(2, 2); POINTS_PER_PACKET]);
    let mut stream = VecDeque::from(raw[..FRAME_SIZE - 1].to_vec());

    assert!(extract_packet(&mut stream).is_none());
    assert_eq!(stream.len(), FRAME_SIZE - 1, "no byte may be dropped");

    stream.push_back(raw[FRAME_SIZE - 1]);
    let packet = extract_packet(&mut stream).expect("the frame is now complete");
    assert_eq!(packet.device_time_ms, 9);
    assert!(stream.is_empty());
}

#[test]
fn extraction_resynchronises_past_leading_garbage() {
    let raw = frame(0, 1100, 7, &[(1, 1); POINTS_PER_PACKET]);
    let mut stream = VecDeque::from([0x00, 0xff, 0x54]);
    stream.extend(raw);

    let packet = extract_packet(&mut stream).expect("the frame after the garbage");
    assert_eq!(packet.device_time_ms, 7);
    assert!(stream.is_empty());
}

#[test]
fn a_frame_with_a_bad_crc_is_dropped_and_the_next_one_is_found() {
    let mut corrupt = frame(0, 1100, 111, &[(3, 3); POINTS_PER_PACKET]);
    corrupt[FRAME_SIZE - 1] ^= 0xff;
    let good = frame(0, 1100, 222, &[(4, 4); POINTS_PER_PACKET]);

    let mut stream = VecDeque::new();
    stream.extend(corrupt);
    stream.extend(good);

    let packet = extract_packet(&mut stream).expect("the intact frame behind the corrupt one");
    assert_eq!(packet.device_time_ms, 222);
}

#[test]
fn assembly_drops_the_first_rotation_and_yields_the_next_whole_one() {
    let mut assembler = ScanAssembler::new();

    // Rotation one sweeps up to 359 degrees without ever having started at a
    // boundary, so it is partial and must never be published.
    for angle in [0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 330.0, 359.0] {
        assert!(assembler.push(point(angle)).is_none(), "angle {angle}");
    }
    assert_eq!(assembler.buffered(), 13);

    // The wrap into rotation two discards rotation one.
    assert!(assembler.push(point(0.0)).is_none());
    assert_eq!(assembler.buffered(), 1);

    for angle in [30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 330.0, 359.0] {
        assert!(assembler.push(point(angle)).is_none(), "angle {angle}");
    }
    assert_eq!(assembler.buffered(), 13);

    // The wrap into rotation three completes and hands back rotation two.
    let completed = assembler.push(point(0.0)).expect("rotation two completes");
    assert_eq!(completed.len(), 13);
    assert_eq!(completed[0].bearing_deg, 0.0);
    assert_eq!(completed[12].bearing_deg, 359.0);
    assert_eq!(assembler.buffered(), 1, "the boundary point opens rotation three");
}

#[test]
fn the_startup_deadline_fires_without_a_scan_and_then_only_warns() {
    let timeout = Duration::from_secs(5);

    assert_eq!(
        idle_action(0, Duration::from_millis(4999), Duration::from_millis(4999), timeout),
        IdleAction::Idle
    );
    assert_eq!(
        idle_action(0, Duration::from_secs(5), Duration::from_secs(5), timeout),
        IdleAction::Fail,
        "no complete scan inside the deadline is fatal"
    );
    assert_eq!(
        idle_action(1, Duration::from_secs(600), Duration::from_millis(4999), timeout),
        IdleAction::Idle
    );
    assert_eq!(
        idle_action(1, Duration::from_secs(600), Duration::from_secs(5), timeout),
        IdleAction::Warn,
        "a stalled stream that already produced a scan only warns"
    );
}
