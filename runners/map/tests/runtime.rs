//! End-to-end checks of the `qualia-map` binary: the child process attaches to
//! a region this test created, integrates a scan this test published, and
//! writes the result back where this test can read it.

use qualia_shm::{LayerWriter, ShmRegion};
use qualia_types::{LidarPoint, LidarScanSnapshot, NavPose, MAP_GRID_H, MAP_GRID_W};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BIN: &str = env!("CARGO_BIN_EXE_qualia-map");

fn unique_name(tag: &str) -> String {
    format!("/qualia-map-it-{tag}-{}", std::process::id())
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos() as u64
}

fn cell_index(gx: i32, gz: i32) -> usize {
    gz as usize * MAP_GRID_W + gx as usize
}

fn read_until_map_line(child: &mut Child, wanted: &str) -> Vec<String> {
    let stdout = child.stdout.take().expect("child stdout is piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let mut lines = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                let done = line.contains(wanted);
                lines.push(line);
                if done {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    lines
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn empty_nav_pose(confidence: f32, timestamp_ns: u64) -> NavPose {
    NavPose {
        x_m: 0.0,
        y_m: 0.0,
        z_m: 0.0,
        yaw_rad: 0.0,
        pitch_rad: 0.0,
        roll_rad: 0.0,
        confidence,
        _pad0: 0.0,
        timestamp_ns,
    }
}

#[test]
fn binary_integrates_a_published_scan_and_reports_it() {
    let name = unique_name("run");
    let shm = ShmRegion::create(&name).expect("create region");

    let start_ns = now_ns();
    let end_ns = start_ns + 1_000_000;
    shm.set_robot_pose(empty_nav_pose(1.0, start_ns));

    let mut snapshot = LidarScanSnapshot::default();
    snapshot.scan_start_ns = start_ns;
    snapshot.scan_end_ns = end_ns;
    snapshot.point_count = 1;
    snapshot.points[0] = LidarPoint {
        angle_rad: 0.0,
        distance_m: 2.0,
        intensity: 255,
        _pad: [0; 3],
    };
    shm.lidar_scan_mut().publish(&snapshot).expect("publish scan");

    let mut child = Command::new(BIN)
        .env("QUALIA_SHM_NAME", &name)
        .env("QUALIA_MAP_POLL_MS", "10")
        .env("QUALIA_MAP_LOG_EVERY_UPDATES", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn qualia-map");

    let lines = read_until_map_line(&mut child, "map_seq=");
    stop(&mut child);

    assert!(
        lines
            .iter()
            .any(|line| line == &format!("qualia-map: starting persistent lidar mapping from shm={name}")),
        "startup line missing from {lines:?}"
    );
    let map_line = lines
        .iter()
        .find(|line| line.contains("map_seq="))
        .unwrap_or_else(|| panic!("map progress line missing from {lines:?}"));
    assert_eq!(
        map_line,
        &format!(
            "qualia-map: map_seq=2 occupied_cells=0 observed_cells=0 bin_occ=0 bin_free=29 bin_unknown=65507 last_update_ns={end_ns}"
        )
    );

    // The child wrote the integrated grids back into the arena this test maps.
    assert_eq!(shm.map_grid().seq.load(std::sync::atomic::Ordering::Acquire), 2);
    assert_eq!(shm.map_grid().log_odds[cell_index(148, 128)], 10);
    assert_eq!(shm.map_grid().last_update_ns, end_ns);
    assert_eq!(shm.binary_map().free_cells, 29);
    assert_eq!(shm.binary_map().unknown_cells, (MAP_GRID_W * MAP_GRID_H) as u32 - 29);
    assert_eq!(shm.binary_map().last_update_ns, end_ns);
}

#[test]
fn publishes_when_belief_lag_exceeded() {
    let name = unique_name("pace");
    let shm = ShmRegion::create(&name).expect("create region");

    // The newest belief commit is far past the stale window, so the pace gate
    // must let the map publish anyway rather than hold it for belief.
    let writer = LayerWriter::new(shm.layer_slot(0));
    writer.back_buffer().timestamp_ns = now_ns() - 10_000_000_000;
    writer.publish();

    let start_ns = now_ns();
    let end_ns = start_ns + 1_000_000;
    shm.set_robot_pose(empty_nav_pose(1.0, start_ns));

    let mut snapshot = LidarScanSnapshot::default();
    snapshot.scan_start_ns = start_ns;
    snapshot.scan_end_ns = end_ns;
    snapshot.point_count = 1;
    snapshot.points[0] = LidarPoint {
        angle_rad: 0.0,
        distance_m: 2.0,
        intensity: 255,
        _pad: [0; 3],
    };
    shm.lidar_scan_mut().publish(&snapshot).expect("publish scan");

    let mut child = Command::new(BIN)
        .env("QUALIA_SHM_NAME", &name)
        .env("QUALIA_MAP_POLL_MS", "10")
        .env("QUALIA_MAP_LOG_EVERY_UPDATES", "1")
        .env("QUALIA_BELIEF_PACE_MS", "250")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn qualia-map");

    let lines = read_until_map_line(&mut child, "map_seq=");
    stop(&mut child);

    let stale_line = lines
        .iter()
        .find(|line| line.contains("belief pace: stale, publishing map at"))
        .unwrap_or_else(|| panic!("stale pace line missing from {lines:?}"));
    // The line reports the lag it measured: roughly the ten seconds the belief
    // tick has been sitting in the slot.
    let lag_ms: u64 = stale_line
        .trim_end_matches(" ms")
        .rsplit(' ')
        .next()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or_else(|| panic!("no lag in the stale pace line {stale_line:?}"));
    assert!(
        (10_000..11_000).contains(&lag_ms),
        "reported lag {lag_ms} ms in {stale_line:?}"
    );

    // Publishing anyway means the map still advanced.
    assert_eq!(
        shm.map_grid().seq.load(std::sync::atomic::Ordering::Acquire),
        2
    );
    assert_eq!(shm.map_grid().log_odds[cell_index(148, 128)], 10);
}

#[test]
fn waits_while_belief_is_inside_the_stale_window() {
    let name = unique_name("pace-wait");
    let shm = ShmRegion::create(&name).expect("create region");

    // A belief tick 300 ms old under a 200 ms pace: the map may not publish
    // until the layers have been silent for 200 * 8 ms, so the stale window is
    // what finally lets it through.
    let writer = LayerWriter::new(shm.layer_slot(0));
    writer.back_buffer().timestamp_ns = now_ns() - 300_000_000;
    writer.publish();

    let start_ns = now_ns();
    let end_ns = start_ns + 1_000_000;
    shm.set_robot_pose(empty_nav_pose(1.0, start_ns));

    let mut snapshot = LidarScanSnapshot::default();
    snapshot.scan_start_ns = start_ns;
    snapshot.scan_end_ns = end_ns;
    snapshot.point_count = 1;
    snapshot.points[0] = LidarPoint {
        angle_rad: 0.0,
        distance_m: 2.0,
        intensity: 255,
        _pad: [0; 3],
    };
    shm.lidar_scan_mut().publish(&snapshot).expect("publish scan");

    let started = Instant::now();
    let mut child = Command::new(BIN)
        .env("QUALIA_SHM_NAME", &name)
        .env("QUALIA_MAP_POLL_MS", "10")
        .env("QUALIA_MAP_LOG_EVERY_UPDATES", "1")
        .env("QUALIA_BELIEF_PACE_MS", "200")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn qualia-map");

    let lines = read_until_map_line(&mut child, "map_seq=");
    let waited = started.elapsed();
    stop(&mut child);

    // The tick is already 300 ms old when the runner starts, so publishing can
    // only happen once the lag passes the 1.6 s stale window.
    assert!(
        waited >= Duration::from_millis(1_000),
        "published after only {waited:?}: {lines:?}"
    );
    let stale_line = lines
        .iter()
        .find(|line| line.contains("belief pace: stale, publishing map at"))
        .unwrap_or_else(|| panic!("stale pace line missing from {lines:?}"));
    let lag_ms: u64 = stale_line
        .trim_end_matches(" ms")
        .rsplit(' ')
        .next()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or_else(|| panic!("no lag in the stale pace line {stale_line:?}"));
    assert!(lag_ms >= 1_600, "released below the stale window: {lag_ms} ms");
    assert_eq!(
        shm.map_grid().seq.load(std::sync::atomic::Ordering::Acquire),
        2
    );
}

#[test]
fn binary_refuses_a_missing_region() {
    let name = unique_name("missing");
    let output = Command::new(BIN)
        .env("QUALIA_SHM_NAME", &name)
        .output()
        .expect("run qualia-map");

    assert!(!output.status.success(), "expected a failing exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("qualia-map: failed to open shm '{name}'")),
        "stderr was {stderr:?}"
    );
}
