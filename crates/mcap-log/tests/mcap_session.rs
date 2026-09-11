//! Behavioural tests for the crash-safe MCAP session writer.
//!
//! Every case here is written against the public surface a downstream consumer
//! sees: the session writer, the sealing reference, the read window helpers and
//! the quarantine entry point. The sealing and quarantine paths are the two
//! places where an interrupted process can lose evidence, so both are exercised
//! through a real crash (a dropped writer that never sealed) rather than a mock.

use qualia_mcap::{
    inventory, quarantine_partials, read_window, read_window_topics, ChannelInventory,
    LoggedMessage, McapReference, McapSessionWriter, QuarantinedPartial, JSON_SCHEMA_NAME,
    MCAP_PROFILE, REQUIRED_TOPICS, TOPIC_ACTION_APPLIED, TOPIC_CAMERA, TOPIC_LIDAR, TOPIC_POSE,
};
use serde_json::json;
use std::fs;
use std::path::Path;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn append_and_read_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = McapSessionWriter::create(temp.path(), "arena-1").unwrap();

    // While the session is open only the partial exists; the sealed name must
    // never appear before `finish`, otherwise a crash would leave a file that
    // looks complete but is not.
    assert!(temp.path().join("arena-1.mcap.partial").is_file());
    assert!(!temp.path().join("arena-1.mcap").exists());

    writer
        .write_json(TOPIC_CAMERA, 1, 100, 101, &json!({"frame": 1}))
        .unwrap();
    writer
        .write_json(TOPIC_LIDAR, 2, 200, 202, &json!({"points": 4}))
        .unwrap();
    writer
        .write_json(
            TOPIC_ACTION_APPLIED,
            3,
            300,
            303,
            &json!({"left": 0.0, "right": 0.0}),
        )
        .unwrap();

    let reference = writer.finish().unwrap();

    assert_eq!(reference.schema_version, "qualia.mcap-reference.v1");
    assert_eq!(reference.recovery_state, "complete");
    assert_eq!(reference.path, temp.path().join("arena-1.mcap").to_string_lossy().into_owned());
    assert!(!temp.path().join("arena-1.mcap.partial").exists());

    let sealed = fs::read(&reference.path).unwrap();
    assert_eq!(reference.byte_length, sealed.len() as u64);
    assert_eq!(reference.sha256, sha256_hex(&sealed));

    // The reference's channel inventory is sorted by topic and carries the
    // schema name every message was written under.
    let topics: Vec<&str> = reference
        .channels
        .iter()
        .map(|channel| channel.topic.as_str())
        .collect();
    assert_eq!(
        topics,
        vec![TOPIC_ACTION_APPLIED, TOPIC_CAMERA, TOPIC_LIDAR]
    );
    assert!(reference
        .channels
        .iter()
        .all(|channel| channel.schema == JSON_SCHEMA_NAME));
    assert!(reference
        .channels
        .iter()
        .all(|channel| channel.message_count == 1));
    assert_eq!(reference.min_log_time_ns, 100);
    assert_eq!(reference.max_log_time_ns, 300);

    // Re-reading the sealed segment reproduces the payload bytes exactly, with
    // inclusive time bounds and an optional topic filter.
    let camera = read_window(&reference.path, Some(TOPIC_CAMERA), 90, 150).unwrap();
    assert_eq!(camera.len(), 1);
    assert_eq!(camera[0].sequence, 1);
    assert_eq!(camera[0].log_time_ns, 100);
    assert_eq!(camera[0].publish_time_ns, 101);
    assert_eq!(camera[0].data, br#"{"frame":1}"#);

    // A window that excludes everything yields nothing rather than an error.
    assert!(read_window(&reference.path, None, 1_000, 2_000)
        .unwrap()
        .is_empty());
    // The bounds are inclusive on both ends.
    assert_eq!(read_window(&reference.path, None, 100, 100).unwrap().len(), 1);

    let all = read_window(&reference.path, None, 0, u64::MAX).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter().map(|message| message.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    // A topic subset keeps only the requested channels.
    let physical = read_window_topics(&reference.path, &[TOPIC_CAMERA, TOPIC_LIDAR], 0, u64::MAX)
        .unwrap();
    assert_eq!(physical.len(), 2);
    assert_eq!(physical[0].topic, TOPIC_CAMERA);
    assert_eq!(physical[1].topic, TOPIC_LIDAR);
    // An empty topic set is an explicit "nothing" rather than "everything".
    assert!(read_window_topics(&reference.path, &[], 0, u64::MAX)
        .unwrap()
        .is_empty());

    // `inventory` recomputes the same shape directly from the sealed bytes.
    let inventory = inventory(&reference.path).unwrap();
    assert_eq!(inventory.len(), 3);
    assert_eq!(
        inventory
            .iter()
            .map(|channel| channel.topic.clone())
            .collect::<Vec<_>>(),
        vec![
            TOPIC_ACTION_APPLIED.to_string(),
            TOPIC_CAMERA.to_string(),
            TOPIC_LIDAR.to_string()
        ]
    );
    assert_eq!(inventory[1].min_log_time_ns, 100);
    assert_eq!(inventory[1].max_log_time_ns, 100);
}

#[test]
fn sealing_produces_a_readable_segment() {
    let temp = tempfile::tempdir().unwrap();
    let mut writer = McapSessionWriter::create(temp.path(), "sealed").unwrap();

    // Registering the full required topic set must make every topic present in
    // the sealed segment even when no message was ever written to it: an
    // operator reading the segment needs to see the channels that stayed silent.
    writer.register_required_topics().unwrap();
    writer
        .write_json(TOPIC_POSE, 7, 5_000, 5_001, &json!({"x": 1.0}))
        .unwrap();

    let reference = writer.finish().unwrap();
    let bytes = fs::read(&reference.path).unwrap();

    // The written profile and schema are part of the on-disk contract, so a
    // reader that never runs qualia-mcap can still identify the segment.
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(MCAP_PROFILE), "profile missing from segment");
    assert!(
        text.contains(JSON_SCHEMA_NAME),
        "schema name missing from segment"
    );
    assert!(text.contains("jsonschema"), "schema encoding missing");
    assert!(text.contains("qualia-mcap/"), "library tag missing");

    assert_eq!(reference.channels.len(), REQUIRED_TOPICS.len());
    for topic in REQUIRED_TOPICS {
        let channel = reference
            .channels
            .iter()
            .find(|channel| channel.topic == topic)
            .unwrap_or_else(|| panic!("required topic {topic} absent from sealed segment"));
        assert_eq!(channel.message_count, u64::from(topic == TOPIC_POSE));
        assert_eq!(channel.schema, JSON_SCHEMA_NAME);
    }
    // Twelve silent channels contribute no time bounds of their own.
    assert_eq!(reference.min_log_time_ns, 5_000);
    assert_eq!(reference.max_log_time_ns, 5_000);

    // The sealed segment is independently readable.
    let messages = read_window(&reference.path, Some(TOPIC_POSE), 0, u64::MAX).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].sequence, 7);
    assert_eq!(messages[0].data, br#"{"x":1.0}"#);
    assert_eq!(inventory(&reference.path).unwrap().len(), REQUIRED_TOPICS.len());

    // Registering a topic twice is idempotent, not a second channel.
    let mut writer = McapSessionWriter::create(temp.path(), "idempotent").unwrap();
    writer.register_required_topics().unwrap();
    writer.register_required_topics().unwrap();
    let reference = writer.finish().unwrap();
    assert_eq!(reference.channels.len(), REQUIRED_TOPICS.len());
}

#[test]
fn quarantine_moves_a_partial_aside_without_data_loss() {
    let temp = tempfile::tempdir().unwrap();
    let partial = temp.path().join("crashed.mcap.partial");
    let payload = b"incomplete evidence that must survive the move".to_vec();
    fs::write(&partial, &payload).unwrap();
    let unrelated = temp.path().join("crashed.mcap");
    fs::write(&unrelated, b"a sealed neighbour").unwrap();
    let evidence_dir = temp.path().join("other");
    fs::create_dir(&evidence_dir).unwrap();

    let records = quarantine_partials(temp.path()).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.original_path, partial.to_string_lossy().into_owned());
    assert_eq!(
        record.quarantine_path,
        temp.path().join("crashed.mcap.quarantine").to_string_lossy().into_owned()
    );
    assert_eq!(
        record.reason,
        "writer did not publish an atomic completed MCAP"
    );

    // The partial is gone from the live path, its bytes are untouched at the
    // quarantine path, and nothing else in the directory moved.
    assert!(!partial.exists());
    assert_eq!(fs::read(&record.quarantine_path).unwrap(), payload);
    assert_eq!(fs::read(&unrelated).unwrap(), b"a sealed neighbour");
    assert!(evidence_dir.is_dir());
    assert!(inventory(&record.quarantine_path).is_err());

    // A second sweep finds nothing: quarantine is idempotent.
    assert!(quarantine_partials(temp.path()).unwrap().is_empty());

    // A later crash of the same session cannot clobber the first quarantine.
    fs::write(&partial, b"second crash").unwrap();
    let records = quarantine_partials(temp.path()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].quarantine_path,
        temp.path().join("crashed.mcap.quarantine.1").to_string_lossy().into_owned()
    );
    assert_eq!(
        fs::read(temp.path().join("crashed.mcap.quarantine")).unwrap(),
        payload
    );
    assert_eq!(fs::read(&records[0].quarantine_path).unwrap(), b"second crash");

    // Sweeping a directory that does not exist is a no-op, not an error.
    assert!(quarantine_partials(temp.path().join("absent"))
        .unwrap()
        .is_empty());
}

#[test]
fn reopen_after_crash_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let partial = temp.path().join("crashy.mcap.partial");
    let sealed = temp.path().join("crashy.mcap");

    // A live session writes two messages, then the process dies before it can
    // seal: the writer is dropped without `finish`, so the atomic rename never
    // happens and only the partial remains.
    {
        let mut writer = McapSessionWriter::create(temp.path(), "crashy").unwrap();
        writer
            .write_json(TOPIC_CAMERA, 1, 10, 11, &json!({"frame": 1}))
            .unwrap();
        writer
            .write_json(TOPIC_LIDAR, 2, 20, 21, &json!({"points": 2}))
            .unwrap();
        drop(writer);
    }
    assert!(partial.is_file());
    assert!(!sealed.exists(), "a crash must not produce a sealed segment");

    // Re-opening the same session refuses to write over the abandoned partial,
    // so a crashed run can never be silently overwritten.
    assert!(McapSessionWriter::create(temp.path(), "crashy").is_err());

    // Recovery is an explicit quarantine: the bytes are preserved, and the
    // partial is never promoted to the sealed path.
    let quarantined = quarantine_partials(temp.path()).unwrap();
    assert_eq!(quarantined.len(), 1);
    assert!(!partial.exists());
    assert!(!sealed.exists());
    let preserved = fs::read(&quarantined[0].quarantine_path).unwrap();
    assert!(preserved.len() > 16);
    // The crashed run's evidence is still attributable: its messages survive in
    // the quarantined bytes (either as a readable segment or as raw evidence).
    if let Ok(messages) = read_window(&quarantined[0].quarantine_path, None, 0, u64::MAX) {
        let camera = messages
            .iter()
            .find(|message| message.topic == TOPIC_CAMERA)
            .expect("camera message survived the crash");
        assert_eq!(camera.data, br#"{"frame":1}"#);
    }

    // With the partial out of the way the session can be started again and is
    // sealed normally.
    let mut writer = McapSessionWriter::create(temp.path(), "crashy").unwrap();
    writer
        .write_json(TOPIC_CAMERA, 3, 30, 31, &json!({"frame": 3}))
        .unwrap();
    let reference = writer.finish().unwrap();
    assert_eq!(reference.recovery_state, "complete");
    assert_eq!(reference.path, sealed.to_string_lossy().into_owned());
    assert!(Path::new(&reference.path).is_file());
    let messages = read_window(&reference.path, None, 0, u64::MAX).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].data, br#"{"frame":3}"#);
}

#[test]
fn unsafe_session_ids_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for rejected in ["", "../escape", "a/b", "a b", "sess.mcap", "café"] {
        assert!(
            McapSessionWriter::create(temp.path(), rejected).is_err(),
            "session id {rejected:?} must be rejected"
        );
    }
    // Nothing was created for any rejected id.
    assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
}

#[test]
fn serialized_contracts_keep_their_field_names_and_order() {
    let inventory = ChannelInventory {
        topic: TOPIC_CAMERA.to_string(),
        schema: JSON_SCHEMA_NAME.to_string(),
        message_count: 3,
        min_log_time_ns: 10,
        max_log_time_ns: 30,
    };
    assert_eq!(
        serde_json::to_string(&inventory).unwrap(),
        r#"{"topic":"/qualia/camera","schema":"qualia.evidence.v1","message_count":3,"min_log_time_ns":10,"max_log_time_ns":30}"#
    );

    let message = LoggedMessage {
        topic: TOPIC_CAMERA.to_string(),
        sequence: 4,
        log_time_ns: 40,
        publish_time_ns: 41,
        data: vec![1, 2, 3],
    };
    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        r#"{"topic":"/qualia/camera","sequence":4,"log_time_ns":40,"publish_time_ns":41,"data":[1,2,3]}"#
    );

    let quarantined = QuarantinedPartial {
        original_path: "a.mcap.partial".to_string(),
        quarantine_path: "a.mcap.quarantine".to_string(),
        reason: "writer did not publish an atomic completed MCAP".to_string(),
    };
    assert_eq!(
        serde_json::to_string(&quarantined).unwrap(),
        r#"{"original_path":"a.mcap.partial","quarantine_path":"a.mcap.quarantine","reason":"writer did not publish an atomic completed MCAP"}"#
    );

    // The reference is the largest serialized type; pin both the field order
    // and that it survives a JSON round trip unchanged.
    let reference = McapReference {
        schema_version: "qualia.mcap-reference.v1".to_string(),
        path: "arena.mcap".to_string(),
        sha256: "0".repeat(64),
        byte_length: 128,
        min_log_time_ns: 5,
        max_log_time_ns: 9,
        channels: vec![inventory],
        recovery_state: "complete".to_string(),
    };
    let encoded = serde_json::to_string(&reference).unwrap();
    let mut cursor = 0;
    for key in [
        "\"schema_version\":\"qualia.mcap-reference.v1\"",
        "\"path\":\"arena.mcap\"",
        "\"sha256\":",
        "\"byte_length\":128",
        "\"min_log_time_ns\":5",
        "\"max_log_time_ns\":9",
        "\"channels\":[",
        "\"recovery_state\":\"complete\"",
    ] {
        let found = encoded[cursor..]
            .find(key)
            .unwrap_or_else(|| panic!("field {key} missing or out of order in {encoded}"));
        cursor += found + key.len();
    }
    let decoded: McapReference = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, reference);
}
