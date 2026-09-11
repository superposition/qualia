//! The binary's own contract: the refusals an operator hits, and the startup
//! line a running observe-only runner prints.

use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};
use qualia_jepa_model::{
    empty_candidate_manifest, initialize_deterministic, write_candidate_checkpoint, ActionSupport,
    BaselineGate, GroundingGeometry, HeldOutMetrics, JepaCandidateModel,
};
use qualia_shm::ShmRegion;
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[test]
fn binary_attaches_to_a_live_region_and_reports_its_generation() {
    let artifacts = TempDir::new().unwrap();
    let id = "smoke-gen";
    let variables = VarMap::new();
    JepaCandidateModel::load(VarBuilder::from_varmap(&variables, DType::F32, &Device::Cpu)).unwrap();
    initialize_deterministic(&variables, 7).unwrap();
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
        write_candidate_checkpoint(artifacts.path(), id, &variables, &variables, manifest).unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let pointer = serde_json::json!({
        "schema_version": "qualia.jepa-generation.v1",
        "generation": 1,
        "checkpoint_dir": artifacts.path().join(id).display().to_string(),
        "checkpoint_id": id,
        "weights_sha256": written["weights_sha256"],
        "mode": "observe-only",
        "approved_observe_only": true,
    });
    let pointer_path = artifacts.path().join("current.json");
    fs::write(&pointer_path, serde_json::to_vec_pretty(&pointer).unwrap()).unwrap();

    let shm_name = format!("/qualia_jepa_c42_smoke_{}", std::process::id());
    let region = ShmRegion::create(&shm_name).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_qualia-jepa-runtime"))
        .env("QUALIA_JEPA_ENABLE", "1")
        .env("QUALIA_JEPA_MODE", "observe-only")
        .env("QUALIA_JEPA_GENERATION_FILE", &pointer_path)
        .env("QUALIA_SHM_NAME", &shm_name)
        .env("QUALIA_JEPA_POLL_MS", "25")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The binary logs its startup line only after it has loaded and
    // digest-checked the checkpoint, which outruns any fixed sleep while the
    // host is compiling, so wait for the line itself under a deadline.
    let pipe = child.stderr.take().expect("stderr pipe");
    let (sink, lines) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut transcript = String::new();
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            transcript.push_str(&line);
            transcript.push('\n');
            if sink.send(line).is_err() {
                break;
            }
        }
        transcript
    });

    let expected = format!(
        "qualia-jepa-runtime: observe-only backend=cpu generation=1 checkpoint={id} shm={shm_name}"
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut started = false;
    while !started && Instant::now() < deadline {
        match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => started = line == expected,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let transcript = reader.join().expect("stderr reader");
    eprintln!("stderr: {transcript}");
    assert!(
        started,
        "the runner never logged its startup line; stderr: {transcript}"
    );
    drop(region);
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
