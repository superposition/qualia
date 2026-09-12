//! `qualia-leash-sensors`: publish the body's own sensor surface into the arena.
//!
//! The body's hardware has exactly one owner. On Pinkie that owner is the
//! leash (`leash serve http`): it holds the LiDAR's serial port and the drive
//! port, and it republishes what they carry on its own HTTP surface. A second
//! opener of the same port would be a second owner, so the stack sees those
//! sensors by subscribing to the owner instead: this runner polls the leash's
//! `observe` tool and writes the range scan into the region every other runner
//! reads, through `qualia-lidar`'s own scan publisher, so the polar scan and
//! the occupancy grid keep their single implementation.
//!
//! What the leash carries and where it goes:
//!
//! * `sensors.range_scan` — the LD06 rotation, published here as the arena's
//!   polar scan and occupancy grid.
//! * `sensors.imu`, `sensors.odometry` — read and reported by this runner, but
//!   deliberately not written into the arena: the arena has no inertial slot
//!   and the canonical pose slot carries a confidence claim the leash's wheel
//!   odometry cannot make (its localization provider reports the pose itself
//!   as unavailable), so publishing it would mean inventing the claim a
//!   downstream gate reads.
//!
//! Configuration:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `QUALIA_LEASH_BASE_URL` | The leash's HTTP root; default [`DEFAULT_BASE_URL`]. |
//! | `QUALIA_SHM_NAME` | The arena to attach to; default [`DEFAULT_SHM_NAME`]. |
//! | `QUALIA_LEASH_SENSORS_POLL_MS` | Poll interval; default [`DEFAULT_POLL_MS`]. |
//! | `QUALIA_LEASH_SENSORS_TIMEOUT_MS` | Request timeout; default [`DEFAULT_TIMEOUT_MS`]. |
//!
//! Exit codes: 1 cannot attach to shared memory.

use qualia_leash_sensors::{
    agent, observe, ScanSample, BASE_URL_ENV, DEFAULT_BASE_URL, DEFAULT_SHM_NAME, DEFAULT_TIMEOUT_MS,
    SHM_NAME_ENV, TIMEOUT_MS_ENV,
};
use qualia_lidar::{publish_scan, summarize, DevicePoint};
use qualia_shm::ShmRegion;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `QUALIA_LEASH_SENSORS_POLL_MS`, default `100`.
pub const POLL_MS_ENV: &str = "QUALIA_LEASH_SENSORS_POLL_MS";
/// Time between two `observe` calls when none is named: 10 Hz, the LD06's rate.
pub const DEFAULT_POLL_MS: u64 = 100;
/// Published scans between summary lines once the first scan has been logged.
pub const LOG_EVERY_SCANS: u64 = 50;

/// Everything the process learns from the environment before the loop starts.
#[derive(Debug, Clone)]
struct Settings {
    base_url: String,
    shm_name: String,
    poll: Duration,
    timeout: Duration,
}

impl Settings {
    /// Reads every key, substituting the documented default for anything
    /// absent or unparseable: a malformed interval must not stop the runner.
    fn from_env() -> Self {
        let number = |key: &str, fallback: u64| {
            std::env::var(key)
                .ok()
                .and_then(|raw| raw.trim().parse().ok())
                .unwrap_or(fallback)
        };
        let text = |key: &str, fallback: &str| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| fallback.to_owned())
        };
        Self {
            base_url: text(BASE_URL_ENV, DEFAULT_BASE_URL),
            shm_name: text(SHM_NAME_ENV, DEFAULT_SHM_NAME),
            poll: Duration::from_millis(number(POLL_MS_ENV, DEFAULT_POLL_MS).max(1)),
            timeout: Duration::from_millis(number(TIMEOUT_MS_ENV, DEFAULT_TIMEOUT_MS).max(1)),
        }
    }
}

fn main() {
    let settings = Settings::from_env();

    let shm = match ShmRegion::open(&settings.shm_name) {
        Ok(shm) => shm,
        Err(error) => {
            eprintln!(
                "qualia-leash-sensors: failed to open shm '{}': {error}",
                settings.shm_name
            );
            std::process::exit(1);
        }
    };

    let agent = agent(settings.timeout);

    println!(
        "qualia-leash-sensors: subscribing to {}/mcp observe every {}ms; publishing range scans into {}",
        settings.base_url,
        settings.poll.as_millis(),
        settings.shm_name
    );

    let mut last_scan_ms = 0u64;
    let mut published = 0u64;
    let mut failures = 0u64;
    let mut reported_unavailable = false;
    let mut reported_surface = false;

    loop {
        match observe(&agent, &settings.base_url) {
            Ok(sensors) => {
                failures = 0;
                if !reported_surface {
                    reported_surface = true;
                    println!(
                        "qualia-leash-sensors: leash surface {} (range scan is what this runner publishes)",
                        sensors.summary()
                    );
                }
                let scan = sensors.range_scan;
                let available = scan
                    .as_ref()
                    .is_some_and(|scan| scan.is_available());
                if available {
                    let scan = scan.expect("range scan checked above");
                    let sample = scan.sample.expect("range scan checked above");
                    if sample.ts_ms > last_scan_ms {
                        let points = device_points(&sample);
                        let summary = summarize(&points);
                        publish_scan(&shm, &points);
                        last_scan_ms = sample.ts_ms;
                        published += 1;
                        reported_unavailable = false;
                        if published == 1 || published % LOG_EVERY_SCANS == 0 {
                            println!(
                                "qualia-leash-sensors: scan {} source={} frame={} points={} valid={} rate={:.3}Hz range_mm={}..{} age_ms={}",
                                published,
                                scan.source,
                                sample.frame_id,
                                summary.total,
                                summary.valid,
                                sample.scan_rate_hz,
                                summary.min_mm,
                                summary.max_mm,
                                now_ms().saturating_sub(scan.last_ms),
                            );
                        }
                    }
                } else if !reported_unavailable {
                    reported_unavailable = true;
                    match &scan {
                        Some(scan) => eprintln!(
                            "qualia-leash-sensors: range scan is not available (status={} source={} error={:?})",
                            scan.status, scan.source, scan.error
                        ),
                        None => {
                            eprintln!("qualia-leash-sensors: the leash reports no range scan")
                        }
                    }
                }
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures == 1 || failures % 20 == 0 {
                    eprintln!(
                        "qualia-leash-sensors: observe unavailable (failure {failures}): {error}"
                    );
                }
            }
        }
        thread::sleep(settings.poll);
    }
}

/// Turns one rotation in the leash's units into the arena's device points.
///
/// Every ray keeps its place in the rotation: a reading with no range, or a
/// range that is not a finite positive distance, becomes a zero range and a
/// zero signal, which is how the scan publisher marks a ray that carried
/// nothing back. Dropping those rays instead would make every scan look fully
/// valid and hide the returns the device actually missed. The intensity is
/// clamped into the device point's byte because the arena's point is the LD06's
/// own encoding, not a float channel.
fn device_points(sample: &ScanSample) -> Vec<DevicePoint> {
    let mut points = Vec::with_capacity(sample.ranges_m.len());
    for (index, range) in sample.ranges_m.iter().enumerate() {
        let returned = range.filter(|range_m| range_m.is_finite() && *range_m > 0.0);
        let intensity = match returned {
            Some(_) => sample
                .intensities
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(0.0),
            None => 0.0,
        };
        points.push(DevicePoint {
            bearing_deg: (sample.angle_min_rad
                + index as f32 * sample.angle_increment_rad)
                .to_degrees(),
            range_mm: returned
                .map(|range_m| (range_m * 1000.0).round().clamp(0.0, u16::MAX as f32) as u16)
                .unwrap_or(0),
            signal: intensity.clamp(0.0, 255.0) as u8,
        });
    }
    points
}

/// Wall-clock milliseconds since the Unix epoch, or zero if the clock is set
/// before it.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        angle_min_rad: f32,
        angle_increment_rad: f32,
        ranges_m: Vec<Option<f32>>,
    ) -> ScanSample {
        ScanSample {
            angle_min_rad,
            angle_increment_rad,
            intensities: ranges_m
                .iter()
                .map(|range| range.map(|_| 1.0))
                .collect(),
            ranges_m,
            frame_id: "base_scan".to_string(),
            scan_rate_hz: 10.0,
            ts_ms: 1,
        }
    }

    #[test]
    fn a_reading_without_a_return_keeps_its_ray() {
        let sample = ScanSample {
            intensities: vec![Some(120.0), Some(5.0), Some(7.0), Some(9.0)],
            ..sample(
                0.0,
                std::f32::consts::FRAC_PI_2,
                vec![Some(1.5), None, Some(0.0), Some(f32::NAN)],
            )
        };
        let points = device_points(&sample);
        assert_eq!(points.len(), 4);
        assert_eq!(points[0].range_mm, 1500);
        assert_eq!(points[0].signal, 120);
        for point in &points[1..] {
            assert_eq!(point.range_mm, 0);
            assert_eq!(point.signal, 0);
        }
        assert_eq!(points[3].bearing_deg, 270.0);
    }

    #[test]
    fn the_bearing_walks_by_the_increment() {
        let sample = sample(
            -std::f32::consts::FRAC_PI_2,
            std::f32::consts::FRAC_PI_2,
            vec![Some(1.0), Some(2.0), Some(3.0)],
        );
        let bearings: Vec<f32> = device_points(&sample)
            .iter()
            .map(|point| point.bearing_deg)
            .collect();
        assert_eq!(bearings, vec![-90.0, 0.0, 90.0]);
    }
}
