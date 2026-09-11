//! Mission envelope wire-contract tests, including the `fly_governed` field
//! added for T24 (issue #38).

use qualia_sync_types::{MissionEnvelopeV1, MISSION_ENVELOPE_SCHEMA_VERSION};
use serde_json::{json, Value};

/// A v1 envelope as an older producer — one that predates `fly_governed` —
/// would have serialized it.
fn legacy_envelope_json() -> Value {
    json!({
        "schema_version": MISSION_ENVELOPE_SCHEMA_VERSION,
        "broker_id": "oh-my-pi",
        "producer_epoch": 7,
        "sequence": 1,
        "mission_id": "room-frontier-1",
        "idempotency_key": "room-frontier-1:start",
        "command": "start",
        "issued_at_ms": 1_000u64,
        "deadline_ms": 31_000u64,
        "objective": {
            "kind": "explore_frontier",
            "summary": "Inspect one fresh frontier",
            "target_x_m": null,
            "target_y_m": null,
            "tolerance_m": 0.2
        },
        "constraints": {
            "operating_area": {
                "frame_id": "odom",
                "min_x_m": -2.0,
                "min_y_m": -2.0,
                "max_x_m": 2.0,
                "max_y_m": 2.0
            },
            "speed_ceiling_mps": 0.15,
            "max_distance_m": 1.0,
            "max_runtime_ms": 20_000u64,
            "max_replans": 2,
            "evidence_max_age_ms": 2_000u64
        },
        "evidence_refs": ["spatial-7-1-32"]
    })
}

#[test]
fn fly_governed_defaults_false_and_round_trips() {
    // An envelope written before the field existed must still decode.
    let legacy: MissionEnvelopeV1 =
        serde_json::from_value(legacy_envelope_json()).expect("legacy envelope decodes");
    assert!(!legacy.fly_governed);
    legacy.validate().expect("legacy envelope still validates");

    // An envelope that carries it must survive a full JSON round trip.
    let mut governed = legacy.clone();
    governed.fly_governed = true;
    let encoded = serde_json::to_string(&governed).expect("encode governed envelope");
    let decoded: MissionEnvelopeV1 =
        serde_json::from_str(&encoded).expect("governed envelope round-trips");
    assert_eq!(decoded, governed);
    assert!(decoded.fly_governed);
}
