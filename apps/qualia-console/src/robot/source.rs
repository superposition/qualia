use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use serde_json::Value;

#[derive(Clone, Default)]
pub struct Reading {
    pub value: Value,
    pub received_ms: u64,
    pub error: Option<String>,
}

impl Reading {
    pub fn fresh_at(&self, now: u64, deadline: u64) -> bool {
        self.received_ms > 0 && self.received_ms <= now.saturating_add(100) && self.error.is_none() && now.saturating_sub(self.received_ms) <= deadline
    }
}

#[derive(Clone, Default)]
pub struct Data {
    pub sources: BTreeMap<String, Reading>,
    pub camera: Option<(u64, Arc<egui::ColorImage>)>,
    pub camera_error: Option<String>,
    pub observing: bool,
    pub observation: Option<Reading>,
    pub neuron_type: String,
}

pub struct Monitor {
    pub data: Arc<Mutex<Data>>,
    stop: Arc<AtomicBool>,
    robot_url: String,
}

pub fn now_ms() -> u64 { crate::now_ns() / 1_000_000 }

fn bytes(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client.get(url).send()
        .map_err(|error| format!("GET {url}: {error}"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 { return Err(format!("{url}: response exceeds 4 MiB")); }
    if !status.is_success() {
        let detail = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v|
            v["subsystem"].as_str().map(|s| format!("subsystem unavailable: {s}")))
            .unwrap_or_default();
        return Err(format!("GET {url}: HTTP {status} {detail}"));
    }
    Ok(bytes)
}

fn validate(name: &str, value: Value) -> Result<Value, String> {
    let valid = match name {
        "sensors" => value["ok"] == true && value["sensors"].is_object(),
        "health" => value["ok"] == true && value["role"].is_string(),
        "cognition" | "host-belief" => value["ok"] == true && value["layers"].is_array(),
        "spatial" => value["sensors"].is_object() && value.get("odometry_pose").is_some(),
        "actions" => value["schema_version"] == "leash.applied-action-page.v1" && value["entries"].is_array(),
        "lighting" => super::lighting::valid_status(&value),
        "pretrained" => super::pretrained::valid_status(&value),
        "neurons" => super::neurons::valid_status(&value),
        "wheel-shadow" => super::wheels::valid_status(&value),
        "model-matrices" => super::matrices::valid_status(&value),
        "wheel-transport" => super::wheels::valid_transport(&value),
        "host-health" => value["healthy"].is_boolean() && value["ready"].is_boolean(),
        "host-world" => value["schema_version"] == "world.v1" && value["nav"].is_object(),
        "host-sketch" => value["schema_version"] == "qualia.belief-sketch.v1" && value["canonical"].is_array(),
        "host-costmap" => value["schema_version"] == "qualia.planner-costmap.v1" && value["lidar_points"].is_array(),
        "host-arena" => value["schema_version"] == "qualia.arena-state.v1" && value["events"].is_array(),
        "host-perception" => value["camera"].is_object() && value["vslam"].is_object(),
        _ => value.is_object(),
    };
    if valid { Ok(value) } else { Err(format!("{name}: response does not match the published source contract")) }
}

fn mcp_error(value: &Value) -> Option<String> {
    if !value.is_object() || value["jsonrpc"] != "2.0" || value["id"] != 1 {
        Some("MCP reply has an invalid envelope".into())
    } else if value.get("error").is_some_and(|e| !e.is_null()) {
        Some(format!("MCP observe refused: {}", value["error"]["message"].as_str().unwrap_or("JSON-RPC error")))
    } else if !value["result"].is_object() || !value["result"]["content"].is_array() {
        Some("MCP reply has no tool result".into())
    } else if value["result"]["isError"] == true {
        Some("MCP observe returned a tool error; inspect the actual result".into())
    } else { None }
}

impl Monitor {
    pub fn new(robot_url: String, agent_url: String) -> Self {
        let data = Arc::new(Mutex::new(Data { neuron_type: "T4a".into(), ..Data::default() }));
        let stop = Arc::new(AtomicBool::new(false));
        // Sensor and camera cadence is independent of optional host services.
        for lane in 0..11 {
            let shared = Arc::clone(&data);
            let stopping = Arc::clone(&stop);
            let robot = robot_url.clone();
            let agent = agent_url.clone();
            let pretrained = super::pretrained::endpoint(&robot_url);
            let wheels = super::wheels::endpoint(&robot_url);
            let transport = super::wheels::transport_endpoint();
            std::thread::spawn(move || {
                let mut builder = reqwest::blocking::Client::builder()
                    .connect_timeout(Duration::from_millis(700))
                    .timeout(Duration::from_millis(1500));
                if let Some(cert) = crate::client::agent_certificate() {
                    builder = builder.add_root_certificate(cert);
                }
                let client = match builder.build() {
                    Ok(client) => client,
                    Err(error) => {
                        shared.lock().unwrap().sources.insert(format!("client-{lane}"), Reading {
                            error: Some(error.to_string()), ..Reading::default()
                        });
                        return;
                    }
                };
                while !stopping.load(Ordering::Acquire) {
                    if lane == 1 {
                        let result = bytes(&client, &format!("{robot}/camera/snapshot"))
                            .and_then(|bytes| {
                                let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                                    .with_guessed_format().map_err(|e| e.to_string())?;
                                let mut limits = image::Limits::default();
                                limits.max_image_width = Some(4096);
                                limits.max_image_height = Some(4096);
                                reader.limits(limits);
                                let rgba = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
                                Ok(egui::ColorImage::from_rgba_unmultiplied(
                                    [rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
                            });
                        let mut guard = shared.lock().unwrap();
                        match result {
                            Ok(image) => { guard.camera = Some((now_ms(), Arc::new(image))); guard.camera_error = None; }
                            Err(error) => guard.camera_error = Some(error),
                        }
                    } else {
                        let neuron_type = shared.lock().unwrap().neuron_type.clone();
                        let neuron_url = reqwest::Url::parse(&pretrained).ok().map(|mut url| {
                            url.set_path("/cells"); url.set_query(None);
                            url.query_pairs_mut().append_pair("type", &neuron_type);
                            url.to_string()
                        }).unwrap_or_else(|| "invalid neuron producer URL".into());
                        let matrix_url = reqwest::Url::parse(&neuron_url).ok().map(|mut url| {
                            url.set_path("/matrices"); url.to_string()
                        }).unwrap_or_else(|| "invalid model matrix URL".into());
                        let paths: &[(&str, &str, &str)] = if lane == 0 {
                            &[("sensors", &robot, "/sensors")]
                        } else if lane == 2 {
                            &[
                                ("health", &robot, "/health"),
                                ("lighting", &robot, "/camera/lights"),
                                ("spatial", &robot, "/telemetry/compact"),
                                ("actions", &robot, "/action-evidence?limit=1"),
                                ("robot-agent", &robot, "/agent/state"),
                            ]
                        } else if lane == 3 {
                            &[
                                ("host-health", &agent, "/health/ready"),
                                ("host-world", &agent, "/world/snapshot"),
                                ("host-perception", &agent, "/perception/status"),
                                ("host-arena", &agent, "/arena/status"),
                                ("host-costmap", &agent, "/planner/costmap"),
                                ("host-explore", &agent, "/explore/status"),
                                ("host-entities", &agent, "/entities"),
                            ]
                        } else if lane == 4 {
                            &[("pretrained", &pretrained, "")]
                        } else if lane == 5 {
                            &[("neurons", &neuron_url, "")]
                        } else if lane == 6 {
                            &[("wheel-shadow", &wheels, "")]
                        } else if lane == 7 {
                            &[("model-matrices", &matrix_url, "")]
                        } else if lane == 8 {
                            &[("cognition", &robot, "/cognition/status")]
                        } else if lane == 9 {
                            &[("host-belief", &agent, "/belief/status"), ("host-sketch", &agent, "/belief/sketch")]
                        } else {
                            &[("wheel-transport", &transport, "")]
                        };
                        for (name, base, path) in paths {
                            if stopping.load(Ordering::Acquire) { return; }
                            let result = bytes(&client, &format!("{base}{path}"))
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()))
                                .and_then(|value| {
                                    if *name != "actions" { return Ok(value); }
                                    // The API starts from the oldest retained entry by default.
                                    // Ask for a recent window using the published latest sequence.
                                    let latest = value["latest_sequence"].as_u64().ok_or("Action ledger omitted latest_sequence")?;
                                    bytes(&client, &format!("{base}/action-evidence?limit=16&after_sequence={}", latest.saturating_sub(16)))
                                        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
                                }).and_then(|value| validate(name, value));
                            let mut guard = shared.lock().unwrap();
                            let reading = guard.sources.entry((*name).to_owned()).or_default();
                            match result {
                                Ok(value) => { reading.value = value; reading.received_ms = now_ms(); reading.error = None; }
                                Err(error) => reading.error = Some(error),
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(if lane >= 4 { 250 } else if lane >= 2 { 1000 } else { 200 }));
                }
            });
        }
        for (environment, source_name) in [("QUALIA_FLY_STATUS", "fly-inputs"), ("QUALIA_PERCEPTION_STATUS", "perception-observation"), ("QUALIA_WHEEL_PROBE_STATUS", "wheel-probe")] {
        if let Ok(path) = std::env::var(environment) {
            let shared = Arc::clone(&data);
            let stopping = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    let result = std::fs::File::open(&path).map_err(|e| e.to_string()).and_then(|file| {
                        let mut bytes = Vec::new();
                        file.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                        if bytes.len() > 65536 { return Err("Runtime status exceeds 64 KiB".into()); }
                        let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        if source_name == "wheel-probe" {
                            if !super::wheels::valid_probe(&value) { return Err("Wheel probe has no valid dated integration result".into()); }
                        } else if value["published_ms"].as_u64().is_none() { return Err("Runtime status has no publication timestamp".into()); }
                        Ok(value)
                    });
                    let mut guard = shared.lock().unwrap();
                    let reading = guard.sources.entry(source_name.into()).or_default();
                    match result {
                        Ok(value) => { reading.received_ms = if source_name == "wheel-probe" { now_ms() } else { value["published_ms"].as_u64().unwrap() }; reading.value = value; reading.error = None; }
                        Err(error) => reading.error = Some(error),
                    }
                    drop(guard);
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        }
        }
        Self { data, stop, robot_url }
    }

    /// Only an observation tool is offered here. This cannot enqueue a drive,
    /// goal, explore, estop or authorization request.
    pub fn observe(&self) {
        {
            let mut guard = self.data.lock().unwrap();
            if guard.observing { return; }
            guard.observing = true;
        }
        let data = Arc::clone(&self.data);
        let url = format!("{}/mcp", self.robot_url.trim_end_matches('/'));
        std::thread::spawn(move || {
            let mut builder = reqwest::blocking::Client::builder().connect_timeout(Duration::from_millis(700)).timeout(Duration::from_secs(20));
            if let Some(cert) = crate::client::agent_certificate() { builder = builder.add_root_certificate(cert); }
            let result = builder.build().map_err(|e| e.to_string()).and_then(|client| {
                let response = client.post(url).json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"observe","arguments":{}}}))
                    .send().and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                response.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > 1024 * 1024 { return Err("Observation response exceeds 1 MiB".into()); }
                serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())
            });
            let mut guard = data.lock().unwrap();
            guard.observing = false;
            guard.observation = Some(match result {
                Ok(value) => Reading {error: mcp_error(&value), value, received_ms: now_ms()},
                Err(error) => Reading {error: Some(error), ..Reading::default()},
            });
        });
    }
}

impl Drop for Monitor {
    fn drop(&mut self) { self.stop.store(true, Ordering::Release); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_successful_http_body_from_the_wrong_endpoint_is_not_a_world_reading() {
        assert!(validate("host-world", serde_json::json!({"ok":true})).is_err());
        assert!(validate("host-world", serde_json::json!({"schema_version":"world.v1","nav":{}})).is_ok());
    }
    #[test]
    fn http_success_does_not_hide_mcp_refusal() {
        assert!(mcp_error(&serde_json::json!({"jsonrpc":"2.0","id":1,"error":{"message":"denied"}})).is_some());
        assert!(mcp_error(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"content":[],"isError":true}})).is_some());
        assert!(mcp_error(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"content":[]}})).is_none());
    }
    #[test]
    fn retained_error_readings_age_out() {
        let mut reading = Reading {received_ms: 1000, ..Reading::default()};
        assert!(reading.fresh_at(1100, 500));
        assert!(!reading.fresh_at(2000, 500));
        reading.error = Some("offline".into());
        assert!(!reading.fresh_at(1100, 500));
    }
}
