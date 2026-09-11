//! End-to-end checks of the `qualia-arena-recorder` binary.
//!
//! Each test starts the real child process against shared-memory regions this
//! test created and publishes into, then reads the session back the way a
//! downstream consumer does: the operator lines on stderr, the exit status, and
//! the JSON records on each MCAP topic.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use qualia_mcap::{
    read_window, LoggedMessage, TOPIC_ACTION_APPLIED, TOPIC_ACTION_CLAMPED,
    TOPIC_ACTION_REQUESTED, TOPIC_BELIEF, TOPIC_CAMERA, TOPIC_HEALTH, TOPIC_LIDAR, TOPIC_PLANNER,
    TOPIC_POSE, TOPIC_VSLAM,
};
use qualia_shm::{LayerWriter, ShmRegion};
use qualia_types::{
    AppliedActionSnapshot, CameraFrameSnapshot, LidarOccupancyGridSnapshot, LidarPoint,
    LidarScanSnapshot, NavGoal, NavPose, ACTION_AUTHORITY_LEASH, LIDAR_GRID_H, LIDAR_GRID_W,
};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_qualia-arena-recorder");
const SESSION: &str = "it-session";
const WAIT: Duration = Duration::from_secs(60);

fn unique_region(tag: &str) -> String {
    format!("/qualia-arena-recorder-{tag}-{}", std::process::id())
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos() as u64
}

fn single_source_json(entity: &str, shm_name: &str) -> String {
    format!(
        r#"[{{"entity":"{entity}","shm_name":"{shm_name}","calibration_id":"{entity}-cal","primary":true}}]"#
    )
}

/// Publishes one coherent physical frame of every kind the recorder reads.
fn publish_physical_state(region: &ShmRegion, stamp: u64) {
    let mut scan = LidarScanSnapshot {
        seq: 0,
        scan_start_ns: stamp,
        scan_end_ns: stamp + 1_000_000,
        point_count: 2,
        ..LidarScanSnapshot::default()
    };
    scan.points[0] = LidarPoint {
        angle_rad: 0.0,
        distance_m: 2.0,
        intensity: 200,
        _pad: [0; 3],
    };
    scan.points[1] = LidarPoint {
        angle_rad: 0.5,
        distance_m: 3.0,
        intensity: 100,
        _pad: [0; 3],
    };
    region.lidar_scan_mut().publish(&scan).expect("scan");

    let mut grid = LidarOccupancyGridSnapshot::default();
    grid.resolution_m = 0.05;
    grid.origin_x_m = -(LIDAR_GRID_W as f32 * grid.resolution_m * 0.5);
    grid.origin_y_m = -(LIDAR_GRID_H as f32 * grid.resolution_m * 0.5);
    region.lidar_grid_mut().publish(&grid).expect("grid");

    let frame = CameraFrameSnapshot {
        seq: 1,
        timestamp_ns: stamp,
        source_width: 64,
        source_height: 48,
        thumb_width: 64,
        thumb_height: 48,
        luminance_mean: 0.5,
        luminance_stddev: 0.25,
        valid: true,
        ..CameraFrameSnapshot::default()
    };
    region.camera_frame_mut().publish(&frame).expect("frame");

    {
        let vslam = region.vslam_frontend_mut();
        vslam.timestamp_ns = stamp;
        vslam.frame_seq = 7;
        vslam.feature_count = 120;
        vslam.keyframe_count = 3;
        vslam.loop_closure_count = 1;
        vslam.tracking_confidence = 0.9;
        vslam.pose_confidence = 0.8;
        vslam.tracking_ok.store(true, Ordering::Release);
        vslam.seq.store(1, Ordering::Release);
    }

    {
        let world = region.world_model_mut();
        world.robot_pose = NavPose {
            x_m: 1.0,
            y_m: 2.0,
            z_m: 0.0,
            yaw_rad: 0.1,
            pitch_rad: 0.0,
            roll_rad: 0.0,
            confidence: 0.9,
            _pad0: 0.0,
            timestamp_ns: stamp,
        };
        world.nav_goal = NavGoal {
            active: 1,
            _pad0: [0; 3],
            cell_x: 3,
            cell_z: 4,
            x_m: 5.0,
            y_m: 6.0,
            z_m: 0.0,
            yaw_rad: 0.2,
            timestamp_ns: stamp,
        };
        world.nav_seq.store(1, Ordering::Release);
    }

    let belief = LayerWriter::new(region.layer_slot(0));
    belief.back_buffer().timestamp_ns = stamp;
    belief.back_buffer().vfe = 1.25;
    belief.back_buffer().challenge_vfe = 0.5;
    belief.back_buffer().cycle_us = 400;
    belief.back_buffer().compression = 3;
    belief.publish();
}

/// A running recorder child plus the stderr lines it has produced so far.
struct ChildRun {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl ChildRun {
    fn start(root: &Path, sources: &str, duration_secs: Option<u64>) -> Self {
        let mut command = Command::new(BIN);
        command
            .env("QUALIA_MCAP_ROOT", root)
            .env("QUALIA_MCAP_SOURCES_JSON", sources)
            .env("QUALIA_ARENA_SESSION", SESSION)
            .env_remove("QUALIA_MCAP_DURATION_SECONDS")
            .env_remove("QUALIA_AGENT_URL")
            .env_remove("QUALIA_SESSION_STORE")
            .env_remove("QUALIA_SESSION_ID");
        if let Some(secs) = duration_secs {
            command.env("QUALIA_MCAP_DURATION_SECONDS", secs.to_string());
        }
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("recorder binary spawns");
        let stderr = child.stderr.take().expect("stderr is piped");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    fn wait_for_line(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out waiting for {needle:?}; saw {:?}",
                self.seen
            );
            match self.lines.recv_timeout(left.min(Duration::from_millis(500))) {
                Ok(line) => {
                    let matched = line.contains(needle);
                    self.seen.push(line.clone());
                    if matched {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("stderr closed waiting for {needle:?}; saw {:?}", self.seen)
                }
            }
        }
    }

    fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().expect("child status") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "recorder did not exit; saw {:?}",
                self.seen
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn drain(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            self.seen.push(line);
        }
    }
}

impl Drop for ChildRun {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn records(path: &Path, topic: &str) -> Vec<Value> {
    logged(path, topic)
        .iter()
        .map(|message| serde_json::from_slice(&message.data).expect("json record"))
        .collect()
}

fn logged(path: &Path, topic: &str) -> Vec<LoggedMessage> {
    read_window(path, Some(topic), 0, u64::MAX).expect("session replays")
}

/// Splits `qualia-arena-recorder: completed <path> sha256=<hex> channels=<n>`.
fn parse_completed(line: &str, root: &Path) -> (PathBuf, String, usize) {
    let rest = line
        .strip_prefix("qualia-arena-recorder: completed ")
        .expect("completed line prefix");
    let (path, tail) = rest.rsplit_once(" sha256=").expect("sha256 field");
    let (sha256, channels) = tail.split_once(" channels=").expect("channels field");
    assert_eq!(
        PathBuf::from(path).parent(),
        Some(root),
        "session sealed under the configured root"
    );
    (PathBuf::from(path), sha256.to_string(), channels.parse().unwrap())
}

#[test]
fn an_empty_source_list_fails_with_the_operator_diagnostic() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut run = ChildRun::start(temp.path(), "[]", None);
    let line = run.wait_for_line("qualia-arena-recorder: ");
    assert_eq!(
        line,
        "qualia-arena-recorder: at least one MCAP entity source is required"
    );
    assert_eq!(run.wait_for_exit().code(), Some(1));
}

#[test]
fn two_primary_sources_are_rejected_before_any_region_opens() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sources = r#"[
        {"entity":"guard","shm_name":"/qualia-arena-recorder-absent-a","calibration_id":"a","primary":true},
        {"entity":"pinkie","shm_name":"/qualia-arena-recorder-absent-b","calibration_id":"b","primary":true}
    ]"#;
    let mut run = ChildRun::start(temp.path(), sources, None);
    let line = run.wait_for_line("qualia-arena-recorder: ");
    assert_eq!(
        line,
        "qualia-arena-recorder: exactly one MCAP entity source must be primary"
    );
    assert_eq!(run.wait_for_exit().code(), Some(1));
}

#[test]
fn one_session_records_the_whole_physical_and_belief_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let shm_name = unique_region("guard");
    let region = ShmRegion::create(&shm_name).expect("region");
    let stamp = now_ns();
    publish_physical_state(&region, stamp);

    let mut run = ChildRun::start(&root, &single_source_json("guard", &shm_name), Some(2));
    let banner = run.wait_for_line("recording session=");
    assert_eq!(
        banner,
        format!(
            "qualia-arena-recorder: recording session={SESSION} sources=guard:{shm_name}(primary) root={}",
            root.display()
        )
    );

    // Appended after startup on purpose: the recorder must only carry intervals
    // that completed inside the session, and must drain all of them.
    let first = AppliedActionSnapshot {
        producer_epoch: 11,
        action_sequence: 1,
        interval_start_ns: stamp,
        interval_end_ns: stamp + 1_000_000,
        speed_scale: 1.0,
        authority: ACTION_AUTHORITY_LEASH,
        valid: true,
        ..AppliedActionSnapshot::default()
    };
    let second = AppliedActionSnapshot {
        action_sequence: 2,
        interval_start_ns: first.interval_end_ns,
        interval_end_ns: stamp + 2_000_000,
        ..first
    };
    region
        .applied_action_history()
        .append(first)
        .expect("first interval");
    region
        .applied_action_history()
        .append(second)
        .expect("second interval");
    region.applied_action().publish(second).expect("slot");

    let completed = run.wait_for_line("qualia-arena-recorder: completed ");
    let status = run.wait_for_exit();
    run.drain();
    assert!(status.success(), "exit {status:?}; stderr {:?}", run.seen);
    let (path, sha256, channels) = parse_completed(&completed, &root);
    assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("it-session.mcap"));
    assert_eq!(sha256.len(), 64, "sha256 of the sealed file");
    assert!(sha256.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(channels, 12, "every required topic is declared");

    // The first record of the session is the pre-published camera frame, and
    // the session sequence only ever grows.
    let all = read_window(&path, None, 0, u64::MAX).expect("session replays");
    assert_eq!(all[0].topic, TOPIC_CAMERA);
    assert!(
        all.windows(2).all(|pair| pair[0].sequence < pair[1].sequence),
        "session sequence is monotonic: {:?}",
        all.iter().map(|message| message.sequence).collect::<Vec<_>>()
    );

    let cameras = records(&path, TOPIC_CAMERA);
    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0]["schema_version"], "qualia.camera-evidence.v1");
    assert_eq!(cameras[0]["entity"], "guard");
    assert_eq!(cameras[0]["calibration_id"], "guard-cal");
    assert_eq!(cameras[0]["source_sequence"], 1);
    assert_eq!(cameras[0]["timestamp_ns"], stamp);
    assert_eq!(cameras[0]["valid"], true);
    assert_eq!(logged(&path, TOPIC_CAMERA)[0].log_time_ns, stamp);

    let lidar = records(&path, TOPIC_LIDAR);
    assert_eq!(lidar.len(), 1);
    assert_eq!(lidar[0]["schema_version"], "qualia.lidar-evidence.v1");
    assert_eq!(lidar[0]["entity"], "guard");
    assert_eq!(lidar[0]["points"].as_array().map(Vec::len), Some(2));
    assert_eq!(lidar[0]["points"][1]["distance_m"], 3.0);
    assert_eq!(lidar[0]["validity_mask"], serde_json::json!([true, true]));
    let occupancy = &lidar[0]["occupancy"];
    assert_eq!(occupancy["width"], LIDAR_GRID_W as u64);
    assert_eq!(occupancy["height"], LIDAR_GRID_H as u64);
    assert_eq!(
        occupancy["observed"].as_array().map(Vec::len),
        occupancy["cells"].as_array().map(Vec::len)
    );
    assert!(
        occupancy["observed"]
            .as_array()
            .expect("observed mask")
            .iter()
            .any(|cell| cell == &Value::Bool(true)),
        "the scan marks the cells its rays cross"
    );

    for (topic, stage) in [
        (TOPIC_ACTION_REQUESTED, "requested"),
        (TOPIC_ACTION_CLAMPED, "safety_clamped"),
        (TOPIC_ACTION_APPLIED, "transport_accepted"),
    ] {
        let actions = records(&path, topic);
        assert_eq!(actions.len(), 2, "{topic} drains every committed interval");
        assert_eq!(actions[0]["source_sequence"], 1);
        assert_eq!(actions[1]["source_sequence"], 2);
        for action in &actions {
            assert_eq!(action["schema_version"], "qualia.action-evidence.v1");
            assert_eq!(action["entity"], "guard");
            assert_eq!(action["stage"], stage);
            assert_eq!(action["producer_epoch"], 11);
            assert!(action["history_sequence"].as_u64().unwrap() > 0);
            assert_eq!(action["timestamp_ns"], action["interval_end_ns"]);
        }
    }
    let applied = records(&path, TOPIC_ACTION_APPLIED);
    assert_eq!(applied[1]["left"], applied[1]["applied_left"]);
    assert_eq!(applied[1]["right"], applied[1]["applied_right"]);
    assert_eq!(applied[1]["interval_end_ns"], stamp + 2_000_000);

    let poses = records(&path, TOPIC_POSE);
    assert_eq!(poses.len(), 1);
    assert_eq!(poses[0]["schema_version"], "qualia.pose-evidence.v1");
    assert_eq!(poses[0]["entity"], "guard");
    assert_eq!(poses[0]["source_sequence"], 1);
    assert_eq!(poses[0]["x_m"], 1.0);

    let planners = records(&path, TOPIC_PLANNER);
    assert_eq!(planners.len(), 1);
    assert_eq!(planners[0]["authority"], "existing-planner-observe-only");
    assert_eq!(planners[0]["goal"]["active"], true);
    assert_eq!(planners[0]["goal"]["cell_x"], 3);
    assert_eq!(planners[0]["goal"]["selected_at_ns"], stamp);

    let vslam = records(&path, TOPIC_VSLAM);
    assert_eq!(vslam.len(), 1);
    assert_eq!(vslam[0]["schema_version"], "qualia.vslam-evidence.v1");
    assert_eq!(vslam[0]["tracking"], true);
    assert_eq!(vslam[0]["frame_seq"], 7);
    assert_eq!(vslam[0]["feature_count"], 120);

    let beliefs = records(&path, TOPIC_BELIEF);
    assert_eq!(beliefs.len(), 1);
    assert_eq!(beliefs[0]["schema_version"], "qualia.belief-evidence.v1");
    assert_eq!(beliefs[0]["layer"], 0);
    assert_eq!(beliefs[0]["vfe"], 1.25);
    assert_eq!(beliefs[0]["source_sequence"], stamp);

    let health = records(&path, TOPIC_HEALTH);
    assert_eq!(health.len(), 1);
    assert_eq!(health[0]["schema_version"], "qualia.health-evidence.v1");
    let layers = health[0]["layers"].as_array().expect("layer summaries");
    assert_eq!(layers.len(), 8);
    assert_eq!(layers[0]["layer"], 0);
    assert_eq!(layers[0]["timestamp_ns"], stamp);
    assert_eq!(layers[0]["cycle_us"], 400);
}

#[test]
fn a_duration_bounded_session_seals_without_a_signal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let shm_name = unique_region("timed");
    let region = ShmRegion::create(&shm_name).expect("region");
    publish_physical_state(&region, now_ns());

    let mut run = ChildRun::start(&root, &single_source_json("timed", &shm_name), Some(1));
    let completed = run.wait_for_line("qualia-arena-recorder: completed ");
    assert!(run.wait_for_exit().success());
    run.drain();
    let (path, _, _) = parse_completed(&completed, &root);
    assert!(path.is_file(), "the partial was renamed to {path:?}");
    assert!(!root.join("it-session.mcap.partial").exists());
}
