//! Camera-only commands. This client has no wheel, pilot or mission operation.
use std::{io::Read, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::Duration};
use egui::{Color32, Ui};
use serde_json::{json, Value};
use super::source::{now_ms, Data};

#[derive(Default)]
struct State {
    status: Value,
    received_ms: u64,
    busy: bool,
    response: Option<Value>,
    error: Option<String>,
    poll_error: Option<String>,
}

pub struct GimbalView {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    endpoint: String,
    token: Option<Arc<String>>,
    pan: f64,
    tilt: f64,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5)).redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| e.to_string())
}

fn response(response: reqwest::blocking::Response) -> Result<Value, String> {
    let status = response.status();
    let mut bytes = Vec::new();
    response.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 { return Err("Camera reply exceeds 64 KiB".into()); }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid camera reply".to_string())?;
    if !status.is_success() || value["ok"] != true {
        return Err(format!("Camera command HTTP {}: {}", status.as_u16(), value["error"].as_str().unwrap_or("request refused")));
    }
    Ok(value)
}

impl GimbalView {
    pub fn new(robot: &str) -> Self {
        let endpoint = format!("{}/camera/aim", robot.trim_end_matches('/'));
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(&state); let stopping = Arc::clone(&stop); let url = endpoint.clone();
        std::thread::spawn(move || {
            let http = match client() { Ok(client) => client, Err(error) => { shared.lock().unwrap().error=Some(error); return; } };
            while !stopping.load(Ordering::Acquire) {
                let result = http.get(&url).send().map_err(|e| e.to_string()).and_then(response);
                let mut state = shared.lock().unwrap();
                match result {
                    Ok(value) => { state.status=value; state.received_ms=now_ms(); state.poll_error=None; }
                    Err(error) => state.poll_error=Some(error),
                }
                drop(state);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        let token = std::env::var("QUALIA_CAMERA_AIM_TOKEN").ok().filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_whitespace)).map(Arc::new);
        Self { state, stop, endpoint, token, pan:0.0, tilt:0.0 }
    }

    fn send(&self, pan: f64, tilt: f64) {
        if !pan.is_finite() || !tilt.is_finite() || !(-45.0..=45.0).contains(&pan) || !(-20.0..=30.0).contains(&tilt) { return; }
        let Some(token) = self.token.clone() else { return; };
        {
            let mut state = self.state.lock().unwrap();
            if state.busy { return; }
            state.busy=true; state.error=None;
        }
        let shared = Arc::clone(&self.state); let url = self.endpoint.clone();
        std::thread::spawn(move || {
            let result = client().and_then(|http| http.post(url).bearer_auth(token.as_str())
                .json(&json!({"pan_deg":pan,"tilt_deg":tilt,"speed":8,"accel":4,"approval":true}))
                .send().map_err(|e|e.to_string()).and_then(response));
            let mut state=shared.lock().unwrap(); state.busy=false;
            match result {
                Ok(value) => state.response=Some(value),
                Err(error) => state.error=Some(error.replace(token.as_str(),"[redacted]")),
            }
        });
    }

    pub fn render(&mut self, ui: &mut Ui, _data: &Data) {
        ui.strong("Camera gimbal");
        ui.label("Head control only · wheels remain locked");
        let state=self.state.lock().unwrap();
        let fresh=state.received_ms > 0 && now_ms().saturating_sub(state.received_ms) <= 3000;
        let authority=&state.status["gimbal"]["dedicated_authority"];
        let enabled=fresh && authority["enabled"] == true && authority["token_configured"] == true && self.token.is_some() && !state.busy;
        let last=state.status["gimbal"]["pose"].clone();
        let busy=state.busy; let error=state.error.clone().or_else(||state.poll_error.clone()); let outcome=state.response.clone();
        drop(state);
        ui.add_enabled_ui(enabled, |ui| {
            ui.add(egui::Slider::new(&mut self.pan,-45.0..=45.0).text("Pan °"));
            ui.add(egui::Slider::new(&mut self.tilt,-20.0..=30.0).text("Tilt °"));
            ui.horizontal(|ui| {
                if ui.button("Aim camera").clicked() { self.send(self.pan,self.tilt); }
                if ui.button("Recenter").clicked() { self.pan=0.0; self.tilt=0.0; self.send(0.0,0.0); }
            });
            ui.horizontal(|ui| {
                for (label,pan,tilt) in [("Look left",-10.0,0.0),("Look right",10.0,0.0),("Look up",0.0,10.0),("Look down",0.0,-10.0)] {
                    if ui.button(label).clicked() { self.pan=pan;self.tilt=tilt;self.send(pan,tilt); }
                }
            });
        });
        if busy { ui.label("Sending camera command…"); }
        else if !enabled { ui.colored_label(Color32::YELLOW,"Waiting for current camera-only authority"); }
        if let Some(value)=outcome {
            ui.label(format!("Last requested: pan {}°, tilt {}°",value["pan_deg"],value["tilt_deg"]));
            ui.label(format!("Serial receipt: {}",value["serial_receipt"]["controller_sequence"]));
        } else if !last.is_null() { ui.label(format!("Last commanded pose: {last}")); }
        ui.small("Angles are commanded positions. Physical head-position feedback is unavailable.");
        if error.is_some() {
            egui::CollapsingHeader::new("Camera command error").show(ui,|ui| { ui.colored_label(Color32::YELLOW,error.unwrap()); });
        }
    }
}

impl Drop for GimbalView { fn drop(&mut self) { self.stop.store(true,Ordering::Release); } }
