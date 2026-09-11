//! The Leash HTTP path, against a stub Leash.
//!
//! What the agent sends, what Leash answers, and what each answer becomes:
//! accepted, refused and timeout stay distinct, an e-stop the declaration does
//! not require acknowledgement for is refused before a request leaves the
//! process, and nothing on this path writes a motor or an arena goal.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use qualia_agent::config::LeashEndpoint;
use qualia_agent::leash::{
    authority_for, LeashClient, LeashOutcomeKind, ESTOP_COMMAND, NAVIGATE_COMMAND,
};
use qualia_agent::mission_control::MissionPlanV1;
use qualia_types::EntityAuthority;
use serde_json::{json, Value};
use support::{Harness, ADMIN_TOKEN};

/// What the stub Leash answers, and how long it waits before answering.
struct Stub {
    goals: (StatusCode, Value),
    stop: (StatusCode, Value),
    delay: Duration,
}

impl Stub {
    fn new() -> Self {
        Self {
            goals: (
                StatusCode::CREATED,
                json!({"ok": true, "active": true, "status": "active", "message": "accepted"}),
            ),
            stop: (
                StatusCode::OK,
                json!({"acknowledged": true, "statement": "adapter confirmed zero output"}),
            ),
            delay: Duration::ZERO,
        }
    }
}

/// A stub Leash that records every request it receives.
async fn stub_leash(stub: Stub) -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
    let requests = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let stub = Arc::new(stub);
    let goals = requests.clone();
    let stops = requests.clone();
    let goals_stub = stub.clone();
    let stops_stub = stub.clone();
    let app = Router::new()
        .route(
            "/navigation/goals",
            post(move |Json(body): Json<Value>| {
                let goals = goals.clone();
                let stub = goals_stub.clone();
                async move {
                    goals
                        .lock()
                        .expect("stub requests")
                        .push(("/navigation/goals".to_string(), body));
                    if !stub.delay.is_zero() {
                        tokio::time::sleep(stub.delay).await;
                    }
                    (stub.goals.0, Json(stub.goals.1.clone()))
                }
            }),
        )
        .route(
            "/motors/stop/verified",
            post(move |Json(body): Json<Value>| {
                let stops = stops.clone();
                let stub = stops_stub.clone();
                async move {
                    stops
                        .lock()
                        .expect("stub requests")
                        .push(("/motors/stop/verified".to_string(), body));
                    if !stub.delay.is_zero() {
                        tokio::time::sleep(stub.delay).await;
                    }
                    (stub.stop.0, Json(stub.stop.1.clone()))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stub listener");
    let address = listener.local_addr().expect("stub address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("stub serves");
    });
    (format!("http://{address}"), requests)
}

/// A plan as the spatial world model proposes one.
fn plan() -> MissionPlanV1 {
    serde_json::from_value(json!({
        "schema_version": "qualia.mission-plan.v1",
        "plan_id": "plan-7",
        "mission_id": "mission-frontier-1",
        "evidence_id": "evidence-3",
        "frame_id": "map",
        "target_x_m": 1.5,
        "target_y_m": -0.25,
        "tolerance_m": 0.2,
        "speed_ceiling_mps": 0.25,
        "operating_area": {
            "frame_id": "map",
            "min_x_m": -4.0,
            "min_y_m": -4.0,
            "max_x_m": 4.0,
            "max_y_m": 4.0
        },
        "created_at_ms": 1_700_000_000_000u64,
        "expires_at_ms": 1_700_000_060_000u64,
        "safety_authority": "proposal only"
    }))
    .expect("plan decodes")
}

/// A client pointed at `base_url`, with the operator label written to a file
/// that outlives the call.
fn leash_client(
    base_url: &str,
    token: Option<(&tempfile::TempDir, &str)>,
    timeout: Duration,
) -> LeashClient {
    let operator_token_file = token.map(|(dir, label)| {
        let path = dir.path().join("leash.token");
        std::fs::write(&path, format!("{label}\n")).expect("token file");
        path
    });
    LeashClient::with_timeout(
        LeashEndpoint {
            base_url: base_url.to_string(),
            operator_token_file,
        },
        timeout,
    )
}

#[tokio::test]
async fn navigate_sends_the_goal_contract_and_records_acceptance() {
    let (base_url, requests) = stub_leash(Stub::new()).await;
    let tokens = tempfile::tempdir().expect("token dir");
    let client = leash_client(&base_url, Some((&tokens, "private-test-token")), Duration::from_secs(6));

    let outcome = client
        .navigate(&authority_for(&[], NAVIGATE_COMMAND), &plan())
        .await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Accepted);
    assert_eq!(outcome.code, "accepted");
    assert!(
        outcome.detail.contains("accepted"),
        "the outcome carries Leash's own message: {outcome:?}"
    );

    let requests = requests.lock().expect("stub requests");
    assert_eq!(requests.len(), 1, "one proposal, one request: {requests:?}");
    let (path, body) = &requests[0];
    assert_eq!(path, "/navigation/goals");
    assert_eq!(body["schema_version"], "leash.navigation-goal.v1");
    assert_eq!(body["mission_id"], "mission-frontier-1");
    assert_eq!(body["idempotency_key"], "plan-7");
    assert_eq!(body["token"], "private-test-token");
    assert_eq!(body["approval"], true);
    assert_eq!(body["frame_id"], "map");
    assert_eq!(body["x_m"], 1.5);
    assert_eq!(body["y_m"], -0.25);
    let tolerance = body["tolerance_m"].as_f64().expect("tolerance is a number");
    assert!(
        (tolerance - 0.2).abs() < 1e-6,
        "tolerance_m survives the f32 round trip: {tolerance}"
    );
    assert_eq!(body["speed_mode"], "low");
    assert_eq!(body["deadline_ms"], 1_700_000_060_000u64);
}

#[tokio::test]
async fn a_leash_refusal_carries_leashs_own_answer() {
    let (base_url, requests) = stub_leash(Stub {
        goals: (
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "active": false, "status": "rejected", "message": "invalid"}),
        ),
        ..Stub::new()
    })
    .await;
    let tokens = tempfile::tempdir().expect("token dir");
    let client = leash_client(&base_url, Some((&tokens, "private-test-token")), Duration::from_secs(6));

    let outcome = client
        .navigate(&authority_for(&[], NAVIGATE_COMMAND), &plan())
        .await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Refused);
    assert_eq!(outcome.code, "leash_refused");
    assert!(
        outcome.detail.contains("rejected") && outcome.detail.contains("invalid"),
        "Leash's own refusal is what is recorded: {outcome:?}"
    );
    assert_eq!(requests.lock().expect("stub requests").len(), 1);
}

#[tokio::test]
async fn a_navigation_goal_without_an_operator_label_is_refused_locally() {
    let (base_url, requests) = stub_leash(Stub::new()).await;
    let client = leash_client(&base_url, None, Duration::from_secs(6));

    let outcome = client
        .navigate(&authority_for(&[], NAVIGATE_COMMAND), &plan())
        .await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Refused);
    assert_eq!(outcome.code, "operator_token_unavailable");
    assert!(
        requests.lock().expect("stub requests").is_empty(),
        "a locally refused proposal never reaches Leash"
    );
}

#[tokio::test]
async fn an_estop_the_declaration_does_not_require_acknowledgement_for_is_refused_locally() {
    let (base_url, requests) = stub_leash(Stub::new()).await;
    let client = leash_client(&base_url, None, Duration::from_secs(6));
    let unacknowledged = EntityAuthority {
        command: ESTOP_COMMAND.to_string(),
        owner: "leash".to_string(),
        transport: "leash:http".to_string(),
        acknowledgement_required: false,
    };

    let outcome = client.estop(&unacknowledged).await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Refused);
    assert_eq!(outcome.code, "acknowledgement_required");
    assert!(
        requests.lock().expect("stub requests").is_empty(),
        "an unacknowledged e-stop never reaches Leash"
    );
}

#[tokio::test]
async fn a_command_the_declaration_does_not_carry_is_refused_locally() {
    let (base_url, requests) = stub_leash(Stub::new()).await;
    let client = leash_client(&base_url, None, Duration::from_secs(6));
    let other_owner = EntityAuthority {
        command: ESTOP_COMMAND.to_string(),
        owner: "harness".to_string(),
        transport: "adapter:commands".to_string(),
        acknowledgement_required: true,
    };

    let outcome = client.estop(&other_owner).await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Refused);
    assert_eq!(outcome.code, "authority_denied");
    assert!(requests.lock().expect("stub requests").is_empty());
}

#[tokio::test]
async fn an_estop_leash_does_not_acknowledge_is_refused() {
    let (base_url, requests) = stub_leash(Stub {
        stop: (
            StatusCode::OK,
            json!({"acknowledged": false, "statement": "wheels not verified"}),
        ),
        ..Stub::new()
    })
    .await;
    let client = leash_client(&base_url, None, Duration::from_secs(6));

    let outcome = client
        .estop(&authority_for(&[], ESTOP_COMMAND))
        .await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Refused);
    assert_eq!(outcome.code, "stop_unverified");
    assert!(outcome.detail.contains("wheels not verified"));
    let requests = requests.lock().expect("stub requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "/motors/stop/verified");
    assert_eq!(requests[0].1, json!({"reason": "operator-request"}));
}

#[tokio::test]
async fn an_acknowledged_estop_is_accepted() {
    let (base_url, _requests) = stub_leash(Stub::new()).await;
    let client = leash_client(&base_url, None, Duration::from_secs(6));

    let outcome = client
        .estop(&authority_for(&[], ESTOP_COMMAND))
        .await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Accepted);
    assert_eq!(outcome.code, "accepted");
    assert!(outcome.detail.contains("adapter confirmed zero output"));
}

#[tokio::test]
async fn a_silent_leash_is_an_explicit_timeout() {
    let (base_url, _requests) = stub_leash(Stub {
        delay: Duration::from_millis(400),
        ..Stub::new()
    })
    .await;
    let tokens = tempfile::tempdir().expect("token dir");
    let client = leash_client(
        &base_url,
        Some((&tokens, "private-test-token")),
        Duration::from_millis(40),
    );

    let outcome = client
        .navigate(&authority_for(&[], NAVIGATE_COMMAND), &plan())
        .await;

    assert_eq!(outcome.outcome, LeashOutcomeKind::Timeout);
    assert_eq!(outcome.code, "leash_timeout");
    assert!(
        outcome.detail.contains("40 ms"),
        "the silence names its bound: {outcome:?}"
    );
    assert_ne!(outcome.outcome, LeashOutcomeKind::Accepted);
}

#[tokio::test]
async fn the_agent_routes_forward_and_report_the_outcome() {
    let (base_url, requests) = stub_leash(Stub::new()).await;
    let tokens = tempfile::tempdir().expect("token dir");
    let token_file = tokens.path().join("leash.token");
    std::fs::write(&token_file, "private-test-token\n").expect("token file");
    let harness = Harness::with(|config| {
        config.leash = Some(LeashEndpoint {
            base_url: base_url.clone(),
            operator_token_file: Some(token_file.clone()),
        });
    });

    let navigate = harness
        .post_json(
            "/leash/navigate",
            Some(ADMIN_TOKEN),
            serde_json::to_value(plan()).expect("plan encodes"),
        )
        .await;
    assert_eq!(navigate.status, StatusCode::OK);
    assert_eq!(navigate.json()["outcome"], "accepted");
    assert_eq!(navigate.json()["command"], NAVIGATE_COMMAND);
    assert_eq!(
        navigate.json()["schema_version"],
        "qualia.leash-outcome.v1",
        "the reply is self-describing"
    );

    let estop = harness
        .post_json("/leash/estop", Some(ADMIN_TOKEN), json!({}))
        .await;
    assert_eq!(estop.status, StatusCode::OK);
    assert_eq!(estop.json()["outcome"], "accepted");
    assert_eq!(estop.json()["code"], "accepted");

    let requests = requests.lock().expect("stub requests");
    assert_eq!(
        requests.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        vec!["/navigation/goals", "/motors/stop/verified"],
        "the agent reached Leash's own routes: {requests:?}"
    );
}

#[tokio::test]
async fn the_leash_routes_require_operator_authority() {
    let (base_url, requests) = stub_leash(Stub::new()).await;
    let harness = Harness::with(|config| {
        config.leash = Some(LeashEndpoint {
            base_url: base_url.clone(),
            operator_token_file: None,
        });
    });

    let response = harness.post_json("/leash/estop", None, json!({})).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(requests.lock().expect("stub requests").is_empty());
}

#[tokio::test]
async fn forwarding_writes_no_motor_or_arena_goal() {
    let (base_url, _requests) = stub_leash(Stub::new()).await;
    let tokens = tempfile::tempdir().expect("token dir");
    let token_file = tokens.path().join("leash.token");
    std::fs::write(&token_file, "private-test-token\n").expect("token file");
    let harness = Harness::with(|config| {
        config.leash = Some(LeashEndpoint {
            base_url: base_url.clone(),
            operator_token_file: Some(token_file.clone()),
        });
    });

    let outcome = harness
        .post_json(
            "/leash/navigate",
            Some(ADMIN_TOKEN),
            serde_json::to_value(plan()).expect("plan encodes"),
        )
        .await;
    assert_eq!(outcome.json()["outcome"], "accepted");

    let region = harness.state.shm().expect("scratch shm");
    assert_eq!(
        region.world_model().nav_goal.active, 0,
        "a forwarded proposal is not an arena goal"
    );
}
