//! Behavioural tests for the `qualia` operator binary.
//!
//! Everything here is asserted against the built executable: argv in, stdout,
//! stderr and exit status out. No test reaches into the implementation, so the
//! suite fails whenever the operator surface changes its observable contract.

use qualia_ipc::{ControlListener, ControlMsg};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// The binary under test, placed in the target directory by cargo.
const BIN: &str = env!("CARGO_BIN_EXE_qualia");

/// A fresh per-test directory under the system temp root.
fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "qualia-cli-{label}-{}-{serial}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

/// A socket path both the test and the CLI can name, spelled portably.
fn control_socket(dir: &Path) -> String {
    dir.join("control.sock")
        .to_string_lossy()
        .replace('\\', "/")
}

/// Write a stack manifest naming `runners` and pointing at `socket`.
fn write_manifest(dir: &Path, socket: &str, runners: &[&str]) -> PathBuf {
    let runners = runners
        .iter()
        .map(|name| format!(r#"{{"name":"{name}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let body = format!(
        r#"{{"schema_version":"qualia.stack.v1","stack_name":"qualia-cli-test","shared_memory":{{"name":"/qualia_cli_test"}},"control":{{"socket":"{socket}"}},"runners":[{runners}]}}"#
    );
    let path = dir.join("stack.json");
    std::fs::write(&path, body).expect("write stack manifest");
    path
}

/// A child with the endpoint environment cleared, so the suite never inherits
/// an operator's agent.
fn base_command(args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command.args(args);
    command.env_remove("QUALIA_AGENT_URL");
    command.env_remove("QUALIA_WEB_PORT");
    command
}

fn run(args: &[&str]) -> Output {
    base_command(args).output().expect("spawn qualia")
}

fn run_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = base_command(args);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("spawn qualia")
}

fn run_with(args: &[&str], key: &str, value: &str) -> Output {
    run_env(args, &[(key, value)])
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("qualia exited without a status code")
}

#[test]
fn no_arguments_prints_usage_on_stderr_and_succeeds() {
    let output = run(&[]);
    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "");
    let err = stderr(&output);
    assert!(err.starts_with("Usage:\n"), "unexpected usage: {err}");
    assert!(err.contains("qualia verify-sync"));
    assert!(err.contains("qualia propose nav-goal"));
}

#[test]
fn unknown_command_is_a_usage_error_with_exit_code_two() {
    let output = run(&["bogus"]);
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).starts_with("qualia: unknown command 'bogus'\n"));
    assert!(stderr(&output).contains("Usage:"));
}

#[test]
fn missing_runner_name_is_a_usage_error() {
    let output = run(&["logs"]);
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("qualia: qualia logs requires a runner name"));
}

#[test]
fn non_numeric_flag_value_is_rejected_before_any_work() {
    let output = run(&["logs", "qualia-agent", "--lines", "many"]);
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("qualia: invalid --lines value 'many'"));
}

#[test]
fn missing_required_proposal_flags_are_usage_errors() {
    let missing_z = run(&["propose", "nav-goal", "--x", "1.0"]);
    assert_eq!(code(&missing_z), 2);
    assert!(stderr(&missing_z).contains("qualia propose nav-goal requires --z"));

    let missing_label = run(&["propose", "object", "--x", "1.0", "--z", "2.0"]);
    assert_eq!(code(&missing_label), 2);
    assert!(stderr(&missing_label).contains("qualia propose object requires --label"));
}

#[test]
fn scene_alias_shares_the_world_diagnostics() {
    let output = run(&["scene", "--bogus"]);
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("qualia: unknown qualia world argument '--bogus'"));
}

#[test]
fn status_prints_the_manifest_and_a_runner_summary() {
    let dir = scratch("status");
    let socket = control_socket(&dir);
    let manifest = write_manifest(
        &dir,
        &socket,
        &["qualia-cli-test-alpha", "qualia-cli-test-beta"],
    );

    let output = run(&["status", "--manifest", manifest.to_str().unwrap()]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    let expected = format!(
        "stack: qualia-cli-test\n\
         schema_version: qualia.stack.v1\n\
         shared_memory: /qualia_cli_test\n\
         control_socket: {socket}\n\
         \n\
         [stopped] qualia-cli-test-alpha\n\
         [stopped] qualia-cli-test-beta\n\
         \n\
         summary: 0/2 manifest runners active\n"
    );
    assert_eq!(stdout(&output), expected);
}

#[test]
fn status_rejects_a_malformed_manifest() {
    let dir = scratch("bad-manifest");
    let manifest = dir.join("stack.json");
    std::fs::write(&manifest, "{ not json").unwrap();

    let output = run(&["status", "--manifest", manifest.to_str().unwrap()]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to parse stack manifest"));
}

#[test]
fn logs_prints_the_tail_of_the_named_runner_log() {
    let dir = scratch("logs");
    let log = dir.join("qualia-agent.log");
    std::fs::write(&log, "one\ntwo\nthree\nfour\nfive\n").unwrap();

    let output = run_with(
        &["logs", "qualia-agent", "--lines", "2"],
        "QUALIA_LOG_DIR",
        dir.to_str().unwrap(),
    );
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "four\nfive\n");
}

#[test]
fn logs_for_an_absent_runner_exits_nonzero() {
    let dir = scratch("logs-missing");
    let output = run_with(
        &["logs", "qualia-absent"],
        "QUALIA_LOG_DIR",
        dir.to_str().unwrap(),
    );
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to read log"));
}

#[test]
fn stop_delivers_a_shutdown_frame_over_the_control_socket() {
    let dir = scratch("stop");
    let socket = control_socket(&dir);
    let manifest = write_manifest(&dir, &socket, &["qualia-cli-test-alpha"]);
    let listener = ControlListener::bind(&socket).expect("bind control listener");

    let output = Command::new(BIN)
        .args(["stop", "--manifest", manifest.to_str().unwrap()])
        .env_remove("QUALIA_SOCK_PATH")
        .output()
        .expect("spawn qualia stop");

    let mut stream = listener
        .accept_blocking()
        .expect("accept the control connection");
    let frame = stream.recv().expect("read the control frame");

    assert_eq!(frame, (ControlMsg::Shutdown, None));
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), format!("shutdown sent to {socket}\n"));
}

#[test]
fn stop_without_a_listener_exits_nonzero() {
    let dir = scratch("stop-refused");
    let socket = control_socket(&dir);
    let manifest = write_manifest(&dir, &socket, &["qualia-cli-test-alpha"]);

    let output = Command::new(BIN)
        .args(["stop", "--manifest", manifest.to_str().unwrap()])
        .env_remove("QUALIA_SOCK_PATH")
        .output()
        .expect("spawn qualia stop");

    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to connect control socket"));
}

#[test]
fn health_reports_unavailable_when_no_agent_answers() {
    let started = Instant::now();
    let output = run_with(&["health"], "QUALIA_WEB_PORT", "1");
    let elapsed = started.elapsed();
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with("agent_url: https://127.0.0.1:1\n"), "{text}");
    assert!(text.contains("service_instance: unavailable\n"));
    assert!(text.contains("status_type: stack_health\n"));
    assert!(text.contains("healthy: false\n"));
    assert!(text.contains("health_reason: "));
    assert!(elapsed < BOUND, "a dead port held the CLI for {elapsed:?}");
}

#[test]
fn cuda_reports_unavailable_when_the_service_socket_is_dead() {
    let output = run_with(
        &["cuda"],
        "QUALIA_COMPUTE_SOCKET",
        "/nonexistent/qualia-cli-test.sock",
    );
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with("compute_socket: /nonexistent/qualia-cli-test.sock\n"));
    assert!(text.contains("service_instance: unavailable\n"));
    assert!(text.contains("sm: 0\n"));
    assert!(text.contains("cuda_status: unavailable\n"));
    assert!(text.contains("cuda_reason: "));
}

#[test]
fn planner_reports_unavailable_when_no_agent_answers() {
    let started = Instant::now();
    let output = run_with(&["planner"], "QUALIA_WEB_PORT", "1");
    let elapsed = started.elapsed();
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("status_type: planner_status\n"));
    assert!(text.contains("planner_ready: false\n"));
    assert!(text.contains("last_result: none\n"));
    assert!(text.contains("planner_reason: "));
    assert!(elapsed < BOUND, "a dead port held the CLI for {elapsed:?}");
}

#[test]
fn world_fails_fast_when_the_agent_is_unreachable() {
    let output = run_with(&["world"], "QUALIA_WEB_PORT", "1");
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to fetch world-model proposals"));
}

#[test]
fn decisions_fails_fast_when_the_agent_is_unreachable() {
    let output = run_with(&["decisions", "--json"], "QUALIA_WEB_PORT", "1");
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to fetch world-model decisions"));
}

#[test]
fn propose_enrolls_the_cli_replica_before_submitting() {
    let output = run_with(
        &["propose", "nav-goal", "--x", "4.0", "--z", "-1.5"],
        "QUALIA_WEB_PORT",
        "1",
    );
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to enroll CLI replica"));
}

#[test]
fn decide_enrolls_the_cli_replica_before_submitting() {
    let output = run_with(
        &["decide", "reject", "proposal/nav-goal-1", "--reason", "stale"],
        "QUALIA_WEB_PORT",
        "1",
    );
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("qualia: failed to enroll CLI replica"));
}

#[test]
fn host_reports_the_running_platform() {
    let output = run(&["host"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    let mut lines = text.lines();
    let host = lines.next().expect("host line");
    assert!(host.starts_with("host: ") && host.len() > "host: ".len());
    assert_eq!(lines.next(), Some(format!("os: {}", std::env::consts::OS).as_str()));
    assert_eq!(
        lines.next(),
        Some(format!("arch: {}", std::env::consts::ARCH).as_str())
    );
    assert!(text.contains("\ngpu: "));
}

#[test]
fn verify_sync_rejects_an_unknown_argument() {
    let output = run(&["verify-sync", "--bogus"]);
    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("qualia: unknown argument '--bogus'"));
}

// ---------------------------------------------------------------------------
// Local stubs
//
// The timeout and scheme-fallback paths are exercised against servers this
// process owns on ephemeral loopback ports. No test here reaches a network,
// so the suite is deterministic and says nothing about any operator's host.
// ---------------------------------------------------------------------------

/// The health document `qualia health` reports, field for field.
const HEALTH_BODY: &str = concat!(
    r#"{"service_instance":"stub-agent","status_type":"stack_health","status":"ok","#,
    r#""healthy":true,"ready":true,"#,
    r#""integration":{"ros_connected":true,"planner_ready":true,"#,
    r#""pose":{"fresh":true},"lidar":{"fresh":true}},"#,
    r#""compute":{"healthy":true,"planner_algorithms":["astar"],"#,
    r#""cuda":{"device_name":"stub-cuda","sm":8,"status":"ready","reason":null}}}"#,
);

/// The planner document `qualia planner` reports.
const PLANNER_BODY: &str = concat!(
    r#"{"service_instance":"stub-agent","status_type":"planner_status","status":"ok","#,
    r#""planner_ready":true,"planner_algorithms":["astar"],"#,
    r#""belief_features":null,"last_result":null}"#,
);

/// Run `handler` on a fresh thread for every connection accepted on an
/// ephemeral loopback port, and return that port.
fn serve<F>(handler: F) -> u16
where
    F: Fn(TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
    let port = listener.local_addr().expect("stub address").port();
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let handler = Arc::clone(&handler);
            thread::spawn(move || handler(stream));
        }
    });
    port
}

/// Read one HTTP request head. `None` when the peer spoke something that is
/// not HTTP — a TLS ClientHello arriving on a cleartext port, say.
fn read_head<S: Read>(stream: &mut S) -> Option<String> {
    let mut text = String::new();
    let mut buffer = [0u8; 1024];
    while !text.contains("\r\n\r\n") {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        text.push_str(std::str::from_utf8(&buffer[..read]).ok()?);
        if text.len() > 16 * 1024 {
            return None;
        }
    }
    Some(text)
}

/// Answer with `body` as `content-type: application/json`.
fn write_json<S: Write>(stream: &mut S, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Turn the connection away and close it.
fn refuse<S: Write>(stream: &mut S) {
    let _ =
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    let _ = stream.flush();
}

/// A cleartext stub that answers HTTP requests with `body` and refuses
/// anything else, the way a plain HTTP server meets a TLS handshake.
fn http_stub(body: &'static str) -> u16 {
    serve(move |mut stream| match read_head(&mut stream) {
        Some(_) => write_json(&mut stream, body),
        None => refuse(&mut stream),
    })
}

/// A stub that accepts connections and then says nothing at all.
fn hung_stub() -> u16 {
    serve(|stream| {
        thread::sleep(Duration::from_secs(30));
        drop(stream);
    })
}

/// A stub whose headers arrive and whose body never does.
fn slow_body_stub() -> u16 {
    serve(|mut stream| {
        if read_head(&mut stream).is_none() {
            refuse(&mut stream);
            return;
        }
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 64\r\n\
              connection: close\r\n\r\n{\"service_instance\":",
        );
        let _ = stream.flush();
        thread::sleep(Duration::from_secs(30));
    })
}

/// A TLS-only stub behind a self-signed loopback certificate.
fn tls_stub(body: &'static str) -> u16 {
    install_crypto_provider();
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("generate stub certificate");
    let certificate = rustls::pki_types::CertificateDer::from(certified.cert.der().to_vec());
    let key = rustls::pki_types::PrivateKeyDer::from(
        rustls::pki_types::PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()),
    );
    let config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .expect("stub TLS configuration"),
    );

    serve(move |stream| {
        let Ok(connection) = rustls::ServerConnection::new(Arc::clone(&config)) else {
            return;
        };
        let mut stream = rustls::StreamOwned::new(connection, stream);
        if read_head(&mut stream).is_some() {
            write_json(&mut stream, body);
        }
    })
}

fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// The ceiling every bounded call must stay under. The real bound is the
/// transport's own timeouts; this only fails a call that hangs.
const BOUND: Duration = Duration::from_secs(30);

#[test]
fn health_bounds_a_hung_server() {
    let port = hung_stub();
    let url = format!("https://127.0.0.1:{port}");

    let started = Instant::now();
    let output = run_env(&["health"], &[("QUALIA_AGENT_URL", &url)]);
    let elapsed = started.elapsed();

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("service_instance: unavailable\n"), "{text}");
    assert!(text.contains("health_reason: "), "{text}");
    assert!(elapsed < BOUND, "a hung server held the CLI for {elapsed:?}");
}

#[test]
fn health_reaches_a_cleartext_server_on_the_configured_port() {
    let port = http_stub(HEALTH_BODY);
    let configured = format!("https://127.0.0.1:{port}");

    let started = Instant::now();
    let output = run_env(&["health"], &[("QUALIA_WEB_PORT", &port.to_string())]);
    let elapsed = started.elapsed();

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with(&format!("agent_url: {configured}\n")), "{text}");
    assert!(text.contains("service_instance: stub-agent\n"), "{text}");
    assert!(text.contains("healthy: true\n"), "{text}");
    assert!(
        stderr(&output).contains(&format!(
            "qualia: agent url http://127.0.0.1:{port} (fallback from {configured})"
        )),
        "the discovered endpoint must be announced: {}",
        stderr(&output)
    );
    assert!(elapsed < BOUND, "the fallback took {elapsed:?}");
}

#[test]
fn a_wrong_scheme_override_falls_back_to_cleartext() {
    let port = http_stub(HEALTH_BODY);
    let url = format!("https://127.0.0.1:{port}");

    let output = run_env(&["health"], &[("QUALIA_AGENT_URL", &url)]);

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with(&format!("agent_url: {url}\n")), "{text}");
    assert!(text.contains("service_instance: stub-agent\n"), "{text}");
    assert!(
        stderr(&output).contains(&format!("qualia: agent url http://127.0.0.1:{port}")),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_configured_agent_url_wins_over_the_web_port() {
    let port = http_stub(HEALTH_BODY);
    let url = format!("http://127.0.0.1:{port}");

    let output = run_env(
        &["health"],
        &[("QUALIA_AGENT_URL", &url), ("QUALIA_WEB_PORT", "1")],
    );

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with(&format!("agent_url: {url}\n")), "{text}");
    assert!(text.contains("service_instance: stub-agent\n"), "{text}");
    assert_eq!(stderr(&output), "", "the override answered first");
}

#[test]
fn health_reaches_a_tls_only_server_over_https() {
    let port = tls_stub(HEALTH_BODY);
    let url = format!("https://127.0.0.1:{port}");

    let output = run_env(&["health"], &[("QUALIA_AGENT_URL", &url)]);

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with(&format!("agent_url: {url}\n")), "{text}");
    assert!(text.contains("service_instance: stub-agent\n"), "{text}");
    assert!(text.contains("compute_healthy: true\n"), "{text}");
    assert_eq!(stderr(&output), "", "https answered; nothing was discovered");
}

#[test]
fn health_bounds_a_server_that_never_finishes_its_body() {
    let port = slow_body_stub();
    let url = format!("http://127.0.0.1:{port}");

    let started = Instant::now();
    let output = run_env(&["health"], &[("QUALIA_AGENT_URL", &url)]);
    let elapsed = started.elapsed();

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("service_instance: unavailable\n"), "{text}");
    assert!(text.contains("health_reason: "), "{text}");
    assert!(elapsed < BOUND, "a slow body held the CLI for {elapsed:?}");
}

#[test]
fn planner_reaches_a_cleartext_server_on_the_configured_port() {
    let port = http_stub(PLANNER_BODY);
    let configured = format!("https://127.0.0.1:{port}");

    let output = run_env(&["planner"], &[("QUALIA_WEB_PORT", &port.to_string())]);

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with(&format!("agent_url: {configured}\n")), "{text}");
    assert!(text.contains("status_type: planner_status\n"), "{text}");
    assert!(text.contains("planner_ready: true\n"), "{text}");
    assert!(text.contains("planner_algorithms: astar\n"), "{text}");
    assert!(
        stderr(&output).contains(&format!("qualia: agent url http://127.0.0.1:{port}")),
        "{}",
        stderr(&output)
    );
}
