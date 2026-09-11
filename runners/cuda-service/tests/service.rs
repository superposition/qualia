//! What a compute client observes from the `qualia-cuda-service` process.
//!
//! The tests drive the real binary (`CARGO_BIN_EXE_qualia-cuda-service`) and
//! speak the wire protocol at it: one newline-delimited JSON request per
//! connection, one JSON response back. Nothing here reaches into the crate, so
//! the JSON field names, the result types, the error codes and the operator log
//! are pinned by the process rather than by a private item.

use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::UnixStream as Wire;
#[cfg(windows)]
use std::net::TcpStream as Wire;

const BIN: &str = env!("CARGO_BIN_EXE_qualia-cuda-service");
const INSTANCE: &str = "cuda-service-test-instance";
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs one check against one worker the check owns.
///
/// The checks are serialized and each worker is killed when its body returns:
/// the harness must not leave a listener or a CUDA context behind, and the
/// device must only ever have one context open at a time.
fn with_worker(body: impl FnOnce(&Service)) {
    static ONLY_ONE: Mutex<()> = Mutex::new(());
    let _serial = ONLY_ONE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let worker = Service::start();
    body(&worker);
}

/// One running worker, with its operator log captured to a file.
struct Service {
    child: Mutex<Child>,
    endpoint: String,
    log_path: PathBuf,
}

impl Service {
    fn start() -> Service {
        let endpoint = free_endpoint();
        let log_path = std::env::temp_dir().join(format!(
            "qualia-cuda-service-{}.log",
            std::process::id()
        ));
        let log = File::create(&log_path).expect("create the service log file");
        let child = Command::new(BIN)
            .env("QUALIA_COMPUTE_SOCKET", &endpoint)
            .env("QUALIA_SERVICE_INSTANCE", INSTANCE)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().expect("clone the log handle")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn qualia-cuda-service");

        let service = Service {
            child: Mutex::new(child),
            endpoint,
            log_path,
        };
        service.await_listener();
        service
    }

    /// Blocks until the endpoint accepts a connection, or panics with the log
    /// the worker left when it died first.
    fn await_listener(&self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if connect(&self.endpoint).is_ok() {
                return;
            }
            {
                let mut child = self.child.lock().expect("service child lock");
                if let Some(status) = child.try_wait().expect("probe the service") {
                    panic!(
                        "the worker exited with {status} before listening on {}\n{}",
                        self.endpoint,
                        self.log()
                    );
                }
            }
            if Instant::now() >= deadline {
                panic!(
                    "no listener on {} after {READY_TIMEOUT:?}\n{}",
                    self.endpoint,
                    self.log()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Sends one request and returns the decoded response.
    fn request(&self, request: &Value) -> Value {
        round_trip(
            &self.endpoint,
            &serde_json::to_string(request).expect("encode request"),
        )
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let mut child = self.child.lock().expect("service child lock");
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn connect(endpoint: &str) -> std::io::Result<Wire> {
    Wire::connect(endpoint)
}

/// One request per connection: write the line, flush, read the answer line.
fn round_trip(endpoint: &str, line: &str) -> Value {
    let mut stream = connect(endpoint).expect("connect to the worker");
    stream.write_all(line.as_bytes()).expect("write the request");
    stream.write_all(b"\n").expect("terminate the request");
    stream.flush().expect("flush the request");

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).expect("read the response");
    assert!(
        !response.trim().is_empty(),
        "the worker answered nothing for {line}"
    );
    serde_json::from_str(response.trim()).expect("decode the response")
}

#[cfg(unix)]
fn free_endpoint() -> String {
    // The worker removes a stale socket file itself before binding.
    std::env::temp_dir()
        .join(format!("qualia-cuda-service-{}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn free_endpoint() -> String {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe a free port");
    let endpoint = probe.local_addr().expect("probe the address").to_string();
    drop(probe);
    endpoint
}

fn capabilities_request() -> Value {
    json!({
        "schema_version": "compute.v1",
        "request_id": "req_caps",
        "request_type": "capabilities",
        "timestamp_ns": 1,
    })
}

fn pose(cell: [i32; 2], x_m: f32, z_m: f32) -> Value {
    json!({
        "cell_x": cell[0],
        "cell_z": cell[1],
        "x_m": x_m,
        "y_m": 0.0,
        "z_m": z_m,
        "yaw_rad": 0.0,
    })
}

fn planner_request(
    algorithm: &str,
    width: u32,
    depth: u32,
    start: [i32; 2],
    goal: [i32; 2],
    occupied: Vec<u8>,
    cost: Vec<u8>,
) -> Value {
    json!({
        "schema_version": "compute.v1",
        "request_id": "req_plan",
        "request_type": "plan_path",
        "timestamp_ns": 42,
        "algorithm": algorithm,
        "start": pose(start, -0.91, 0.19),
        "goal": pose(goal, 0.23, -0.17),
        "grid": {
            "width": width,
            "depth": depth,
            "resolution_m": 0.4,
            "occupied": occupied,
            "cost": cost,
        },
        "constraints": {
            "robot_radius_cells": 0,
            "allow_unknown": false,
            "max_iterations": 50000,
        },
        "world_context": { "nav_seq": 3, "voxel_seq": 8, "footprint_seq": 5 },
    })
}

fn cells(response: &Value) -> Vec<[i64; 2]> {
    response["path"]
        .as_array()
        .expect("path array")
        .iter()
        .map(|point| {
            [
                point["cell_x"].as_i64().expect("cell_x"),
                point["cell_z"].as_i64().expect("cell_z"),
            ]
        })
        .collect()
}

#[test]
fn capabilities_announce_the_planner_limits_and_the_service_identity() {
    with_worker(|service| {
        let response = service.request(&capabilities_request());

        assert_eq!(response["schema_version"], "compute.v1");
        assert_eq!(response["service_instance"], INSTANCE);
        assert_eq!(response["request_id"], "req_caps");
        assert_eq!(response["result_type"], "capabilities");
        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["planner_algorithms"],
            json!(["grid_astar", "uniform_cost"])
        );
        assert_eq!(response["max_grid_width"], 32);
        assert_eq!(response["max_grid_depth"], 32);
        assert_eq!(response["max_path_points"], 256);
        assert_eq!(response["supports_object_footprints"], true);
        assert_eq!(response["supports_costmap_debug"], true);
        assert!(
            response["healthy"].is_boolean(),
            "healthy must be a flag: {response}"
        );
        assert!(
            response["cuda"]["device_name"].is_string(),
            "the cuda block names a device: {response}"
        );
        assert!(response["cuda"]["sm"].is_u64(), "sm is a number");
        assert!(
            ["ready", "disabled", "init_failed"].contains(
                &response["cuda"]["status"].as_str().expect("cuda status")
            ),
            "unexpected cuda status: {response}"
        );
    });
}

#[test]
fn the_operator_log_names_the_endpoint_and_the_cuda_runtime() {
    with_worker(|service| {
        // A served request proves the worker is past initialization, so both
        // log lines are already on disk.
        let _ = service.request(&capabilities_request());
        let log = service.log();

        assert!(
            log.contains(&format!(
                "qualia-cuda-service: listening on {}",
                service.endpoint()
            )),
            "the listener announces its endpoint: {log}"
        );
        assert!(
            log.contains("qualia-cuda-service: cuda runtime status="),
            "the worker reports the CUDA runtime it found: {log}"
        );
    });
}

#[test]
fn plan_path_returns_a_straight_grid_path_and_preserves_world_endpoints() {
    with_worker(|service| {
        let response = service.request(&planner_request(
            "grid_astar",
            5,
            3,
            [0, 0],
            [4, 0],
            vec![0; 15],
            vec![0; 15],
        ));

        assert_eq!(response["result_type"], "path_result");
        assert_eq!(response["status"], "ok");
        assert_eq!(response["planning_ms"], 0.0);
        assert_eq!(
            cells(&response),
            vec![[0, 0], [1, 0], [2, 0], [3, 0], [4, 0]]
        );
        assert_eq!(response["summary"]["reachable"], true);
        assert_eq!(response["debug"]["algorithm"], "grid_astar");
        assert_eq!(response["debug"]["costmap_seq"], 8);
        assert!(response["error"].is_null());

        let path = response["path"].as_array().expect("path array");
        assert_eq!(path[0]["x_m"], -0.91, "the first point is the start pose");
        assert_eq!(path[0]["z_m"], 0.19);
        let last = path.last().expect("a last point");
        assert_eq!(last["x_m"], 0.23, "the last point is the goal pose");
        assert_eq!(last["z_m"], -0.17);
    });
}

#[test]
fn a_blocked_goal_is_reported_as_goal_blocked() {
    with_worker(|service| {
        let mut occupied = vec![0u8; 16];
        occupied[3] = 1;
        let response = service.request(&planner_request(
            "grid_astar",
            4,
            4,
            [0, 0],
            [3, 0],
            occupied,
            vec![0; 16],
        ));

        assert_eq!(response["result_type"], "error");
        assert_eq!(response["status"], "error");
        assert_eq!(response["error"]["code"], "goal_blocked");
        assert_eq!(response["error"]["retryable"], false);
        assert!(response["error"]["message"].is_string());
    });
}

#[test]
fn a_start_that_intersects_its_footprint_is_reported_as_start_blocked() {
    with_worker(|service| {
        let mut occupied = vec![0u8; 36];
        occupied[2 * 6 + 2 + 6] = 1; // one cell below the start, inside radius one
        let mut request =
            planner_request("grid_astar", 6, 6, [2, 2], [5, 5], occupied, vec![0; 36]);
        request["constraints"]["robot_radius_cells"] = json!(1);

        let response = service.request(&request);
        assert_eq!(response["error"]["code"], "start_blocked");
    });
}

#[test]
fn uniform_cost_routes_around_expensive_cells() {
    with_worker(|service| {
        let mut cost = vec![0u8; 16];
        cost[1] = 200;
        cost[2] = 200;
        let response = service.request(&planner_request(
            "uniform_cost",
            4,
            4,
            [0, 0],
            [3, 0],
            vec![0; 16],
            cost,
        ));

        assert_eq!(response["status"], "ok");
        assert_eq!(response["debug"]["algorithm"], "uniform_cost");
        assert_eq!(
            cells(&response),
            vec![[0, 0], [0, 1], [1, 1], [2, 1], [3, 1], [3, 0]]
        );
    });
}

#[test]
fn a_walled_off_goal_is_reported_as_no_path() {
    with_worker(|service| {
        let mut occupied = vec![0u8; 16];
        for z in 0..4 {
            occupied[z * 4 + 1] = 1;
        }
        let response = service.request(&planner_request(
            "grid_astar",
            4,
            4,
            [0, 0],
            [3, 0],
            occupied,
            vec![0; 16],
        ));

        assert_eq!(response["error"]["code"], "no_path");
    });
}

#[test]
fn an_unknown_request_type_is_refused_as_unsupported_version() {
    with_worker(|service| {
        let response = service.request(&json!({
            "schema_version": "compute.v1",
            "request_id": "req_unknown",
            "request_type": "warp_drive",
            "timestamp_ns": 1,
        }));

        assert_eq!(response["result_type"], "error");
        assert_eq!(response["status"], "error");
        assert_eq!(response["error"]["code"], "unsupported_version");
        assert!(
            response["error"]["message"]
                .as_str()
                .expect("message")
                .contains("unsupported request_type: warp_drive"),
            "the refusal names the request type: {response}"
        );
    });
}

#[test]
fn a_foreign_schema_version_is_refused_by_the_planner() {
    with_worker(|service| {
        let mut request =
            planner_request("grid_astar", 4, 4, [0, 0], [3, 0], vec![0; 16], vec![0; 16]);
        request["schema_version"] = json!("compute.v0");

        let response = service.request(&request);
        assert_eq!(response["error"]["code"], "unsupported_version");
        assert!(response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("compute.v0"));
    });
}

#[test]
fn costmap_stats_reduces_the_grid() {
    with_worker(|service| {
        let occupied = vec![0u8, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
        let cost = vec![
            0u8, 255, 32, 4, 10, 20, 30, 40, 210, 3, 2, 1, 0, 0, 0, 0,
        ];
        let response = service.request(&json!({
            "schema_version": "compute.v1",
            "request_id": "req_stats",
            "request_type": "costmap_stats",
            "timestamp_ns": 7,
            "grid": {
                "width": 4,
                "depth": 4,
                "resolution_m": 0.4,
                "occupied": occupied,
                "cost": cost,
            },
        }));

        assert_eq!(response["result_type"], "costmap_stats");
        assert_eq!(response["status"], "ok");
        assert_eq!(response["request_id"], "req_stats");
        assert_eq!(response["total_cells"], 16);
        assert_eq!(response["occupied_count"], 2);
        assert_eq!(response["high_cost_count"], 2);
        assert_eq!(response["blocked_count"], 2);
        assert_eq!(response["cost_sum"], 607);
        assert_eq!(response["mean_cost"], 607.0 / 16.0);
    });
}

#[test]
fn a_costmap_grid_that_does_not_match_its_dimensions_is_refused() {
    with_worker(|service| {
        let response = service.request(&json!({
            "schema_version": "compute.v1",
            "request_id": "req_bad_stats",
            "request_type": "costmap_stats",
            "timestamp_ns": 7,
            "grid": {
                "width": 4,
                "depth": 4,
                "resolution_m": 0.4,
                "occupied": vec![0u8; 16],
                "cost": vec![0u8; 15],
            },
        }));

        assert_eq!(response["result_type"], "error");
        assert_eq!(response["error"]["code"], "invalid_request");
    });
}
