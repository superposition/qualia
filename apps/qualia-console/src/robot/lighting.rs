//! Read-only presentation of Leash's camera-light policy and write evidence.
use egui::{Color32, Ui};
use serde_json::Value;

use super::{
    panels,
    source::{now_ms, Data},
};

pub fn valid_status(value: &Value) -> bool {
    value["schema_version"] == "leash.camera-lights.v1"
        && value["enabled"].is_boolean()
        && matches!(value["mode"].as_str(), Some("auto" | "manual" | "off"))
        && value["auto_on_latched"].is_boolean()
        && value["physical_state_known"] == false
        && value["physical_feedback"] == "unknown"
        && ["configured_camera_pwm", "configured_chassis_pwm"]
            .iter()
            .all(|field| value[field].as_u64().is_some_and(|pwm| pwm <= 255))
        && value["events"].is_array()
}

fn luma(value: &Value) -> Option<f64> {
    value["mean_luma"]
        .as_f64()
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

fn sample_fresh(value: &Value, now: u64) -> bool {
    let (Some(started), Some(received)) = (
        value["request_started_ms"].as_u64(),
        value["received_ms"].as_u64(),
    ) else {
        return false;
    };
    luma(value).is_some()
        && started > 0
        && started <= received
        && received <= now.saturating_add(100)
        && now.saturating_sub(started) <= 2000
}

fn age(value: &Value, now: u64) -> String {
    match value.as_u64() {
        Some(stamp) if stamp > now.saturating_add(100) => "timestamp is ahead of this host".into(),
        Some(stamp) if stamp > 0 => format!("{} ms ago", now.saturating_sub(stamp)),
        _ => "time unavailable".into(),
    }
}

fn brightness(value: &Value) -> String {
    luma(value)
        .map(|v| format!("{v:.3}"))
        .unwrap_or_else(|| "unavailable".into())
}

pub fn render(ui: &mut Ui, data: &Data) {
    let Some(reading) = data.sources.get("lighting") else {
        ui.label("Waiting for camera lighting status");
        ui.label("Physical lamp feedback: unknown");
        return;
    };
    let now = now_ms();
    panels::freshness(ui, reading, 5000);
    if let Some(error) = &reading.error {
        ui.colored_label(Color32::YELLOW, error);
    }
    let status = &reading.value;
    if !valid_status(status) {
        ui.colored_label(Color32::YELLOW, "No valid camera lighting status received");
        ui.label("Physical lamp feedback: unknown");
        return;
    }

    let policy = if status["enabled"] != true {
        "Lighting disabled by service policy"
    } else {
        match (status["mode"].as_str(), status["auto_on_latched"] == true) {
            (Some("auto"), true) => "Automatic light command latched on",
            (Some("auto"), false) => "Auto armed: waiting for sustained darkness",
            (Some("manual"), _) => "Manual policy: automatic writes disarmed",
            _ => "Off policy: automatic writes disarmed",
        }
    };
    ui.strong(policy);
    if !reading.fresh_at(now, 5000) {
        ui.colored_label(
            Color32::YELLOW,
            "Displayed policy and PWM are the last received status",
        );
    }
    ui.label("Physical lamp feedback: unknown");
    ui.small("A serial receipt confirms a write; visible LED operation is unverified.");

    egui::Grid::new("lighting-pwm")
        .striped(true)
        .show(ui, |ui| {
            for label in ["PWM (0..255)", "Configured", "Last written"] {
                ui.strong(label);
            }
            ui.end_row();
            for (name, output) in [("Camera", "camera"), ("Chassis", "chassis")] {
                ui.label(name);
                ui.label(panels::text(&status[format!("configured_{output}_pwm")]));
                ui.label(panels::text(&status[format!("last_written_{output}_pwm")]));
                ui.end_row();
            }
        });
    if let Some(error) = status["last_error"].as_str() {
        ui.colored_label(Color32::YELLOW, format!("Lighting error: {error}"));
    }

    let sample = &status["last_sample"];
    let fresh = reading.fresh_at(now, 5000) && sample_fresh(sample, now);
    ui.colored_label(
        if fresh {
            Color32::LIGHT_GREEN
        } else {
            Color32::YELLOW
        },
        format!(
            "{} camera luma: {} | sample {}",
            if fresh {
                "Current"
            } else {
                "STALE / unavailable"
            },
            brightness(sample),
            age(&sample["received_ms"], now),
        ),
    );
    ui.small(format!(
        "Dark below {} | reset at {} | sustain {} ms | retry cooldown {} ms",
        panels::text(&status["dark_threshold"]),
        panels::text(&status["bright_reset_threshold"]),
        panels::text(&status["sustained_ms"]),
        panels::text(&status["cooldown_ms"])
    ));

    let events = status["events"].as_array().expect("validated event array");
    if let Some(event) = events.last() {
        ui.separator();
        ui.strong(format!(
            "Latest attempt #{} | {}",
            panels::text(&event["sequence"]),
            panels::text(&event["origin"])
        ));
        ui.label(format!(
            "Requested camera {} / chassis {} | {}",
            panels::text(&event["camera_pwm"]),
            panels::text(&event["chassis_pwm"]),
            age(&event["requested_at_ms"], now)
        ));
        if let Some(error) = event["error"].as_str() {
            ui.colored_label(Color32::YELLOW, format!("Attempt error: {error}"));
        }
        let receipt = &event["receipt"];
        if receipt["controller_sequence"].as_u64().is_some() {
            ui.label(format!(
                "Serial write #{} | {}",
                panels::text(&receipt["controller_sequence"]),
                age(&receipt["write_completed_at_ms"], now)
            ));
        } else {
            ui.colored_label(Color32::YELLOW, "No serial write receipt for this attempt");
        }
        ui.label(format!(
            "Before luma {} -> after {}",
            brightness(&event["before"]),
            brightness(&event["after"])
        ));
        if let (Some(before), Some(after)) = (luma(&event["before"]), luma(&event["after"])) {
            ui.label(format!(
                "Measured image change: {:+.3}; cause unverified",
                after - before
            ));
        }
        ui.small(
            "Before/after are historical image observations. Sensor exposure time is unknown.",
        );
    } else {
        ui.label("No lighting write attempts reported");
    }

    egui::CollapsingHeader::new("Current sample details")
        .show(ui, |ui| panels::object(ui, "Sample", sample));
    egui::CollapsingHeader::new(format!("Raw lighting events ({})", events.len()))
        .show(ui, |ui| panels::object(ui, "Events", &status["events"]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_status_does_not_make_old_or_future_camera_samples_current() {
        let sample =
            serde_json::json!({"request_started_ms":1000,"received_ms":1010,"mean_luma":0.16});
        assert!(sample_fresh(&sample, 1100));
        assert!(!sample_fresh(&sample, 4000));
        assert!(!sample_fresh(&sample, 500));
        assert!(!sample_fresh(&Value::Null, 1100));
    }

    #[test]
    fn lighting_contract_does_not_promote_pwm_to_physical_feedback() {
        let mut status = serde_json::json!({"schema_version":"leash.camera-lights.v1",
            "enabled":true,"mode":"auto","auto_on_latched":true,
            "physical_state_known":false,"physical_feedback":"unknown",
            "configured_camera_pwm":180,"configured_chassis_pwm":0,"events":[]});
        assert!(valid_status(&status));
        status["physical_state_known"] = Value::Bool(true);
        assert!(!valid_status(&status));
    }
}
