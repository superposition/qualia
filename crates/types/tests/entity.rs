//! Behavioural tests for entity profile parsing and validation.

use qualia_types::{
    parse_entity_profile, EntityAuthority, ENTITY_PROFILE_SCHEMA_VERSION,
};

const VALID: &str = r#"{
  "schema_version": "qualia.entity.v1",
  "entity": {
    "id": "entity:rover-01",
    "display_name": "Rover 01",
    "embodiment": "ground",
    "implementation": "waveshare-ugv-pinkie"
  },
  "runtime": { "agent_url": "http://127.0.0.1:8080" },
  "frames": [
    { "id": "world", "role": "world" },
    { "id": "base", "parent_id": "world", "role": "base" },
    { "id": "camera", "parent_id": "base", "role": "camera" }
  ],
  "streams": [
    { "id": "pose", "kind": "pose", "frame_id": "world", "source": "adapter:pose" },
    { "id": "points", "kind": "lidar", "frame_id": "base", "source": "adapter:lidar" }
  ],
  "capabilities": [
    { "id": "spatial.pose", "category": "spatial", "stream_ids": ["pose"] },
    { "id": "spatial.map", "category": "spatial", "stream_ids": ["points"] }
  ],
  "authorities": [
    { "command": "motion.navigate", "owner": "leash", "transport": "leash:http", "acknowledgement_required": false },
    { "command": "safety.estop", "owner": "leash", "transport": "leash:http", "acknowledgement_required": true }
  ]
}"#;

#[test]
fn accepts_an_open_entity_profile() {
    let profile = parse_entity_profile(VALID).expect("valid profile");
    assert_eq!(profile.schema_version, ENTITY_PROFILE_SCHEMA_VERSION);
    assert_eq!(profile.entity.embodiment, "ground");
    assert_eq!(
        profile.entity.implementation.as_deref(),
        Some("waveshare-ugv-pinkie")
    );
    assert!(profile.has_capability("spatial.map"));
    assert!(!profile.has_capability("manipulation.grasp"));
    assert_eq!(
        profile.authority("motion.navigate").map(|a| a.owner.as_str()),
        Some("leash")
    );
    assert!(profile.authority("joints.goal").is_none());
    assert_eq!(
        profile.runtime.as_ref().map(|r| r.agent_url.as_str()),
        Some("http://127.0.0.1:8080")
    );
}

#[test]
fn rejects_an_unknown_field() {
    let broken = VALID.replace(
        "\"display_name\": \"Rover 01\",",
        "\"display_name\": \"Rover 01\", \"surprise\": 1,",
    );
    let error = parse_entity_profile(&broken).expect_err("unknown field must fail");
    assert!(error.contains("invalid entity profile"), "got: {error}");
}

#[test]
fn rejects_a_wrong_schema_version() {
    let broken = VALID.replace("qualia.entity.v1", "qualia.entity.v2");
    let error = parse_entity_profile(&broken).expect_err("wrong version must fail");
    assert!(
        error.contains("unsupported entity profile schema_version"),
        "got: {error}"
    );
}

#[test]
fn rejects_a_capability_bound_to_a_missing_stream() {
    let broken = VALID.replace("\"stream_ids\": [\"points\"]", "\"stream_ids\": [\"ghost\"]");
    let error = parse_entity_profile(&broken).expect_err("missing stream must fail");
    assert!(error.contains("unknown stream 'ghost'"), "got: {error}");
}

#[test]
fn rejects_a_stream_bound_to_a_missing_frame() {
    let broken = VALID.replace("\"frame_id\": \"base\"", "\"frame_id\": \"sky\"");
    let error = parse_entity_profile(&broken).expect_err("missing frame must fail");
    assert!(error.contains("unknown frame 'sky'"), "got: {error}");
}

#[test]
fn rejects_a_duplicate_identifier() {
    let broken = VALID.replace(
        "\"id\": \"base\", \"parent_id\": \"world\"",
        "\"id\": \"world\", \"parent_id\": null",
    );
    let error = parse_entity_profile(&broken).expect_err("duplicate frame must fail");
    assert!(error.contains("duplicate frame id 'world'"), "got: {error}");
}

#[test]
fn rejects_a_frame_that_parents_itself() {
    let broken = VALID.replace(
        "\"id\": \"base\", \"parent_id\": \"world\"",
        "\"id\": \"base\", \"parent_id\": \"base\"",
    );
    let error = parse_entity_profile(&broken).expect_err("self-parent must fail");
    assert!(error.contains("cannot parent itself"), "got: {error}");
}

#[test]
fn rejects_an_unacknowledged_emergency_stop() {
    let broken = VALID.replace(
        "\"command\": \"safety.estop\", \"owner\": \"leash\", \"transport\": \"leash:http\", \"acknowledgement_required\": true",
        "\"command\": \"safety.estop\", \"owner\": \"leash\", \"transport\": \"leash:http\", \"acknowledgement_required\": false",
    );
    let error = parse_entity_profile(&broken).expect_err("unacknowledged e-stop must fail");
    assert!(error.contains("must require acknowledgement"), "got: {error}");
}

#[test]
fn rejects_a_runtime_url_without_a_scheme() {
    let broken = VALID.replace("http://127.0.0.1:8080", "127.0.0.1:8080");
    let error = parse_entity_profile(&broken).expect_err("scheme-less url must fail");
    assert!(error.contains("must use http:// or https://"), "got: {error}");
}

#[test]
fn validate_catches_a_duplicate_authority() {
    let mut profile = parse_entity_profile(VALID).expect("valid profile");
    let duplicate: EntityAuthority = profile.authorities[0].clone();
    profile.authorities.push(duplicate);
    let error = profile.validate().expect_err("duplicate command must fail");
    assert!(error.contains("duplicate authority id"), "got: {error}");
}

#[test]
fn empty_collections_are_acceptable() {
    let minimal = r#"{
      "schema_version": "qualia.entity.v1",
      "entity": { "id": "entity:x", "display_name": "X", "embodiment": "abstract" }
    }"#;
    let profile = parse_entity_profile(minimal).expect("minimal profile");
    assert!(profile.runtime.is_none());
    assert!(profile.frames.is_empty());
    assert!(profile.streams.is_empty());
    assert!(profile.capabilities.is_empty());
    assert!(profile.authorities.is_empty());
    assert!(profile.validate().is_ok());
}
