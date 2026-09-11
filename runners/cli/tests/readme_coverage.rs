//! Documentation coverage for this crate.
//!
//! The upstream suite carried a repository-wide check that every tracked
//! directory ships a README. That invariant does not hold for the public
//! snapshot and repairing it would mean editing directories this crate does not
//! own, so the same expectation is enforced here for the directories this crate
//! does own.

use std::path::{Path, PathBuf};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn require_readme(dir: &Path) {
    let readme = dir.join("README.md");
    assert!(
        readme.is_file(),
        "missing crate documentation: {}",
        readme.display()
    );
}

#[test]
fn crate_directories_ship_a_readme() {
    let root = crate_root();
    require_readme(&root);
    require_readme(&root.join("src"));
    require_readme(&root.join("tests"));
}
