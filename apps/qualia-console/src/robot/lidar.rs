use serde_json::Value;

pub const SIDE: usize = 160;

#[derive(Clone, Default)]
pub struct Scan {
    pub timestamp_ms: u64,
    pub frame: String,
    pub rate_hz: Option<f64>,
    pub points: Vec<[f32; 2]>,
    pub rays: Vec<([f32; 2], bool)>,
}

impl Scan {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let scan = &value["sensors"]["range_scan"];
        if scan["status"].as_str() != Some("available") {
            return Err(format!("Lidar unavailable: {}", scan["error"]));
        }
        let sample = &scan["sample"];
        let number = |key: &str| sample[key].as_f64().filter(|v| v.is_finite())
            .ok_or_else(|| format!("Lidar has no valid {key}"));
        let angle = number("angle_min_rad")?;
        let increment = number("angle_increment_rad")?;
        let min = number("range_min_m")?;
        let max = number("range_max_m")?;
        if max <= min || min < 0.0 || max > 100.0 || increment == 0.0 {
            return Err("Lidar scan geometry is invalid".into());
        }
        let ranges = sample["ranges_m"].as_array().filter(|v| !v.is_empty() && v.len() <= 10000)
            .ok_or("Lidar has no bounded range array")?;
        let mut result = Self {
            timestamp_ms: sample["ts_ms"].as_u64().filter(|v| *v > 0).ok_or("Lidar timestamp absent")?,
            frame: sample["frame_id"].as_str().unwrap_or("unspecified").to_owned(),
            rate_hz: sample["scan_rate_hz"].as_f64(), ..Self::default()
        };
        for (index, value) in ranges.iter().enumerate() {
            let Some(range) = value.as_f64().filter(|r| r.is_finite() && *r >= min && *r <= max) else { continue; };
            let theta = angle + index as f64 * increment;
            let point = [(range * theta.cos()) as f32, (range * theta.sin()) as f32];
            let hit = range < max;
            result.rays.push((point, hit));
            if hit { result.points.push(point); }
        }
        Ok(result)
    }

    pub fn is_fresh(&self, now: u64, advanced_ms: u64) -> bool {
        self.timestamp_ms <= now.saturating_add(1000)
            && now.saturating_sub(self.timestamp_ms) <= 1500
            && now.saturating_sub(advanced_ms) <= 1500
    }

    /// A fresh grid in the scan's own frame. Free space follows measured rays;
    /// endpoint hits are marked last so crossing rays cannot erase obstacles.
    pub fn occupancy(&self, extent: f32) -> Vec<u8> {
        let mut cells = vec![0; SIDE * SIDE];
        if !extent.is_finite() || extent <= 0.0 { return cells; }
        let mut occupied = Vec::new();
        let cell = 2.0 * extent / SIDE as f32;
        let index = |p: [f32; 2]| -> Option<usize> {
            let x = ((p[0] + extent) / cell).floor() as isize;
            let y = ((extent - p[1]) / cell).floor() as isize;
            (x >= 0 && y >= 0 && x < SIDE as isize && y < SIDE as isize)
                .then_some(y.max(0) as usize * SIDE + x.max(0) as usize)
        };
        for (point, hit) in &self.rays {
            let distance = point[0].hypot(point[1]);
            let steps = (distance / (cell * 0.5)).ceil().clamp(1.0, 4000.0) as usize;
            for step in 0..steps {
                let fraction = step as f32 / steps as f32;
                if let Some(i) = index([point[0] * fraction, point[1] * fraction]) { cells[i] = 1; }
            }
            if *hit { if let Some(i) = index(*point) { occupied.push(i); } }
        }
        for i in occupied { cells[i] = 2; }
        cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn occupancy_keeps_unknown_space_and_measured_hits() {
        let scan = Scan { rays: vec![([2.0, 0.0], true)], ..Scan::default() };
        let cells = scan.occupancy(4.0);
        assert_eq!(cells[80 * SIDE + 120], 2);
        assert_eq!(cells[80 * SIDE + 100], 1);
        assert_eq!(cells[80 * SIDE + 140], 0);
        assert_eq!(cells[20 * SIDE + 20], 0);
    }
    #[test]
    fn missing_timestamp_and_invalid_ranges_are_not_live_points() {
        let mut value = serde_json::json!({"sensors":{"range_scan":{"status":"available","sample":{
            "angle_min_rad":0,"angle_increment_rad":0.1,"range_min_m":0.02,"range_max_m":12,
            "ranges_m":[null,-1,0,2,12,15],"ts_ms":100
        }}}});
        let scan = Scan::parse(&value).unwrap();
        assert_eq!(scan.points.len(), 1);
        assert_eq!(scan.rays.len(), 2);
        value["sensors"]["range_scan"]["sample"]["ts_ms"] = Value::Null;
        assert!(Scan::parse(&value).is_err());
    }

    #[test]
    fn a_future_clock_or_stalled_scan_cannot_stay_live() {
        let mut scan = Scan { timestamp_ms: 5000, ..Scan::default() };
        assert!(!scan.is_fresh(1000, 1000));
        scan.timestamp_ms = 1100;
        assert!(scan.is_fresh(1000, 1000));
        assert!(!scan.is_fresh(2700, 1000));
        assert!(scan.occupancy(f32::NAN).iter().all(|v| *v == 0));
    }
}
