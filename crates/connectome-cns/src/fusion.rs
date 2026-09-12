//! Measured body state conditions visual input without projecting uncalibrated
//! lidar points or inventing camera poses. All gains are an engineering encoder,
//! not fitted biological parameters. Evidence records derived features and
//! source timestamps; it is not a raw JPEG or full lidar recording.
use std::io::{Read, Write};
use std::path::Path;
use serde::Serialize;
use serde_json::Value;
use qualia_connectome_cns::CnsError;

const MAX_AGE_MS: u64 = 1000;
const MAX_SKEW_MS: u64 = 500;
const CLEARANCE_M: f32 = 0.25;

#[derive(Clone, Debug, Serialize)]
pub struct Measured {
    pub imu_ts_ms: u64,
    pub lidar_ts_ms: u64,
    pub odometry_ts_ms: u64,
    pub angular_velocity_radps: [f32; 3],
    pub acceleration_including_gravity_mps2: [f32; 3],
    pub odometry_pose: [f32; 3],
    pub valid_ranges: usize,
    pub total_ranges: usize,
    pub nearest_return_m: Option<f32>,
}

fn invalid(message: impl Into<String>) -> CnsError { CnsError::Read(message.into()) }
fn number(value: &Value, key: &str) -> Result<f32, CnsError> {
    value[key].as_f64().filter(|v| v.is_finite() && v.abs() <= f32::MAX as f64)
        .map(|v| v as f32).ok_or_else(|| invalid(format!("sensor field {key} is absent or nonfinite")))
}
fn timestamp(value: &Value, now: u64, label: &str) -> Result<u64, CnsError> {
    let ts = value["ts_ms"].as_u64().filter(|ts| *ts > 0).ok_or_else(|| invalid(format!("{label} timestamp is absent")))?;
    if ts > now.saturating_add(100) || now.saturating_sub(ts) > MAX_AGE_MS {
        return Err(invalid(format!("{label} is stale or its clock is ahead: source={ts}, received={now}")));
    }
    Ok(ts)
}

impl Measured {
    pub fn parse(value: &Value, received_ms: u64) -> Result<Self, CnsError> {
        let sensors = &value["sensors"];
        for key in ["imu", "range_scan", "odometry"] {
            if sensors[key]["status"] != "available" { return Err(invalid(format!("required sensor {key} is unavailable"))); }
        }
        let imu = &sensors["imu"]["sample"];
        let scan = &sensors["range_scan"]["sample"];
        let odom = &value["odometry_pose"]["pose"];
        if imu["frame_id"] != "base_link" || scan["frame_id"] != "base_scan" || odom["frame_id"] != "odom" {
            return Err(invalid("unrecognized IMU, scan or odometry coordinate frame"));
        }
        let imu_ts_ms = timestamp(imu, received_ms, "IMU")?;
        let lidar_ts_ms = timestamp(scan, received_ms, "lidar")?;
        let odometry_ts_ms = timestamp(odom, received_ms, "odometry")?;
        let earliest = imu_ts_ms.min(lidar_ts_ms).min(odometry_ts_ms);
        let latest = imu_ts_ms.max(lidar_ts_ms).max(odometry_ts_ms);
        if latest - earliest > MAX_SKEW_MS { return Err(invalid("sensor timestamps differ by more than 500 ms")); }
        let vector = |field: &str| -> Result<[f32; 3], CnsError> {
            let v = &imu[field]; Ok([number(v,"x")?, number(v,"y")?, number(v,"z")?])
        };
        let min = number(scan, "range_min_m")?;
        let max = number(scan, "range_max_m")?;
        if min < 0.0 || max <= min || max > 100.0 { return Err(invalid("invalid lidar range limits")); }
        let ranges = scan["ranges_m"].as_array().filter(|r| !r.is_empty() && r.len() <= 10000)
            .ok_or_else(|| invalid("invalid lidar range array"))?;
        let valid: Vec<f32> = ranges.iter().filter_map(|v| v.as_f64()).map(|v| v as f32)
            .filter(|v| v.is_finite() && *v >= min && *v <= max).collect();
        if valid.len() * 2 < ranges.len() { return Err(invalid("fewer than half of lidar beams have valid measurements")); }
        let angular_velocity = vector("angular_velocity_radps")?;
        let acceleration = vector("linear_acceleration_mps2")?;
        let pose = [number(odom,"x_m")?, number(odom,"y_m")?, number(odom,"yaw_rad")?];
        if angular_velocity.iter().chain(acceleration.iter()).any(|v| v.abs() > 100.0) || pose.iter().any(|v| v.abs() > 1_000_000.0) {
            return Err(invalid("IMU or odometry exceeds the encoder's numerical envelope"));
        }
        Ok(Self {
            imu_ts_ms, lidar_ts_ms, odometry_ts_ms,
            angular_velocity_radps: angular_velocity,
            acceleration_including_gravity_mps2: acceleration,
            odometry_pose: pose,
            valid_ranges: valid.len(), total_ranges: ranges.len(),
            nearest_return_m: valid.into_iter().filter(|r| *r < max).reduce(f32::min),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FusionEvidence {
    pub schema_version: &'static str,
    pub tick: u64,
    pub received_ms: u64,
    pub camera_request_started_ms: u64,
    pub camera_received_ms: u64,
    pub camera_capture_timestamp_known: bool,
    pub measured: Measured,
    pub odometry_speed_mps: Option<f32>,
    pub odometry_yaw_rate_radps: Option<f32>,
    pub fused_yaw_rate_radps: f32,
    pub acceleration_norm_mps2: f32,
    pub motion_attenuation: f32,
    pub proximity_attention: f32,
    pub image_columns: Vec<f32>,
    pub conditioned_columns: Vec<f32>,
    pub encoder: &'static str,
    pub hold_reason: Option<String>,
}

#[derive(Default)]
pub struct Fusion {
    previous_odom: Option<(u64, [f32; 3])>,
    velocity: Option<(f32, f32)>,
    previous_columns: Option<Vec<f32>>,
}

impl Fusion {
    pub fn condition(&mut self, measured: Measured, columns: &[f32], tick: u64, camera_start: u64, camera_end: u64, received: u64) -> Result<FusionEvidence, CnsError> {
        if columns.is_empty() || columns.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return Err(invalid("camera columns are invalid"));
        }
        if camera_end < camera_start || camera_end - camera_start > MAX_AGE_MS || received.saturating_sub(camera_end) > MAX_SKEW_MS {
            return Err(invalid("camera retrieval and sensor bundle are not recent enough"));
        }
        let oldest = measured.imu_ts_ms.min(measured.lidar_ts_ms).min(measured.odometry_ts_ms);
        if camera_start == 0 || camera_end > received.saturating_add(100) || oldest.saturating_add(MAX_SKEW_MS) < camera_start {
            return Err(invalid("camera retrieval interval and sensor timestamps are misaligned"));
        }
        let mut hold_reason = None;
        if let Some((previous_ts, previous_pose)) = self.previous_odom {
            if measured.odometry_ts_ms < previous_ts { return Err(invalid("odometry timestamp moved backwards")); }
            if measured.odometry_ts_ms > previous_ts {
                let dt = (measured.odometry_ts_ms - previous_ts) as f32 / 1000.0;
                if dt > 2.0 { self.velocity = None; }
                else {
                    let pose = measured.odometry_pose;
                    let speed = (pose[0] - previous_pose[0]).hypot(pose[1] - previous_pose[1]) / dt;
                    let angle = (pose[2] - previous_pose[2] + std::f32::consts::PI)
                        .rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    self.velocity = Some((speed, angle / dt));
                }
            }
        }
        self.previous_odom = Some((measured.odometry_ts_ms, measured.odometry_pose));
        if self.velocity.is_some_and(|(speed, yaw)| !speed.is_finite() || !yaw.is_finite() || speed > 5.0 || yaw.abs() > 20.0) {
            self.velocity = None;
            return Err(invalid("derived odometry velocity is invalid or outside the operating envelope"));
        }
        let gyro_yaw = measured.angular_velocity_radps[2];
        // Equal-weight fusion is explicit; neither sensor reports enough noise
        // information here to claim a statistically calibrated Kalman estimate.
        let fused_yaw_rate = self.velocity.map(|(_, yaw)| (gyro_yaw + yaw) * 0.5).unwrap_or(gyro_yaw);
        if self.velocity.is_none() { hold_reason = Some("waiting for two fresh odometry samples".into()); }
        if self.velocity.is_some_and(|(_, yaw)| (yaw - gyro_yaw).abs() > 0.5) {
            hold_reason = Some("IMU and odometry yaw rates disagree by more than 0.5 rad/s".into());
        }
        let acceleration_norm = measured.acceleration_including_gravity_mps2.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !(4.0..=16.0).contains(&acceleration_norm) { hold_reason = Some("acceleration magnitude outside stationary/gravity operating envelope".into()); }
        if measured.nearest_return_m.is_some_and(|range| range < CLEARANCE_M) {
            hold_reason = Some(format!("measured lidar return is inside the {CLEARANCE_M} m clearance hold"));
        }
        let motion_attenuation = 1.0 / (1.0 + fused_yaw_rate.abs()
            + self.velocity.map(|v| v.0).unwrap_or(0.0) + (acceleration_norm - 9.80665).abs() * 0.1);
        let proximity_attention = measured.nearest_return_m.map(|r| (1.0 - r / 2.0).clamp(0.0, 1.0)).unwrap_or(0.0);
        let conditioned: Vec<f32> = columns.iter().enumerate().map(|(index, value)| {
            let contrast = self.previous_columns.as_ref().and_then(|v| v.get(index)).map(|old| (value - old).abs()).unwrap_or(0.0);
            // The body-motion estimate attenuates temporal visual contrast.
            // Range contributes global attention, without an uncalibrated
            // claim that a lidar ray is aligned with an image column.
            (0.75 * value + 0.25 * contrast * motion_attenuation * (1.0 + proximity_attention)).clamp(0.0, 1.0)
        }).collect();
        if !fused_yaw_rate.is_finite() || !acceleration_norm.is_finite() || !motion_attenuation.is_finite()
            || conditioned.iter().any(|v| !v.is_finite()) { return Err(invalid("nonfinite fused input")); }
        self.previous_columns = Some(columns.to_vec());
        Ok(FusionEvidence {
            schema_version: "qualia.sensor-conditioned-retina.v1", tick, received_ms: received,
            camera_request_started_ms: camera_start, camera_received_ms: camera_end,
            camera_capture_timestamp_known: false, measured,
            odometry_speed_mps: self.velocity.map(|v| v.0), odometry_yaw_rate_radps: self.velocity.map(|v| v.1),
            fused_yaw_rate_radps: fused_yaw_rate, acceleration_norm_mps2: acceleration_norm,
            motion_attenuation, proximity_attention, image_columns: columns.to_vec(), conditioned_columns: conditioned,
            encoder: "photoreceptors; brightness plus body-conditioned temporal contrast; no pixel/range projection",
            hold_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn measured(ts: u64) -> Measured {
        Measured {imu_ts_ms: ts, lidar_ts_ms: ts, odometry_ts_ms: ts,
            angular_velocity_radps: [0.0;3], acceleration_including_gravity_mps2: [0.0,0.0,9.80665],
            odometry_pose: [0.0;3],valid_ranges: 300,total_ranges:360,nearest_return_m:Some(2.0)}
    }
    #[test]
    fn odometry_and_imu_agreement_conditions_the_exact_visual_current() {
        let mut fusion = Fusion::default();
        let first = fusion.condition(measured(1000), &[0.8;8],0,990,1000,1000).unwrap();
        assert!(first.hold_reason.is_some());
        let mut next = measured(1100);
        next.odometry_pose[2] = 0.02;
        next.angular_velocity_radps[2] = 0.2;
        let second = fusion.condition(next, &[0.2;8],1,1090,1100,1100).unwrap();
        assert!(second.hold_reason.is_none());
        assert!((second.fused_yaw_rate_radps - 0.2).abs() < 0.0001);
        assert!((second.conditioned_columns[0] - 0.275).abs() < 0.0001);
        assert_eq!(second.image_columns, vec![0.2;8]);
    }
    #[test]
    fn an_obstacle_holds_output_while_preserving_measured_neural_input() {
        let mut fusion = Fusion::default();
        fusion.condition(measured(1000), &[0.8;8],0,990,1000,1000).unwrap();
        let mut next = measured(1100);
        next.nearest_return_m = Some(0.15);
        let second = fusion.condition(next, &[0.2;8],1,1090,1100,1100).unwrap();
        assert!(second.hold_reason.as_ref().unwrap().contains("clearance"));
        assert!(second.proximity_attention > 0.9);
        assert!(second.conditioned_columns[0] > 0.3);
        assert!(second.conditioned_columns.iter().all(|v| v.is_finite()));
    }
    #[test]
    fn an_old_bundle_cannot_be_relabelled_with_a_fresh_receive_time() {
        assert!(timestamp(&serde_json::json!({"ts_ms":1000}),3000,"IMU").is_err());
        assert!(timestamp(&serde_json::json!({"ts_ms":4000}),3000,"IMU").is_err());
        assert!(Measured::parse(&serde_json::json!({"sensors":{}}),1000).is_err());
        let mut fusion = Fusion::default();
        assert!(fusion.condition(measured(1000), &[0.5;8],0,1700,1710,1710).is_err());
    }
}

pub struct LiveFusion {
    url: String,
    client: ureq::Agent,
    fusion: Fusion,
    evidence: std::fs::File,
}

impl LiveFusion {
    pub fn new(url: &str, path: &Path) -> Result<Self, CnsError> {
        let evidence = std::fs::OpenOptions::new().write(true).create_new(true).open(path)
            .map_err(|e| qualia_connectome_cns::read_error(path, e))?;
        let client = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_millis(700))
            .timeout(std::time::Duration::from_millis(1500)).build();
        Ok(Self {url: url.into(), client, fusion: Fusion::default(), evidence})
    }

    pub fn acquire(&mut self, columns: &[f32], tick: u64, camera_start: u64, camera_end: u64) -> Result<FusionEvidence, CnsError> {
        let response = self.client.get(&self.url).set("Connection", "close").call().map_err(|e| invalid(format!("sensor telemetry: {e}")))?;
        let mut bytes = Vec::new();
        response.into_reader().take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| invalid(format!("sensor telemetry: {e}")))?;
        if bytes.len() > 4 * 1024 * 1024 { return Err(invalid("sensor telemetry exceeds 4 MiB")); }
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("sensor telemetry JSON: {e}")))?;
        let received = super::unix_ns() / 1_000_000;
        let measured = Measured::parse(&value, received)?;
        let evidence = self.fusion.condition(measured, columns, tick, camera_start, camera_end, received)?;
        serde_json::to_writer(&mut self.evidence, &evidence).map_err(|e| invalid(format!("fusion evidence: {e}")))?;
        writeln!(self.evidence).and_then(|()| self.evidence.flush()).map_err(|e| invalid(format!("fusion evidence: {e}")))?;
        Ok(evidence)
    }

    pub fn failure(&mut self, tick: u64, error: &CnsError) -> Result<(), CnsError> {
        self.fusion = Fusion::default();
        let value = serde_json::json!({"schema_version":"qualia.sensor-conditioned-retina.v1", "tick":tick,
            "received_ms":super::unix_ns()/1_000_000,"hold_reason":error.to_string(),"model_stepped":false});
        serde_json::to_writer(&mut self.evidence, &value).map_err(|e| invalid(e.to_string()))?;
        writeln!(self.evidence).and_then(|()| self.evidence.flush()).map_err(|e| invalid(e.to_string()))
    }

    pub fn output(&mut self, tick: u64, neural_command: i32, gated_command: i32, throttle: f32, max_wheel_speed: f32, fresh_at_output: bool) -> Result<(), CnsError> {
        let turn = gated_command as f32 * throttle * max_wheel_speed;
        let value = serde_json::json!({"schema_version":"qualia.sensor-conditioned-retina.output.v1", "tick":tick,
            "received_ms":super::unix_ns()/1_000_000,"model_stepped":true,"neural_command":neural_command,
            "gated_command":gated_command,"throttle":throttle,"proposed_left":turn,"proposed_right":-turn,
            "max_wheel_speed":max_wheel_speed,"input_fresh_at_output":fresh_at_output});
        serde_json::to_writer(&mut self.evidence, &value).map_err(|e| invalid(e.to_string()))?;
        writeln!(self.evidence).and_then(|()| self.evidence.flush()).map_err(|e| invalid(e.to_string()))
    }
}
