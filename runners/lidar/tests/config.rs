//! The crate's configuration contract: the serial environment keys and their
//! documented defaults.

use std::time::Duration;

use qualia_lidar::{
    LidarConfig, DEFAULT_BAUD, DEFAULT_LOG_EVERY_SCANS, DEFAULT_PORT, DEFAULT_SHM_NAME,
    DEFAULT_TIMEOUT_SECS,
};

fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl FnMut(&str) -> Option<String> {
    move |key| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.to_string())
    }
}

#[test]
fn an_empty_environment_yields_the_documented_defaults() {
    let config = LidarConfig::from_lookup(|_| None);

    assert_eq!(DEFAULT_PORT, "/dev/ttyACM0");
    assert_eq!(DEFAULT_BAUD, 230_400);
    assert_eq!(DEFAULT_TIMEOUT_SECS, 5);
    assert_eq!(DEFAULT_LOG_EVERY_SCANS, 10);
    assert_eq!(DEFAULT_SHM_NAME, "/qualia_body");

    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.baud, DEFAULT_BAUD);
    assert_eq!(config.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    assert_eq!(config.log_every_scans, DEFAULT_LOG_EVERY_SCANS);
    assert_eq!(config.shm_name, DEFAULT_SHM_NAME);
}

#[test]
fn every_serial_key_is_read_from_the_environment() {
    let config = LidarConfig::from_lookup(lookup(&[
        ("QUALIA_LIDAR_PORT", "/dev/ttyUSB1"),
        ("QUALIA_LIDAR_BAUD", "115200"),
        ("QUALIA_LIDAR_TIMEOUT_SECS", "2"),
        ("QUALIA_LIDAR_LOG_EVERY_SCANS", "3"),
        ("QUALIA_SHM_NAME", "/qualia_test"),
    ]));

    assert_eq!(config.port, "/dev/ttyUSB1");
    assert_eq!(config.baud, 115_200);
    assert_eq!(config.timeout, Duration::from_secs(2));
    assert_eq!(config.log_every_scans, 3);
    assert_eq!(config.shm_name, "/qualia_test");
}

#[test]
fn unparseable_numbers_fall_back_instead_of_failing_startup() {
    let config = LidarConfig::from_lookup(lookup(&[
        ("QUALIA_LIDAR_BAUD", "fast"),
        ("QUALIA_LIDAR_TIMEOUT_SECS", ""),
        ("QUALIA_LIDAR_LOG_EVERY_SCANS", "-1"),
    ]));

    assert_eq!(config.baud, DEFAULT_BAUD);
    assert_eq!(config.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    assert_eq!(config.log_every_scans, DEFAULT_LOG_EVERY_SCANS);
}
