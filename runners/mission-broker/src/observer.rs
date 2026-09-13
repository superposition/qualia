//! Actual provider advice from measured snapshots, without mission dispatch.
use std::{fs::{File, OpenOptions}, io::{Read, Write}, path::Path, time::{Duration, Instant}};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use crate::{config::CoachConfig, now_ms, redact, status::{DecisionRow, ModelState, StatusHandle}};

const PROMPT: &str = "You are the operator coach for a robot whose goal is learning visually guided exploration. Judge the supplied real observations: image quality, visual model responses, localization, map coverage and verified action outcomes. Automatic camera lighting is installed; use its supplied status and measured luma rather than inventing its state. Some snapshots contain a frozen pretrained flyvis optic model with graded voltages. These are not spikes, object detections, a calibrated map or learned driving decisions. Other snapshots contain the engineering sensor-conditioned CNS encoder and its documented limitations. Measured telemetry supplied separately does not enter the visual model unless the snapshot explicitly says so. No RL policy update has been measured. Return only JSON with instruction and reason, each a short plain-English string, grounded in the actual snapshot. Suggest one specific next observation or teaching prerequisite. Do not invent neural firing, object identification, a map, learned motor skill or executed action. This observer cannot dispatch missions or wheel commands. Motion acknowledgement repair remains unconfirmed and controls are locked. Do not instruct motion, motor authorization or estop reset. Do not call visual activity successful driving.";

pub fn snapshot(value: Value, now: u64) -> Result<Value, String> {
    if value["schema"] == "qualia.flyvis-live.v1" {
        return pretrained_snapshot(value, now);
    }
    let input = &value["input"];
    if value["state"] != "live" || input["schema_version"] != "qualia.sensor-conditioned-retina.v1"
        || !input["measured"].is_object() || input["model_stepped"] == false {
        return Err("No fresh measured sensor bundle; no coach request sent".into());
    }
    for stamp in [&value["published_ms"], &input["received_ms"], &input["measured"]["imu_ts_ms"],
        &input["measured"]["lidar_ts_ms"], &input["measured"]["odometry_ts_ms"], &input["camera_request_started_ms"], &input["camera_received_ms"]] {
        let stamp = stamp.as_u64().filter(|v| *v > 0).ok_or("Missing source timestamp")?;
        if stamp > now.saturating_add(100) || now.saturating_sub(stamp) > 1500 {
            return Err("Measured input is stale or future-dated; no coach request sent".into());
        }
    }
    if value["transport_attached"] != false { return Err("Observer expects the declared disconnected wheel transport".into()); }
    if value["run_id"].as_str().is_none_or(str::is_empty) || input["run_id"] != value["run_id"] || !input["tick"].is_u64()
        || !input["camera_capture_timestamp_known"].is_boolean() { return Err("Missing measured run identity or camera provenance".into()); }
    for (vector, count) in [(&input["measured"]["angular_velocity_radps"],3),
        (&input["measured"]["acceleration_including_gravity_mps2"],3), (&input["measured"]["odometry_pose"],3),
        (&input["image_columns"],8), (&input["conditioned_columns"],8)] {
        if !vector.as_array().is_some_and(|v| v.len()==count && v.iter().all(|n| n.as_f64().is_some_and(f64::is_finite))) {
            return Err("Measured sensor vector is missing or invalid".into());
        }
    }
    if input["encoder"].as_str().is_none_or(str::is_empty) { return Err("Missing encoder provenance".into()); }
    let valid = input["measured"]["valid_ranges"].as_u64().ok_or("Missing valid range count")?;
    let total = input["measured"]["total_ranges"].as_u64().ok_or("Missing total range count")?;
    if total == 0 || total > 10000 || valid > total || valid * 2 < total { return Err("Insufficient measured lidar coverage".into()); }
    let output = &value["output"];
    let paired = input["tick"].is_u64() && input["tick"] == output["tick"]
        && input["run_id"].is_string() && input["run_id"] == output["run_id"];
    Ok(json!({"run_id":value["run_id"], "snapshot_tick":input["tick"], "source_received_ms":input["received_ms"], "camera_request_started_ms":input["camera_request_started_ms"], "camera_received_ms":input["camera_received_ms"],
        "measured":input["measured"], "camera_capture_timestamp_known":input["camera_capture_timestamp_known"],
        "image_columns":input["image_columns"], "conditioned_columns":input["conditioned_columns"], "encoder":input["encoder"], "output_hold_reason":input["hold_reason"],
        "odometry_speed_mps":input["odometry_speed_mps"], "fused_yaw_rate_radps":input["fused_yaw_rate_radps"],
        "model_output":if paired {output.clone()} else {Value::Null}, "transport_attached":false}))
}

fn pretrained_snapshot(value: Value, now: u64) -> Result<Value, String> {
    if value["state"] != "running" || value["parameters_frozen"] != true || value["controls_locked"] != true
        || value["activity_kind"] != "graded_model_voltage" || value["model_inputs"] != json!(["camera_luminance"])
        || value["run_id"].as_str().is_none_or(str::is_empty) || !value["tick"].is_u64() {
        return Err("No current frozen visual model observation; no coach request sent".into());
    }
    let source = &value["source"];
    let output = &value["output"];
    for stamp in [&value["published_unix_ns"], &source["request_started_unix_ns"], &source["received_unix_ns"], &output["completed_unix_ns"]] {
        let stamp = stamp.as_u64().filter(|v| *v > 0).ok_or("Missing visual source timestamp")? / 1_000_000;
        if stamp > now.saturating_add(100) || now.saturating_sub(stamp) > 1500 {
            return Err("Visual observation is stale; no coach request sent".into());
        }
    }
    for age in [&source["received_age_ms"], &output["age_ms"], &output["input_age_ms"]] {
        if !age.as_f64().is_some_and(|v| v.is_finite() && (0.0..=1500.0).contains(&v)) {
            return Err("Visual observation age is invalid".into());
        }
    }
    if source["jpeg_sha256"].as_str().is_none_or(str::is_empty)
        || source["jpeg_sha256"] != output["input_sha256"]
        || source["received_unix_ns"] != output["input_received_unix_ns"] {
        return Err("Visual output does not match its source image".into());
    }
    for key in ["voltage_min", "voltage_max", "voltage_mean", "mean_abs_delta_from_previous"] {
        if !output[key].as_f64().is_some_and(f64::is_finite) { return Err("Invalid graded model response".into()); }
    }
    if !output["per_type"].as_array().is_some_and(|rows| !rows.is_empty() && rows.len() <= 256
        && rows.iter().all(|row| row["cell_type"].is_string() && row["mean_voltage"].as_f64().is_some_and(f64::is_finite))) {
        return Err("Missing graded cell-type observations".into());
    }
    Ok(json!({"run_id":value["run_id"],"snapshot_tick":value["tick"],
        "source_received_ms":source["received_unix_ns"].as_u64().map(|v| v/1_000_000),
        "source":source,"model":value["model"],"activity_kind":value["activity_kind"],
        "model_output":output,"model_inputs":value["model_inputs"],
        "measured_context":value["measured_context"],"measured_context_drives_model":false,
        "parameters_frozen":true,"controls_locked":true,"transport_attached":false}))
}

fn read_snapshot(path: &Path) -> Result<Value, String> {
    let mut bytes = Vec::new();
    File::open(path).map_err(|e| e.to_string())?.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 { return Err("Sensor status exceeds 64 KiB".into()); }
    snapshot(serde_json::from_slice(&bytes).map_err(|e| format!("Sensor status: {e}"))?, now_ms() as u64)
}

fn advice_text(value: &Value) -> Result<String, String> {
    let instruction = value["instruction"].as_str().filter(|v| !v.trim().is_empty() && v.chars().count() <= 600).ok_or("Provider instruction is missing or too long")?;
    let reason = value["reason"].as_str().filter(|v| !v.trim().is_empty() && v.chars().count() <= 600).ok_or("Provider rationale is missing or too long")?;
    Ok(format!("{instruction}\n{reason}"))
}

pub fn run(mut config: CoachConfig, source: &Path, evidence: &Path, ticks: u32, interval: Duration) -> Result<(), String> {
    if !(1..=60).contains(&ticks) || !(Duration::from_secs(5)..=Duration::from_secs(60)).contains(&interval) { return Err("Observer bounds refused".into()); }
    let key = config.api_key.as_deref().ok_or("DeepSeek credential is unavailable; no model request was sent")?.to_owned();
    config.timeout = config.timeout.clamp(Duration::from_millis(1000), Duration::from_secs(15));
    let http = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(3)).timeout(config.timeout).build().map_err(|e| e.to_string())?;
    let mut journal = OpenOptions::new().write(true).create_new(true).open(evidence).map_err(|e| format!("New observer evidence file: {e}"))?;
    let status = StatusHandle::new(ModelState {configured:true, model_id:config.model.clone(), base_url:config.base_url.clone(),
        key_presence:config.key_presence(), status:"waiting".into(), reason:Some("Awaiting measured input; advisory only".into()),
        last_latency_ms:None, last_error:None}, Some(key.clone()));
    // No AgentClient, mission bearer, transport or motor endpoint exists here.
    let status_host = std::env::var("QUALIA_COACH_STATUS_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let status_port = std::env::var("QUALIA_COACH_STATUS_PORT").ok().map(|v| v.parse::<u16>()).transpose().map_err(|_| "Invalid coach status port")?.unwrap_or(8091);
    status.serve(&status_host, status_port)?;
    let mut last_source = None;
    let mut recorded = 0;
    let mut evidence_failed = false;
    for tick in 0..ticks {
        let result = read_snapshot(source).and_then(|measured| {
            let identity = (measured["run_id"].clone(), measured["snapshot_tick"].clone());
            if last_source.as_ref() == Some(&identity) { return Err("No newer measured snapshot; no repeated model request".into()); }
            last_source = Some(identity);
            let request = json!({"model":config.model, "messages":[{"role":"system","content":PROMPT},
                {"role":"user","content":serde_json::to_string(&measured).map_err(|e|e.to_string())?}],
                "response_format":{"type":"json_object"}, "max_tokens":512, "temperature":0.2});
            let request = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
            let digest = format!("sha256:{:x}", Sha256::digest(&request));
            status.set_model(|m| {m.status="requesting".into(); m.last_error=None;});
            let start = Instant::now();
            let response = http.post(format!("{}/chat/completions", config.base_url.trim_end_matches('/')))
                .bearer_auth(&key).header("content-type","application/json").body(request).send().map_err(|e|e.to_string())?;
            if !response.status().is_success() { return Err(format!("Coach HTTP {}; no guidance accepted", response.status().as_u16())); }
            let mut bytes = Vec::new();
            response.take(65537).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
            if bytes.len() > 65536 { return Err("Coach response exceeds 64 KiB".into()); }
            let response: Value = serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
            if response["choices"][0]["finish_reason"] != "stop" { return Err("Provider did not return a complete response; no guidance accepted".into()); }
            let content = response["choices"][0]["message"]["content"].as_str().ok_or("Provider returned no guidance")?;
            let advice: Value = serde_json::from_str(content).map_err(|e|e.to_string())?;
            let reason = advice_text(&advice)?;
            let latency = start.elapsed().as_millis() as u64;
            let model = response["model"].as_str().unwrap_or(&config.model).to_owned();
            let row = DecisionRow {decision_id:format!("observation-{}-{tick}",std::process::id()), decision_kind:"operator_guidance".into(),
                target_proposal_ids:vec![], output_ids:vec![], reason:Some(reason), llm_priors_ablated:false,
                model_id:Some(model.clone()), prompt_digest:Some(digest), response_id:response["id"].as_str().map(str::to_owned),
                prompt_tokens:response["usage"]["prompt_tokens"].as_u64(), completion_tokens:response["usage"]["completion_tokens"].as_u64(),
                usage_reported:response["usage"].is_object(), latency_ms:Some(latency), decided_at_ms:now_ms() as u64, source_observed_at_ms:measured["source_received_ms"].as_u64(),
                disposition:format!("Advisory only; source {} tick {}; no mission or motor command dispatched", measured["run_id"], measured["snapshot_tick"])};
            let entry = json!({"source":measured, "decision":row});
            let entry = redact::scrub(&serde_json::to_string(&entry).map_err(|e|e.to_string())?, &[&key]);
            if let Err(error) = writeln!(journal, "{entry}").and_then(|()|journal.flush()).and_then(|()|journal.sync_data()) {
                evidence_failed = true;
                return Err(format!("Evidence write failed; guidance was not published: {error}"));
            }
            status.push_decision(row);
            status.set_model(|m| {m.model_id=model; m.status="ok".into(); m.reason=Some("Historical sensor-snapshot guidance; not dispatched".into()); m.last_latency_ms=Some(latency); m.last_error=None;});
            recorded += 1;
            crate::say(&format!("coach-observer: model answered in {latency} ms; advisory recorded, no dispatch"));
            Ok(())
        });
        if let Err(error) = result {
            let error = redact::scrub(&error, &[&key]);
            status.set_model(|m| {m.status="error".into();m.last_error=Some(error.clone());});
            crate::warn(&format!("coach-observer: {error}"));
        }
        if evidence_failed { return Err("Observer stopped after evidence write failure".into()); }
        if tick + 1 < ticks { std::thread::sleep(interval); }
    }
    if recorded == 0 { Err("No guidance was successfully recorded".into()) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_or_failed_source_cannot_be_relabelled_as_live_advice() {
        let mut source=json!({"state":"live","run_id":"test","transport_attached":false,"published_ms":1000,
            "input":{"schema_version":"qualia.sensor-conditioned-retina.v1","received_ms":1000,"camera_request_started_ms":1000,"camera_received_ms":1000,"tick":1,"run_id":"test",
            "camera_capture_timestamp_known":false,"encoder":"test engineering encoder","image_columns":[0.2,0.2,0.2,0.2,0.2,0.2,0.2,0.2],"conditioned_columns":[0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1],
            "measured":{"imu_ts_ms":1000,"lidar_ts_ms":1000,"odometry_ts_ms":1000,
            "angular_velocity_radps":[0,0,0],"acceleration_including_gravity_mps2":[0,0,9.8],"odometry_pose":[0,0,0],"valid_ranges":250,"total_ranges":360}}});
        let projected = snapshot(source.clone(),1001).unwrap();
        assert_eq!(projected["image_columns"][0],0.2);
        assert_eq!(projected["conditioned_columns"][0],0.1);
        assert_eq!(projected["encoder"],"test engineering encoder");
        assert!(projected.get("camera_features").is_none());
        assert!(snapshot(source.clone(),3000).is_err());
        assert!(snapshot(source.clone(),800).is_err());
        source["input"]["model_stepped"]=json!(false);
        assert!(snapshot(source,1001).is_err());
    }
    #[test]
    fn blank_or_unstructured_advice_is_not_a_provider_instruction() {
        assert!(advice_text(&json!({"instruction":"", "reason":"test"})).is_err());
        assert!(advice_text(&json!({"instruction":"check the scan"})).is_err());
        assert_eq!(advice_text(&json!({"instruction":"Check the scan", "reason":"Near return"})).unwrap(),"Check the scan\nNear return");
    }
}
