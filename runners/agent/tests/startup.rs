//! The binary's start failures are interface: a supervisor keys on the stderr
//! diagnostic and the exit code.

use std::process::Command;

/// A keypair that does not load is the reference's panic, not a formatted
/// error: the same stderr text and the same exit code.
#[test]
fn an_unloadable_tls_keypair_is_the_reference_start_failure() {
    let dir = tempfile::tempdir().expect("temp dir");
    let tls = dir.path().join("tls");
    std::fs::create_dir_all(&tls).expect("tls dir");
    std::fs::write(tls.join("cert.pem"), "not a certificate").expect("cert");
    std::fs::write(tls.join("key.pem"), "not a key").expect("key");

    let output = Command::new(env!("CARGO_BIN_EXE_qualia-agent"))
        .env("QUALIA_TLS_DIR", tls.to_string_lossy().as_ref())
        .env("QUALIA_WEB_PORT", "0")
        .output()
        .expect("the agent binary runs");

    assert_eq!(
        output.status.code(),
        Some(101),
        "the reference panics on an unloadable keypair"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to load TLS cert"),
        "stderr was: {stderr}"
    );
}
