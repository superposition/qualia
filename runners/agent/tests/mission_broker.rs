//! The mission broker's observable contract: an accepted envelope opens a
//! braid mission, a terminal mission closes it, and a replayed idempotency key
//! never opens a second one.

mod support;

use axum::http::StatusCode;
use support::{envelope, Harness, BROKER_TOKEN};

const MISSION: &str = "mission-frontier-1";

#[tokio::test]
async fn accepted_mission_opens_the_braid_and_cancel_closes_it() {
    let harness = Harness::new();

    let cancelled = envelope(MISSION, "idem-start", 1, "cancel");
    let rejected = harness
        .post_json("/mission-control/envelopes", Some(BROKER_TOKEN), cancelled)
        .await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);

    let accepted = harness
        .post_json(
            "/mission-control/envelopes",
            Some(BROKER_TOKEN),
            envelope(MISSION, "idem-start", 1, "start"),
        )
        .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    let body = accepted.json();
    assert_eq!(body["accepted"], true);
    assert_eq!(body["idempotent_replay"], false);
    assert_eq!(body["mission"]["status"], "queued");

    let braid = harness.get("/braid").await.json();
    assert_eq!(braid["open_missions"], 1);

    // The evidence-grounded planner is not part of this build, so the mission
    // parks at the documented wait instead of leaving the arena.
    qualia_agent::mission_control::supervise_once(&harness.state).await;
    let missions = harness.get("/mission-control/missions").await.json();
    assert_eq!(missions["missions"][0]["status"], "paused");
    assert_eq!(missions["missions"][0]["last_code"], "awaiting_fresh_evidence");

    let cancelled = harness
        .post_json(
            "/mission-control/envelopes",
            Some(BROKER_TOKEN),
            envelope(MISSION, "idem-cancel", 2, "cancel"),
        )
        .await;
    assert_eq!(cancelled.status, StatusCode::ACCEPTED);
    qualia_agent::mission_control::supervise_once(&harness.state).await;

    let missions = harness
        .get("/mission-control/missions")
        .await
        .json();
    assert_eq!(missions["missions"][0]["status"], "cancelled");
    assert_eq!(missions["missions"][0]["stage"], "terminal");
    assert_eq!(missions["missions"][0]["last_code"], "cancelled");

    let braid = harness.get("/braid").await.json();
    assert_eq!(braid["open_missions"], 0);

    let events = harness.get("/mission-control/events").await.json();
    let codes: Vec<&str> = events["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter_map(|event| event["code"].as_str())
        .collect();
    assert_eq!(
        codes,
        vec!["accepted", "awaiting_fresh_evidence", "stop_verified", "cancelled"],
        "the broker's event stream is the audit trail: {events}"
    );
}

#[tokio::test]
async fn replayed_envelope_is_idempotent() {
    let harness = Harness::new();
    let payload = envelope(MISSION, "idem-once", 1, "start");

    let first = harness
        .post_json("/mission-control/envelopes", Some(BROKER_TOKEN), payload.clone())
        .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);

    let replay = harness
        .post_json("/mission-control/envelopes", Some(BROKER_TOKEN), payload)
        .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.json()["idempotent_replay"], true);

    let braid = harness.get("/braid").await.json();
    assert_eq!(braid["open_missions"], 1, "a replay must not open a second mission");
}

#[tokio::test]
async fn malformed_envelope_is_refused_and_leaves_the_braid_alone() {
    let harness = Harness::new();
    let mut payload = envelope(MISSION, "idem-bad", 1, "start");
    payload["schema_version"] = serde_json::json!("qualia.mission-envelope.v0");

    let refused = harness
        .post_json("/mission-control/envelopes", Some(BROKER_TOKEN), payload)
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.json()["error"].as_str().is_some_and(|error| error.contains("mission")));

    let braid = harness.get("/braid").await.json();
    assert_eq!(braid["open_missions"], 0);
}

#[tokio::test]
async fn broker_scope_requires_its_token() {
    let harness = Harness::new();
    let refused = harness
        .post_json(
            "/mission-control/envelopes",
            None,
            envelope(MISSION, "idem-no-token", 1, "start"),
        )
        .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
}
