//! Environment keys are interface: their spellings, defaults and the values
//! they accept are what an operator's deploy manifest and runbook depend on.

use std::net::IpAddr;
use std::sync::Mutex;

use axum::http::HeaderMap;
use qualia_agent::auth::{AuthConfig, AuthScope};
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

/// `QUALIA_AUTH_ALLOW_LOOPBACK` has the reference's off-only spelling, not the
/// stack's five-literal one: every trimmed, case-folded value but `0`, `false`
/// and `no` leaves the bypass on — `2` and `off` included — so a deploy
/// manifest that writes `Yes` or `on` still gets the tokenless console its own
/// host is trusted with.
#[test]
fn the_loopback_bypass_keeps_the_reference_off_only_spellings() {
    let _guard = ENV_LOCK.lock().expect("env lock");

    // A read token is configured so the bypass, not an absent secret, is what
    // admits the tokenless request — the operator console's own posture.
    std::env::set_var("QUALIA_READ_TOKEN", "read-test-token");
    for key in [
        "QUALIA_READ_TOKEN_FILE",
        "QUALIA_ADMIN_TOKEN",
        "QUALIA_ADMIN_TOKEN_FILE",
    ] {
        std::env::remove_var(key);
    }
    let loopback: IpAddr = "127.0.0.1".parse().expect("loopback address");
    let no_headers = HeaderMap::new();

    for spelling in [
        "true", "Yes", "yes", "on", "ON", "True", "TRUE", " 1 ", "2", "off",
    ] {
        std::env::set_var("QUALIA_AUTH_ALLOW_LOOPBACK", spelling);
        let auth = AuthConfig::from_env();
        assert!(
            auth.allow_loopback,
            "{spelling:?} is not one of the reference's three off words"
        );
        assert!(
            auth.authorize(AuthScope::Read, &no_headers, loopback).is_ok(),
            "{spelling:?} leaves a tokenless loopback read admitted while a read token is set"
        );
    }

    for spelling in ["0", "false", "no", "NO", " No "] {
        std::env::set_var("QUALIA_AUTH_ALLOW_LOOPBACK", spelling);
        let auth = AuthConfig::from_env();
        assert!(
            !auth.allow_loopback,
            "{spelling:?} is one of the reference's three off words"
        );
        assert!(
            auth.authorize(AuthScope::Read, &no_headers, loopback).is_err(),
            "{spelling:?} turns the bypass off, so the read needs its token"
        );
    }

    std::env::remove_var("QUALIA_AUTH_ALLOW_LOOPBACK");
    assert!(
        AuthConfig::from_env().allow_loopback,
        "the reference defaults this key to on"
    );
    std::env::remove_var("QUALIA_READ_TOKEN");
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

/// `QUALIA_STACK_MANIFEST` names the manifest the supervisor is handed the
/// coupling dial through. With none named there is no target and no default:
/// `runners/init` reads its own embedded `config/stack-manifest.default.json`
/// when the key is unset, so an agent that fell back to that tracked path would
/// rewrite a file the running stack never reads (T30, #46).
#[test]
fn an_unset_stack_manifest_names_no_handover_target() {
    let _guard = ENV_LOCK.lock().expect("env lock");

    std::env::remove_var("QUALIA_STACK_MANIFEST");
    assert_eq!(
        AgentConfig::from_env().stack_manifest,
        None,
        "an unset key leaves the handover with no target, never the tracked default"
    );

    std::env::set_var("QUALIA_STACK_MANIFEST", "C:/tmp/deploy/manifest.json");
    assert_eq!(
        AgentConfig::from_env().stack_manifest.as_deref(),
        Some("C:/tmp/deploy/manifest.json"),
        "a manifest the environment names is the handover target"
    );

    std::env::remove_var("QUALIA_STACK_MANIFEST");
}
