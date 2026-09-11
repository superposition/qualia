//! The operator-facing surface: the braid view the console and the web page
//! poll, the stack health and perception status derived from shared memory, and
//! the honest answer for a route whose backing subsystem has not landed.

mod support;

use axum::http::StatusCode;
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

    let mcp = harness.get("/mcp/status").await.json();
    assert_eq!(mcp["ok"], true);
    assert_eq!(mcp["transport"], "streamable-http");
    assert_eq!(mcp["server"], "qualia-agent");
    assert_eq!(mcp["protocol_version"], "2025-11-25");
    assert!(mcp["tool_count"].as_u64().is_some_and(|count| count > 0));

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
    assert_eq!(body["schema_version"], "qualia.entity.v1");
    assert!(body["entities"].as_array().is_some_and(Vec::is_empty));
}
