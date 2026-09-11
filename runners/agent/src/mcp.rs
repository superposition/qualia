//! The model-facing surface: MCP over HTTP, and the JEPA studio status.
//!
//! The tools this build advertises are the ones it can actually answer —
//! observation of the braid, the mission broker, perception, the planner and
//! the JEPA lanes. A model that asks for a tool this build does not have is
//! told so, rather than handed an empty success.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use qualia_types::{
    JepaEvidencePayload, JepaTelemetryPayload, JEPA_BACKEND_CPU, JEPA_BACKEND_CUDA,
    JEPA_BACKEND_METAL, JEPA_FLAG_CHECKPOINT_VERIFIED, JEPA_FLAG_GROUNDING_AVAILABLE,
    JEPA_FLAG_OUTPUT_FINITE, JEPA_FLAG_SOURCES_COHERENT, JEPA_FLAG_VALID, JEPA_ID_BYTES,
    JEPA_MODE_OBSERVE_ONLY, JEPA_MODE_REPLAY,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::AppState;

/// The protocol revision this server speaks.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
/// Revisions this server will negotiate down to.
pub const MCP_SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// The schema literal `/jepa/status` carries.
pub const JEPA_STUDIO_SCHEMA: &str = "qualia.jepa-studio-status.v1";

#[derive(Debug, Deserialize)]
pub struct McpCallRequest {
    tool: String,
    #[serde(default)]
    args: Value,
}

/// `GET /mcp/status` — what this server is and what it serves.
pub async fn status_get(State(state): State<AppState>) -> Response {
    Json(json!({
        "ok": true,
        "transport": "streamable-http",
        "server": "qualia-agent",
        "protocol_version": MCP_PROTOCOL_VERSION,
        "tool_count": tool_descriptors().len(),
        "world_model": "sync.v1 + world.model.v1",
        "arena": {
            "available": false,
            "reason": "the arena projection lands with the sync arena runtime",
            "replica_id": state.config.replica.id,
        },
    }))
    .into_response()
}

/// `GET /mcp/tools` — the tool descriptors, with their safety class.
pub async fn tools_get() -> Json<Value> {
    Json(json!({ "ok": true, "tools": tool_descriptors() }))
}

/// `POST /mcp` — JSON-RPC over the streamable-HTTP transport.
pub async fn protocol_post(State(state): State<AppState>, Json(request): Json<Value>) -> Response {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return rpc_error(id, -32600, "invalid JSON-RPC request");
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return rpc_error(id, -32600, "method is required");
    };

    match method {
        "initialize" => {
            let requested = request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let negotiated = if MCP_SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                requested
            } else {
                MCP_PROTOCOL_VERSION
            };
            rpc_result(
                id,
                json!({
                    "protocolVersion": negotiated,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "qualia-agent",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Qualia is the shared superposed world model. Observe through the status tools; physical authority stays with Leash.",
                }),
            )
        }
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": protocol_tool_descriptors() })),
        "tools/call" => {
            let Some(name) = request.pointer("/params/name").and_then(Value::as_str) else {
                return rpc_error(id, -32602, "tool name is required");
            };
            let args = request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(&state, name, args).await {
                Ok(result) => rpc_result(id, tool_result(result, false)),
                Err(error) => rpc_result(id, tool_result(json!({ "error": error }), true)),
            }
        }
        _ => rpc_error(id, -32601, "method not found"),
    }
}

/// `POST /mcp/call` — one tool call without the JSON-RPC envelope.
pub async fn call_post(State(state): State<AppState>, Json(request): Json<McpCallRequest>) -> Response {
    match call_tool(&state, &request.tool, request.args).await {
        Ok(result) => Json(json!({ "ok": true, "tool": request.tool, "result": result }))
            .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "tool": request.tool, "error": error })),
        )
            .into_response(),
    }
}

/// Run one advertised tool.
async fn call_tool(state: &AppState, name: &str, _args: Value) -> Result<Value, String> {
    match name {
        "braid_status" => Ok(serde_json::to_value(state.braid.view()).unwrap_or(Value::Null)),
        "mission_control_missions" => Ok(state.mission_control.missions_envelope()),
        "stack_health" => {
            let (_, Json(health)) = crate::health::ready_get(State(state.clone())).await;
            Ok(serde_json::to_value(health).unwrap_or(Value::Null))
        }
        "planner_status" => {
            let (_, Json(planner)) = crate::health::planner_status_get(State(state.clone())).await;
            Ok(serde_json::to_value(planner).unwrap_or(Value::Null))
        }
        "perception_status" => {
            let (_, Json(perception)) = crate::perception::status_get(State(state.clone())).await;
            Ok(serde_json::to_value(perception).unwrap_or(Value::Null))
        }
        "jepa_status" => jepa_status(state).await,
        other => Err(format!("unknown tool: {other}")),
    }
}

/// The tool list, with the safety class the studio renders.
pub fn tool_descriptors() -> Vec<Value> {
    vec![
        tool(
            "braid_status",
            "Read the braid view: generation, session, open missions and the last promotion and quarantine stamps",
            "observe-only",
        ),
        tool(
            "mission_control_missions",
            "Read every mission the broker holds, with its stage, status and last decision code",
            "observe-only",
        ),
        tool(
            "stack_health",
            "Read stack readiness: integration inputs, planner state and the accelerator the compute service reports",
            "observe-only",
        ),
        tool(
            "planner_status",
            "Read whether the planner has fresh inputs and what the compute service can plan with",
            "observe-only",
        ),
        tool(
            "perception_status",
            "Read the camera and VSLAM lanes and the movement gate they feed",
            "observe-only",
        ),
        tool(
            "jepa_status",
            "Read the coherent JEPA evidence and telemetry slots, with their gates and provenance",
            "observe-only",
        ),
    ]
}

fn tool(name: &str, description: &str, safety: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "safety": safety,
        "inputSchema": object_schema(&[]),
    })
}

fn object_schema(fields: &[(&str, &str, bool)]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, kind, is_required) in fields {
        properties.insert((*name).to_string(), json!({ "type": kind }));
        if *is_required {
            required.push((*name).to_string());
        }
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
    })
}

/// The MCP tool list omits the studio-only safety class.
fn protocol_tool_descriptors() -> Vec<Value> {
    tool_descriptors()
        .into_iter()
        .map(|mut descriptor| {
            if let Some(object) = descriptor.as_object_mut() {
                object.remove("safety");
            }
            descriptor
        })
        .collect()
}

fn rpc_result(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
        .into_response()
}

fn tool_result(result: Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string()),
        }],
        "isError": is_error,
    })
}

/// `GET /jepa/status` — the read-only view of the JEPA evidence and telemetry
/// slots. `measured` is true only for a correlated accelerator inference; the
/// route never manufactures accelerator activity.
pub async fn jepa_status_get(State(state): State<AppState>) -> Response {
    match jepa_status(&state).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => Json(json!({
            "contract": JEPA_STUDIO_SCHEMA,
            "available": false,
            "valid": false,
            "mode": "unavailable",
            "reason": "Qualia SHM is unavailable",
            "posterior": Value::Null,
        }))
        .into_response(),
    }
}

async fn jepa_status(state: &AppState) -> Result<Value, String> {
    let region = state.shm_opt().ok_or("Qualia SHM is unavailable")?;
    let evidence = region
        .jepa_evidence()
        .snapshot(8)
        .map_err(|error| format!("JEPA evidence snapshot unavailable: {error}"))?;
    let telemetry = region.jepa_telemetry().snapshot(8).ok();
    let evidence_model = fixed_id_or_unknown(&evidence.model_id);
    let evidence_checkpoint = fixed_id_or_unknown(&evidence.checkpoint_id);
    let telemetry_correlated = telemetry
        .as_ref()
        .is_some_and(|telemetry| telemetry_matches_evidence(&evidence, telemetry));
    let accelerator_measured = telemetry.as_ref().is_some_and(|telemetry| {
        telemetry_correlated
            && matches!(telemetry.backend, JEPA_BACKEND_METAL | JEPA_BACKEND_CUDA)
            && telemetry.inference_count > 0
            && telemetry.latency_last_us > 0
            && evidence.flags & JEPA_FLAG_OUTPUT_FINITE != 0
    });
    let now_ms = crate::now_ms();
    let source_age_ms = (evidence.timestamp_ns != 0)
        .then(|| now_ms.saturating_sub(evidence.timestamp_ns as u128) / 1_000_000)
        .unwrap_or(u128::MAX);
    let valid = evidence.flags & JEPA_FLAG_VALID != 0 && evidence.inference_seq > 0;

    Ok(json!({
        "contract": JEPA_STUDIO_SCHEMA,
        "available": true,
        "valid": valid,
        "fresh": source_age_ms <= 500,
        "mode": jepa_mode_name(evidence.mode),
        "source": {
            "producer_epoch": evidence.producer_epoch,
            "runner_epoch": evidence.runner_epoch,
            "inference_seq": evidence.inference_seq,
            "timestamp_ns": evidence.timestamp_ns,
            "age_ms": source_age_ms,
            "camera_seq": evidence.camera_seq,
            "lidar_seq": evidence.lidar_seq,
            "pose_seq": evidence.pose_seq,
            "action_seq": evidence.action_seq,
            "camera_age_ms": evidence.camera_age_ms,
            "lidar_age_ms": evidence.lidar_age_ms,
            "pose_age_ms": evidence.pose_age_ms,
            "action_age_ms": evidence.action_age_ms,
            "source_skew_ms": evidence.source_skew_ns as f64 / 1_000_000.0,
        },
        "provenance": {
            "model_id": evidence_model,
            "checkpoint_id": evidence_checkpoint,
            "backend": jepa_backend_name(evidence.backend),
            "abi_version": evidence.abi_version,
        },
        "gates": {
            "valid": evidence.flags & JEPA_FLAG_VALID != 0,
            "sources_coherent": evidence.flags & JEPA_FLAG_SOURCES_COHERENT != 0,
            "checkpoint_verified": evidence.flags & JEPA_FLAG_CHECKPOINT_VERIFIED != 0,
            "output_finite": evidence.flags & JEPA_FLAG_OUTPUT_FINITE != 0,
            "grounding_available": evidence.flags & JEPA_FLAG_GROUNDING_AVAILABLE != 0,
            "action_covered": evidence.flags & qualia_types::JEPA_FLAG_ACTION_COVERED != 0,
        },
        "calibration": {
            "observation_quality": evidence.observation_quality,
            "transition_nll": evidence.transition_nll,
            "occupancy_confidence": evidence.occupancy_confidence,
            "predicted_uncertainty": uncertainty_summary(&evidence.predicted_log_variance),
        },
        "telemetry": telemetry.map(|telemetry| json!({
            "correlated": telemetry_correlated,
            "measured": accelerator_measured,
            "backend": jepa_backend_name(telemetry.backend),
            "inference_count": telemetry.inference_count,
            "dropped_frames": telemetry.dropped_frames,
            "stale_frames": telemetry.stale_frames,
            "non_finite_outputs": telemetry.non_finite_outputs,
            "hot_swaps": telemetry.hot_swaps,
            "last_inference_ns": telemetry.last_inference_ns,
            "latency_last_us": telemetry.latency_last_us,
            "latency_p50_us": telemetry.latency_p50_us,
            "latency_p95_us": telemetry.latency_p95_us,
            "latency_max_us": telemetry.latency_max_us,
            "last_error_code": telemetry.last_error_code,
            "last_error": fixed_text(&telemetry.last_error, telemetry.last_error_len as usize),
        })),
        "posterior": Value::Null,
        "safety": {
            "planner_authority": false,
            "motor_authority": false,
            "canonical_belief_write": false,
        },
    }))
}

fn jepa_backend_name(backend: u32) -> &'static str {
    match backend {
        JEPA_BACKEND_CPU => "cpu",
        JEPA_BACKEND_METAL => "metal",
        JEPA_BACKEND_CUDA => "cuda",
        _ => "none",
    }
}

fn jepa_mode_name(mode: u32) -> &'static str {
    match mode {
        JEPA_MODE_REPLAY => "replay",
        JEPA_MODE_OBSERVE_ONLY => "observe-only",
        _ => "unavailable",
    }
}

fn fixed_id_or_unknown(bytes: &[u8]) -> String {
    let text = fixed_text(bytes, JEPA_ID_BYTES);
    if text.is_empty() {
        "unknown".to_string()
    } else {
        text
    }
}

fn fixed_text(bytes: &[u8], len: usize) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let end = end.min(len);
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn telemetry_matches_evidence(
    evidence: &JepaEvidencePayload,
    telemetry: &JepaTelemetryPayload,
) -> bool {
    telemetry.producer_epoch == evidence.producer_epoch
        && telemetry.runner_epoch == evidence.runner_epoch
        && telemetry.inference_count == evidence.inference_seq
        && fixed_id_or_unknown(&telemetry.model_id) == fixed_id_or_unknown(&evidence.model_id)
        && fixed_id_or_unknown(&telemetry.checkpoint_id)
            == fixed_id_or_unknown(&evidence.checkpoint_id)
}

/// Summarise the predicted log-variance as standard deviations, so an operator
/// reads the same units the calibration reports.
fn uncertainty_summary(log_variance: &[f32]) -> Value {
    let mut finite = 0_u64;
    let mut sum = 0.0_f64;
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    for value in log_variance.iter().copied().filter(|value| value.is_finite()) {
        let standard_deviation = f64::from((0.5 * value.clamp(-20.0, 20.0)).exp());
        finite += 1;
        sum += standard_deviation;
        minimum = minimum.min(standard_deviation);
        maximum = maximum.max(standard_deviation);
    }
    json!({
        "finite_dimensions": finite,
        "mean_stddev": if finite == 0 { 0.0 } else { sum / finite as f64 },
        "min_stddev": if finite == 0 { 0.0 } else { minimum },
        "max_stddev": if finite == 0 { 0.0 } else { maximum },
    })
}
