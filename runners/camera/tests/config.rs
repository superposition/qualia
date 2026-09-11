//! The camera runner's configuration contract: the environment keys it reads,
//! their documented defaults, and what a malformed value falls back to.

use std::time::Duration;

use qualia_camera::{
    CameraConfig, SnapshotSource, DEFAULT_HTTP_POLL_MS, DEFAULT_HTTP_TIMEOUT_MS, DEFAULT_POLL_MS,
    DEFAULT_SHM_NAME, DEFAULT_SNAPSHOT_PATH, HTTP_TIMEOUT_MS_ENV, MIN_POLL_MS, POLL_MS_ENV,
    SHM_NAME_ENV, SNAPSHOT_PATH_ENV, SNAPSHOT_URL_ENV, STREAM_URL_ENV,
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
fn the_environment_keys_are_the_camera_contract() {
    assert_eq!(SHM_NAME_ENV, "QUALIA_SHM_NAME");
    assert_eq!(STREAM_URL_ENV, "QUALIA_CAMERA_STREAM_URL");
    assert_eq!(SNAPSHOT_URL_ENV, "QUALIA_CAMERA_SNAPSHOT_URL");
    assert_eq!(SNAPSHOT_PATH_ENV, "QUALIA_CAMERA_SNAPSHOT_PATH");
    assert_eq!(POLL_MS_ENV, "QUALIA_CAMERA_POLL_MS");
    assert_eq!(HTTP_TIMEOUT_MS_ENV, "QUALIA_CAMERA_HTTP_TIMEOUT_MS");
}

#[test]
fn an_empty_environment_yields_the_documented_defaults() {
    let config = CameraConfig::from_lookup(|_| None);

    assert_eq!(DEFAULT_SHM_NAME, "/qualia_body");
    assert_eq!(DEFAULT_SNAPSHOT_PATH, "/tmp/qualia_orin_snap.jpg");
    assert_eq!(DEFAULT_POLL_MS, 100);
    assert_eq!(DEFAULT_HTTP_POLL_MS, 500);
    assert_eq!(DEFAULT_HTTP_TIMEOUT_MS, 2_000);

    assert_eq!(config.shm_name, DEFAULT_SHM_NAME);
    assert_eq!(config.stream_url, None);
    assert_eq!(config.poll, Duration::from_millis(DEFAULT_POLL_MS));
    assert_eq!(
        config.http_timeout,
        Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS)
    );
    match &config.source {
        SnapshotSource::File(path) => assert_eq!(path, DEFAULT_SNAPSHOT_PATH),
        SnapshotSource::Http { url, .. } => panic!("no snapshot URL is set, got {url}"),
    }
}

#[test]
fn every_camera_key_is_read_from_the_environment() {
    let config = CameraConfig::from_lookup(lookup(&[
        ("QUALIA_SHM_NAME", "/qualia_test"),
        ("QUALIA_CAMERA_STREAM_URL", "http://body.local:8080/stream"),
        ("QUALIA_CAMERA_SNAPSHOT_URL", "http://body.local:8080/snapshot.jpg"),
        ("QUALIA_CAMERA_SNAPSHOT_PATH", "/var/run/other.jpg"),
        ("QUALIA_CAMERA_POLL_MS", "250"),
        ("QUALIA_CAMERA_HTTP_TIMEOUT_MS", "750"),
    ]));

    assert_eq!(config.shm_name, "/qualia_test");
    assert_eq!(
        config.stream_url.as_deref(),
        Some("http://body.local:8080/stream")
    );
    assert_eq!(config.poll, Duration::from_millis(250));
    assert_eq!(config.http_timeout, Duration::from_millis(750));
    match &config.source {
        SnapshotSource::Http { url, .. } => {
            assert_eq!(url, "http://body.local:8080/snapshot.jpg")
        }
        SnapshotSource::File(path) => panic!("a snapshot URL was set, got file {path}"),
    }
}

#[test]
fn a_snapshot_url_raises_the_default_poll_interval() {
    let http = CameraConfig::from_lookup(lookup(&[(
        "QUALIA_CAMERA_SNAPSHOT_URL",
        "http://body.local/snapshot.jpg",
    )]));
    assert_eq!(http.poll, Duration::from_millis(DEFAULT_HTTP_POLL_MS));

    let file = CameraConfig::from_lookup(lookup(&[(
        "QUALIA_CAMERA_SNAPSHOT_PATH",
        "/var/run/snap.jpg",
    )]));
    assert_eq!(file.poll, Duration::from_millis(DEFAULT_POLL_MS));
}

#[test]
fn the_poll_interval_has_a_floor_and_malformed_numbers_fall_back() {
    assert_eq!(MIN_POLL_MS, 100);

    let floored = CameraConfig::from_lookup(lookup(&[("QUALIA_CAMERA_POLL_MS", "1")]));
    assert_eq!(floored.poll, Duration::from_millis(MIN_POLL_MS));

    let malformed = CameraConfig::from_lookup(lookup(&[
        ("QUALIA_CAMERA_POLL_MS", "fast"),
        ("QUALIA_CAMERA_HTTP_TIMEOUT_MS", ""),
    ]));
    assert_eq!(malformed.poll, Duration::from_millis(DEFAULT_POLL_MS));
    assert_eq!(
        malformed.http_timeout,
        Duration::from_millis(DEFAULT_HTTP_TIMEOUT_MS)
    );
}

#[test]
fn blank_urls_are_treated_as_unset() {
    let config = CameraConfig::from_lookup(lookup(&[
        ("QUALIA_CAMERA_STREAM_URL", "   "),
        ("QUALIA_CAMERA_SNAPSHOT_URL", ""),
    ]));

    assert_eq!(config.stream_url, None);
    match &config.source {
        SnapshotSource::File(path) => assert_eq!(path, DEFAULT_SNAPSHOT_PATH),
        SnapshotSource::Http { url, .. } => panic!("blank URLs are not endpoints, got {url}"),
    }
}
