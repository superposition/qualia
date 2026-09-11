//! The stack manifest must carry the fly keys, because the belief runners read
//! them from the environment the supervisor hands down.

use std::path::PathBuf;

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config")
        .join("stack-manifest.default.json")
}

#[test]
fn stack_manifest_has_fly_keys() {
    let text = std::fs::read_to_string(manifest_path()).expect("default stack manifest is readable");
    let value: serde_json::Value = serde_json::from_str(&text).expect("manifest is JSON");

    assert_eq!(
        value["schema_version"], "qualia.stack.v1",
        "the manifest schema string is part of the wire contract"
    );

    let env = value
        .get("env")
        .expect("the manifest carries an `env` block for the children");
    assert_eq!(
        env["QUALIA_FLY_MODE"], "off",
        "the prior is off by default: a bug in the prior must never be able to move the robot"
    );

    let prior = env
        .get("QUALIA_FLY_PRIOR_PATH")
        .expect("QUALIA_FLY_PRIOR_PATH is declared so operators can point at an artifact");
    assert_eq!(prior, "", "unset by default; the runners disable the prior when it is empty");

    // The dial ships at its default so the console and the belief layers read
    // the same value when the agent has stepped nothing yet (T30, #46).
    let scale = env
        .get("QUALIA_FLY_COUPLING_SCALE")
        .expect("QUALIA_FLY_COUPLING_SCALE is declared so the dial has one home");
    assert_eq!(scale, "1.0", "the shipped dial is the coupling's identity");
    assert_eq!(
        scale.as_str().and_then(|text| text.parse::<f32>().ok()),
        Some(qualia_jepa::prior::COUPLING_SCALE_DEFAULT),
        "the manifest's spelling parses to the coupling's own default"
    );
}
