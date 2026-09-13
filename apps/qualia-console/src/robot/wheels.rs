//! Observe the engineered wheel readout; this module has no command client.
use egui::{Color32, Ui};
use serde_json::Value;
use super::{panels, source::{now_ms, Data, Reading}};

pub fn endpoint(robot: &str) -> String {
    std::env::var("QUALIA_WHEEL_SHADOW_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| {
        reqwest::Url::parse(robot).ok().and_then(|mut url| {
            url.set_port(Some(8092)).ok()?;
            url.set_path("/status"); url.set_query(None); url.set_fragment(None);
            Some(url.to_string())
        }).unwrap_or_else(|| "invalid wheel readout URL".into())
    })
}

pub fn transport_endpoint() -> String {
    std::env::var("QUALIA_WHEEL_TRANSPORT_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "http://127.0.0.1:8093/status".into())
}

fn number(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite()) }
fn decimal(value: &Value) -> String {
    number(value).map(|v| format!("{v:.3}")).unwrap_or_else(|| "unavailable".into())
}
pub fn valid_status(value: &Value) -> bool {
    value["schema"] == "qualia.wheel-shadow.v1"
        && value["policy_kind"] == "engineered_frozen_vision_readout"
        && value["motion_output"] == false
        && (value["transport_attached"] == false || value["transport_attached"].is_null())
        && value["run_id"].as_str().is_some_and(|v| !v.is_empty() && v.len() <= 128)
        && value["tick"].as_u64().is_some() && value["published_unix_ns"].as_u64().is_some()
        && matches!(value["state"].as_str(), Some("starting" | "shadow_proposal" | "held" | "stopped" | "failed"))
        && (!matches!(value["state"].as_str(), Some("shadow_proposal" | "held")) || value["emitted_frame"].is_object())
        && value["hold_reasons"].as_array().is_some_and(|rows| rows.len() <= 64 && rows.iter().all(|v| v.as_str().is_some_and(|s| s.len() <= 2048)))
        && (value["proposed"].is_null() || ["left_mps", "right_mps", "neural_turn_mps", "gyro_damping_mps", "forward_mps"].iter().all(|key| number(&value["proposed"][key]).is_some()))
        && (value["emitted_frame"].is_null() || (value["emitted_frame"]["T"].as_u64() == value["tick"].as_u64()
            && ["L", "R"].iter().all(|key| number(&value["emitted_frame"][key]).is_some())
            && (value["state"] != "held" || (number(&value["emitted_frame"]["L"]) == Some(0.0) && number(&value["emitted_frame"]["R"]) == Some(0.0)))))
}

pub fn valid_probe(value: &Value) -> bool {
    value["kind"] == "zero-speed drive integration"
        && value["started_ms"].as_u64().is_some_and(|v| v > 0)
        && number(&value["requested_left"]) == Some(0.0) && number(&value["requested_right"]) == Some(0.0)
        && value["drive"]["http_status"].as_u64().is_some()
        && value["drive"]["response"].is_object() && value["verified_stop"].is_object()
}

pub fn valid_transport(value: &Value) -> bool {
    let zero_only = value["zero_only"].as_bool();
    let enabled = value["nonzero_enabled"].as_bool();
    let maximum = number(&value["max_wheel_speed_mps"]);
    value["schema"] == "qualia.wheel-transport.v1" && zero_only.is_some() && enabled.is_some() && zero_only != enabled
        && maximum.is_some_and(|v| v > 0.0 && v <= 0.1)
        && value["run_id"].as_str().is_some_and(|v| !v.is_empty() && v.len() <= 128)
        && value["published_unix_ns"].as_u64().is_some() && value["deadline_unix_ns"].as_u64().is_some()
        && value["transport_attached"].is_boolean() && value["transport"].is_object()
        && matches!(value["state"].as_str(), Some("starting" | "connecting" | "connected_zero_only" | "connected_bounded_motion" | "source_hold" | "source_reacquiring" | "interlock_rejected" | "draining" | "stopped" | "failed"))
        && (value["forwarded_frame"].is_null() || (value["forwarded_frame"]["T"].as_u64().is_some()
            && ["L", "R"].iter().all(|key| number(&value["forwarded_frame"][key])
                .is_some_and(|v| if zero_only == Some(true) { v == 0.0 } else { maximum.is_some_and(|max| v.abs() <= max) }))))
        && (value["ledger"].is_null() || (value["ledger"]["matched_to_transport"] == false
            && value["ledger"]["entries"].as_array().is_some_and(|v| v.len() <= 8)))
}

fn transport(ui: &mut Ui, data: &Data) {
    ui.strong("Host wheel transport");
    let Some(reading) = data.sources.get("wheel-transport") else { ui.small("Waiting for bridge evidence on host port8093"); return; };
    let value = &reading.value;
    if !valid_transport(value) {
        ui.colored_label(Color32::YELLOW, "Transport bridge unavailable or contract invalid");
        if let Some(error) = &reading.error { ui.small(error); } return;
    }
    let now = now_ms();
    let publication = value["published_unix_ns"].as_u64().unwrap_or(0) / 1_000_000;
    let current = reading.fresh_at(now, 1500) && publication > 0 && publication <= now.saturating_add(100)
        && now.saturating_sub(publication) <= 1500 && value["deadline_unix_ns"].as_u64().is_some_and(|v| v / 1_000_000 > now)
        && !matches!(value["state"].as_str(), Some("stopped" | "failed"));
    let attached = current && value["transport_attached"] == true && value["transport"]["process_alive"] == true
        && value["transport"]["stdin_open"] == true && value["transport"]["last_consumed_tick"].as_u64().is_some();
    let reacquiring = value["state"] == "source_reacquiring";
    let mode = if value["nonzero_enabled"] == true { "Armed bounded transport" } else { "Zero-only transport" };
    let state = if !current { "Historical / unavailable" } else if reacquiring { "Reacquiring input; no writes" } else if attached { "Connected" } else { "No verified open attachment" };
    ui.colored_label(if attached && !reacquiring {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!("{state} · {mode}"));
    ui.label(format!("Consumed tick {} · forwarded tick {}", panels::text(&value["transport"]["last_consumed_tick"]), panels::text(&value["forwarded_frame"]["T"])));
    ui.small(format!("Reported speed bound {} m/s. Forwarding and mode flags do not prove wheel movement.", decimal(&value["max_wheel_speed_mps"])));
    if let Some(error) = value["error"].as_str() { ui.colored_label(Color32::YELLOW, error); }
    egui::CollapsingHeader::new("Transport process and global ledger").show(ui, |ui| {
        if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
        ui.label("Global Leash ledger entries are not matched to this transport session.");
        for key in ["run_id", "state", "zero_only", "nonzero_enabled", "max_wheel_speed_mps", "source_failures", "last_source_error", "source_recoveries", "transport", "forwarded_frame", "forwarded_unix_ns", "forwarded_count", "ledger", "rejected_frame", "final_stop", "published_unix_ns", "deadline_unix_ns"] {
            panels::object(ui, key, &value[key]);
        }
    });
}

pub fn current_decision(reading: &Reading, now: u64) -> bool {
    let value = &reading.value;
    let publication = value["published_unix_ns"].as_u64().unwrap_or(0) / 1_000_000;
    let deadline = value["deadline_unix_ns"].as_u64().unwrap_or(0) / 1_000_000;
    let elapsed = now.saturating_sub(reading.received_ms);
    valid_status(value) && reading.fresh_at(now, 500) && publication > 0 && publication <= now.saturating_add(100)
        && now.saturating_sub(publication) <= 500 && deadline > now
        && number(&value["decision_age_ms"]).is_some_and(|v| v >= 0.0 && v + elapsed as f64 <= 500.0)
        && matches!(value["state"].as_str(), Some("shadow_proposal" | "held"))
}

pub fn probe(ui: &mut Ui, data: &Data) {
    let Some(reading) = data.sources.get("wheel-probe") else { return; };
    let value = &reading.value;
    if !valid_probe(value) { ui.colored_label(Color32::YELLOW, "Recorded drive probe unavailable"); return; }
    let stamp = value["started_ms"].as_u64().unwrap_or(0);
    ui.strong("Recorded zero-speed integration result");
    let now = now_ms();
    let age = if stamp > now.saturating_add(100) { "timestamp ahead of console clock".into() }
        else { format!("recorded {} s ago", now.saturating_sub(stamp) / 1000) };
    ui.small(format!("Unix ms {stamp} · {age} · not a live health reading"));
    if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, format!("Retained result; file read failed: {error}")); }
    let response = &value["drive"]["response"];
    ui.label(format!("Zero drive: HTTP {} · ok {}", panels::text(&value["drive"]["http_status"]), panels::text(&response["ok"])));
    if let Some(error) = response["error"].as_str() { ui.colored_label(Color32::YELLOW, error); }
    ui.label(format!("Verified stop: HTTP {} · acknowledged {}", panels::text(&value["verified_stop"]["http_status"]), panels::text(&value["verified_stop"]["response"]["acknowledged"])));
    ui.small("A zero-speed acknowledgement does not demonstrate nonzero motion.");
}

pub fn probe_summary(ui: &mut Ui, data: &Data) {
    let Some(reading) = data.sources.get("wheel-probe").filter(|r| valid_probe(&r.value)) else { return; };
    let value = &reading.value;
    ui.label(format!("Recorded zero probe: HTTP {} · stop ACK {}", panels::text(&value["drive"]["http_status"]), panels::text(&value["verified_stop"]["response"]["acknowledged"])))
        .on_hover_text(format!("Started at Unix ms {}. Historical integration result, not current health; full response in Action evidence.", panels::text(&value["started_ms"])));
}

pub fn render(ui: &mut Ui, data: &Data) {
    transport(ui, data);
    ui.strong("Wheel readout · shadow evidence");
    ui.small("Frozen vision + body measurements; engineered mapping, not learned driving.");
    let Some(reading) = data.sources.get("wheel-shadow") else {
        ui.colored_label(Color32::YELLOW, "Waiting for wheel readout status; no applied motion is claimed."); return;
    };
    let now = now_ms(); let value = &reading.value;
    if !valid_status(value) {
        ui.colored_label(Color32::YELLOW, "Wheel readout unavailable or contract invalid");
        if let Some(error) = &reading.error { ui.small(error); } return;
    }
    let current = current_decision(reading, now);
    ui.colored_label(if current {Color32::LIGHT_GREEN} else {Color32::YELLOW}, format!(
        "{} | {} | tick {}", if current {"Current shadow decision"} else {"Historical / unavailable decision"}, panels::text(&value["state"]), panels::text(&value["tick"])));
    ui.label(if value["transport_attached"] == false {"Shadow producer owns no transport; external bridge shown above."} else {"Producer cannot establish external transport attachment or applied motion."});
    ui.label(format!("Proposed L {} / R {} m/s", decimal(&value["proposed"]["left_mps"]), decimal(&value["proposed"]["right_mps"])));
    ui.label(format!("JSONL frame T {} · L {} / R {} m/s", panels::text(&value["emitted_frame"]["T"]), decimal(&value["emitted_frame"]["L"]), decimal(&value["emitted_frame"]["R"])));
    if let Some(reasons) = value["hold_reasons"].as_array() {
        for reason in reasons { ui.colored_label(Color32::YELLOW, panels::text(reason)); }
    }
    ui.label(format!("All-around nearest {} m · gyro yaw {} rad/s", decimal(&value["body"]["nearest_return_m"]), decimal(&value["body"]["gyro_yaw_radps"])));
    if let Some(sector) = value["body"]["forward_sector"].as_object() {
        let coverage = sector.get("coverage_fraction").and_then(number).filter(|v| (0.0..=1.0).contains(v))
            .map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "unavailable".into());
        ui.label(format!("Forward sector: nearest {} m · coverage {coverage} · {} unknown beams",
            decimal(&value["body"]["forward_sector"]["min_return_m"]), panels::text(&value["body"]["forward_sector"]["unknown_beams"])));
        ui.small(format!("Readout direction {} · selected {} clearance {} m; rear/side clearance remains separate.",
            panels::text(&value["proposed"]["direction"]), panels::text(&value["proposed"]["clearance_sector"]),
            decimal(&value["proposed"]["selected_clearance_m"])));
    }
    egui::CollapsingHeader::new("Wheel input and frame evidence").show(ui, |ui| {
        if let Some(error) = &reading.error { ui.colored_label(Color32::YELLOW, error); }
        for key in ["run_id", "cruise_speed_mps", "directional_clearance_enabled", "vision", "body", "proposed", "emitted_frame", "frame_written_unix_ns", "decision_age_ms", "published_unix_ns", "deadline_unix_ns"] {
            panels::object(ui, key, &value[key]);
        }
        ui.small("T is the trace tick label. A JSONL frame is a readout result, not a robot acknowledgement.");
    });
}
