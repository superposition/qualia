//! The stack manifest a deployment names is read, never re-invented.
//!
//! `docs/frontend-lessons.md` (source 1's avoid) counts two hard-coded runner
//! lists beside `QUALIA_STACK_MANIFEST` as a mistake. These pin the in-scope
//! route: a named stack contributes exactly its sensing runners, in its own
//! order, and a stack that names none gains none — the console's rows must not
//! be made to depend on a stack declaration.
//!
//! The product's default stack (`config/stack-manifest.default.json`) is not
//! the console's row set: it names `qualia-camera` and `qualia-leash-sensors`,
//! while with no `QUALIA_STACK_MANIFEST` the rows are the ABI's sensing slots.
//! No test here derives a row set from the shipped default.

use qualia_console::stack::{self, SensingSet};

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
fn a_named_stack_contributes_its_sensing_runners_in_its_own_order() {
    let text = manifest(
        r#"{"name": "qualia-agent"}, {"name": "qualia-vslam"}, {"name": "qualia-lidar"}"#,
    );
    assert_eq!(
        stack::sensing_runner_names(&text).expect("manifest parses"),
        vec!["qualia-vslam", "qualia-lidar"],
        "the rows are the named stack's sensing runners, in the manifest's order"
    );
}

#[test]
fn a_stack_that_names_no_sensing_runner_gets_none() {
    let text = manifest(r#"{"name": "qualia-health"}"#);
    assert!(
        stack::sensing_runner_names(&text)
            .expect("manifest parses")
            .is_empty(),
        "a stack that names no sensing runner must not be given a built-in one"
    );
}

/// The shipped default: with no `QUALIA_STACK_MANIFEST` named, the console does
/// not wait on a stack that can name a sensing runner. The rows are the ABI's
/// own sensing slots whatever the compiled-in default declares — which is what
/// keeps the panel populated in the configuration the repo ships.
#[test]
fn with_no_manifest_named_the_rows_are_the_abi_sensing_slots() {
    std::env::remove_var("QUALIA_STACK_MANIFEST");
    assert_eq!(
        stack::load_sensing_set().expect("the default row set needs no manifest"),
        SensingSet::EverySlot,
        "the console's rows must not be narrowed by the compiled-in default stack"
    );
}
