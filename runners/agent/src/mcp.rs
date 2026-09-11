//! The model-facing surface: MCP over HTTP, and the JEPA studio status.
//!
//! The tool manifest is the reference's: the names, safety classes and input
//! schemas a model discovers here are the ones it discovers from the reference,
//! so an external agent's tool selection does not change with this build. A call
//! is answered with the name of the subsystem that has not landed, rather than
//! with a result the process cannot substantiate.

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
    match arena_projection(&state) {
        Ok(arena) => Json(json!({
            "ok": true,
            "transport": "streamable-http",
            "server": "qualia-agent",
            "protocol_version": MCP_PROTOCOL_VERSION,
            "tool_count": tool_descriptors().len(),
            "world_model": "sync.v1 + world.model.v1",
            "arena": arena,
        }))
        .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

/// The sync.v1-projected arena and Studio state `/mcp/status` nests under
/// `arena`. It is the sync arena runtime's projection, read out of the session
/// store; this build carries neither, so the route reports what the reference
/// reports when its store cannot be opened. The projection's fields arrive with
/// the runtime that owns them.
fn arena_projection(_state: &AppState) -> Result<Value, String> {
    Err("session store unavailable".to_string())
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
                    "instructions": "Qualia is the shared superposed world model. Read arena_observe/world_query, submit typed intent or world proposals, and let Leash retain all physical safety authority.",
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
///
/// Every tool in this manifest reads a subsystem this build does not carry, so
/// the call names the missing subsystem instead of answering with a result the
/// process cannot substantiate. A name outside the manifest stays an error.
async fn call_tool(_state: &AppState, name: &str, _args: Value) -> Result<Value, String> {
    if !tool_manifest().iter().any(|entry| entry.0 == name) {
        return Err(format!("unknown tool: {name}"));
    }
    Err(format!(
        "{name} is not available in this build: {} has not landed",
        tool_subsystem(name)
    ))
}

/// The subsystem an advertised tool reads, for the answer a call gets here.
fn tool_subsystem(name: &str) -> &'static str {
    if name.starts_with("belief_") {
        "the cognition runtime"
    } else if name.starts_with("embodiment_") || name.starts_with("guard_") {
        "the Leash operator client"
    } else {
        "the sync arena runtime"
    }
}

/// The reference's tool manifest: name, description, safety class, input schema.
///
/// Names, safety classes and schemas are the interface a model selects against.
/// The descriptions are the reference's own, character for character, so a
/// model reading them here reads what it reads there.
fn tool_manifest() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "arena_status",
            "Read the sync.v1-projected arena and Studio state",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "arena_observe",
            "Read one correlated Leash observation plus Qualia SHM, GPU/planner state, and typed ontology",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "multimodal_observe",
            "Read one correlated camera frame, spatial voxel/world and route-plan state, plus L3-L6 belief sketches for multimodal reasoning",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "arena_start_session",
            "Start a Guard arena activity in the shared Qualia sync state",
            "state-change",
            object_schema(&[("label", "string", false)]),
        ),
        (
            "arena_stop_session",
            "Stop Guard through Leash and close the shared arena activity",
            "physical-stop",
            object_schema(&[("reason", "string", false)]),
        ),
        (
            "arena_submit_intent",
            "Submit a bounded LLM intent into the shared directive register while fast GPU lanes continue",
            "state-change",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "intent": { "type": "string" },
                    "constraints": { "type": "array" },
                    "evidence_refs": { "type": "array" },
                },
                "required": ["intent"],
            }),
        ),
        (
            "studio_focus",
            "Select the native Studio projection the human should inspect",
            "state-change",
            object_schema(&[("focus", "string", true)]),
        ),
        (
            "studio_control",
            "Select a native Studio entity, workspace, and surface; the app acknowledges the applied command",
            "state-change",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "entity_id": { "type": "string" },
                    "workspace": { "type": "string", "enum": ["history", "review", "mission", "world"] },
                    "surface": { "type": "string", "enum": ["spatial", "camera", "calibration", "graph", "plan", "agent"] },
                },
            }),
        ),
        (
            "world_query",
            "Query typed sync namespaces including proposals, canonical objects, regions, factors, and beliefs",
            "observe-only",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "namespace": { "type": "string" },
                    "key": { "type": "string" },
                    "limit": { "type": "integer" },
                },
            }),
        ),
        (
            "world_propose",
            "Add a weighted world.model.v1 proposal without bypassing curator promotion",
            "state-change",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "proposal": { "type": "object" } },
                "required": ["proposal"],
            }),
        ),
        (
            "belief_status",
            "Read continuous Qualia L3-L6 predictive belief state and Leash boundary freshness",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "belief_query",
            "Query L3-L6 summaries and active evidence-backed semantic priors",
            "observe-only",
            object_schema(&[("query", "string", false)]),
        ),
        (
            "belief_sketch",
            "Read compact activation and weight-matrix sketches for canonical and shadow L3-L6 beliefs",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "belief_shadow_status",
            "Read the operator-unlocked shadow experiment, patches, and prediction-error comparison",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "belief_shadow_patch",
            "Apply one bounded delta to an operator-unlocked shadow L3-L6 weight coordinate; canonical weights and motors are never changed",
            "shadow-only",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "layer": { "type": "integer", "minimum": 3, "maximum": 6 },
                    "row": { "type": "integer", "minimum": 0, "maximum": 1023 },
                    "column": { "type": "integer", "minimum": 0, "maximum": 1023 },
                    "delta": { "type": "number", "minimum": -0.25, "maximum": 0.25 },
                    "reason": { "type": "string" },
                },
                "required": ["layer", "row", "column", "delta"],
            }),
        ),
        (
            "belief_shadow_publish",
            "Publish the bounded shadow patch set as an append-only CRDT cognition candidate; every device evaluates it against local weights before any local promotion",
            "evidence-gated-learning",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "evidence_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": 64,
                    },
                },
            }),
        ),
        (
            "belief_propose_feedback",
            "Propose one typed interpretation against the short-lived receipt returned by multimodal_observe; fresh physical evidence gates a bounded canonical L3-L6 learning update and Leash retains all motor authority",
            "evidence-gated-learning",
            model_feedback_schema(),
        ),
        (
            "belief_propose_prior",
            "Propose a typed, JEPA-evidence-bound Hermes/Kimi semantic view at L6; accepted priors influence canonical learning only while fresh physical evidence is present and never gain planner or motor authority",
            "state-change",
            semantic_prior_schema(),
        ),
        (
            "belief_withdraw_prior",
            "Withdraw a semantic prior from L6 by id",
            "state-change",
            object_schema(&[("prior_id", "string", true)]),
        ),
        (
            "embodiment_health",
            "Read the selected Leash embodiment adapter health and safety gates",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "embodiment_observe",
            "Read telemetry from the selected Leash embodiment adapter",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "embodiment_invoke_capability",
            "Invoke a typed Leash adapter capability; physical actions retain every Leash safety gate",
            "leash-gated",
            json!({
                "type": "object",
                "additionalProperties": true,
                "properties": { "capability": { "type": "string" } },
                "required": ["capability"],
            }),
        ),
        (
            "embodiment_stop",
            "Send a non-latching zero-speed stop through the selected Leash adapter",
            "physical-stop",
            object_schema(&[]),
        ),
        (
            "embodiment_estop",
            "Latch e-stop through the selected Leash adapter",
            "physical-stop",
            object_schema(&[]),
        ),
        // Compatibility aliases for existing Guard operators: new clients use
        // the adapter-neutral embodiment_* names above.
        (
            "guard_health",
            "Read Guard health and Leash safety gates",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "guard_observe",
            "Read Guard telemetry and sensor state through Leash",
            "observe-only",
            object_schema(&[]),
        ),
        (
            "guard_invoke_capability",
            "Invoke a Leash capability; physical actions still require authorization, deadman, collision, and e-stop gates",
            "leash-gated",
            json!({
                "type": "object",
                "additionalProperties": true,
                "properties": { "capability": { "type": "string" } },
                "required": ["capability"],
            }),
        ),
        (
            "guard_stop",
            "Send a non-latching zero-speed stop through Leash",
            "physical-stop",
            object_schema(&[]),
        ),
        (
            "guard_estop",
            "Latch Guard emergency stop through Leash",
            "physical-stop",
            object_schema(&[]),
        ),
    ]
}

/// The tool list, with the safety class the studio renders.
pub fn tool_descriptors() -> Vec<Value> {
    tool_manifest()
        .into_iter()
        .map(|(name, description, safety, input_schema)| {
            tool(name, description, safety, input_schema)
        })
        .collect()
}

/// One typed interpretation, as `belief_propose_feedback` carries it.
fn proposition_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "class": { "enum": ["obstacle", "traversable", "object_identity", "person", "goal_preference", "motion", "location", "uncertainty"] },
            "subject": { "type": "string", "minLength": 1, "maxLength": 256 },
            "relation": { "enum": ["is", "near", "left_of", "right_of", "ahead_of", "behind", "blocks", "supports"] },
            "object": { "type": ["string", "null"] },
            "polarity": { "enum": ["supports", "contradicts"] },
        },
        "required": ["class", "subject", "relation", "polarity"],
    })
}

/// The JEPA-evidence reference a proposed semantic prior must carry.
fn evidence_ref_schema() -> Value {
    let id = || json!({ "type": "string", "minLength": 1, "maxLength": 256 });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "producer_epoch": { "type": "integer", "minimum": 1 },
            "runner_epoch": { "type": "integer", "minimum": 1 },
            "inference_seq": { "type": "integer", "minimum": 1 },
            "timestamp_ms": { "type": "integer", "minimum": 1 },
            "camera_seq": { "type": "integer", "minimum": 0 },
            "lidar_seq": { "type": "integer", "minimum": 0 },
            "pose_seq": { "type": "integer", "minimum": 0 },
            "action_seq": { "type": "integer", "minimum": 0 },
            "physical_model_id": id(),
            "checkpoint_id": id(),
        },
        "required": [
            "producer_epoch",
            "runner_epoch",
            "inference_seq",
            "timestamp_ms",
            "camera_seq",
            "lidar_seq",
            "pose_seq",
            "action_seq",
            "physical_model_id",
            "checkpoint_id",
        ],
    })
}

fn model_feedback_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "observation_id": { "type": "string", "minLength": 1, "maxLength": 256 },
            "proposition": proposition_schema(),
            "confidence": { "type": "number", "exclusiveMinimum": 0, "maximum": 1 },
            "model_id": { "type": "string" },
            "model_version": { "type": "string" },
            "request_id": { "type": "string" },
            "runtime": { "enum": ["hermes", "kimi", "local_slm"] },
        },
        "required": ["observation_id", "proposition", "confidence"],
    })
}

fn semantic_prior_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "prior": prior_schema() },
        "required": ["prior"],
    })
}

/// The `prior` object: the typed L6 view `belief_propose_prior` submits.
fn prior_schema() -> Value {
    let id = || json!({ "type": "string", "minLength": 1, "maxLength": 256 });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "schema_version": { "const": "qualia.semantic-prior.v2" },
            "prior_id": id(),
            "proposition": proposition_schema(),
            "evidence_refs": {
                "type": "array",
                "minItems": 1,
                "maxItems": 8,
                "items": evidence_ref_schema(),
            },
            "confidence": { "type": "number", "exclusiveMinimum": 0, "maximum": 1 },
            "created_at_ms": { "type": "integer", "minimum": 1 },
            "expires_at_ms": { "type": "integer", "minimum": 1 },
            "target_layer": { "const": 6 },
            "projection_version": { "const": "qualia.l6-to-l3.fixed.v1" },
            "source": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "runtime": { "enum": ["hermes", "kimi"] },
                    "model_id": id(),
                    "model_version": id(),
                    "request_id": id(),
                },
                "required": ["runtime", "model_id", "model_version", "request_id"],
            },
            "goal_preference": {
                "type": ["object", "null"],
                "properties": {
                    "canonical_goal_id": id(),
                    "weight": { "type": "number", "minimum": 0, "maximum": 1 },
                },
                "required": ["canonical_goal_id", "weight"],
                "additionalProperties": false,
            },
        },
        "required": ["schema_version", "prior_id", "proposition", "evidence_refs", "confidence", "created_at_ms", "expires_at_ms", "target_layer", "projection_version", "source", "goal_preference"],
    })
}

fn tool(name: &str, description: &str, safety: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "safety": safety,
        "inputSchema": input_schema,
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
