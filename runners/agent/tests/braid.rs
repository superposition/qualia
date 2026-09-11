//! The braid endpoint: every strand reports through one state machine.
//!
//! `GET /braid` is the read the console and the operator page poll;
//! `POST /braid` is where a strand that lives in another process — the evidence
//! recorder, the JEPA runtime — hands over the `BraidEvent` the braid crate
//! fixes. A reader must always get the state the agent knows, even when the
//! agent has never heard from every strand: a runtime that started before the
//! promotion wiring reports nothing, and the read still answers with the state
//! it holds instead of failing.

mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use support::Harness;

/// Hand one strand's event to the braid and return the state it folded into.
async fn report(harness: &Harness, event: Value) -> Value {
    let reply = harness.post_json("/braid", None, event).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "a strand's report is accepted: {}",
        reply.text()
    );
    reply.json()
}

/// The braid's own count of open missions, as the operator page reads it.
async fn open_missions(harness: &Harness) -> u64 {
    harness.get("/braid").await.json()["open_missions"]
        .as_u64()
        .expect("the count is a number")
}

#[tokio::test]
async fn braid_endpoint_reports_state() {
    let harness = Harness::new();

    // Nothing has reported yet. The state the agent knows is the default one,
    // under the console's schema, and reading it is not an error.
    let reply = harness.get("/braid").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["schema_version"], "qualia.braid-state.v1");
    assert_eq!(body["generation"], 0);
    assert_eq!(body["session_id"], "");
    assert_eq!(body["open_missions"], 0);
    assert_eq!(body["last_promotion_ns"], 0);
    assert!(body["last_quarantine_ns"].is_null());

    // The evidence strand seals a segment: the braid is told, and a seal moves
    // no pointer.
    let sealed = report(
        &harness,
        json!({
            "event": "evidence_sealed",
            "sha256": "9f2c".repeat(16),
        }),
    )
    .await;
    assert_eq!(sealed["generation"], 0);
    assert_eq!(sealed["last_promotion_ns"], 0);

    // The promotion strand accepts a generation: the pointer moves and the
    // acceptance is stamped.
    let accepted = report(&harness, json!({ "event": "promotion_accepted", "generation": 7 })).await;
    assert_eq!(accepted["generation"], 7);
    let promoted_at = accepted["last_promotion_ns"]
        .as_u64()
        .expect("an acceptance is stamped with a time");
    assert!(promoted_at > 0, "the acceptance carries a timestamp");

    // A rollback moves the pointer back and must not look like a promotion.
    let rolled_back = report(
        &harness,
        json!({
            "event": "promotion_rolled_back",
            "generation": 3,
            "reason": "health gate failed",
        }),
    )
    .await;
    assert_eq!(rolled_back["generation"], 3);
    assert_eq!(
        rolled_back["last_promotion_ns"], promoted_at,
        "a rollback keeps the last acceptance's stamp"
    );

    // The mission strand's two events are the same edge: the count the operator
    // page shows is the braid's, not a second copy the broker keeps.
    report(&harness, json!({ "event": "mission_opened", "mission_id": "m-1" })).await;
    assert_eq!(open_missions(&harness).await, 1);
    report(
        &harness,
        json!({ "event": "mission_closed", "mission_id": "m-1", "outcome": "aborted" }),
    )
    .await;
    assert_eq!(open_missions(&harness).await, 0);

    // The failure mode the ticket names: a stack whose runtime started before
    // the promotion wiring never reports a promotion. Its other strands still
    // speak, and the reader gets the state the agent does know, not an error.
    let unwired = Harness::new();
    let reported = report(
        &unwired,
        json!({ "event": "evidence_sealed", "sha256": "0".repeat(64) }),
    )
    .await;
    assert_eq!(
        reported["generation"], 0,
        "an agent never told about a promotion reports the state it knows"
    );
    assert_eq!(reported["last_promotion_ns"], 0);
    let read = unwired.get("/braid").await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.json()["generation"], 0);

    // A strand this build has never heard of is accepted too, and leaves the
    // known state alone rather than breaking the stream.
    let before = harness.get("/braid").await.json();
    let unknown = report(&harness, json!({ "event": "lane_changed", "lane": "left" })).await;
    assert_eq!(unknown, before, "an unknown event leaves the known state alone");
    assert_eq!(harness.get("/braid").await.status, StatusCode::OK);
}
