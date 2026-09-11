//! CRDT merge laws and wire-format tolerance for the sync.v1 envelopes.

use qualia_sync_types::*;
use serde_json::{json, Value};

fn register_body(envelope: &SyncEnvelope) -> &SyncRegisterValue {
    match &envelope.body {
        SyncBody::Register(body) => body,
        other => panic!("expected register body, got {other:?}"),
    }
}

fn or_map_body(envelope: &SyncEnvelope) -> &SyncOrMapOp {
    match &envelope.body {
        SyncBody::OrMap(body) => body,
        other => panic!("expected or-map body, got {other:?}"),
    }
}

fn embedding_body(envelope: &SyncEnvelope) -> &SyncEmbeddingOp {
    match &envelope.body {
        SyncBody::Embedding(body) => body,
        other => panic!("expected embedding body, got {other:?}"),
    }
}

fn tile_body(envelope: &SyncEnvelope) -> &SyncWeightTileDelta {
    match &envelope.body {
        SyncBody::WeightTile(body) => body,
        other => panic!("expected weight-tile body, got {other:?}"),
    }
}

fn register_envelope(
    replica_id: &str,
    counter: u64,
    hlc: HlcTimestamp,
    key: &str,
    value: Value,
) -> SyncEnvelope {
    SyncEnvelope {
        schema_version: SYNC_SCHEMA_VERSION.to_string(),
        replica_id: replica_id.to_string(),
        replica_role: ReplicaRole::Host,
        run_id: "run-a".to_string(),
        counter,
        timestamp_hlc: hlc,
        namespace: SyncNamespace::Directive,
        key: key.to_string(),
        body: SyncBody::Register(SyncRegisterValue {
            value,
            lease_duration_ms: None,
        }),
    }
}

fn or_map_envelope(
    replica_id: &str,
    counter: u64,
    hlc: HlcTimestamp,
    key: &str,
    action: OrMapAction,
) -> SyncEnvelope {
    SyncEnvelope {
        schema_version: SYNC_SCHEMA_VERSION.to_string(),
        replica_id: replica_id.to_string(),
        replica_role: ReplicaRole::Host,
        run_id: "run-a".to_string(),
        counter,
        timestamp_hlc: hlc,
        namespace: SyncNamespace::Object,
        key: key.to_string(),
        body: SyncBody::OrMap(SyncOrMapOp { action }),
    }
}

#[test]
fn register_and_or_map_merges_are_commutative_and_idempotent() {
    let older = register_envelope("replica-a", 1, HlcTimestamp::new(10, 0), "mode", json!("idle"));
    let newer = register_envelope("replica-b", 2, HlcTimestamp::new(20, 0), "mode", json!("scout"));

    let left = merge_register(None, &older, register_body(&older))
        .expect("initial merge")
        .state;
    let left = merge_register(Some(&left), &newer, register_body(&newer))
        .expect("left merge")
        .state;
    let right = merge_register(None, &newer, register_body(&newer))
        .expect("initial merge")
        .state;
    let right = merge_register(Some(&right), &older, register_body(&older))
        .expect("right merge")
        .state;
    assert_eq!(left, right, "register merge must be commutative");
    assert_eq!(left.value, json!("scout"), "newest stamp wins");

    // Replaying the winning op leaves the state identical; a stale op is a
    // no-op that reports `changed == false`.
    let replayed = merge_register(Some(&left), &newer, register_body(&newer)).expect("replay");
    assert_eq!(replayed.state, left);
    let stale = merge_register(Some(&left), &older, register_body(&older)).expect("stale replay");
    assert!(!stale.changed);
    assert_eq!(stale.state, left);

    let first_op = or_map_envelope(
        "replica-a",
        1,
        HlcTimestamp::new(10, 0),
        "object/1",
        OrMapAction::Upsert {
            value: json!({"name": "crate"}),
            replaces_tags: Vec::new(),
        },
    );
    let second_op = or_map_envelope(
        "replica-b",
        2,
        HlcTimestamp::new(20, 0),
        "object/1",
        OrMapAction::Upsert {
            value: json!({"name": "pallet"}),
            replaces_tags: vec![first_op.op_id()],
        },
    );

    let forward = merge_or_map(None, &first_op, or_map_body(&first_op))
        .expect("initial upsert")
        .state;
    let forward = merge_or_map(Some(&forward), &second_op, or_map_body(&second_op))
        .expect("forward merge")
        .state;
    let reverse = merge_or_map(None, &second_op, or_map_body(&second_op))
        .expect("initial upsert")
        .state;
    let reverse = merge_or_map(Some(&reverse), &first_op, or_map_body(&first_op))
        .expect("reverse merge")
        .state;
    assert_eq!(forward, reverse, "or-map merge must be commutative");
    assert_eq!(forward.visible_value, Some(json!({"name": "pallet"})));
    assert_eq!(forward.removed_tags, vec![first_op.op_id()]);

    let replayed = merge_or_map(Some(&forward), &first_op, or_map_body(&first_op))
        .expect("replay identical upsert");
    assert!(!replayed.changed);
    assert_eq!(replayed.state, forward);
}

#[test]
fn accumulator_merges_are_order_independent() {
    let embedding = |replica_id: &str, counter: u64, value: f32, weight: f32| SyncEnvelope {
        schema_version: SYNC_SCHEMA_VERSION.to_string(),
        replica_id: replica_id.to_string(),
        replica_role: ReplicaRole::Host,
        run_id: "run-a".to_string(),
        counter,
        timestamp_hlc: HlcTimestamp::new(counter * 10, 0),
        namespace: SyncNamespace::SceneEmbedding,
        key: "scene/embedding".to_string(),
        body: SyncBody::Embedding(SyncEmbeddingOp {
            values: vec![value; qualia_types::STATE_DIM],
            merge_mode: EmbeddingMergeMode::WeightedAverage,
            weight: Some(weight),
        }),
    };
    let first = embedding("replica-a", 1, 2.0, 0.25);
    let second = embedding("replica-b", 2, 4.0, 0.75);
    let forward = merge_embedding(None, embedding_body(&first))
        .expect("initial")
        .state;
    let forward = merge_embedding(Some(&forward), embedding_body(&second))
        .expect("forward")
        .state;
    let reverse = merge_embedding(None, embedding_body(&second))
        .expect("initial")
        .state;
    let reverse = merge_embedding(Some(&reverse), embedding_body(&first))
        .expect("reverse")
        .state;
    assert_eq!(forward, reverse);
    assert_eq!(forward.total_weight, 1.0);
    assert_eq!(forward.effective_values()[0], 3.5);

    let tile = |replica_id: &str, counter: u64, delta: f32| SyncEnvelope {
        schema_version: SYNC_SCHEMA_VERSION.to_string(),
        replica_id: replica_id.to_string(),
        replica_role: ReplicaRole::Host,
        run_id: "run-a".to_string(),
        counter,
        timestamp_hlc: HlcTimestamp::new(counter * 10, 0),
        namespace: SyncNamespace::WeightTile,
        key: "layer/0/tile/0/0".to_string(),
        body: SyncBody::WeightTile(SyncWeightTileDelta {
            layer_id: 0,
            tile_row: 0,
            tile_col: 0,
            tile_size: WEIGHT_TILE_SIZE as u16,
            base_checkpoint: "checkpoint-a".to_string(),
            merge_mode: WeightTileMergeMode::Additive,
            delta: vec![delta; WEIGHT_TILE_VALUE_COUNT],
            weight: None,
        }),
    };
    let read_base = || vec![0.0_f32; WEIGHT_TILE_VALUE_COUNT];

    let first = tile("replica-a", 1, 1.5);
    let second = tile("replica-b", 2, 2.5);
    let forward = merge_weight_tile(None, tile_body(&first), read_base)
        .expect("initial")
        .state;
    let forward = merge_weight_tile(Some(&forward), tile_body(&second), read_base)
        .expect("forward")
        .state;
    let reverse = merge_weight_tile(None, tile_body(&second), read_base)
        .expect("initial")
        .state;
    let reverse = merge_weight_tile(Some(&reverse), tile_body(&first), read_base)
        .expect("reverse")
        .state;
    assert_eq!(forward, reverse);
    assert_eq!(forward.effective_values()[0], 4.0);
}

#[test]
fn unknown_fields_are_ignored_rather_than_fatal() {
    let envelope = register_envelope("replica-a", 1, HlcTimestamp::new(10, 0), "mode", json!("idle"));
    let mut payload = serde_json::to_value(&envelope).expect("encode envelope");
    payload["future_top_level"] = json!({"nested": true});
    payload["body"]["data"]["future_payload_field"] = json!(7);

    let decoded: SyncEnvelope = serde_json::from_value(payload).expect("unknown fields ignored");
    assert_eq!(decoded, envelope);
    decoded.validate_shape().expect("decoded envelope is valid");
}
