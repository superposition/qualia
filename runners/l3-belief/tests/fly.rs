//! The front-door contract of the fly coupling.
//!
//! `QUALIA_FLY_MODE` decides whether the layer couples to a prior, and the
//! artifact `QUALIA_FLY_PRIOR_PATH` names is verified before the layer is
//! entered. What an operator can observe is the process, so these tests drive
//! the real binary: the mode it accepts, the one line it prints when the
//! artifact cannot be used, and the status it leaves with when the mode is one
//! this build does not implement.
//!
//! The default build on a non-macOS host has no backend that can enter a
//! belief layer, so a non-zero exit after the fly lines proves the front door
//! let the process through rather than stopping at it. That refusal belongs to
//! the backend's own tests; only the fly lines are asserted here.

#![cfg(all(not(target_os = "macos"), not(feature = "cuda")))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

/// The binary the stack manifest spawns for this layer.
const BIN: &str = env!("CARGO_BIN_EXE_qualia-l3-belief");

/// Distinguishes scratch arenas and directories within one test binary.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A name no other process on the host uses, so a test never touches the
/// running stack's arena even if the backend were to open it.
fn scratch_name(tag: &str) -> String {
    format!(
        "/qualia_l3_fly_{tag}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "qualia-l3-fly-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the runner with a controlled environment: only the caller decides
/// what the layer sees, never the shell this test inherited.
fn launch(tag: &str, mode: Option<&str>, prior: Option<&Path>) -> Output {
    let mut command = Command::new(BIN);
    command
        .env("QUALIA_SHM_NAME", scratch_name(tag))
        .env_remove("QUALIA_FLY_MODE")
        .env_remove("QUALIA_FLY_PRIOR_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(mode) = mode {
        command.env("QUALIA_FLY_MODE", mode);
    }
    if let Some(prior) = prior {
        command.env("QUALIA_FLY_PRIOR_PATH", prior);
    }
    command.output().expect("spawn the l3-belief runner")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Did the process get past the front door into the backend? The metal build
/// on a non-macOS host refuses there, naming its platform.
fn entered_the_backend(text: &str) -> bool {
    text.contains("qualia-metal is only supported on macOS")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Writes a faithful artifact: three types, four edges, and a manifest whose
/// digests reproduce the three sections of `graph.bin`.
///
/// The shape is the artifact ABI, so the numbers also pin the in-strengths the
/// coupling uses: type 0 sums to 4, type 1 to 8 and type 2 to 2.
fn write_artifact(dir: &Path) {
    let rowptr: [u64; 4] = [0, 1, 3, 4];
    let cols: [u32; 4] = [1, 1, 2, 0];
    let weights: [u32; 4] = [3, 5, 2, 4];

    let mut graph = Vec::new();
    for value in rowptr {
        graph.extend_from_slice(&value.to_le_bytes());
    }
    let cols_at = graph.len();
    for value in cols {
        graph.extend_from_slice(&value.to_le_bytes());
    }
    let weights_at = graph.len();
    for value in weights {
        graph.extend_from_slice(&value.to_le_bytes());
    }

    let manifest = format!(
        concat!(
            "{{\"schema\":\"qualia.connectome-prior.v1\",\"type_count\":3,",
            "\"edge_count\":4,\"rowptr_sha256\":\"{}\",\"cols_sha256\":\"{}\",",
            "\"weights_sha256\":\"{}\"}}"
        ),
        sha256_hex(&graph[..cols_at]),
        sha256_hex(&graph[cols_at..weights_at]),
        sha256_hex(&graph[weights_at..]),
    );

    std::fs::write(dir.join("graph.bin"), &graph).expect("write graph.bin");
    std::fs::write(dir.join("manifest.json"), manifest).expect("write manifest.json");
}

#[test]
fn an_unrecognised_mode_is_refused_before_the_layer() {
    let output = launch("unknown", Some("warp"), None);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a mode the runner does not implement must stop it, not be ignored"
    );
    let text = stderr(&output);
    assert!(
        text.contains("unknown QUALIA_FLY_MODE \"warp\""),
        "the refusal names the key and the value it read: {text}"
    );
    assert!(
        !entered_the_backend(&text),
        "the layer must not be entered on a mode the operator did not ask for: {text}"
    );
}

#[test]
fn off_and_sim_never_read_a_prior() {
    let scratch = Scratch::new("off");
    write_artifact(scratch.path());
    for mode in [None, Some("off"), Some("sim"), Some("")] {
        let output = launch("off", mode, Some(scratch.path()));
        let text = stderr(&output);
        assert!(
            !text.contains("fly prior:"),
            "mode {mode:?} must not couple: {text}"
        );
        assert!(
            entered_the_backend(&text),
            "mode {mode:?} must reach the backend: {text}"
        );
    }
}

#[test]
fn missing_artifact_disables_prior() {
    let scratch = Scratch::new("missing");

    let unset = launch("missing-unset", Some("prior"), None);
    let text = stderr(&unset);
    assert!(
        text.contains("fly prior: disabled (QUALIA_FLY_PRIOR_PATH is not set)"),
        "an unset path is reported and the layer still starts: {text}"
    );
    assert!(entered_the_backend(&text), "{text}");

    let empty = launch("missing-empty", Some("prior"), Some(Path::new("")));
    let text = stderr(&empty);
    assert!(
        text.contains("fly prior: disabled (QUALIA_FLY_PRIOR_PATH is not set)"),
        "an empty path is the same as an unset one: {text}"
    );
    assert!(entered_the_backend(&text), "{text}");

    let absent = launch("missing-absent", Some("prior"), Some(scratch.path()));
    let text = stderr(&absent);
    assert!(
        text.contains("fly prior: disabled (read:"),
        "a path with no artifact is reported by the loader: {text}"
    );
    assert!(entered_the_backend(&text), "{text}");
}

#[test]
fn a_tampered_artifact_disables_prior() {
    let scratch = Scratch::new("tampered");
    write_artifact(scratch.path());

    // One flipped byte inside `graph.bin`: the file still has the length the
    // manifest describes, so only the recorded digest can refuse it.
    let graph = scratch.path().join("graph.bin");
    let mut bytes = std::fs::read(&graph).expect("read graph.bin");
    bytes[0] ^= 0xff;
    std::fs::write(&graph, bytes).expect("write the edited graph.bin");

    let output = launch("tampered", Some("prior"), Some(scratch.path()));
    let text = stderr(&output);
    assert!(
        text.contains("fly prior: disabled (digest:"),
        "an edited artifact is refused by its digest: {text}"
    );
    assert!(entered_the_backend(&text), "{text}");
}

#[cfg(feature = "fly-prior")]
#[test]
fn a_verified_prior_is_handed_to_the_layer() {
    let scratch = Scratch::new("verified");
    write_artifact(scratch.path());

    let output = launch("verified", Some("prior"), Some(scratch.path()));
    let text = stderr(&output);
    assert!(
        !text.contains("fly prior: disabled"),
        "a verified artifact is not a disabled prior: {text}"
    );
    assert!(
        entered_the_backend(&text),
        "the layer is entered with the prior in hand: {text}"
    );
}

#[cfg(not(feature = "fly-prior"))]
#[test]
fn a_verified_prior_without_the_feature_is_reported() {
    let scratch = Scratch::new("uncompiled");
    write_artifact(scratch.path());

    let output = launch("uncompiled", Some("prior"), Some(scratch.path()));
    let text = stderr(&output);
    assert!(
        text.contains("fly prior: disabled (built without the fly-prior feature)"),
        "a build that cannot couple says so instead of dropping the prior: {text}"
    );
    assert!(entered_the_backend(&text), "{text}");
}
