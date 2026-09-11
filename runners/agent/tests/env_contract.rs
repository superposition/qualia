//! Environment keys are interface: their spellings, defaults and the values
//! they accept are what an operator's deploy manifest and runbook depend on.

use std::sync::Mutex;

use qualia_agent::config::AgentConfig;

/// These tests mutate the process environment, which every test in this binary
/// shares, so they take turns.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn boolean_keys_accept_only_the_reference_spellings() {
    let _guard = ENV_LOCK.lock().expect("env lock");

    std::env::set_var("QUALIA_ROSBRIDGE_ENABLED", "Yes");
    assert!(
        !AgentConfig::from_env().rosbridge_enabled,
        "the reference reads only 1/true/TRUE/yes/YES as on"
    );
    std::env::set_var("QUALIA_ROSBRIDGE_ENABLED", "YES");
    assert!(AgentConfig::from_env().rosbridge_enabled);
    std::env::set_var("QUALIA_ROSBRIDGE_ENABLED", "0");
    assert!(!AgentConfig::from_env().rosbridge_enabled);

    std::env::set_var("QUALIA_SHM_AUTOCREATE", "1");
    assert!(AgentConfig::from_env().shm_autocreate);
    std::env::set_var("QUALIA_SHM_AUTOCREATE", "on");
    assert!(!AgentConfig::from_env().shm_autocreate);

    std::env::remove_var("QUALIA_ROSBRIDGE_ENABLED");
    std::env::remove_var("QUALIA_SHM_AUTOCREATE");
    assert!(AgentConfig::from_env().rosbridge_enabled, "the default holds");
    assert!(!AgentConfig::from_env().shm_autocreate, "the default holds");
}

#[test]
fn replica_role_parses_and_falls_back_to_host() {
    let _guard = ENV_LOCK.lock().expect("env lock");

    std::env::remove_var("QUALIA_REPLICA_ROLE");
    assert_eq!(AgentConfig::from_env().replica.role.to_string(), "host");

    std::env::set_var("QUALIA_REPLICA_ROLE", "jetson");
    assert_eq!(AgentConfig::from_env().replica.role.to_string(), "jetson");

    std::env::set_var("QUALIA_REPLICA_ROLE", "not-a-role");
    assert_eq!(
        AgentConfig::from_env().replica.role.to_string(),
        "host",
        "an unparsable role falls back to host, as the reference does"
    );

    std::env::remove_var("QUALIA_REPLICA_ROLE");
}

#[test]
fn the_camera_source_names_the_configured_url() {
    let _guard = ENV_LOCK.lock().expect("env lock");

    std::env::remove_var("QUALIA_CAMERA_STREAM_URL");
    std::env::remove_var("QUALIA_CAMERA_SNAPSHOT_URL");
    assert_eq!(
        qualia_agent::perception::camera_source(),
        "qualia-shm:camera_frame"
    );

    std::env::set_var("QUALIA_CAMERA_SNAPSHOT_URL", "http://camera/snapshot.jpg");
    assert_eq!(
        qualia_agent::perception::camera_source(),
        "http://camera/snapshot.jpg"
    );

    std::env::set_var("QUALIA_CAMERA_STREAM_URL", "http://camera/stream");
    assert_eq!(qualia_agent::perception::camera_source(), "http://camera/stream");

    std::env::remove_var("QUALIA_CAMERA_STREAM_URL");
    std::env::remove_var("QUALIA_CAMERA_SNAPSHOT_URL");
}
