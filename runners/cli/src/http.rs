//! JSON-over-HTTP plumbing.
//!
//! Requests are made with the platform `curl` rather than a linked HTTP client:
//! the operator CLI must reach a self-signed loopback endpoint exactly the way
//! the rest of the stack's tooling does, and curl already speaks that contract
//! without pulling a TLS stack into the binary.
//!
//! Every request is bounded: curl gets a connection timeout and a total
//! timeout, so no server — silent, hung or trickling a body — can hold the
//! operator's terminal. The configured URL is tried first; if the transport
//! never completes an exchange there, the same host is tried on the twin
//! scheme, because a stack that serves cleartext on a port the CLI guessed was
//! TLS must still be reachable. Only the configured host is ever contacted.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use std::sync::Mutex;

/// Delimiter curl appends after the body, carrying the status code out of band.
const STATUS_MARKER: &str = "__qualia_http_code__:";

/// Seconds curl may spend on the connection phase (DNS, TCP, TLS handshake).
const CONNECT_TIMEOUT_SECS: u32 = 2;

/// Seconds the whole exchange may take, connection included. A server that
/// accepts and then says nothing fails on this bound, not on the operator's
/// patience.
const TOTAL_TIMEOUT_SECS: u32 = 5;

pub fn get_json<T>(agent_url: &str, path: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let body = request("GET", &format!("{agent_url}{path}"), None)?;
    serde_json::from_str(&body).map_err(|err| err.to_string())
}

pub fn post_json<T, S>(agent_url: &str, path: &str, payload: &S) -> Result<T, String>
where
    T: DeserializeOwned,
    S: Serialize,
{
    let payload = serde_json::to_string(payload).map_err(|err| err.to_string())?;
    let body = request("POST", &format!("{agent_url}{path}"), Some(&payload))?;
    serde_json::from_str(&body).map_err(|err| err.to_string())
}

fn request(method: &str, url: &str, payload: Option<&str>) -> Result<String, String> {
    let (status, body) = fetch(method, url, payload)?;
    if !(200..300).contains(&status) {
        return Err(format!("http {status}: {}", error_text(body.trim())));
    }
    Ok(body)
}

/// Perform one exchange against `url`, resolving the scheme against the
/// configured origin. A transport failure moves on to the twin scheme; an
/// exchange the server answered — whatever its status — never does, because
/// the URL was right and only the status was not.
fn fetch(method: &str, url: &str, payload: Option<&str>) -> Result<(u16, String), String> {
    let (origin, path) = split_origin(url);
    if let Some(resolved) = resolved_origin(&origin) {
        return attempt(method, &format!("{resolved}{path}"), payload);
    }

    let mut last_error = None;
    for candidate in candidate_origins(&origin) {
        match attempt(method, &format!("{candidate}{path}"), payload) {
            Ok(answer) => {
                remember_origin(&origin, &candidate);
                if candidate != origin {
                    eprintln!("qualia: agent url {candidate} (fallback from {origin})");
                }
                return Ok(answer);
            }
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or_else(|| format!("no candidate URL for {origin}")))
}

/// One bounded curl exchange. `Ok((status, body))` means an HTTP response
/// arrived; `Err` means curl never completed an exchange — refused, unroutable,
/// the wrong protocol or over the timeout — so another candidate may still.
fn attempt(method: &str, url: &str, payload: Option<&str>) -> Result<(u16, String), String> {
    let connect_timeout = CONNECT_TIMEOUT_SECS.to_string();
    let total_timeout = TOTAL_TIMEOUT_SECS.to_string();
    let mut command = Command::new(curl_binary());
    command.args([
        "-ksS",
        "--connect-timeout",
        &connect_timeout,
        "--max-time",
        &total_timeout,
        "-X",
        method,
        "-w",
        &format!("{STATUS_MARKER}%{{http_code}}"),
        url,
    ]);
    if let Some(payload) = payload {
        command.args([
            "-H",
            "content-type: application/json",
            "--data-raw",
            payload,
        ]);
    }

    let output = command
        .output()
        .map_err(|err| format!("curl failed: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("curl returned {}", output.status)
        } else {
            format!("curl returned {}: {stderr}", output.status)
        });
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let Some(index) = text.rfind(STATUS_MARKER) else {
        return Err("curl response did not contain an HTTP status trailer".to_string());
    };
    let body = text[..index].to_string();
    let status = text[index + STATUS_MARKER.len()..]
        .trim()
        .parse::<u16>()
        .map_err(|err| format!("failed to parse HTTP status: {err}"))?;
    Ok((status, body))
}

/// Split a request URL into `(origin, path)`, where the origin is the scheme
/// and authority alone.
fn split_origin(url: &str) -> (String, String) {
    let Some(scheme_end) = url.find("://") else {
        return (url.to_string(), String::new());
    };
    let authority = scheme_end + 3;
    match url[authority..].find('/') {
        Some(offset) => (
            url[..authority + offset].to_string(),
            url[authority + offset..].to_string(),
        ),
        None => (url.to_string(), String::new()),
    }
}

/// The origins to try for a configured one, in order: what the operator asked
/// for, then the same host on the other scheme. No other host or port is ever
/// guessed at.
fn candidate_origins(origin: &str) -> Vec<String> {
    if let Some(rest) = origin.strip_prefix("https://") {
        vec![origin.to_string(), format!("http://{rest}")]
    } else if let Some(rest) = origin.strip_prefix("http://") {
        vec![origin.to_string(), format!("https://{rest}")]
    } else {
        vec![format!("http://{origin}"), format!("https://{origin}")]
    }
}

/// The origin that answered last, remembered for the rest of the process: a
/// command that makes several requests resolves the scheme once, so a wrong
/// guess costs one timeout per run rather than one per request.
static RESOLVED_ORIGIN: Mutex<Option<(String, String)>> = Mutex::new(None);

fn resolved_origin(origin: &str) -> Option<String> {
    let slot = RESOLVED_ORIGIN.lock().ok()?;
    match slot.as_ref() {
        Some((configured, resolved)) if configured == origin => Some(resolved.clone()),
        _ => None,
    }
}

fn remember_origin(configured: &str, resolved: &str) {
    if let Ok(mut slot) = RESOLVED_ORIGIN.lock() {
        *slot = Some((configured.to_string(), resolved.to_string()));
    }
}

/// Prefer the server's structured `error` field; fall back to raw text.
fn error_text(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("error").and_then(Value::as_str).map(str::to_string))
        .or_else(|| (!body.is_empty()).then(|| body.to_string()))
        .unwrap_or_else(|| "request failed without an error body".to_string())
}

fn curl_binary() -> &'static str {
    if cfg!(windows) {
        "curl.exe"
    } else {
        "curl"
    }
}

/// The agent's health document, as `qualia health` reports it.
#[derive(Debug, serde::Deserialize)]
pub struct StackHealth {
    pub service_instance: String,
    pub status_type: String,
    pub status: String,
    pub healthy: bool,
    pub ready: bool,
    pub integration: IntegrationHealth,
    pub compute: ComputeHealth,
}

#[derive(Debug, serde::Deserialize)]
pub struct IntegrationHealth {
    pub ros_connected: bool,
    pub planner_ready: bool,
    pub pose: InputHealth,
    pub lidar: InputHealth,
}

#[derive(Debug, serde::Deserialize)]
pub struct InputHealth {
    pub fresh: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct ComputeHealth {
    pub healthy: bool,
    pub planner_algorithms: Vec<String>,
    pub cuda: crate::world::CudaInfo,
}

/// The planner status document, as `qualia planner` reports it.
#[derive(Debug, serde::Deserialize)]
pub struct PlannerStatus {
    pub service_instance: String,
    pub status_type: String,
    pub status: String,
    pub planner_ready: bool,
    pub planner_algorithms: Vec<String>,
    pub belief_features: Option<BeliefFeatures>,
    pub last_result: Option<LastResult>,
}

#[derive(Debug, serde::Deserialize)]
pub struct BeliefFeatures {
    pub directive_present: bool,
    pub activity_present: bool,
    pub scene_embedding_l2: f32,
    pub scene_embedding_peak: f32,
    pub llm_age_ms: u64,
    pub vision_age_ms: u64,
}

#[derive(Debug, serde::Deserialize)]
pub struct LastResult {
    pub request_id: String,
    pub status: String,
    pub algorithm: Option<String>,
    pub path_len: usize,
    pub planning_ms: Option<f32>,
    pub reachable: Option<bool>,
}

/// Read the planner status document.
pub fn fetch_planner_status(agent_url: &str) -> Result<PlannerStatus, String> {
    curl_json(&format!("{agent_url}/planner/status"))
}

/// Read the aggregate readiness document.
pub fn fetch_stack_health(agent_url: &str) -> Result<StackHealth, String> {
    curl_json(&format!("{agent_url}/health/ready"))
}

fn curl_json<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    // The readiness documents carry the verdict in their body, and a stack
    // that is not ready may answer 503 with that body, so the status is not
    // consulted here.
    let (_status, body) = fetch("GET", url, None)?;
    serde_json::from_str(&body).map_err(|err| err.to_string())
}
