//! Explicit evidence checks for exploration. Missing observations earn no credit.
use egui::{Color32, Ui};
use super::{panels::text, source::{Data, now_ms}};

pub(super) fn source_fresh(value: &serde_json::Value, now: u64) -> bool {
    let input = &value["input"];
    value["state"] == "live" && input["schema_version"] == "qualia.sensor-conditioned-retina.v1"
        && input["run_id"] == value["run_id"] && value["run_id"].as_str().is_some_and(|v| !v.is_empty())
        && input["measured"].is_object() && input["model_stepped"] != false
        && ["imu_ts_ms", "lidar_ts_ms", "odometry_ts_ms"].iter().all(|key| stamp_fresh(&input["measured"][key], now))
        && ["received_ms", "camera_request_started_ms", "camera_received_ms"].iter().all(|key| stamp_fresh(&input[key], now))
}
fn stamp_fresh(stamp: &serde_json::Value, now: u64) -> bool {
    stamp.as_u64().is_some_and(|v| v > 0 && v <= now.saturating_add(100) && now.saturating_sub(v) <= 1500)
}

pub fn render(ui: &mut Ui, data: &Data) {
    let now = now_ms();
    ui.strong("Goal: explore and build a localized map");
    ui.colored_label(Color32::YELLOW, "Physical execution held: acknowledgement repair unconfirmed");
    let perception = data.sources.get("perception-observation")
        .filter(|r| r.fresh_at(now, 1500) && r.value["state"] == "live" && r.value["schema_version"] == "qualia.perception-observation.v1"
            && stamp_fresh(&r.value["camera_request_started_ms"], now) && stamp_fresh(&r.value["camera_received_ms"], now));
    let fly = data.sources.get("fly-inputs")
        .filter(|r| r.fresh_at(now, 1500) && r.value["state"] == "live");
    let camera = perception.and_then(|p| Some((p.value["luminance_mean"].as_f64()?, p.value["luminance_stddev"].as_f64()?)));
    let vision = camera.map(|(brightness, contrast)| (0.10..=0.90).contains(&brightness) && contrast >= 0.05);
    let sensors = fly.map(|r| source_fresh(&r.value, now));
    let motor = fly.and_then(|r| {
        let output = &r.value["output"];
        if !source_fresh(&r.value, now) || output["run_id"] != r.value["run_id"] || !stamp_fresh(&output["received_ms"], now) || output["tick"] != r.value["input"]["tick"] || output["input_fresh_at_output"] != true { return None; }
        output["neural_command"].as_i64().map(|v| v != 0)
    });
    let checks = [
        ("Usable current image (brightness 0.10..0.90, contrast >=0.05)", vision),
        ("Fresh measured bundle reaches the fly", sensors),
        ("Fly produces a nonzero motor proposal", motor),
        ("Localized map gains verified new area", None),
        ("Requested motion matches verified wheel outcome", None),
    ];
    let passed = checks.iter().filter(|(_, v)| *v == Some(true)).count();
    let unknown = checks.iter().filter(|(_, v)| v.is_none()).count();
    ui.strong(format!("Evidence score: {passed}/5 checks passed; {unknown} unverified"));
    ui.label("This is an auditable readiness rubric, not a trained reward or a claim of successful exploration.");
    for (label, result) in checks {
        ui.colored_label(match result {Some(true)=>Color32::LIGHT_GREEN, _=>Color32::YELLOW},
            format!("{} | {label}", match result {Some(true)=>"PASS",Some(false)=>"FAIL",None=>"UNVERIFIED"}));
    }
    if let Some((brightness, _)) = camera {
        if brightness < 0.10 { ui.colored_label(Color32::YELLOW, "Image is dark. See Lighting for automatic policy, serial write receipts, and measured luminance."); }
    }
    if let Some(fly) = fly {
        let input = &fly.value["input"];
        ui.label(format!("Odometry position: {} (odom frame; global location unverified)", text(&input["measured"]["odometry_pose"])));
        ui.label(format!("Fly host: {} | {}", text(&fly.value["producer_host"]), text(&fly.value["stream"])));
        ui.label(format!("Output hold: {}", text(&input["hold_reason"])));
    }
    ui.label("VSLAM metric pose/map: unverified. RL updates: no trained update evidence. DeepSeek belief update: not connected.");
    ui.separator();
    ui.strong("MCP observe -> robot");
    if data.observing { ui.label("Request in progress"); }
    if let Some(observation) = &data.observation {
        super::panels::freshness(ui, observation, 5000);
        if let Some(error) = &observation.error { ui.colored_label(Color32::YELLOW, error); }
        super::panels::object(ui, "Actual tool result", &observation.value);
    } else { ui.label("No MCP tool call recorded in this console session"); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refreshed_outer_status_does_not_make_old_sensors_fresh() {
        let value = serde_json::json!({"state":"live","run_id":"r","published_ms":5000,
            "input":{"schema_version":"qualia.sensor-conditioned-retina.v1","run_id":"r","received_ms":1000,
            "camera_request_started_ms":1000,"camera_received_ms":1000,"measured":{"imu_ts_ms":1000,"lidar_ts_ms":1000,"odometry_ts_ms":1000}}});
        assert!(source_fresh(&value,1000));
        assert!(!source_fresh(&value,5000));
        assert!(!source_fresh(&value,800));
    }
}
