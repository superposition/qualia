//! The stack supervisor.
//!
//! One process owns the shared arena and the control endpoint: it allocates the
//! arena, starts the children the stack manifest names, hands each of them the
//! environment the manifest and the supervisor's own variables describe, and
//! tears the whole stack down when an operator sends `Shutdown` or `Estop` over
//! the control channel. Every other runner in this workspace is one of those
//! children; nothing else may create the arena.
//!
//! Inputs; every one of them is optional:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `QUALIA_STACK_MANIFEST` | Path to the manifest; the embedded default is used when unset. |
//! | `QUALIA_SHM_NAME` | Overrides `shared_memory.name`. |
//! | `QUALIA_SOCK_PATH` | Overrides `control.socket`. |
//! | `QUALIA_LOG_DIR` | Where child stdout/stderr land; defaults to `artifacts/logs`. |
//! | `RUST_LOG` | Handed to every child; defaults to `info`. |
//! | `QUALIA_INIT_OWNER_ONLY` | Truthy: hold the arena and the endpoint without spawning children. |
//!
//! The manifest's top-level `env` block is the stack's declared child
//! environment and wins over the ambient shell, so an operator's stale export
//! cannot silently promote, say, the fly prior. A runner's `env_passthrough`
//! list is the explicit exception: those keys are forwarded from the
//! supervisor's environment when they are set.

use qualia_ipc::{ControlListener, ControlMsg};
use qualia_shm::{ShmRegion, StatsRegion, SHM_SIZE};
use qualia_types::{parse_stack_manifest, RunnerStdout, StackManifest};
use std::collections::BTreeMap;
#[cfg(unix)]
use std::ffi::CString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Used when `QUALIA_STACK_MANIFEST` names no file.
const DEFAULT_MANIFEST: &str = include_str!("../../../config/stack-manifest.default.json");
/// Interior width of the startup banner.
const BANNER_WIDTH: usize = 42;
/// How often the supervisor looks at the control endpoint.
const CONTROL_POLL: Duration = Duration::from_millis(50);
/// How long a child may take to honour a termination request.
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
/// Spellings of `QUALIA_INIT_OWNER_ONLY` that mean "yes".
const TRUTHY: [&str; 5] = ["1", "true", "TRUE", "yes", "YES"];

/// Cleared by a termination signal so the main loop can unwind cleanly.
static RUNNING: AtomicBool = AtomicBool::new(true);
/// Set by the connection thread that hears `Shutdown` or `Estop`, so the main
/// loop can leave its wait without depending on the peer that sent it.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

fn main() {
    let manifest_text = match read_manifest_text() {
        Ok(text) => text,
        Err(err) => fatal(&format!("[init] {err}")),
    };
    let manifest = match parse_stack_manifest(&manifest_text) {
        Ok(manifest) => manifest,
        Err(err) => fatal(&format!("[init] {err}")),
    };
    let stack_env = match stack_env(&manifest_text) {
        Ok(env) => env,
        Err(err) => fatal(&format!("[init] {err}")),
    };

    let shm_name = env_or("QUALIA_SHM_NAME", &manifest.shared_memory.name);
    let sock_path = env_or("QUALIA_SOCK_PATH", &manifest.control.socket);
    let log_dir = env_or("QUALIA_LOG_DIR", "artifacts/logs");
    let rust_log = env_or("RUST_LOG", "info");
    let owner_only = is_truthy(std::env::var("QUALIA_INIT_OWNER_ONLY").ok().as_deref());

    banner(&manifest.stack_name);

    cleanup_stale_arena(&shm_name);
    eprintln!("[init] Creating shared memory '{shm_name}'...");
    let arena = match ShmRegion::create(&shm_name) {
        Ok(arena) => arena,
        Err(err) => fatal(&format!("[init] Failed to create shm: {err}")),
    };
    eprintln!(
        "[init] Shared memory created: {} MB",
        SHM_SIZE / (1024 * 1024)
    );

    // The telemetry stats region beside the arena: every producer publishes its
    // fixed-width frame there, and the console's HUD reads them without
    // touching the ABI layout above. The supervisor owns its lifetime, so a
    // stale one is cleared first and the region is created before any child
    // starts.
    let stats_name = qualia_shm::stats_region_name_from_env(&shm_name);
    let _ = StatsRegion::unlink(&stats_name);
    let _stats = match StatsRegion::create(&stats_name) {
        Ok(region) => region,
        Err(err) => fatal(&format!("[init] Failed to create stats shm: {err}")),
    };
    eprintln!("[init] Stats region created: '{stats_name}'");

    eprintln!("[init] Binding control socket '{sock_path}'...");
    let control = match ControlListener::bind(&sock_path) {
        Ok(control) => control,
        Err(err) => fatal(&format!("Failed to bind control socket: {err:?}")),
    };
    if let Err(err) = ensure_log_dir(Path::new(&log_dir)) {
        fatal(&format!("Failed to create log directory: {err:?}"));
    }
    install_signal_handlers();

    let mut children = if owner_only {
        eprintln!("[init] Owner-only mode: holding shared memory without spawning runners.");
        Vec::new()
    } else {
        spawn_runners(
            &manifest, &stack_env, &shm_name, &sock_path, &log_dir, &rust_log,
        )
    };

    eprintln!();
    eprintln!("[init] {} runners launched.", children.len());
    eprintln!("[init] Run 'qualia-watch' in another terminal for the TUI.");
    eprintln!();

    watch_control(&control);
    stop_children(&mut children);

    eprintln!("[init] Cleaning up...");
    drop(arena);
    let _ = std::fs::remove_file(&sock_path);
    eprintln!("[init] Done.");
}

/// Terminates the supervisor when the stack cannot start without whatever the
/// message names.
///
/// Every fatal path goes through the panic machinery, as the reference does:
/// the payload reaches stderr unchanged, the arena is released while the stack
/// unwinds, and the process exits 101. Callers pass the payload the reference
/// emits, including its `[init]` prefix where it has one.
fn fatal(message: &str) -> ! {
    panic!("{message}");
}

fn banner(stack_name: &str) {
    let rule = format!("+{}+", "-".repeat(BANNER_WIDTH));
    eprintln!("{rule}");
    eprintln!(
        "|{}|",
        pad_line(&format!("QUALIA ENGINE v{}", env!("CARGO_PKG_VERSION")))
    );
    eprintln!("|{}|", pad_line(&format!("stack: {}", stack_name.trim())));
    eprintln!("{rule}");
    eprintln!();
}

fn pad_line(value: &str) -> String {
    let mut line: String = value.chars().take(BANNER_WIDTH).collect();
    line.push_str(&" ".repeat(BANNER_WIDTH.saturating_sub(line.chars().count())));
    line
}

/// The manifest's optional top-level `env` block: child-process defaults.
///
/// `qualia-types` owns the typed manifest vocabulary, but it does not model this
/// block, so it is read from the same document here. Reading one document twice
/// keeps the two from ever disagreeing about the bytes on disk.
fn stack_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    let document: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| format!("failed to parse stack manifest: {err}"))?;
    let Some(block) = document.get("env") else {
        return Ok(BTreeMap::new());
    };
    let object = block
        .as_object()
        .ok_or_else(|| "stack manifest `env` must be a JSON object".to_string())?;

    let mut env = BTreeMap::new();
    for (key, value) in object {
        let value = value
            .as_str()
            .ok_or_else(|| format!("stack manifest `env.{key}` must be a string"))?;
        env.insert(key.clone(), value.to_string());
    }
    Ok(env)
}

fn read_manifest_text() -> Result<String, String> {
    match std::env::var("QUALIA_STACK_MANIFEST") {
        Ok(path) => std::fs::read_to_string(&path)
            .map_err(|err| format!("failed to read stack manifest '{path}': {err}")),
        Err(_) => Ok(DEFAULT_MANIFEST.to_string()),
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

/// Whether an optional switch variable is one of the documented true spellings.
fn is_truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| TRUTHY.contains(&value))
}

/// Starts every runner the manifest names, skipping the ones whose binary is
/// missing (a partial stack beats no stack) and returning the ones that started.
fn spawn_runners(
    manifest: &StackManifest,
    stack_env: &BTreeMap<String, String>,
    shm_name: &str,
    sock_path: &str,
    log_dir: &str,
    rust_log: &str,
) -> Vec<(String, Child)> {
    let self_path = std::env::current_exe()
        .unwrap_or_else(|err| fatal(&format!("Cannot get self path: {err:?}")));
    let Some(bin_dir) = self_path.parent() else {
        fatal("Cannot get bin dir");
    };

    let mut children = Vec::new();
    for runner in &manifest.runners {
        eprintln!("[init] Spawning {}...", runner.name);

        let mut cmd = Command::new(bin_dir.join(&runner.name));
        // The stack's declared environment comes first so it overrides whatever
        // the operator's shell exported.
        for (key, value) in stack_env {
            cmd.env(key, value);
        }
        // Then the addresses the supervisor itself allocated, which no manifest
        // block may redirect.
        cmd.env("QUALIA_SHM_NAME", shm_name)
            .env("QUALIA_SOCK_PATH", sock_path)
            .env("QUALIA_LOG_DIR", log_dir)
            .env("RUST_LOG", rust_log);
        // Finally the per-runner values the operator exported for this child
        // only, which are the declared exception to the stack environment.
        for key in &runner.env_passthrough {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }

        match log_stdio(log_dir, &runner.name) {
            Ok(stderr) => cmd.stderr(stderr),
            Err(err) => fatal(&format!("runner stderr log: {err:?}")),
        };
        match runner.stdout {
            RunnerStdout::Null => cmd.stdout(Stdio::null()),
            RunnerStdout::Inherit => match log_stdio(log_dir, &runner.name) {
                Ok(stdout) => cmd.stdout(stdout),
                Err(err) => fatal(&format!("runner stdout log: {err:?}")),
            },
        };

        match cmd.spawn() {
            Ok(child) => {
                eprintln!("[init]   pid {}", child.id());
                children.push((runner.name.clone(), child));
            }
            Err(err) => eprintln!("[init] WARNING: {}: {err}", runner.name),
        }
    }
    children
}

/// Waits until the control channel delivers `Shutdown` or `Estop`, or a
/// termination signal arrives. Other control messages are for the runners.
///
/// Each accepted connection is read on its own thread. A peer may connect and
/// then say nothing at all; the supervisor must still be able to hear the next
/// caller — and to notice the `RUNNING` flag a signal clears — while that peer
/// holds its connection open, so the loop never waits on a stream itself.
fn watch_control(control: &ControlListener) {
    while RUNNING.load(Ordering::Relaxed) && !STOP_REQUESTED.load(Ordering::Relaxed) {
        std::thread::sleep(CONTROL_POLL);
        let Some(mut stream) = control.try_accept() else {
            continue;
        };
        std::thread::spawn(move || {
            if let Ok((msg, _)) = stream.recv() {
                if matches!(msg, ControlMsg::Shutdown | ControlMsg::Estop) {
                    eprintln!("[init] {msg:?} received");
                    STOP_REQUESTED.store(true, Ordering::Relaxed);
                }
            }
        });
    }
}

/// Asks every child to stop, then kills whatever is still alive once the grace
/// period has passed. Returns with every child reaped.
fn stop_children(children: &mut [(String, Child)]) {
    eprintln!();
    eprintln!("[init] Shutting down {} processes...", children.len());

    for (name, child) in children.iter_mut() {
        terminate(child);
        eprintln!("[init]   terminate -> {name} (pid {})", child.id());
    }

    let deadline = Instant::now() + TERMINATION_GRACE;
    loop {
        let all_gone = children
            .iter_mut()
            .all(|(_, child)| matches!(child.try_wait(), Ok(Some(_))));
        if all_gone || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(CONTROL_POLL);
    }

    for (name, child) in children.iter_mut() {
        if matches!(child.try_wait(), Ok(Some(_))) {
            continue;
        }
        eprintln!("[init]   kill -> {name}");
        let _ = child.kill();
        let _ = child.wait();
    }

    eprintln!("[init] All processes stopped.");
}

#[cfg(unix)]
fn terminate(child: &mut Child) {
    // SAFETY: `child.id()` is this process's own child, so signalling it cannot
    // touch an unrelated process.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
}

/// Removes a region left behind by a supervisor that crashed, so a restart
/// always begins from the documented layout rather than from stale bytes.
#[cfg(unix)]
fn cleanup_stale_arena(name: &str) {
    let Ok(c_name) = CString::new(name) else {
        return;
    };
    // SAFETY: `c_name` is a valid NUL-terminated name; the flags and mode are
    // the ones `shm_open` documents for opening an existing region read-only.
    let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDONLY, 0) };
    if fd >= 0 {
        // SAFETY: `fd` was just opened above and is not used afterwards.
        unsafe {
            libc::close(fd);
            libc::shm_unlink(c_name.as_ptr());
        }
        eprintln!("[init] Cleaned up stale shm '{name}'");
    }
}

/// Windows drops a named mapping when its last handle closes, so there is
/// nothing to clean up before creating a fresh region.
#[cfg(not(unix))]
fn cleanup_stale_arena(_name: &str) {}

fn ensure_log_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|err| format!("failed to create log dir '{}': {err}", path.display()))
}

fn log_stdio(log_dir: &str, runner_name: &str) -> Result<Stdio, String> {
    let path = log_file_path(log_dir, runner_name);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("failed to open log file '{}': {err}", path.display()))?;
    Ok(Stdio::from(file))
}

fn log_file_path(log_dir: &str, runner_name: &str) -> PathBuf {
    Path::new(log_dir).join(format!("{runner_name}.log"))
}

#[cfg(unix)]
fn install_signal_handlers() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, signal_handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, signal_handler as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() {}

#[cfg(unix)]
extern "C" fn signal_handler(_signal: libc::c_int) {
    RUNNING.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_PASSTHROUGH: [&str; 11] = [
        "QUALIA_REPLICA_ID",
        "QUALIA_REPLICA_ROLE",
        "QUALIA_REPLICA_DISPLAY_NAME",
        "QUALIA_REPLICA_CAPABILITIES_JSON",
        "QUALIA_REPLICA_METADATA_JSON",
        "QUALIA_SYNC_ENDPOINT",
        "QUALIA_SYNC_PEER_POLL_MS",
        "QUALIA_SYNC_PEER_PAGE_LIMIT",
        "QUALIA_SYNC_ACCEPT_INVALID_CERTS",
        "QUALIA_LEASH_BASE_URL",
        "QUALIA_LEASH_OPERATOR_TOKEN_FILE",
    ];

    #[test]
    fn embedded_manifest_still_names_the_whole_stack() {
        let manifest = parse_stack_manifest(DEFAULT_MANIFEST).expect("default manifest parses");
        assert_eq!(manifest.schema_version, "qualia.stack.v1");
        assert_eq!(manifest.stack_name, "qualia-default");
        assert_eq!(manifest.shared_memory.name, "/qualia_body");
        assert_eq!(manifest.control.socket, "/tmp/qualia_body.sock");

        let names: Vec<&str> = manifest
            .runners
            .iter()
            .map(|runner| runner.name.as_str())
            .collect();
        for expected in [
            "qualia-l0-superposition",
            "qualia-l1-belief",
            "qualia-l6-semantic",
            "qualia-health",
            "qualia-vision",
            "qualia-agent",
        ] {
            assert!(
                names.contains(&expected),
                "the default manifest no longer starts {expected}"
            );
        }

        let agent = manifest
            .runners
            .iter()
            .find(|runner| runner.name == "qualia-agent")
            .expect("the agent is part of the default stack");
        for key in AGENT_PASSTHROUGH {
            assert!(
                agent.env_passthrough.iter().any(|entry| entry == key),
                "the agent lost its {key} pass-through"
            );
        }
    }

    #[test]
    fn embedded_manifest_declares_the_fly_keys() {
        let env = stack_env(DEFAULT_MANIFEST).expect("default manifest has an env block");
        assert_eq!(env.get("QUALIA_FLY_MODE").map(String::as_str), Some("off"));
        assert_eq!(
            env.get("QUALIA_FLY_PRIOR_PATH").map(String::as_str),
            Some("")
        );
    }

    /// The stack contract points the agent at the board's own Leash. A
    /// household subnet baked into the manifest is the front-end lesson the
    /// ticket cites (`docs/frontend-lessons.md`), so the default must stay
    /// loopback and must be overridable from the operator's shell.
    #[test]
    fn embedded_manifest_defaults_the_agent_to_the_local_leash() {
        let env = stack_env(DEFAULT_MANIFEST).expect("default manifest has an env block");
        let base_url = env
            .get("QUALIA_LEASH_BASE_URL")
            .expect("the stack contract names the Leash base URL");
        assert_eq!(base_url, "http://127.0.0.1:8000");
        assert!(
            !base_url.contains("192.168."),
            "the manifest must not bake a household address in: {base_url}"
        );
        let manifest = parse_stack_manifest(DEFAULT_MANIFEST).expect("default manifest parses");
        let agent = manifest
            .runners
            .iter()
            .find(|runner| runner.name == "qualia-agent")
            .expect("the agent is part of the default stack");
        assert!(
            agent
                .env_passthrough
                .iter()
                .any(|entry| entry == "QUALIA_LEASH_BASE_URL"),
            "the operator's Leash override must reach the agent"
        );
    }

    #[test]
    fn absent_env_block_imposes_nothing() {
        let env = stack_env(
            r#"{
              "schema_version":"qualia.stack.v1",
              "stack_name":"no-env",
              "shared_memory":{"name":"/qualia_body"},
              "control":{"socket":"/tmp/q.sock"},
              "runners":[{"name":"qualia-agent"}]
            }"#,
        )
        .expect("a manifest without an env block is valid");
        assert!(env.is_empty(), "no env block means nothing to impose");
    }

    #[test]
    fn non_string_env_value_is_refused() {
        let err = stack_env(
            r#"{
              "schema_version":"qualia.stack.v1",
              "stack_name":"bad-env",
              "shared_memory":{"name":"/qualia_body"},
              "control":{"socket":"/tmp/q.sock"},
              "runners":[{"name":"qualia-agent"}],
              "env":{"QUALIA_FLY_MODE":7}
            }"#,
        )
        .expect_err("a non-string env value is a manifest bug");
        assert!(err.contains("QUALIA_FLY_MODE"), "{err}");
    }

    #[test]
    fn owner_only_is_enabled_by_the_documented_spellings() {
        for value in ["1", "true", "TRUE", "yes", "YES"] {
            assert!(is_truthy(Some(value)), "{value} means yes");
        }
        for value in ["0", "false", "FALSE", "no", "", "on"] {
            assert!(
                !is_truthy(Some(value)),
                "{value} must not put the supervisor in owner-only mode"
            );
        }
        assert!(!is_truthy(None), "an unset switch means no");
    }

    #[test]
    fn log_files_are_named_after_the_runner() {
        let path = log_file_path("artifacts/logs", "qualia-agent");
        assert!(path.ends_with(Path::new("artifacts/logs/qualia-agent.log")));
    }
}
