//! The stack manifest: the runner set a deployment declares, read from the
//! document the supervisor reads.
//!
//! Traceability (`docs/frontend-lessons.md`, source 1's avoid): the reference
//! front end kept two hard-coded runner lists beside `QUALIA_STACK_MANIFEST`, so
//! a named stack is read here rather than re-invented, and its sensing runners
//! are the telemetry table's rows in the manifest's own order.
//!
//! The console's own row set does not wait on a stack that can name a sensing
//! runner. `config/stack-manifest.default.json` is the *product's* default stack
//! and declares none, so with no `QUALIA_STACK_MANIFEST` the rows are the ABI's
//! sensing slots ([`SensingRunner`]): deriving them from the compiled-in default
//! would blank the table while the region the console is attached to holds
//! frames.

use qualia_types::parse_stack_manifest;

use crate::views::telemetry::SensingRunner;

/// Which sensing rows the stack the console is pointed at produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensingSet {
    /// No `QUALIA_STACK_MANIFEST` names a stack: every sensing slot the
    /// console's ABI read path covers.
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
/// rows are the ABI's sensing slots, because the product's default stack
/// declares no sensing runner and deriving the table from it would empty the
/// panel while the region holds frames.
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
