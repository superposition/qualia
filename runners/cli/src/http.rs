//! JSON-over-HTTP plumbing.
//!
//! Requests are made with the platform `curl` rather than a linked HTTP client:
//! the operator CLI must reach a self-signed loopback endpoint exactly the way
//! the rest of the stack's tooling does, and curl already speaks that contract
//! without pulling a TLS stack into the binary.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::process::Command;

/// Delimiter curl appends after the body, carrying the status code out of band.
const STATUS_MARKER: &str = "__qualia_http_code__:";

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
    let mut command = Command::new(curl_binary());
    command.args([
        "-ksS",
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
    if !(200..300).contains(&status) {
        return Err(format!("http {status}: {}", error_text(body.trim())));
    }
    Ok(body)
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
    let output = Command::new(curl_binary())
        .args(["-ks", url])
        .output()
        .map_err(|err| format!("curl failed: {err}"))?;
    if !output.status.success() {
        return Err(format!("curl returned {}", output.status));
    }
    serde_json::from_slice(&output.stdout).map_err(|err| err.to_string())
}
