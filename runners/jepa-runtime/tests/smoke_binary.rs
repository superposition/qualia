//! The binary's own contract: the refusals an operator hits, the startup line a
//! running observe-only runner prints, and the promotion strand's reports when
//! the generation pointer moves.

use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};
use qualia_jepa_model::{
    empty_candidate_manifest, initialize_deterministic, write_candidate_checkpoint, ActionSupport,
    BaselineGate, GroundingGeometry, HeldOutMetrics, JepaCandidateModel,
};
use qualia_shm::ShmRegion;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Writes one digest-checked observe-only checkpoint under `artifacts` and
/// answers the generation pointer a runner accepts for it.
fn checkpoint(
    artifacts: &Path,
    id: &str,
    generation: u64,
    variables: &VarMap,
) -> serde_json::Value {
    let manifest = empty_candidate_manifest(
        id,
        &"c".repeat(64),
        7,
        "cpu",
        BaselineGate {
            dataset_digest: "c".repeat(64),
            valid_transitions: 60_000,
            sessions: 20,
            conditions: 4,
            environments: 4,
            constant: HeldOutMetrics { transition_nll: 3.0, rollout_error: 3.0 },
            flat_mlp: HeldOutMetrics { transition_nll: 2.0, rollout_error: 2.0 },
            tiny_cnn: HeldOutMetrics { transition_nll: 0.5, rollout_error: 0.5 },
        },
        ActionSupport {
            sample_count: 60_000,
            min_left: -1.0,
            max_left: 1.0,
            min_right: -1.0,
            max_right: 1.0,
            min_effective_forward: -1.0,
            max_effective_forward: 1.0,
            min_effective_turn: -1.0,
            max_effective_turn: 1.0,
            min_speed_scale: 0.0,
            max_speed_scale: 1.0,
            min_delta_seconds: 0.02,
            max_delta_seconds: 0.6,
        },
        GroundingGeometry { width: 64, height: 64, resolution_m: 0.05 },
    );
    let (_, manifest_path) =
        write_candidate_checkpoint(artifacts, id, variables, variables, manifest).unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    serde_json::json!({
        "schema_version": "qualia.jepa-generation.v1",
        "generation": generation,
        "checkpoint_dir": artifacts.join(id).display().to_string(),
        "checkpoint_id": id,
        "weights_sha256": written["weights_sha256"],
        "mode": "observe-only",
        "approved_observe_only": true,
    })
}

/// A deterministic set of weights a checkpoint can be written from.
fn trained_variables() -> VarMap {
    let variables = VarMap::new();
    JepaCandidateModel::load(VarBuilder::from_varmap(&variables, DType::F32, &Device::Cpu)).unwrap();
    initialize_deterministic(&variables, 7).unwrap();
    variables
}

/// A running runner child, its stderr lines, and the scratch region it attached
/// to.
struct Runner {
    child: Child,
    lines: Receiver<String>,
    shm_name: String,
    pointer_path: PathBuf,
    _region: ShmRegion,
}

impl Runner {
    fn start(artifacts: &Path, pointer: &serde_json::Value, agent_url: Option<&str>) -> Self {
        let pointer_path = artifacts.join("current.json");
        install_pointer(&pointer_path, pointer);
        let shm_name = format!("/qualia_jepa_smoke_{}", std::process::id());
        let region = ShmRegion::create(&shm_name).unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_qualia-jepa-runtime"));
        command
            .env("QUALIA_JEPA_ENABLE", "1")
            .env("QUALIA_JEPA_MODE", "observe-only")
            .env("QUALIA_JEPA_GENERATION_FILE", &pointer_path)
            .env("QUALIA_SHM_NAME", &shm_name)
            .env("QUALIA_JEPA_POLL_MS", "25")
            .env_remove("QUALIA_AGENT_URL")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(url) = agent_url {
            command.env("QUALIA_AGENT_URL", url);
        }
        let mut child = command.spawn().unwrap();

        // The binary logs its startup line only after it has loaded and
        // digest-checked the checkpoint, which outruns any fixed sleep while the
        // host is compiling, so wait for the line itself under a deadline.
        let pipe = child.stderr.take().expect("stderr pipe");
        let (sink, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if sink.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            shm_name,
            pointer_path,
            _region: region,
        }
    }

    /// The line the runner logs after loading the pointer's generation.
    fn startup_line(&self, generation: u64, checkpoint: &str) -> String {
        format!(
            "qualia-jepa-runtime: observe-only backend=cpu generation={generation} checkpoint={checkpoint} shm={}",
            self.shm_name
        )
    }

    /// Installs `pointer` and waits for the line the runner logs for the swap.
    fn swap_to(&self, pointer: &serde_json::Value, expected: &str) {
        install_pointer(&self.pointer_path, pointer);
        assert!(
            self.wait_for_line(expected),
            "the runner never logged {expected:?}"
        );
    }

    /// Waits for one stderr line, under a deadline.
    fn wait_for_line(&self, expected: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            match self.lines.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(line) => {
                    if line == expected {
                        return true;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn install_pointer(path: &Path, pointer: &serde_json::Value) {
    fs::write(path, serde_json::to_vec_pretty(pointer).unwrap()).unwrap();
}

/// Stands in for the agent's braid edge: hands back the body of every report,
/// answering each request so the reporter is never left waiting.
fn braid_stub() -> (String, Receiver<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("stub binds");
    let port = listener.local_addr().expect("stub address").port();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || loop {
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        let Some(body) = read_request(&mut socket) else {
            continue;
        };
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}");
        let event = serde_json::from_slice(&body).expect("the report is JSON");
        if sender.send(event).is_err() {
            return;
        }
    });
    (format!("http://127.0.0.1:{port}"), receiver)
}

/// Reads one whole request off `socket`, answering its body.
fn read_request(socket: &mut TcpStream) -> Option<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        if let Some(head_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
        {
            let length = String::from_utf8_lossy(&request[..head_end])
                .split("\r\n")
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= head_end + length {
                return Some(request[head_end..head_end + length].to_vec());
            }
        }
        match socket.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
}

#[test]
fn binary_attaches_to_a_live_region_and_reports_its_generation() {
    let artifacts = TempDir::new().unwrap();
    let variables = trained_variables();
    let pointer = checkpoint(artifacts.path(), "smoke-gen", 1, &variables);

    let runner = Runner::start(artifacts.path(), &pointer, None);
    assert!(
        runner.wait_for_line(&runner.startup_line(1, "smoke-gen")),
        "the runner never logged its startup line"
    );
}

/// The promotion strand: the pointer moving forward is a promotion, and the
/// pointer naming the checkpoint that was parked for rollback is a rollback —
/// with the reason and the wire variant a registry dispatch keys on.
#[test]
fn binary_reports_promotion_and_rollback_to_the_braid() {
    let artifacts = TempDir::new().unwrap();
    let variables = trained_variables();
    let first = checkpoint(artifacts.path(), "smoke-gen", 1, &variables);
    let second = checkpoint(artifacts.path(), "smoke-gen-2", 2, &variables);

    let (agent_url, reported) = braid_stub();
    let runner = Runner::start(artifacts.path(), &first, Some(&agent_url));
    assert!(
        runner.wait_for_line(&runner.startup_line(1, "smoke-gen")),
        "the runner never logged its startup line"
    );

    runner.swap_to(
        &second,
        "qualia-jepa-runtime: swapped generation=2 checkpoint=smoke-gen-2",
    );
    let promoted = reported
        .recv_timeout(Duration::from_secs(30))
        .expect("the promotion is reported to the braid");
    assert_eq!(promoted["event"], "promotion_accepted");
    assert_eq!(promoted["generation"], 2);

    // The parked checkpoint named again, under a higher counter: the runner
    // reuses the parked copy, so it is a rollback and the reason says why.
    let mut rolled_back_to = first.clone();
    rolled_back_to["generation"] = serde_json::json!(3);
    runner.swap_to(
        &rolled_back_to,
        "qualia-jepa-runtime: swapped generation=3 checkpoint=smoke-gen",
    );
    let rolled_back = reported
        .recv_timeout(Duration::from_secs(30))
        .expect("the rollback is reported to the braid");
    assert_eq!(rolled_back["event"], "promotion_rolled_back");
    assert_eq!(rolled_back["generation"], 3);
    assert_eq!(
        rolled_back["reason"], "generation pointer returned to the parked checkpoint",
        "the reason stays on the wire for the registry dispatch"
    );
}

#[test]
fn binary_refuses_to_start_without_the_enable_variables() {
    let binary = env!("CARGO_BIN_EXE_qualia-jepa-runtime");
    let disabled = Command::new(binary)
        .env_remove("QUALIA_JEPA_ENABLE")
        .env_remove("QUALIA_JEPA_MODE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn the refused binary");
    assert_eq!(disabled.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&disabled.stderr).trim_end(),
        "qualia-jepa-runtime: refusing to start: set QUALIA_JEPA_ENABLE=1 explicitly"
    );

    let wrong_mode = Command::new(binary)
        .env("QUALIA_JEPA_ENABLE", "1")
        .env("QUALIA_JEPA_MODE", "canonical")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn the refused binary");
    assert_eq!(wrong_mode.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&wrong_mode.stderr).trim_end(),
        "qualia-jepa-runtime: refusing to start outside QUALIA_JEPA_MODE=observe-only"
    );
}
