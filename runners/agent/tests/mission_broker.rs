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

/// A journal line written before the coupling dial existed still loads.
///
/// The line's digest was taken over a state that carried no `coupling_scale`,
/// so the field must stay out of the digested form when it holds its default:
/// otherwise every journal a previous build wrote fails its digest check and
/// the whole journalled state — the missions, the deliveries, the producer
/// epoch — is discarded.
#[tokio::test]
async fn a_journal_written_before_the_dial_still_loads() {
    use sha2::{Digest, Sha256};

    // One journalled transition under producer epoch 7, with no missions, no
    // deliveries and no dial, in the canonical form the digest covers.
    let state = serde_json::json!({
        "schema_version": "qualia.mission-control-state.v1",
        "producer_epoch": 7,
        "next_event_sequence": 0,
        "broker_producer_epoch": null,
        "broker_sequence": 0,
        "deliveries": {},
        "missions": {},
        "events": [],
    });
    let canonical = serde_json::to_value(&state).expect("the state is JSON");
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical).expect("canonical bytes"))
    );
    let record = serde_json::json!({
        "schema_version": "qualia.mission-control-journal.v1",
        "state_sha256": digest,
        "state": canonical,
    });

    let harness = Harness::with(|config| {
        std::fs::write(&config.mission_journal, format!("{record}\n"))
            .expect("write the pre-dial journal");
    });

    let missions = harness.get("/mission-control/missions").await.json();
    assert_eq!(
        missions["producer_epoch"], 7,
        "the journalled state is resumed, not discarded: {missions}"
    );
}

/// A closed mission steps the dial, and the step is what the supervisor's next
/// start of a belief layer reads.
#[tokio::test]
async fn a_closed_mission_steps_the_coupling_dial() {
    let harness = Harness::new();
    let manifest = harness.stack_manifest.clone();
    assert_eq!(
        dial(&manifest),
        None,
        "no mission has closed, so the manifest declares no dial"
    );

    // The mission starts, parks at the evidence wait, and is cancelled: a
    // cancel is not a success, so it carries the coupling down one step.
    for (key, command, sequence) in [
        ("idem-dial-start", "start", 1u64),
        ("idem-dial-cancel", "cancel", 2),
    ] {
        let accepted = harness
            .post_json(
                "/mission-control/envelopes",
                Some(BROKER_TOKEN),
                envelope(MISSION, key, sequence, command),
            )
            .await;
        assert_eq!(accepted.status, StatusCode::ACCEPTED);
        qualia_agent::mission_control::supervise_once(&harness.state).await;
    }

    let stepped = dial(&manifest).expect("the dial the supervisor reads is written");
    assert!(
        (stepped - 0.90).abs() < 1e-6,
        "one step down from the default reads 0.90, not {stepped}"
    );
}

/// With `QUALIA_STACK_MANIFEST` unset there is no manifest the supervisor will
/// read, so the dial's handover is inert: closing a mission steps and journals
/// the dial, and the repository's own tracked manifest is left untouched.
///
/// The default this pins: a launch that names no manifest must not resolve one
/// to `config/stack-manifest.default.json`. `runners/init` embeds that file and
/// reads the embedded copy when the variable is unset, so the agent's in-place
/// rewrite was both a mutation of the repository and invisible to the stack.
#[tokio::test]
async fn an_unset_stack_manifest_leaves_the_repository_manifest_alone() {
    let tracked = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config")
        .join("stack-manifest.default.json");
    let before = std::fs::read(&tracked).expect("the shipped manifest is readable");

    let mut journal = std::path::PathBuf::new();
    let harness = Harness::with(|config| {
        config.stack_manifest = None;
        journal = std::path::PathBuf::from(&config.mission_journal);
    });

    // The dial only steps on a closed mission; a cancel is not a success.
    for (key, command, sequence) in [
        ("idem-inert-start", "start", 1u64),
        ("idem-inert-cancel", "cancel", 2),
    ] {
        let accepted = harness
            .post_json(
                "/mission-control/envelopes",
                Some(BROKER_TOKEN),
                envelope(MISSION, key, sequence, command),
            )
            .await;
        assert_eq!(accepted.status, StatusCode::ACCEPTED);
        qualia_agent::mission_control::supervise_once(&harness.state).await;
    }

    assert_eq!(
        std::fs::read(&tracked).expect("the shipped manifest is readable"),
        before,
        "an unset QUALIA_STACK_MANIFEST must not rewrite the repository's manifest"
    );
    let partials = std::fs::read_dir(tracked.parent().expect("the config directory"))
        .expect("the config directory is readable")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".stack-manifest.default.json."))
        .count();
    assert_eq!(
        partials, 0,
        "a handover that is skipped must leave no temp file behind"
    );

    // Nothing is lost: the step is durable in the journal, so the agent resumes
    // from it even though the dial was handed to no stack.
    let text = std::fs::read_to_string(&journal).expect("the journal is readable");
    let newest = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("the journal has a newest record");
    let record: serde_json::Value = serde_json::from_str(newest).expect("the record is JSON");
    let stepped = record["state"]["coupling_scale"]
        .as_f64()
        .expect("the stepped dial is journalled");
    assert!(
        (stepped - 0.90).abs() < 1e-6,
        "the step is journalled even when the handover is inert: {stepped}"
    );
}

/// The coupling dial the stack manifest declares, or `None` when it declares
/// none.
fn dial(path: &std::path::Path) -> Option<f32> {
    let text = std::fs::read_to_string(path).expect("the stack manifest is readable");
    let value: serde_json::Value = serde_json::from_str(&text).expect("the manifest is JSON");
    value["env"]["QUALIA_FLY_COUPLING_SCALE"]
        .as_str()
        .map(|raw| raw.parse::<f32>().expect("the dial is a number"))
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
