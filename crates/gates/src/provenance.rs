//! The clean-room provenance gate (D-001), ported from
//! `scripts/provenance_check.py`.
//!
//! Two things are fatal and two are reported:
//!
//! * FATAL — whole-file identity: the text is the reference's and is not the
//!   text the merge base with `origin/main` already had at that path;
//! * FATAL — a run of more than two consecutive identical comment or doc lines;
//! * REPORT — the longest run of identical code lines, and the highest share of
//!   a file's non-trivial lines that also appear in the reference;
//! * REPORT — files whose text is the reference's and is the text the base
//!   revision already had there (EOL-IDENTICAL).
//!
//! The tree checked is `--root`, else the nearest ancestor of the working
//! directory carrying the repository's own marker (`.git`, or a `Cargo.toml`
//! with a `[workspace]` table); a root that cannot be found is an error naming
//! what was searched, and a comparison of zero files fails, so a green cannot
//! come from an empty input (`docs/decisions.md`, D-028).
//!
//! The port keeps the predicates (`code_line`, `prose_line`, `longest_run`), the
//! thresholds (`--max-run 20`, `--max-prose-run 2`), the summary lines and the
//! exit codes identical to the script it replaces: this gate's value is that it
//! has not changed while everything else did.

use crate::git;
use crate::pytext;
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};

/// A line that is only punctuation carries no authorship.
const TRIVIAL: &str = "{}()[];,<>=\\'\"`*";

/// Machine-generated, not authored here.
const GENERATED: [&str; 1] = ["Cargo.lock"];

const COMMENT_PREFIXES: [&str; 8] = ["//", "///", "//!", "#", "/*", "*", "<!--", "--"];

#[derive(Args)]
pub struct ProvenanceArgs {
    /// Reference root to compare against (default: $QUALIA_PRIVATE_ROOT, else
    /// the first of C:/qualia and /c/qualia that exists)
    pub reference: Option<String>,

    /// The tree to check (default: the repository root above the working
    /// directory, found by walking up for `.git` or the workspace Cargo.toml)
    #[arg(long)]
    pub root: Option<String>,

    /// Longest run of identical code lines allowed before it is reported
    #[arg(long = "max-run", default_value_t = 20)]
    pub max_run: usize,

    /// Longest run of identical prose lines allowed before it is fatal
    #[arg(long = "max-prose-run", default_value_t = 2)]
    pub max_prose_run: usize,
}

/// The reference root `default_reference()` resolves: the environment override,
/// then the two conventional checkouts.
fn default_reference() -> Option<String> {
    if let Ok(env) = std::env::var("QUALIA_PRIVATE_ROOT") {
        if !env.is_empty() {
            return Some(env);
        }
    }
    ["C:/qualia", "/c/qualia"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_dir())
        .map(str::to_string)
}

/// A directory is the repository's root when it carries the repository's own
/// marker: `.git` (a directory in a checkout, a file in a worktree) or a
/// `Cargo.toml` with a `[workspace]` table — the workspace manifest, which a
/// member crate's manifest is not.
fn is_repo_root(dir: &Path) -> bool {
    if dir.join(".git").exists() {
        return true;
    }
    fs::read_to_string(dir.join("Cargo.toml"))
        .map(|text| text.lines().any(|line| line.trim() == "[workspace]"))
        .unwrap_or(false)
}

/// The tree the gate checks: the `--root` named, else the nearest ancestor of
/// the working directory carrying the repository's own marker. There is no
/// fallback to the directory holding the executable: with an out-of-tree
/// `CARGO_TARGET_DIR` that directory is not the repository, so the gate would
/// walk a tree with no tracked files and print a green that checked nothing.
/// `Err` is the operator-facing message, naming what was searched.
fn resolve_root(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let path = if Path::new(dir).is_absolute() { PathBuf::from(dir) } else { cwd.join(dir) };
        if !path.is_dir() {
            return Err(format!("root {} is not a directory", pytext::py_repr(dir)));
        }
        return Ok(path);
    }
    let Ok(cwd) = std::env::current_dir() else {
        return Err("no repository root: the working directory is unavailable".to_string());
    };
    let mut searched: Vec<String> = Vec::new();
    let mut dir = Some(cwd.as_path());
    while let Some(candidate) = dir {
        if is_repo_root(candidate) {
            return Ok(candidate.to_path_buf());
        }
        searched.push(candidate.display().to_string());
        dir = candidate.parent();
    }
    Err(format!(
        "no repository root: no .git and no workspace Cargo.toml in {} or any ancestor ({} searched); name one with --root DIR",
        cwd.display(),
        searched.join(", ")
    ))
}

/// `data` with every line ending folded to LF: identity is a property of the
/// text, not of the checkout (`core.autocrlf` differs between the two trees).
pub fn normalise_eol(data: &[u8]) -> Vec<u8> {
    let mut folded = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' {
            folded.push(b'\n');
            if i + 1 < data.len() && data[i + 1] == b'\n' {
                i += 2;
                continue;
            }
        } else {
            folded.push(data[i]);
        }
        i += 1;
    }
    folded
}

/// The bytes `rev` records for `rel`, or `None` (`recorded_bytes`).
fn recorded_bytes(rev: &str, rel: &str, root: &Path) -> Option<Vec<u8>> {
    git::bytes(&["cat-file", "blob", &format!("{rev}:{rel}")], root)
}

/// The reference's text for `rel` at its HEAD, folded to LF, or `None`
/// (`recorded_reference_text`).
fn recorded_reference_text(reference: &str, rel: &str) -> Option<Vec<u8>> {
    let bytes = git::bytes(
        &["cat-file", "blob", &format!("HEAD:{rel}")],
        Path::new(reference),
    )?;
    Some(normalise_eol(&bytes))
}

/// The merge base with `origin/main` (else `main`), or `None` (`base_revision`).
fn base_revision(root: &Path) -> Option<String> {
    for candidate in ["origin/main", "main"] {
        if let Some(merge) = git::output(&["merge-base", "HEAD", candidate], root) {
            if !merge.is_empty() {
                return Some(merge);
            }
        }
    }
    None
}

fn trivial(line: &str) -> bool {
    let stripped = pytext::strip(line);
    if stripped.is_empty() {
        return true;
    }
    stripped.chars().all(|c| TRIVIAL.contains(c))
}

fn is_comment(line: &str) -> bool {
    let stripped = pytext::strip(line);
    COMMENT_PREFIXES.iter().any(|prefix| stripped.starts_with(prefix))
}

fn code_line(line: &str) -> bool {
    !trivial(line) && !is_comment(line)
}

fn prose_line(line: &str) -> bool {
    is_comment(line) && !trivial(line)
}

/// Longest run of consecutive identical lines satisfying `predicate` — the
/// script's DP, unchanged (`longest_run`).
pub fn longest_run(a: &[String], b: &[String], predicate: impl Fn(&str) -> bool) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut previous = vec![0usize; b.len() + 1];
    let mut best = 0usize;
    for left in a {
        let mut current = vec![0usize; b.len() + 1];
        if predicate(left) {
            for (j, right) in b.iter().enumerate() {
                if left == right {
                    current[j + 1] = previous[j] + 1;
                    if current[j + 1] > best {
                        best = current[j + 1];
                    }
                }
            }
        }
        previous = current;
    }
    best
}

fn read_lines(path: &Path) -> Vec<String> {
    pytext::splitlines(&pytext::read_text(path))
}

/// The gate. Returns the exit code: 0 holds, 1 a comparison was made and is
/// fatal or nothing was compared, 2 no usable root or reference.
pub fn run(args: &ProvenanceArgs) -> i32 {
    let root = match resolve_root(args.root.as_deref()) {
        Ok(root) => root,
        Err(message) => {
            eprintln!("provenance: {message}");
            return 2;
        }
    };
    let reference = args.reference.clone().or_else(default_reference);
    let usable = reference.as_deref().is_some_and(|path| Path::new(path).is_dir());
    if !usable {
        let shown = reference
            .as_deref()
            .map(pytext::py_repr)
            .unwrap_or_else(|| "None".to_string());
        eprintln!("provenance: reference root {shown} is not a directory");
        return 2;
    }
    let reference = reference.expect("checked above");

    // Content at the base revision is reviewed; identity with the reference
    // there is reported, and identity authored here is fatal.
    let base_rev = base_revision(&root);
    let tracked = git::ls_files(&root);

    let mut checked = 0usize;
    let mut skipped_generated = 0usize;
    let mut identical = 0usize;
    let mut eol_identical = 0usize;
    let mut prose_offences: Vec<(String, usize)> = Vec::new();
    let mut over_run: Vec<(String, usize)> = Vec::new();
    let (mut worst_share, mut worst_file) = (0.0f64, String::new());

    for rel in &tracked {
        if Path::new(&rel)
            .file_name()
            .is_some_and(|name| GENERATED.iter().any(|generated| name == *generated))
        {
            skipped_generated += 1;
            continue;
        }
        let other = Path::new(&reference).join(&rel);
        if !other.is_file() {
            continue;
        }
        checked += 1;
        let mine = root.join(&rel);
        let (Ok(our_bytes), Ok(ref_bytes)) = (fs::read(&mine), fs::read(&other)) else {
            continue;
        };
        let our_text = normalise_eol(&our_bytes);
        let ref_text = normalise_eol(&ref_bytes);
        if our_text == ref_text || recorded_reference_text(&reference, &rel).as_deref() == Some(&our_text) {
            // The text is the reference's. It is fatal when this worktree's text
            // is not the text the base revision already had at this path.
            let base = recorded_bytes(base_rev.as_deref().unwrap_or("HEAD"), &rel, &root);
            if base.as_deref().map(normalise_eol) == Some(our_text.clone()) {
                println!("EOL-IDENTICAL: {rel}");
                eol_identical += 1;
            } else {
                println!("IDENTICAL: {rel}");
                identical += 1;
                continue;
            }
        }

        let a_lines = read_lines(&mine);
        let b_lines = read_lines(&other);
        if a_lines.is_empty() || b_lines.is_empty() {
            continue;
        }

        let prose = longest_run(&a_lines, &b_lines, prose_line);
        if prose > args.max_prose_run {
            prose_offences.push((rel.clone(), prose));
        }

        let run = longest_run(&a_lines, &b_lines, code_line);
        if run > args.max_run {
            over_run.push((rel.clone(), run));
        }

        let ours = a_lines.iter().filter(|line| code_line(line)).count();
        if ours > 0 {
            let theirs: std::collections::HashSet<&String> =
                b_lines.iter().filter(|line| code_line(line)).collect();
            let shared = a_lines
                .iter()
                .filter(|line| code_line(line) && theirs.contains(line))
                .count();
            let share = shared as f64 / ours as f64;
            if share > worst_share {
                worst_share = share;
                worst_file = rel.clone();
            }
        }
    }

    println!(
        "provenance: compared {} authored file(s) ({} generated skipped); \
         {} identical, {} EOL-identical, {} code runs over {}, {} prose runs over {}",
        checked,
        skipped_generated,
        identical,
        eol_identical,
        over_run.len(),
        args.max_run,
        prose_offences.len(),
        args.max_prose_run
    );
    println!(
        "provenance: highest identical-code-line share {:.3}{}",
        worst_share,
        if worst_file.is_empty() { String::new() } else { format!(" ({worst_file})") }
    );

    // A green from an empty input is worse than a red: if the root resolved but
    // nothing was compared, `provenance: OK` is unreachable.
    if checked == 0 {
        let because = if tracked.is_empty() {
            format!("{} lists no tracked files (is it a git worktree?)", root.display())
        } else {
            format!(
                "{} lists {} tracked file(s) but the reference {} shares no path with it",
                root.display(),
                tracked.len(),
                reference
            )
        };
        eprintln!("provenance: FAIL - compared 0 authored file(s): {because}; nothing was checked");
        return 1;
    }

    prose_offences.sort_by_key(|(_, run)| std::cmp::Reverse(*run));
    for (rel, run) in &prose_offences {
        println!("PROSE RUN {run}: {rel}");
    }
    over_run.sort_by_key(|(_, run)| std::cmp::Reverse(*run));
    for (rel, run) in &over_run {
        println!("CODE RUN {run}: {rel}");
    }

    if identical > 0 || !prose_offences.is_empty() {
        eprintln!("provenance: FAIL - no file may be copied, and no prose may be shared");
        return 1;
    }
    println!("provenance: OK");
    0
}
