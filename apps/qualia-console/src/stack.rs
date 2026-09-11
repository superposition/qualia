//! The stack manifest: the console's runner set comes from the document the
//! supervisor reads, never from a list of its own.
//!
//! Traceability (`docs/frontend-lessons.md`, source 1's avoid): the reference
//! front end kept two hard-coded runner-name lists beside
//! `QUALIA_STACK_MANIFEST`; here the manifest is the only stack definition. The
//! telemetry view's rows are exactly the manifest's sensing runners, and the
//! default is the same compiled-in `config/stack-manifest.default.json`
//! `runners/cli` loads when the operator names none.

use qualia_types::parse_stack_manifest;

use crate::views::telemetry::SensingRunner;

/// The default stack manifest, compiled in: `QUALIA_STACK_MANIFEST` names a
/// deployment's manifest, and this is the one `runners/cli` falls back to.
pub const DEFAULT_MANIFEST: &str = include_str!("../../../config/stack-manifest.default.json");

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

/// The sensing runners the stack the console is pointed at declares.
///
/// Reads the manifest `QUALIA_STACK_MANIFEST` names, or the compiled-in default
/// when it names none.
pub fn load_sensing_runner_names() -> Result<Vec<String>, String> {
    match std::env::var_os("QUALIA_STACK_MANIFEST").filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = std::path::PathBuf::from(path);
            let text = std::fs::read_to_string(&path).map_err(|error| {
                format!("failed to read stack manifest '{}': {error}", path.display())
            })?;
            sensing_runner_names(&text)
        }
        None => sensing_runner_names(DEFAULT_MANIFEST),
    }
}
