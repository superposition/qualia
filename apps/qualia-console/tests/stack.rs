//! The stack manifest is the console's only runner list.
//!
//! `docs/frontend-lessons.md` (source 1's avoid) counts two hard-coded runner
//! lists beside `QUALIA_STACK_MANIFEST` as a mistake. These pin that the
//! telemetry set is read out of the manifest, in manifest order, and that a
//! stack declaring no sensing runner gains none.

use qualia_console::stack;

fn manifest(runners: &str) -> String {
    format!(
        r#"{{
          "schema_version": "qualia.stack.v1",
          "stack_name": "test",
          "shared_memory": {{ "name": "/qualia_body" }},
          "control": {{ "socket": "/tmp/qualia-body.sock" }},
          "runners": [{runners}]
        }}"#
    )
}

#[test]
fn the_sensing_set_comes_from_the_manifest_in_manifest_order() {
    let text = manifest(
        r#"{"name": "qualia-agent"}, {"name": "qualia-vslam"}, {"name": "qualia-lidar"}"#,
    );
    assert_eq!(
        stack::sensing_runner_names(&text).expect("manifest parses"),
        vec!["qualia-vslam", "qualia-lidar"],
        "the rows are the manifest's sensing runners, in the manifest's order"
    );
}

#[test]
fn a_stack_with_no_sensing_runner_gains_none() {
    let text = manifest(r#"{"name": "qualia-agent"}"#);
    assert!(
        stack::sensing_runner_names(&text)
            .expect("manifest parses")
            .is_empty(),
        "a stack that names no sensing runner must not be given a built-in one"
    );
}
