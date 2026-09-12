//! The gate layer's `git` calls, with Python's failure convention.
//!
//! Every helper here mirrors a `subprocess.run([...], capture_output=True)` in
//! the Python gates: a non-zero exit or a missing `git` is `None`, never an
//! error, because the gates use those failures to mean "this fact is not
//! recorded" (a path added since the base revision, a reference blob that is
//! not there). Stderr is discarded, as the Python calls captured but did not
//! read it.

use std::path::Path;
use std::process::Command;

/// Stripped stdout of `git args` in `cwd`, or `None` when git fails or is not
/// installed (`git_output` in `provenance_check.py`).
pub fn output(args: &[&str], cwd: &Path) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(cwd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Unstripped stdout bytes of `git args` in `cwd`, or `None` on failure
/// (`recorded_bytes`: the blob is compared byte for byte, not as text).
pub fn bytes(args: &[&str], cwd: &Path) -> Option<Vec<u8>> {
    let out = Command::new("git").args(args).current_dir(cwd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(out.stdout)
}

/// Every tracked path, as `git ls-files` lists them.
pub fn ls_files(cwd: &Path) -> Vec<String> {
    let Some(text) = output(&["ls-files"], cwd) else {
        return Vec::new();
    };
    text.lines().filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// The worktree root of the current directory: `git rev-parse --show-toplevel`,
/// falling back to the working directory when that is not a worktree (the
/// Python gates fell back to the checkout holding the script; a binary has no
/// script, so the tree it is run in is the only tree it can mean).
pub fn repo_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    output(&["rev-parse", "--show-toplevel"], &cwd)
        .map(std::path::PathBuf::from)
        .unwrap_or(cwd)
}
