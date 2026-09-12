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
}

pub struct Monitor {
    pub data: Arc<Mutex<Data>>,
    stop: Arc<AtomicBool>,
    agent_url: String,
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

impl Monitor {
    pub fn new(robot_url: String, agent_url: String) -> Self {
        let data = Arc::new(Mutex::new(Data::default()));
        let stop = Arc::new(AtomicBool::new(false));
        // Sensor and camera cadence is independent of optional host services.
        for lane in 0..4 {
            let shared = Arc::clone(&data);
            let stopping = Arc::clone(&stop);
            let robot = robot_url.clone();
            let agent = agent_url.clone();
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
                        let paths: &[(&str, &str, &str)] = if lane == 0 {
                            &[("sensors", &robot, "/sensors")]
                        } else if lane == 2 {
                            &[
                                ("health", &robot, "/health"),
                                ("cognition", &robot, "/cognition/status"),
                                ("spatial", &robot, "/telemetry/compact"),
                                ("actions", &robot, "/action-evidence?limit=1"),
                                ("robot-agent", &robot, "/agent/state"),
                            ]
                        } else {
                            &[
                                ("host-health", &agent, "/health/ready"),
                                ("host-world", &agent, "/world/snapshot"),
                                ("host-belief", &agent, "/belief/status"),
                                ("host-perception", &agent, "/perception/status"),
                                ("host-arena", &agent, "/arena/status"),
                                ("host-sketch", &agent, "/belief/sketch"),
                                ("host-costmap", &agent, "/planner/costmap"),
                                ("host-explore", &agent, "/explore/status"),
                                ("host-entities", &agent, "/entities"),
                            ]
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
                    std::thread::sleep(Duration::from_millis(if lane >= 2 { 1000 } else { 200 }));
                }
            });
        }
        if let Ok(path) = std::env::var("QUALIA_FLY_STATUS") {
            let shared = Arc::clone(&data);
            let stopping = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    let result = std::fs::File::open(&path).map_err(|e| e.to_string()).and_then(|file| {
                        let mut bytes = Vec::new();
                        file.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                        if bytes.len() > 65536 { return Err("Fly status exceeds 64 KiB".into()); }
                        let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        if value["published_ms"].as_u64().is_none() { return Err("Fly status has no publication timestamp".into()); }
                        Ok(value)
                    });
                    let mut guard = shared.lock().unwrap();
                    let reading = guard.sources.entry("fly-inputs".into()).or_default();
                    match result {
                        Ok(value) => { reading.received_ms = value["published_ms"].as_u64().unwrap(); reading.value = value; reading.error = None; }
                        Err(error) => reading.error = Some(error),
                    }
                    drop(guard);
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        }
        Self { data, stop, agent_url }
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
        let url = format!("{}/mcp/call", self.agent_url.trim_end_matches('/'));
        std::thread::spawn(move || {
            let mut builder = reqwest::blocking::Client::builder().connect_timeout(Duration::from_millis(700)).timeout(Duration::from_secs(20));
            if let Some(cert) = crate::client::agent_certificate() { builder = builder.add_root_certificate(cert); }
            let result = builder.build().map_err(|e| e.to_string()).and_then(|client| {
                let response = client.post(url).json(&serde_json::json!({"tool":"multimodal_observe","args":{}}))
                    .send().and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                response.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > 1024 * 1024 { return Err("Observation response exceeds 1 MiB".into()); }
                serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())
            });
            let mut guard = data.lock().unwrap();
            guard.observing = false;
            guard.observation = Some(match result {
                Ok(value) => Reading {value, received_ms: now_ms(), error: None},
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
    fn retained_error_readings_age_out() {
        let mut reading = Reading {received_ms: 1000, ..Reading::default()};
        assert!(reading.fresh_at(1100, 500));
        assert!(!reading.fresh_at(2000, 500));
        reading.error = Some("offline".into());
        assert!(!reading.fresh_at(1100, 500));
    }
}
