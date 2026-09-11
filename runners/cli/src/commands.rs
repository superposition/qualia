//! Execution of the parsed operator commands.
//!
//! Every report here is line-oriented and stable: operators read it in a
//! terminal, scripts match it, and a missing service degrades to an
//! `unavailable` report rather than an error.

use crate::cli::{self, Command, Decision, Proposal};
use crate::http;
use crate::platform;
use crate::world;
use qualia_ipc::{ControlMsg, ControlStream};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

/// Parse and execute one invocation.
pub fn dispatch<I>(args: I) -> ExitCode
where
    I: IntoIterator<Item = String>,
{
    match cli::parse(args) {
        Ok(command) => run(command),
        Err(message) => {
            eprintln!("qualia: {message}");
            cli::print_usage();
            ExitCode::from(2)
        }
    }
}

fn run(command: Command) -> ExitCode {
    match command {
        Command::Start { manifest } => start_stack(manifest),
        Command::VerifySync => verify_sync(),
        Command::Status { manifest } => print_status(manifest),
        Command::Stop { manifest } => stop_stack(manifest),
        Command::Logs { runner, lines } => print_logs(&runner, lines),
        Command::Host => platform::print_host(),
        Command::Health => print_health(),
        Command::Cuda => print_cuda(),
        Command::Planner => print_planner(),
        Command::Propose(proposal) => submit_proposal(proposal),
        Command::Decide(decision) => submit_decision(decision),
        Command::World { json } => print_world(json),
        Command::Decisions { json, limit } => print_decisions(json, limit),
        Command::Help => {
            cli::print_usage();
            ExitCode::SUCCESS
        }
    }
}

/// Hand control to the stack supervisor, forwarding the operator's manifest.
fn start_stack(manifest: Option<PathBuf>) -> ExitCode {
    let init = match platform::resolve_init_binary() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("qualia: {err}");
            return ExitCode::from(1);
        }
    };

    let mut command = ProcessCommand::new(init);
    if let Some(path) = manifest {
        command.env("QUALIA_STACK_MANIFEST", path);
    }

    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("qualia: failed to launch qualia-init: {err}");
            ExitCode::from(1)
        }
    }
}

/// Run the trusted sync verification script from the repository root.
fn verify_sync() -> ExitCode {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));

    #[cfg(windows)]
    let mut command = {
        let mut command = ProcessCommand::new("powershell");
        command.args([
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            "scripts/verify-sync.ps1",
        ]);
        command
    };

    #[cfg(not(windows))]
    let mut command = {
        let mut command = ProcessCommand::new("bash");
        command.arg("scripts/verify-sync.sh");
        command
    };

    command.current_dir(&repo_root);
    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("qualia: failed to launch sync verifier: {err}");
            ExitCode::from(1)
        }
    }
}

/// Report the manifest and which of its runners are alive.
fn print_status(manifest: Option<PathBuf>) -> ExitCode {
    let manifest = match platform::load_stack_manifest(manifest.as_deref()) {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("qualia: {err}");
            return ExitCode::from(1);
        }
    };

    let running = match platform::running_process_names() {
        Ok(names) => names,
        Err(err) => {
            eprintln!("qualia: failed to inspect processes: {err}");
            return ExitCode::from(1);
        }
    };

    println!("stack: {}", manifest.stack_name);
    println!("schema_version: {}", manifest.schema_version);
    println!("shared_memory: {}", manifest.shared_memory.name);
    println!("control_socket: {}", manifest.control.socket);
    println!();

    let mut active = 0usize;
    for runner in &manifest.runners {
        let alive = running.contains(&platform::process_name_for_runner(&runner.name));
        if alive {
            active += 1;
        }
        println!("[{}] {}", if alive { "running" } else { "stopped" }, runner.name);
    }

    println!();
    println!(
        "summary: {active}/{} manifest runners active",
        manifest.runners.len()
    );

    ExitCode::SUCCESS
}

/// Print the tail of one runner's log file.
fn print_logs(runner: &str, lines: usize) -> ExitCode {
    let log_dir =
        std::env::var("QUALIA_LOG_DIR").unwrap_or_else(|_| "artifacts/logs".to_string());
    let path = Path::new(&log_dir).join(format!("{runner}.log"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("qualia: failed to read log '{}': {err}", path.display());
            return ExitCode::from(1);
        }
    };

    for line in platform::tail_lines(&text, lines) {
        println!("{line}");
    }

    ExitCode::SUCCESS
}

/// Ask the supervisor to shut the stack down over the control channel.
fn stop_stack(manifest: Option<PathBuf>) -> ExitCode {
    let manifest = match platform::load_stack_manifest(manifest.as_deref()) {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("qualia: {err}");
            return ExitCode::from(1);
        }
    };

    let socket =
        std::env::var("QUALIA_SOCK_PATH").unwrap_or_else(|_| manifest.control.socket.clone());
    let mut stream = match ControlStream::connect(&socket) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("qualia: failed to connect control socket '{socket}': {err}");
            return ExitCode::from(1);
        }
    };

    match stream.send(ControlMsg::Shutdown, None) {
        Ok(()) => {
            println!("shutdown sent to {socket}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("qualia: failed to send shutdown to '{socket}': {err}");
            ExitCode::from(1)
        }
    }
}

/// Report the compute service, or an `unavailable` shape if it is not there.
fn print_cuda() -> ExitCode {
    let socket = platform::default_compute_socket();
    println!("compute_socket: {socket}");
    match platform::fetch_compute_capabilities(&socket) {
        Ok(capabilities) => {
            println!("service_instance: {}", capabilities.service_instance);
            println!("healthy: {}", capabilities.healthy);
            println!(
                "planner_algorithms: {}",
                capabilities.planner_algorithms.join(",")
            );
            println!("device_name: {}", capabilities.cuda.device_name);
            println!("sm: {}", capabilities.cuda.sm);
            println!(
                "cuda_status: {}",
                capabilities.cuda.status.as_deref().unwrap_or("unknown")
            );
            println!(
                "cuda_reason: {}",
                capabilities.cuda.reason.as_deref().unwrap_or("ready")
            );
        }
        Err(err) => {
            println!("service_instance: unavailable");
            println!("healthy: false");
            println!("planner_algorithms:");
            println!("device_name: {}", platform::gpu_device_label());
            println!("sm: 0");
            println!("cuda_status: unavailable");
            println!("cuda_reason: {err}");
        }
    }
    ExitCode::SUCCESS
}

/// Report aggregate stack health, or an `unavailable` shape.
fn print_health() -> ExitCode {
    let agent_url = platform::default_agent_url();
    println!("agent_url: {agent_url}");
    match http::fetch_stack_health(&agent_url) {
        Ok(status) => {
            println!("service_instance: {}", status.service_instance);
            println!("status_type: {}", status.status_type);
            println!("status: {}", status.status);
            println!("healthy: {}", status.healthy);
            println!("ready: {}", status.ready);
            println!("ros_connected: {}", status.integration.ros_connected);
            println!("planner_ready: {}", status.integration.planner_ready);
            println!("pose_fresh: {}", status.integration.pose.fresh);
            println!("lidar_fresh: {}", status.integration.lidar.fresh);
            println!("compute_healthy: {}", status.compute.healthy);
            println!(
                "cuda_status: {}",
                status.compute.cuda.status.as_deref().unwrap_or("unknown")
            );
            println!(
                "cuda_reason: {}",
                status.compute.cuda.reason.as_deref().unwrap_or("ready")
            );
            println!(
                "planner_algorithms: {}",
                status.compute.planner_algorithms.join(",")
            );
        }
        Err(err) => {
            println!("service_instance: unavailable");
            println!("status_type: stack_health");
            println!("status: unavailable");
            println!("healthy: false");
            println!("ready: false");
            println!("health_reason: {err}");
        }
    }
    ExitCode::SUCCESS
}

/// Report planner readiness and its most recent result.
fn print_planner() -> ExitCode {
    let agent_url = platform::default_agent_url();
    println!("agent_url: {agent_url}");
    match http::fetch_planner_status(&agent_url) {
        Ok(status) => {
            println!("service_instance: {}", status.service_instance);
            println!("status_type: {}", status.status_type);
            println!("status: {}", status.status);
            println!("planner_ready: {}", status.planner_ready);
            println!("planner_algorithms: {}", status.planner_algorithms.join(","));
            if let Some(features) = status.belief_features {
                println!("belief_directive_present: {}", features.directive_present);
                println!("belief_activity_present: {}", features.activity_present);
                println!(
                    "belief_scene_embedding_l2: {:.3}",
                    features.scene_embedding_l2
                );
                println!(
                    "belief_scene_embedding_peak: {:.3}",
                    features.scene_embedding_peak
                );
                println!("belief_llm_age_ms: {}", features.llm_age_ms);
                println!("belief_vision_age_ms: {}", features.vision_age_ms);
            } else {
                println!("belief_features: unavailable");
            }
            if let Some(last) = status.last_result {
                println!("last_request_id: {}", last.request_id);
                println!("last_status: {}", last.status);
                println!(
                    "last_algorithm: {}",
                    last.algorithm.unwrap_or_else(|| "unknown".to_string())
                );
                println!("last_path_len: {}", last.path_len);
                println!(
                    "last_planning_ms: {}",
                    last.planning_ms
                        .map(|value| format!("{value:.3}"))
                        .unwrap_or_else(|| "n/a".to_string())
                );
                println!(
                    "last_reachable: {}",
                    last.reachable
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "n/a".to_string())
                );
            } else {
                println!("last_result: none");
            }
        }
        Err(err) => {
            println!("service_instance: unavailable");
            println!("status_type: planner_status");
            println!("status: unavailable");
            println!("planner_ready: false");
            println!("planner_algorithms:");
            println!("last_result: none");
            println!("planner_reason: {err}");
        }
    }
    ExitCode::SUCCESS
}

/// Enroll as an operator replica, then inject a proposal.
fn submit_proposal(proposal: Proposal) -> ExitCode {
    let agent_url = platform::default_agent_url();
    let json = proposal.shared().json;
    let identity = world::replica_identity(proposal.shared().replica_id.clone());

    if let Err(err) = world::register_replica(&agent_url, &identity) {
        eprintln!("qualia: failed to enroll CLI replica: {err}");
        return ExitCode::from(1);
    }

    let envelope = world::proposal_envelope(proposal, &identity);
    if let Err(err) = envelope.validate_shape() {
        eprintln!("qualia: invalid proposal payload: {err}");
        return ExitCode::from(1);
    }

    match http::post_json::<world::ProposalResponse, _>(
        &agent_url,
        "/world-model/proposals",
        &envelope,
    ) {
        Ok(response) => {
            if json {
                return print_pretty(&response, "failed to format JSON response");
            }
            println!("agent_url: {agent_url}");
            println!("proposal_id: {}", response.proposal.proposal_id);
            println!("proposal_kind: {:?}", response.proposal.proposal_kind);
            println!("source_replica_id: {}", response.proposal.source_replica_id);
            println!(
                "source_replica_role: {}",
                response.proposal.source_replica_role
            );
            println!("status: {:?}", response.proposal.status);
            println!("confidence: {:.2}", response.proposal.confidence);
            println!("summary: {}", world::describe_proposal(&response.proposal.body));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("qualia: failed to submit proposal: {err}");
            ExitCode::from(1)
        }
    }
}

/// Enroll as an operator replica, then record a decision.
fn submit_decision(decision: Decision) -> ExitCode {
    let agent_url = platform::default_agent_url();
    let json = decision.shared().json;
    let identity = world::replica_identity(decision.shared().replica_id.clone());

    if let Err(err) = world::register_replica(&agent_url, &identity) {
        eprintln!("qualia: failed to enroll CLI replica: {err}");
        return ExitCode::from(1);
    }

    let envelope = world::decision_envelope(decision, &identity);
    if let Err(err) = envelope.validate_shape() {
        eprintln!("qualia: invalid decision payload: {err}");
        return ExitCode::from(1);
    }

    match http::post_json::<world::DecisionResponse, _>(
        &agent_url,
        "/world-model/decisions",
        &envelope,
    ) {
        Ok(response) => {
            if json {
                return print_pretty(&response, "failed to format JSON response");
            }
            println!("agent_url: {agent_url}");
            println!("decision_id: {}", response.decision.decision_id);
            println!("decision_kind: {:?}", response.decision.decision_kind);
            println!(
                "curator_replica_id: {}",
                response.decision.curator_replica_id
            );
            println!(
                "target_proposal_ids: {}",
                response.decision.target_proposal_ids.join(",")
            );
            if let Some(canonical) = response.canonical {
                println!("canonical_id: {}", canonical.canonical_id);
                println!("canonical_kind: {:?}", canonical.canonical_kind);
                println!(
                    "canonical_summary: {}",
                    world::describe_canonical(&canonical.body)
                );
            } else {
                println!("canonical_id: none");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("qualia: failed to submit decision: {err}");
            ExitCode::from(1)
        }
    }
}

/// Emit one pretty JSON document, mapping a formatting failure to exit 1.
fn print_pretty<T: serde::Serialize>(value: &T, failure: &str) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("qualia: {failure}: {err}");
            ExitCode::from(1)
        }
    }
}

/// Report proposal, canonical and operational scene state.
fn print_world(json: bool) -> ExitCode {
    let agent_url = platform::default_agent_url();
    let proposals = match http::get_json::<world::ProposalsResponse>(
        &agent_url,
        "/world-model/proposals?limit=128",
    ) {
        Ok(response) => response.proposals,
        Err(err) => {
            eprintln!("qualia: failed to fetch world-model proposals: {err}");
            return ExitCode::from(1);
        }
    };
    let canonical = match http::get_json::<world::CanonicalResponse>(
        &agent_url,
        "/world-model/canonical?limit=128",
    ) {
        Ok(response) => response.canonical,
        Err(err) => {
            eprintln!("qualia: failed to fetch canonical world state: {err}");
            return ExitCode::from(1);
        }
    };
    let operational =
        match http::get_json::<world::OperationalResponse>(&agent_url, "/world-model/operational") {
            Ok(response) => response.operational,
            Err(err) => {
                eprintln!("qualia: failed to fetch operational world state: {err}");
                return ExitCode::from(1);
            }
        };

    if json {
        let snapshot = world::WorldSnapshot {
            agent_url,
            proposals,
            canonical,
            operational,
        };
        return print_pretty(&snapshot, "failed to format JSON world snapshot");
    }

    println!("agent_url: {agent_url}");
    println!("proposal_count: {}", proposals.len());
    println!("canonical_count: {}", canonical.len());
    println!("accepted_count: {}", operational.planner.accepted_count);
    println!("nav_goal_present: {}", operational.planner.nav_goal_present);
    println!("hazard_count: {}", operational.hazards.len());
    println!(
        "route_factor_count: {}",
        operational.planner.route_factor_count
    );
    match &operational.pose {
        Some(pose) => println!(
            "pose: {} @ ({:.2}, {:.2}, {:.2}) yaw={:.2}",
            pose.canonical_id, pose.x_m, pose.y_m, pose.z_m, pose.yaw_rad
        ),
        None => println!("pose: none"),
    }
    match &operational.nav_goal {
        Some(goal) => println!(
            "nav_goal: {} @ ({:.2}, {:.2}, {:.2}) yaw={:.2}",
            goal.canonical_id, goal.x_m, goal.y_m, goal.z_m, goal.yaw_rad
        ),
        None => println!("nav_goal: none"),
    }

    println!();
    println!("proposals:");
    if proposals.is_empty() {
        println!("- none");
    } else {
        for proposal in &proposals {
            println!(
                "- {} kind={:?} status={:?} confidence={:.2} {}",
                proposal.proposal_id,
                proposal.proposal_kind,
                proposal.status,
                proposal.confidence,
                world::describe_proposal(&proposal.body)
            );
        }
    }

    println!();
    println!("canonical:");
    if canonical.is_empty() {
        println!("- none");
    } else {
        for entry in &canonical {
            println!(
                "- {} kind={:?} status={:?} {}",
                entry.canonical_id,
                entry.canonical_kind,
                entry.status,
                world::describe_canonical(&entry.body)
            );
        }
    }

    if !operational.hazards.is_empty() {
        println!();
        println!("hazards:");
        for hazard in &operational.hazards {
            println!(
                "- {} kind={} severity={} summary={}",
                hazard.canonical_id,
                hazard.kind,
                hazard
                    .severity
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_else(|| "n/a".to_string()),
                hazard.summary.clone().unwrap_or_else(|| "n/a".to_string())
            );
        }
    }

    ExitCode::SUCCESS
}

/// Report the recorded coach decisions.
fn print_decisions(json: bool, limit: usize) -> ExitCode {
    let agent_url = platform::default_agent_url();
    let path = format!("/world-model/decisions?limit={limit}");
    let response = match http::get_json::<world::DecisionsResponse>(&agent_url, &path) {
        Ok(response) => response,
        Err(err) => {
            eprintln!("qualia: failed to fetch world-model decisions: {err}");
            return ExitCode::from(1);
        }
    };

    if json {
        let output = world::DecisionList {
            agent_url,
            decisions: response.decisions,
        };
        return print_pretty(&output, "failed to format JSON decisions output");
    }

    println!("agent_url: {agent_url}");
    println!("decision_count: {}", response.decisions.len());
    if response.decisions.is_empty() {
        println!("- none");
        return ExitCode::SUCCESS;
    }
    for decision in response.decisions {
        println!(
            "- {} kind={:?} curator={} targets={} reason={}",
            decision.decision_id,
            decision.decision_kind,
            decision.curator_replica_id,
            decision.target_proposal_ids.join(","),
            decision.reason.unwrap_or_else(|| "n/a".to_string())
        );
    }
    ExitCode::SUCCESS
}
