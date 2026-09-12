//! The operator-facing surface: the braid view the console and the web page
//! poll, the stack health and perception status derived from shared memory, and
//! the honest answer for a route whose backing subsystem has not landed.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use qualia_shm::ShmRegion;
use support::Harness;

#[tokio::test]
async fn braid_view_carries_the_console_contract() {
    let harness = Harness::new();
    let reply = harness.get("/braid").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();

    let mut keys: Vec<&str> = body.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "generation",
            "last_promotion_ns",
            "last_quarantine_ns",
            "open_missions",
            "schema_version",
            "session_id",
        ],
        "the braid view is the console's contract, field for field"
    );
    assert_eq!(body["schema_version"], "qualia.braid-state.v1");
    assert_eq!(body["generation"], 0);
    assert_eq!(body["open_missions"], 0);
    assert!(body["last_quarantine_ns"].is_null());
    assert!(body["session_id"].is_string());
}

#[tokio::test]
async fn perception_status_reports_absent_then_fresh_camera() {
    let harness = Harness::new();

    let absent = harness.get("/perception/status").await.json();
    assert_eq!(absent["camera"]["available"], false);
    assert_eq!(absent["camera"]["source"], "qualia-shm:camera_frame");
    assert_eq!(absent["movement_input_ready"], false);
    assert!(absent["movement_blocker"].is_string());

    let region = ShmRegion::open(&harness.state.config.shm_name).expect("scratch region");
    let mut frame = qualia_types::CameraFrameSnapshot::default();
    frame.valid = true;
    frame.timestamp_ns = qualia_agent::now_ns();
    frame.source_width = 640;
    frame.source_height = 480;
    frame.thumb_width = 64;
    frame.thumb_height = 48;
    frame.luminance_mean = 120.0;
    frame.luminance_stddev = 18.0;
    let seq = region
        .camera_frame_mut()
        .publish(&frame)
        .expect("camera frame publishes");

    let fresh = harness.get("/perception/status").await.json();
    assert_eq!(fresh["camera"]["available"], true);
    assert_eq!(fresh["camera"]["fresh"], true);
    assert_eq!(fresh["camera"]["source"], "qualia-shm:camera_frame");
    assert_eq!(fresh["camera"]["frame_seq"], seq);
    assert_eq!(fresh["camera"]["source_width"], 640);
    assert_eq!(fresh["camera"]["quality"], "usable");
}

#[tokio::test]
async fn health_ready_reports_the_stack_shape() {
    let harness = Harness::new();
    let reply = harness.get("/health/ready").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();

    assert_eq!(body["schema_version"], "qualia.service-status.v1");
    assert_eq!(body["status_type"], "stack_health");
    assert_eq!(body["healthy"], false, "no compute service answers in this build");
    assert_eq!(body["ready"], false);
    assert_eq!(body["integration"]["status_type"], "integration_status");
    assert_eq!(body["planner"]["planner_ready"], false);
    assert!(body["compute"]["planner_algorithms"]
        .as_array()
        .is_some_and(Vec::is_empty));
}

#[tokio::test]
async fn nav_goal_writes_the_shared_goal() {
    let harness = Harness::new();
    let reply = harness
        .post_json(
            "/nav/goal",
            None,
            serde_json::json!({
                "cell_x": 16,
                "cell_z": 16,
                "x_m": 0.4,
                "z_m": 0.4,
                "y_m": 0.0,
                "yaw_rad": 0.0
            }),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);

    let region = ShmRegion::open(&harness.state.config.shm_name).expect("scratch region");
    let goal = region.world_model().nav_goal;
    assert_eq!(goal.active, 1);
    assert!((goal.x_m - 0.4).abs() < 1e-6);
    assert!((goal.z_m - 0.4).abs() < 1e-6);
    assert_eq!((goal.cell_x, goal.cell_z), (17, 17));

    let cancel = harness.post_json("/nav/cancel", None, serde_json::json!({})).await;
    assert_eq!(cancel.status, StatusCode::NO_CONTENT);
    assert_eq!(region.world_model().nav_goal.active, 0);
}

#[tokio::test]
async fn operator_page_and_static_assets_come_from_the_web_dir() {
    let harness = Harness::new();
    let reply = harness.get("/").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.text().contains("<title>operator</title>"));
}

#[tokio::test]
async fn routes_whose_subsystem_has_not_landed_say_so() {
    let harness = Harness::new();
    let reply = harness.get("/sessions").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    let body = reply.json();
    assert_eq!(body["subsystem"], "qualia-session-store");
    assert!(body["error"].as_str().is_some_and(|error| error.contains("not available")));
}

#[tokio::test]
async fn unknown_routes_are_not_found() {
    let harness = Harness::new();
    assert_eq!(harness.get("/not-a-route").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn status_routes_keep_their_reference_shapes() {
    let harness = Harness::new();

    let theater = harness.get("/thought-theater/config").await.json();
    assert_eq!(theater["enabled"], false);
    assert_eq!(theater["viewer_url"], "");
    assert_eq!(theater["blueprint_name"], "Thought Theater");

    let mcp = harness.get("/mcp/status").await;
    assert_eq!(mcp.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        mcp.text(),
        "session store unavailable",
        "the reference reports exactly this when it cannot build the arena projection"
    );

    let planner = harness.get("/planner/status").await.json();
    assert_eq!(planner["schema_version"], "qualia.service-status.v1");
    assert_eq!(planner["status_type"], "planner_status");
    assert_eq!(planner["planner_ready"], false);

    let capabilities = harness.get("/compute/capabilities").await.json();
    assert_eq!(capabilities["schema_version"], "compute.v1");
    assert_eq!(capabilities["healthy"], false);
    assert_eq!(capabilities["service_instance"], harness.state.config.compute.service_instance);
}

#[tokio::test]
async fn entities_are_empty_without_profiles() {
    let harness = Harness::new();
    let body = harness.get("/entities").await.json();
    assert_eq!(body["schema_version"], "qualia.entities.v1");
    assert!(body["entities"].as_array().is_some_and(Vec::is_empty));
}

/// The reference's auth refusal, body and all: consumers key on the schema and
/// the code, not on prose.
#[tokio::test]
async fn an_unauthenticated_read_gets_the_reference_auth_error() {
    let harness = Harness::with(|config| {
        config.auth.read_token = Some(Arc::from("read-test-token"));
        config.auth.allow_loopback = false;
    });

    let reply = harness.get("/mission-control/missions").await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    let body = reply.json();
    assert_eq!(body["schema_version"], "qualia.auth-error.v1");
    assert_eq!(body["error"], "authentication_required");

    let admitted = harness
        .get_with_token("/mission-control/missions", "read-test-token")
        .await;
    assert_eq!(admitted.status, StatusCode::OK);
}

/// The tool manifest is the reference's: an external agent selects tools by
/// name, so the advertised set must not shrink with this build.
#[tokio::test]
async fn mcp_advertises_the_reference_tool_manifest() {
    let harness = Harness::new();
    let expected = [
        "arena_status",
        "arena_observe",
        "multimodal_observe",
        "arena_start_session",
        "arena_stop_session",
        "arena_submit_intent",
        "studio_focus",
        "studio_control",
        "world_query",
        "world_propose",
        "belief_status",
        "belief_query",
        "belief_sketch",
        "belief_shadow_status",
        "belief_shadow_patch",
        "belief_shadow_publish",
        "belief_propose_feedback",
        "belief_propose_prior",
        "belief_withdraw_prior",
        "embodiment_health",
        "embodiment_observe",
        "embodiment_invoke_capability",
        "embodiment_stop",
        "embodiment_estop",
        "guard_health",
        "guard_observe",
        "guard_invoke_capability",
        "guard_stop",
        "guard_estop",
    ];

    let body = harness.get("/mcp/tools").await.json();
    assert_eq!(body["ok"], true);
    let names: Vec<&str> = body["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, expected, "the advertised tool set is the reference's");
    for tool in body["tools"].as_array().expect("tools") {
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert!(tool["safety"].is_string(), "a descriptor carries its safety class");
    }

    let listed = harness
        .post_json(
            "/mcp",
            None,
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await
        .json();
    let protocol_names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(protocol_names, expected, "`tools/list` advertises the same set");
    assert!(listed["result"]["tools"][0]["safety"].is_null());

    let initialized = harness
        .post_json(
            "/mcp",
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }),
        )
        .await
        .json();
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        initialized["result"]["instructions"],
        "Qualia is the shared superposed world model. Read arena_observe/world_query, submit typed intent or world proposals, and let Leash retain all physical safety authority.",
        "the protocol instructions are the reference's, word for word"
    );

    let unknown = harness
        .post_json(
            "/mcp",
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "belief_status", "arguments": {} }
            }),
        )
        .await
        .json();
    assert_eq!(unknown["result"]["isError"], true);
    assert!(unknown["result"]["content"][0]["text"]
        .as_str()
        .is_some_and(|text| text.contains("the cognition runtime")));
}

/// `/compute/cuda-smoke` reports a smoke-test result, never the error envelope.
#[tokio::test]
async fn cuda_smoke_reports_a_result_body() {
    let harness = Harness::new();
    let reply = harness.get("/compute/cuda-smoke").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    let body = reply.json();

    let mut keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "binary_path",
            "compiler",
            "reason",
            "request_id",
            "result_type",
            "schema_version",
            "service_instance",
            "status",
            "stderr",
            "stdout",
        ],
        "the result carries the reference's fields, not the error envelope"
    );
    assert_eq!(body["schema_version"], "compute.v1");
    assert_eq!(body["result_type"], "cuda_smoke");
    assert_eq!(body["status"], "error");
    assert!(body["reason"].as_str().is_some_and(|reason| !reason.is_empty()));
}

/// `/compute/path` names the missing planner input before it names the missing
/// service: pose first, then lidar.
#[tokio::test]
async fn compute_path_checks_pose_then_lidar_before_the_service() {
    let harness = Harness::new();

    let no_pose = harness.post_json("/compute/path", None, serde_json::json!({})).await;
    assert_eq!(no_pose.status, StatusCode::SERVICE_UNAVAILABLE);
    let no_pose = no_pose.json();
    assert_eq!(no_pose["error"]["code"], "pose_unavailable");
    assert_eq!(no_pose["result_type"], "error");
    assert_eq!(no_pose["error"]["retryable"], true);

    let region = ShmRegion::open(&harness.state.config.shm_name).expect("scratch region");
    region.set_robot_pose(qualia_types::NavPose {
        x_m: 0.0,
        y_m: 0.0,
        z_m: 0.0,
        yaw_rad: 0.0,
        pitch_rad: 0.0,
        roll_rad: 0.0,
        confidence: 1.0,
        _pad0: 0.0,
        timestamp_ns: qualia_agent::now_ns(),
    });

    let no_lidar = harness.post_json("/compute/path", None, serde_json::json!({})).await;
    assert_eq!(no_lidar.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(no_lidar.json()["error"]["code"], "lidar_unavailable");
}

fn jpeg_preview(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff];
    bytes.extend_from_slice(payload);
    bytes
}

/// `/perception/frame` serves exactly the bytes the camera runner published,
/// with the content type the encoding implies, and refuses an empty slot.
#[tokio::test]
async fn encoded_frame_serves_the_published_preview_verbatim() {
    let harness = Harness::new();
    assert_eq!(
        harness.get("/perception/frame").await.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "an empty slot is not a frame"
    );

    let region = ShmRegion::open(&harness.state.config.shm_name).expect("scratch region");
    let encoded = jpeg_preview(&[0xaa; 4096]);
    region
        .camera_preview_mut()
        .publish(1, 640, 480, qualia_agent::now_ns(), &encoded)
        .expect("publish the preview");

    let reply = harness.get("/perception/frame").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type(), Some("image/jpeg"));
    assert_eq!(reply.bytes, encoded);

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0x55; 512]);
    region
        .camera_preview_mut()
        .publish(2, 64, 48, qualia_agent::now_ns(), &png)
        .expect("publish the preview");
    let reply = harness.get("/perception/frame").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type(), Some("image/png"));
    assert_eq!(reply.bytes, png);

    // A cleared slot carries no bytes: the sequence advanced, the payload did
    // not, which is what the camera runner leaves behind for an unwanted
    // encoding.
    region.camera_preview_mut().clear().expect("clear the slot");
    assert_eq!(
        harness.get("/perception/frame").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

/// The preview endpoint must never hand an operator a copy that mixed two
/// publishes. Its reader is the slot's own snapshot, which copies the payload,
/// fences acquire, and only then re-reads the sequence, so a copy straddling a
/// publish is rejected rather than served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_encoded_frame_endpoint_never_serves_a_torn_preview() {
    let harness = Harness::new();
    // The first request creates the scratch region; open it only after that.
    assert_eq!(
        harness.get("/perception/frame").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let region = Arc::new(ShmRegion::open(&harness.state.config.shm_name).expect("scratch region"));
    let small = jpeg_preview(&[0xaa; 4096]);
    let large = jpeg_preview(&[0x55; 16_384]);
    let stop = Arc::new(AtomicBool::new(false));
    let reading = Arc::new(AtomicBool::new(false));
    let state = harness.state.clone();

    let writer = tokio::task::spawn_blocking({
        let region = Arc::clone(&region);
        let stop = Arc::clone(&stop);
        let reading = Arc::clone(&reading);
        let (small, large) = (small.clone(), large.clone());
        move || {
            let mut round = 0u64;
            while !stop.load(Ordering::Relaxed) {
                // Wait for the reader to enter a copy, then publish just late
                // enough that the publish lands inside that copy. This is the
                // interleaving the endpoint has to survive: a reader that
                // closes its window before the copy serves both fills.
                while !reading.load(Ordering::Acquire) {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::hint::spin_loop();
                }
                let until = Instant::now() + Duration::from_micros(1);
                while Instant::now() < until {
                    std::hint::spin_loop();
                }
                let bytes = if round % 2 == 0 { &small } else { &large };
                region
                    .camera_preview_mut()
                    .publish(1, 64, 48, 1, bytes)
                    .expect("publish the preview");
                round += 1;
                // Let the reader finish the call (and retry past this write)
                // before the next one starts, so it always has a window.
                while reading.load(Ordering::Acquire) && !stop.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            }
        }
    });

    let mut accepted = 0u64;
    let mut torn = 0u64;
    let deadline = Instant::now() + Duration::from_secs(30);
    while accepted < 2000 && Instant::now() < deadline {
        reading.store(true, Ordering::Release);
        // The handler directly rather than through the router, so the reader's
        // copy starts while the flag above is set; the router path itself is
        // covered by the contract test.
        let response = qualia_agent::perception::encoded_frame_get(State(state.clone())).await;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .to_vec();
        reading.store(false, Ordering::Release);
        match status {
            StatusCode::OK => {
                if body.len() < 4 {
                    continue;
                }
                // Both encodings carry the same three-byte JPEG prefix, so
                // every payload byte must match: a copy that mixed two
                // publishes would show both fills.
                let fill = body[3];
                if !body[3..].iter().all(|&byte| byte == fill) {
                    torn += 1;
                }
                accepted += 1;
            }
            StatusCode::SERVICE_UNAVAILABLE => {}
            other => panic!("a published preview must be served: {other}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.await.expect("the writer joins");

    assert_eq!(torn, 0, "the endpoint served a frame that mixed two publishes");
    assert_eq!(accepted, 2000, "the endpoint never served a complete preview");
}
