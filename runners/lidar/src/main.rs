//! `qualia-lidar`: serial LiDAR ingest for the body bus.
//!
//! Reads the ranging device, assembles whole rotations, publishes them into
//! shared memory, and reports every `QUALIA_LIDAR_LOG_EVERY_SCANS` rotations.
//! Exit codes: 1 cannot attach to shared memory, 2 cannot open the port,
//! 3 the port read failed, 4 no complete rotation arrived inside
//! `QUALIA_LIDAR_TIMEOUT_SECS`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use qualia_lidar::{
    extract_packet, idle_action, publish_scan, summarize, DevicePoint, IdleAction, LidarConfig,
    ScanAssembler, FRAME_SIZE,
};
use qualia_shm::ShmRegion;
use tokio::io::AsyncReadExt;
use tokio::time::timeout;
use tokio_serial::SerialPortBuilderExt;

/// How long one read may block before the loop re-checks its deadlines.
const READ_TIMEOUT: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() {
    let config = LidarConfig::from_env();

    let shm = match ShmRegion::open(&config.shm_name) {
        Ok(region) => region,
        Err(error) => {
            eprintln!(
                "qualia-lidar: failed to open shm '{}': {error}",
                config.shm_name
            );
            std::process::exit(1);
        }
    };

    println!(
        "qualia-lidar: opening serial port {} @ {}",
        config.port, config.baud
    );

    let mut port = match tokio_serial::new(&config.port, config.baud).open_native_async() {
        Ok(port) => port,
        Err(error) => {
            eprintln!("qualia-lidar: open failed: {error}");
            std::process::exit(2);
        }
    };

    let boot = Instant::now();
    let mut chunk = [0u8; 4096];
    let mut incoming: VecDeque<u8> = VecDeque::with_capacity(FRAME_SIZE * 4);
    let mut assembler = ScanAssembler::new();
    let mut packet_count = 0u64;
    let mut rotations = 0u64;
    let mut last_publication = Instant::now();
    let log_every_scans = config.log_every_scans.max(1);

    loop {
        match timeout(READ_TIMEOUT, port.read(&mut chunk)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(read)) => {
                incoming.extend(&chunk[..read]);
                while let Some(packet) = extract_packet(&mut incoming) {
                    packet_count += 1;
                    for point in packet.points {
                        let Some(rotation) = assembler.push(point) else {
                            continue;
                        };
                        publish_scan(&shm, &rotation);
                        rotations = rotations.wrapping_add(1);
                        last_publication = Instant::now();
                        if rotations == 1 || rotations % log_every_scans == 0 {
                            report(&shm, packet_count, &rotation);
                        }
                    }
                }
            }
            Ok(Err(error)) => {
                eprintln!("qualia-lidar: read failed: {error}");
                std::process::exit(3);
            }
            Err(_) => {
                let buffered_points = assembler.buffered();
                match idle_action(
                    rotations,
                    boot.elapsed(),
                    last_publication.elapsed(),
                    config.timeout,
                ) {
                    IdleAction::Fail => {
                        eprintln!(
                            "qualia-lidar: no complete scan assembled packet_count={packet_count} buffered_points={buffered_points}"
                        );
                        std::process::exit(4);
                    }
                    IdleAction::Warn => {
                        eprintln!(
                            "qualia-lidar: waiting for complete scan packet_count={packet_count} buffered_points={buffered_points}"
                        );
                        last_publication = Instant::now();
                    }
                    IdleAction::Idle => {}
                }
            }
        }
    }
}

/// Reads the just-published frames back out of shared memory and prints the
/// rotation summary, so the log is evidence about the bytes readers will see
/// rather than about the buffer the runner happened to hold.
fn report(shm: &ShmRegion, packet_count: u64, rotation: &[DevicePoint]) {
    let scan = shm
        .lidar_scan()
        .snapshot(8)
        .expect("the runner just published this scan");
    let grid = shm
        .lidar_grid()
        .snapshot(8)
        .expect("the runner just published this grid");
    let occupied = grid.cells.iter().filter(|&&cell| cell != 0).count();
    println!(
        "qualia-lidar: shm_readback scan_seq={} point_count={} grid_seq={} occupied_cells={}",
        scan.seq, scan.point_count, grid.seq, occupied
    );

    let summary = summarize(rotation);
    let sample = rotation
        .iter()
        .take(8)
        .map(|point| {
            format!(
                "{:.1}deg:{}mm@{}",
                point.bearing_deg, point.range_mm, point.signal
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    println!("qualia-lidar: packets={packet_count}");
    println!(
        "qualia-lidar: scan_points={} valid_points={} min_mm={} max_mm={}",
        summary.total, summary.valid, summary.min_mm, summary.max_mm
    );
    println!("qualia-lidar: sample_points={sample}");
    println!("qualia-lidar: scan ok");
}
