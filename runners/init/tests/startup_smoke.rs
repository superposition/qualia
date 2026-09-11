//! Startup smoke tests for the stack supervisor.
//!
//! Every test here drives the real `qualia-init` binary against a throwaway
//! manifest, so what is asserted is what an operator can observe: the arena
//! exists, the children the manifest names start with the environment the
//! manifest and the supervisor's variables describe, and the control channel
//! stops the whole stack. Nothing here needs CUDA, sensors or a dataset.
//!
//! The children are tiny probes compiled by the same toolchain that builds this
//! test, because the supervisor's contract is about the processes it starts and
//! the environment it hands them, not about any particular runner.

use qualia_ipc::{ControlMsg, ControlStream};
use qualia_shm::ShmRegion;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Longest a test waits for one observable step of the supervisor.
const WAIT: Duration = Duration::from_secs(30);
/// Poll interval for every wait helper.
const POLL: Duration = Duration::from_millis(25);

/// Pass-through variables the probe reports and the test exports.
const PASSTHROUGH_KEYS: [&str; 9] = [
    "QUALIA_REPLICA_ID",
    "QUALIA_REPLICA_ROLE",
    "QUALIA_REPLICA_DISPLAY_NAME",
    "QUALIA_REPLICA_CAPABILITIES_JSON",
    "QUALIA_REPLICA_METADATA_JSON",
    "QUALIA_SYNC_ENDPOINT",
    "QUALIA_SYNC_PEER_POLL_MS",
    "QUALIA_SYNC_PEER_PAGE_LIMIT",
    "QUALIA_SYNC_ACCEPT_INVALID_CERTS",
];

/// The keys the probe dumps, in the order it writes them.
const PROBE_KEYS: [&str; 15] = [
    "QUALIA_FLY_MODE",
    "QUALIA_FLY_PRIOR_PATH",
    "QUALIA_SHM_NAME",
    "QUALIA_SOCK_PATH",
    "QUALIA_LOG_DIR",
    "RUST_LOG",
    "QUALIA_REPLICA_ID",
    "QUALIA_REPLICA_ROLE",
    "QUALIA_REPLICA_DISPLAY_NAME",
    "QUALIA_REPLICA_CAPABILITIES_JSON",
    "QUALIA_REPLICA_METADATA_JSON",
    "QUALIA_SYNC_ENDPOINT",
    "QUALIA_SYNC_PEER_POLL_MS",
    "QUALIA_SYNC_PEER_PAGE_LIMIT",
    "QUALIA_SYNC_ACCEPT_INVALID_CERTS",
];

/// The probe source, compiled per test into the supervisor's own bin directory.
///
/// It reports the environment it was handed, one `key<TAB>value` line per key,
/// and then keeps a heartbeat file counting up so a test can tell a child that
/// was started from one that was stopped. The key list is substituted in from
/// [`PROBE_KEYS`], so the probe can never report a different set of keys than
/// the test asserts.
const PROBE_SOURCE: &str = r#"
use std::env;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

fn main() {
    let log_dir = env::var("QUALIA_LOG_DIR").expect("QUALIA_LOG_DIR is handed down");
    let dir = Path::new(&log_dir);
    let mut report = String::new();
    for key in KEYS {
        let value = env::var(key).unwrap_or_else(|_| "<unset>".to_string());
        report.push_str(key);
        report.push('\t');
        report.push_str(&value);
        report.push('\n');
    }
    fs::write(dir.join("probe.tsv"), report).expect("write environment report");

    let beat = dir.join("heartbeat.txt");
    let mut ticks: u64 = 0;
    loop {
        ticks += 1;
        let _ = fs::write(&beat, ticks.to_string());
        thread::sleep(Duration::from_millis(50));
    }
}

const KEYS: [&str; __KEY_COUNT__] = [__KEYS__];
"#;

/// A scratch directory that removes itself when the test ends.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qualia-init-{tag}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create scratch directory");
        Self { root }
    }

    fn join(&self, leaf: impl AsRef<Path>) -> PathBuf {
        self.root.join(leaf)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A name that cannot collide with a sibling test in this binary.
fn unique_suffix() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{stamp}", std::process::id())
}

fn init_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_qualia-init"))
}

/// Where the supervisor looks for its children: next to its own binary.
fn bin_dir() -> PathBuf {
    init_exe()
        .parent()
        .expect("the init binary has a parent directory")
        .to_path_buf()
}

/// Compiles the probe into the supervisor's bin directory under `runner_name`.
fn build_probe(scratch: &Scratch, runner_name: &str) -> PathBuf {
    let source = scratch.join(format!("{runner_name}.rs"));
    let keys = PROBE_KEYS
        .iter()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let probe_source = PROBE_SOURCE
        .replace("__KEY_COUNT__", &PROBE_KEYS.len().to_string())
        .replace("__KEYS__", &keys);
    fs::write(&source, probe_source).expect("write probe source");
    let output = bin_dir().join(runner_name);
    let status = Command::new("rustc")
        .arg("--edition=2021")
        .arg("--crate-name")
        .arg("qualia_smoke_probe")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .expect("run rustc for the probe");
    assert!(status.success(), "probe failed to compile: {status}");
    output
}

fn write_manifest(
    path: &Path,
    shm_name: &str,
    socket: &Path,
    runners: serde_json::Value,
    env: serde_json::Value,
) {
    let document = serde_json::json!({
        "schema_version": "qualia.stack.v1",
        "stack_name": "qualia-startup-smoke",
        "shared_memory": { "name": shm_name },
        "control": { "socket": socket.to_string_lossy() },
        "runners": runners,
        "env": env,
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&document).expect("encode manifest"),
    )
    .expect("write manifest");
}

fn spawn_init(manifest: &Path, log_dir: &Path, extra_env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(init_exe());
    cmd.env("QUALIA_STACK_MANIFEST", manifest)
        .env("QUALIA_LOG_DIR", log_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.spawn().expect("spawn qualia-init")
}

fn wait_for_control(path: &Path, timeout: Duration) -> ControlStream {
    let deadline = Instant::now() + timeout;
    loop {
        match ControlStream::connect(path) {
            Ok(stream) => return stream,
            Err(err) => {
                if Instant::now() >= deadline {
                    panic!("control endpoint {} never came up: {err}", path.display());
                }
                thread::sleep(POLL);
            }
        }
    }
}

fn wait_for_text(path: &Path, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        match fs::read_to_string(path) {
            Ok(text) if !text.is_empty() => return text,
            _ => {
                if Instant::now() >= deadline {
                    panic!("{} never became readable", path.display());
                }
                thread::sleep(POLL);
            }
        }
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("qualia-init did not exit within {timeout:?}");
        }
        thread::sleep(POLL);
    }
}

#[test]
fn manifest_env_and_passthrough_reach_the_spawned_child() {
    let scratch = Scratch::new("env");
    let manifest = scratch.join("stack.json");
    let log_dir = scratch.join("logs");
    let socket = scratch.join("control.sock");
    let prior_dir = scratch.join("prior");
    let shm_name = format!("/qualia-init-env-{}", unique_suffix());
    let runner = format!(
        "qualia-env-probe-{}{}",
        unique_suffix(),
        std::env::consts::EXE_SUFFIX
    );
    let probe = build_probe(&scratch, &runner);
    assert!(probe.is_file(), "probe binary was not produced");

    write_manifest(
        &manifest,
        &shm_name,
        &socket,
        serde_json::json!([{
            "name": runner,
            "stdout": "null",
            "env_passthrough": PASSTHROUGH_KEYS.to_vec(),
        }]),
        serde_json::json!({
            "QUALIA_FLY_MODE": "prior",
            "QUALIA_FLY_PRIOR_PATH": prior_dir.to_string_lossy(),
        }),
    );

    let mut init = spawn_init(
        &manifest,
        &log_dir,
        &[
            // Deliberately conflicting values: the manifest declares the stack's
            // configuration, so its `env` block wins over the ambient shell.
            ("QUALIA_FLY_MODE", "off"),
            ("QUALIA_FLY_PRIOR_PATH", "/nonexistent"),
            ("QUALIA_REPLICA_ID", "jetson-001"),
            ("QUALIA_REPLICA_ROLE", "jetson"),
            ("QUALIA_REPLICA_DISPLAY_NAME", "Jetson Smoke"),
            (
                "QUALIA_REPLICA_CAPABILITIES_JSON",
                r#"{"motion":true,"vision":false}"#,
            ),
            (
                "QUALIA_REPLICA_METADATA_JSON",
                r#"{"site":"lab-7","tier":"smoke"}"#,
            ),
            ("QUALIA_SYNC_ENDPOINT", "https://sync.example.invalid:8443"),
            ("QUALIA_SYNC_PEER_POLL_MS", "250"),
            ("QUALIA_SYNC_PEER_PAGE_LIMIT", "17"),
            ("QUALIA_SYNC_ACCEPT_INVALID_CERTS", "false"),
        ],
    );

    let mut control = wait_for_control(&socket, WAIT);
    let report = wait_for_text(&log_dir.join("probe.tsv"), WAIT);

    let expected: Vec<(String, String)> = vec![
        ("QUALIA_FLY_MODE".into(), "prior".into()),
        (
            "QUALIA_FLY_PRIOR_PATH".into(),
            prior_dir.to_string_lossy().into_owned(),
        ),
        ("QUALIA_SHM_NAME".into(), shm_name.clone()),
        (
            "QUALIA_SOCK_PATH".into(),
            socket.to_string_lossy().into_owned(),
        ),
        (
            "QUALIA_LOG_DIR".into(),
            log_dir.to_string_lossy().into_owned(),
        ),
        ("RUST_LOG".into(), "info".into()),
        ("QUALIA_REPLICA_ID".into(), "jetson-001".into()),
        ("QUALIA_REPLICA_ROLE".into(), "jetson".into()),
        ("QUALIA_REPLICA_DISPLAY_NAME".into(), "Jetson Smoke".into()),
        (
            "QUALIA_REPLICA_CAPABILITIES_JSON".into(),
            r#"{"motion":true,"vision":false}"#.into(),
        ),
        (
            "QUALIA_REPLICA_METADATA_JSON".into(),
            r#"{"site":"lab-7","tier":"smoke"}"#.into(),
        ),
        (
            "QUALIA_SYNC_ENDPOINT".into(),
            "https://sync.example.invalid:8443".into(),
        ),
        ("QUALIA_SYNC_PEER_POLL_MS".into(), "250".into()),
        ("QUALIA_SYNC_PEER_PAGE_LIMIT".into(), "17".into()),
        (
            "QUALIA_SYNC_ACCEPT_INVALID_CERTS".into(),
            "false".into(),
        ),
    ];
    for (key, value) in &expected {
        let line = format!("{key}\t{value}");
        assert!(
            report.lines().any(|entry| entry == line),
            "child did not receive `{line}`; probe reported:\n{report}"
        );
    }
    assert_eq!(
        report.lines().count(),
        PROBE_KEYS.len(),
        "the probe should report every key it was built for:\n{report}"
    );

    control
        .send(ControlMsg::Shutdown, None)
        .expect("send shutdown");
    let status = wait_for_exit(&mut init, WAIT);
    assert!(status.success(), "qualia-init exited {status}");
    assert!(
        log_dir.join(format!("{runner}.log")).is_file(),
        "a child's stderr belongs in <log_dir>/<name>.log"
    );
    assert!(
        ShmRegion::open(&shm_name).is_err(),
        "graceful shutdown must release the arena"
    );
}

#[test]
fn estop_stops_the_supervisor_and_its_children() {
    let scratch = Scratch::new("estop");
    let manifest = scratch.join("stack.json");
    let log_dir = scratch.join("logs");
    let socket = scratch.join("control.sock");
    let shm_name = format!("/qualia-init-estop-{}", unique_suffix());
    let runner = format!(
        "qualia-beat-probe-{}{}",
        unique_suffix(),
        std::env::consts::EXE_SUFFIX
    );
    build_probe(&scratch, &runner);

    write_manifest(
        &manifest,
        &shm_name,
        &socket,
        serde_json::json!([{ "name": runner, "stdout": "null" }]),
        serde_json::json!({}),
    );

    let mut init = spawn_init(&manifest, &log_dir, &[]);
    let mut control = wait_for_control(&socket, WAIT);

    let beat = log_dir.join("heartbeat.txt");
    let first: u64 = wait_for_text(&beat, WAIT)
        .trim()
        .parse()
        .expect("heartbeat is a counter");

    control.send(ControlMsg::Estop, None).expect("send estop");
    let status = wait_for_exit(&mut init, WAIT);
    assert!(status.success(), "qualia-init exited {status}");

    let stopped: u64 = fs::read_to_string(&beat)
        .expect("read heartbeat")
        .trim()
        .parse()
        .expect("heartbeat is a counter");
    assert!(stopped >= first);
    thread::sleep(Duration::from_millis(300));
    let later: u64 = fs::read_to_string(&beat)
        .expect("read heartbeat")
        .trim()
        .parse()
        .expect("heartbeat is a counter");
    assert_eq!(
        stopped, later,
        "a child outlived the estop: the supervisor must terminate its processes"
    );
}

#[test]
fn owner_only_holds_the_arena_and_spawns_nothing() {
    let scratch = Scratch::new("owner");
    let manifest = scratch.join("stack.json");
    let log_dir = scratch.join("logs");
    let socket = scratch.join("control.sock");
    let shm_name = format!("/qualia-init-owner-{}", unique_suffix());
    let absent = format!(
        "qualia-never-built-{}{}",
        unique_suffix(),
        std::env::consts::EXE_SUFFIX
    );

    write_manifest(
        &manifest,
        &shm_name,
        &socket,
        serde_json::json!([{ "name": absent, "stdout": "null" }]),
        serde_json::json!({}),
    );

    let mut init = spawn_init(&manifest, &log_dir, &[("QUALIA_INIT_OWNER_ONLY", "1")]);
    let mut control = wait_for_control(&socket, WAIT);

    let arena = ShmRegion::open(&shm_name).expect("owner-only mode publishes the arena");
    assert_eq!(arena.header().magic, qualia_types::SHM_MAGIC);
    drop(arena);
    assert!(
        !log_dir.join(format!("{absent}.log")).exists(),
        "owner-only mode must not touch a manifest runner"
    );

    control
        .send(ControlMsg::Shutdown, None)
        .expect("send shutdown");
    let status = wait_for_exit(&mut init, WAIT);
    assert!(status.success(), "owner-only init exited {status}");
    assert!(
        ShmRegion::open(&shm_name).is_err(),
        "owner-only shutdown must release the arena"
    );
}

#[test]
fn unsupported_manifest_is_refused_before_anything_is_allocated() {
    let scratch = Scratch::new("refused");
    let manifest = scratch.join("stack.json");
    let log_dir = scratch.join("logs");
    let stderr_path = scratch.join("init.stderr");
    let shm_name = format!("/qualia-init-refused-{}", unique_suffix());

    fs::write(
        &manifest,
        r#"{
          "schema_version": "qualia.stack.v0",
          "stack_name": "refused",
          "shared_memory": { "name": "shm" },
          "control": { "socket": "socket" },
          "runners": [{ "name": "qualia-agent" }]
        }"#,
    )
    .expect("write manifest");

    let stderr = File::create(&stderr_path).expect("create stderr file");
    let mut init = Command::new(init_exe())
        .env("QUALIA_STACK_MANIFEST", &manifest)
        .env("QUALIA_LOG_DIR", &log_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn qualia-init");

    let status = wait_for_exit(&mut init, WAIT);
    assert!(
        !status.success(),
        "an unsupported manifest must not start the stack"
    );
    let report = fs::read_to_string(&stderr_path).expect("read init stderr");
    assert!(
        report.contains("schema_version"),
        "the supervisor must name what it refused: {report}"
    );
    assert!(
        ShmRegion::open(&shm_name).is_err(),
        "nothing may be allocated for a refused manifest"
    );
    assert!(
        !log_dir.exists(),
        "a refused manifest must not create the stack's log directory"
    );
}
