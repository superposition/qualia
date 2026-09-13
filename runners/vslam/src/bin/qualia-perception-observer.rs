//! Bounded camera/frontend observation. Never publishes a navigation pose.
use std::{fs::{self, OpenOptions}, io::Write, path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use qualia_camera::{capture_once, init_camera_frame, init_camera_preview, CaptureOutcome, SnapshotSource};
use qualia_shm::ShmRegion;
use qualia_vslam::{Frontend, FrontendConfig};
use serde_json::json;

fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }
fn run() -> Result<(), String> {
    let status = PathBuf::from(std::env::var("QUALIA_PERCEPTION_STATUS").map_err(|_| "Missing status path")?);
    let evidence = std::env::var("QUALIA_PERCEPTION_EVIDENCE").map_err(|_| "Missing new evidence path")?;
    let camera = std::env::var("QUALIA_CAMERA_SNAPSHOT_URL").map_err(|_| "Missing real camera URL")?;
    if !camera.starts_with("http://") && !camera.starts_with("https://") { return Err("HTTP camera required".into()); }
    let shm_name = std::env::var("QUALIA_SHM_NAME").map_err(|_| "Missing observation arena name")?;
    let duration: u64 = std::env::var("QUALIA_OBSERVE_SECONDS").unwrap_or_else(|_| "1800".into()).parse().map_err(|_| "Invalid duration")?;
    if !(1..=1800).contains(&duration) { return Err("Duration must be 1..1800 seconds".into()); }
    let shm = ShmRegion::open(&shm_name).map_err(|e| e.to_string())?;
    let mut journal = OpenOptions::new().write(true).create_new(true).open(evidence).map_err(|e| e.to_string())?;
    init_camera_frame(shm.camera_frame_mut());
    init_camera_preview(shm.camera_preview_mut());
    let mut frontend = Frontend::new(FrontendConfig {shm_name, poll_ms:200, publish_pose:false, force_pose:false});
    frontend.init_state(&shm);
    let source = SnapshotSource::http(&camera, Duration::from_millis(1500));
    let start = Instant::now();
    let mut frames = 0;
    while start.elapsed() < Duration::from_secs(duration) {
        let request_ms = now_ms();
        let value = match capture_once(&shm, &source, 0) {
            CaptureOutcome::Published {..} => {
                let frame = shm.camera_frame().snapshot(8).map_err(|e| format!("Camera snapshot: {e:?}"))?;
                let report = frontend.tick(&shm).ok_or("New camera frame was not processed")?;
                frames += 1;
                json!({"schema_version":"qualia.perception-observation.v1","state":"live","published_ms":now_ms(),
                    "camera_url":camera,"camera_request_started_ms":request_ms,"camera_received_ms":frame.timestamp_ns/1_000_000,
                    "capture_timestamp_known":false,"frames":frames,"frame_seq":report.frame_seq,
                    "width":frame.source_width,"height":frame.source_height,"luminance_mean":frame.luminance_mean,
                    "luminance_stddev":frame.luminance_stddev,"features":report.feature_count,
                    "tracking_confidence":report.tracking_confidence,"pose_confidence":report.pose_confidence,
                    "keyframes":report.keyframe_count,"gradient":report.mean_gradient,
                    "pose_writer_active":false,"metric_map_available":false,
                    "limitation":"Thumbnail visual frontend only; camera scale/extrinsics and head motion are uncalibrated. No navigation pose, persistent map or loop closure is established."})
            }
            CaptureOutcome::Unavailable(error) | CaptureOutcome::Corrupt(error) => json!({
                "schema_version":"qualia.perception-observation.v1","state":"unavailable","published_ms":now_ms(),"error":error}),
            CaptureOutcome::Unchanged => continue,
        };
        let payload = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        journal.write_all(&payload).and_then(|()|journal.write_all(b"\n")).and_then(|()|journal.flush()).map_err(|e|e.to_string())?;
        let temp = status.with_extension("tmp");
        if let Err(error) = fs::write(&temp, &payload).and_then(|()|fs::rename(&temp, &status)) {
            eprintln!("perception status publication failed; previous snapshot keeps its age: {error}");
        }
        if frames > 0 && frames % 30 == 0 { println!("perception: processed {frames} real frames; navigation pose publication disabled"); }
        std::thread::sleep(Duration::from_millis(200));
    }
    if frames == 0 { Err("No real camera frames processed".into()) } else { Ok(()) }
}
fn main() -> std::process::ExitCode {
    match run() { Ok(()) => std::process::ExitCode::SUCCESS, Err(e) => {eprintln!("perception observer: {e}"); std::process::ExitCode::FAILURE} }
}
