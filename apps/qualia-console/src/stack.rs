//! The stack manifest: the runner set a deployment declares, read from the
//! document the supervisor reads.
//!
//! Traceability (`docs/frontend-lessons.md`, source 1's avoid): the reference
//! front end kept two hard-coded runner lists beside `QUALIA_STACK_MANIFEST`, so
//! a named stack is read here rather than re-invented, and its sensing runners
//! are the telemetry table's rows in the manifest's own order.
//!
//! The console's own row set does not wait on a stack that can name a sensing
//! runner. With no `QUALIA_STACK_MANIFEST` the rows are the ABI's sensing slots
//! ([`SensingRunner`]), whatever `config/stack-manifest.default.json` — the
//! *product's* default stack, which now names `qualia-camera` and
//! `qualia-leash-sensors` — declares: narrowing the table to a manifest that
//! declares no sensing runner would blank it while the region the console is
//! attached to holds frames.

use qualia_types::parse_stack_manifest;

use crate::views::telemetry::SensingRunner;

/// Which sensing rows the stack the console is pointed at produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensingSet {
    /// No `QUALIA_STACK_MANIFEST` names a stack: every sensing slot the
    /// console's ABI read path covers, whatever the compiled-in default stack
    /// declares — a manifest that names no sensing runner must not narrow the
    /// rows to nothing while the region holds frames.
    EverySlot,
    /// The stack a manifest names: exactly its sensing runners, in its own
    /// order. Empty when the stack declares none, and then the table is
    /// honestly empty — the state `config/stack-manifest.zero-motion.json`
    /// renders.
    Declared(Vec<String>),
}

/// The sensing runners `manifest` declares, in manifest order.
///
/// A runner is sensing when its name is the publisher of a slot the console
/// reads ([`SensingRunner`]); the manifest decides which of those the stack
/// runs, so an undeclared runner contributes no row and a stack with no sensing
/// runner contributes an empty table rather than a built-in one.
pub fn sensing_runner_names(manifest: &str) -> Result<Vec<String>, String> {
    let manifest = parse_stack_manifest(manifest)?;
    Ok(manifest
        .runners
        .iter()
        .map(|runner| runner.name.clone())
        .filter(|name| SensingRunner::for_runner_name(name).is_some())
        .collect())
}

/// The sensing rows the stack the console is pointed at declares.
///
/// `QUALIA_STACK_MANIFEST` names a deployment's manifest; with none named the
/// rows are the ABI's sensing slots — not the compiled-in default stack's
/// sensing runners — because a manifest that declares no sensing runner must
/// not narrow the table to nothing while the region holds frames.
pub fn load_sensing_set() -> Result<SensingSet, String> {
    match std::env::var_os("QUALIA_STACK_MANIFEST").filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = std::path::PathBuf::from(path);
            let text = std::fs::read_to_string(&path).map_err(|error| {
                format!("failed to read stack manifest '{}': {error}", path.display())
            })?;
            Ok(SensingSet::Declared(sensing_runner_names(&text)?))
        }
        None => Ok(SensingSet::EverySlot),
    }
}

/// The product's default stack, compiled in: what runs when no manifest names
/// another one.
const DEFAULT_MANIFEST: &str = include_str!("../../../config/stack-manifest.default.json");

/// Every runner the console should expect a telemetry frame from: the stack's
/// own runner list, then the ABI's sensing slots, in that order and without
/// repeats.
///
/// This is the gap list, not a row table: a runner named here that publishes no
/// frame is a runner the HUD names as missing rather than a panel it invents.
/// The stack is the named deployment manifest, or the compiled-in default when
/// none is named — the product's default stack is what a bare `qualia-init`
/// starts, so its runners are exactly the ones that ought to be publishing.
pub fn load_runner_names() -> Vec<String> {
    let text = match std::env::var_os("QUALIA_STACK_MANIFEST").filter(|path| !path.is_empty()) {
        Some(path) => std::fs::read_to_string(std::path::PathBuf::from(&path)).unwrap_or_default(),
        None => DEFAULT_MANIFEST.to_owned(),
    };
    let mut names: Vec<String> = parse_stack_manifest(&text)
        .map(|manifest| {
            manifest
                .runners
                .iter()
                .map(|runner| runner.name.clone())
                .collect()
        })
        .unwrap_or_default();
    for slot in SensingRunner::ALL {
        let name = slot.runner_name().to_owned();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}
