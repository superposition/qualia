//! The journal publish gate (step 37b), ported from `scripts/journal_gate.py`.
//!
//! An entry is produced by agents and reviewed before it goes live, and every
//! role's output is a comment on the entry's PR, so the gate reads the PR. Legs:
//!
//! * `checklists` — `docs/journal-review.md` holds three distinct checklists;
//! * `roles` — the PR carries an editorial comment per role, each opening with
//!   the `braid-review` fenced block; the last comment per role wins;
//! * `entry` — the entry text (`--entry`, or the file the PR changes read at
//!   its head commit, or `--diff`'s added lines for an entry-creating diff)
//!   carries no surviving `<!-- ASK: -->` and holds the style form;
//! * `gate` — `journal-gate: OK`, exit 0, only when BOTH the roles and the entry
//!   legs were evaluated and all three verdicts are `approve`; with `--url` the
//!   live entry and every absolute figure URL must return 200.
//!
//! The port keeps the wording, the exit codes and the "which legs ran" contract:
//! a bare run, an `--entry`-only run and an entry-less roles run each print their
//! own line and exit 1, and never print `journal-gate: OK`.

use crate::git;
use crate::pytext;
use clap::Args;
use regex::Regex;
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const ROLES: [&str; 3] = ["accuracy", "teaching", "style"];
const EDITORIAL_FENCE: &str = "braid-review";
const VERDICTS: [&str; 2] = ["approve", "request-changes"];
const REQUIRED_HEADINGS: [&str; 4] = [
    "**The claim.**",
    "## What we tried",
    "## Evidence",
    "## What this does not establish",
];

fn ask_re() -> Regex {
    Regex::new(r"<!--\s*ASK:").expect("ask regex")
}
fn numbered_re() -> Regex {
    Regex::new(r"^\s*\d+\.\s+(.*\S)\s*$").expect("numbered regex")
}
fn heading_re() -> Regex {
    Regex::new(r"^##\s+(.*\S)\s*$").expect("heading regex")
}
fn field_re() -> Regex {
    Regex::new(r"(?i)^(role|verdict|notes)\s*:\s*(.*)$").expect("field regex")
}
fn integer_re() -> Regex {
    Regex::new(r"^\d+$").expect("integer regex")
}
fn src_re() -> Regex {
    Regex::new(r#"src\s*=\s*"([^"]+)""#).expect("src regex")
}
fn markdown_image_re() -> Regex {
    Regex::new(r"!\[[^\]]*\]\((\S+?)\)").expect("markdown image regex")
}

#[derive(Args)]
pub struct JournalArgs {
    /// OWNER/NAME of the repository that holds the entry PR
    #[arg(long)]
    pub repo: Option<String>,

    /// Entry PR number
    #[arg(long)]
    pub pr: Option<i64>,

    /// JSON array of comment bodies, instead of the PR
    #[arg(long)]
    pub comments: Option<String>,

    /// Unified diff of the entry PR, instead of the PR
    #[arg(long)]
    pub diff: Option<String>,

    /// A local entry markdown file, instead of a PR
    #[arg(long)]
    pub entry: Option<String>,

    /// Live entry URL; its figures must return 200 too
    #[arg(long)]
    pub url: Option<String>,

    /// Run the built-in fixtures
    #[arg(long = "self-test")]
    pub self_test: bool,
}

/// `parse_checklists`: each `## <role>` section mapped to its numbered items.
fn parse_checklists(text: &str) -> Vec<(String, Vec<String>)> {
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in text.replace("\r\n", "\n").split('\n') {
        if let Some(found) = heading_re().captures(line) {
            let name = found[1].trim().to_lowercase();
            sections.push((name, Vec::new()));
            current = Some(sections.len() - 1);
            continue;
        }
        if let Some(index) = current {
            if let Some(item) = numbered_re().captures(line) {
                sections[index].1.push(item[1].to_string());
            }
        }
    }
    sections
}

fn section_of<'a>(sections: &'a [(String, Vec<String>)], role: &str) -> Option<&'a Vec<String>> {
    sections.iter().find(|(name, _)| name == role).map(|(_, items)| items)
}

/// `(problems, summary)` for the checklists in `text`.
fn checklists_of(text: &str) -> (Vec<String>, String) {
    let sections = parse_checklists(text);
    let mut problems = Vec::new();
    for role in ROLES {
        match section_of(&sections, role) {
            None => problems.push(format!("checklists: no `## {role}` section")),
            Some(items) if items.is_empty() => {
                problems.push(format!("checklists: `## {role}` lists no items"))
            }
            Some(_) => {}
        }
    }
    let mut extra: Vec<String> = sections
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| !ROLES.contains(&name.as_str()))
        .collect();
    extra.sort();
    extra.dedup();
    if !extra.is_empty() {
        problems.push(format!(
            "checklists: unexpected section(s) {}; there are exactly three jobs",
            extra
                .iter()
                .map(|name| format!("`## {name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let items: Vec<Vec<String>> = ROLES
        .iter()
        .map(|role| section_of(&sections, role).cloned().unwrap_or_default())
        .collect();
    let distinct = {
        let mut unique = items.clone();
        unique.sort();
        unique.dedup();
        unique.len()
    };
    if items.iter().all(|items| !items.is_empty()) && distinct != items.len() {
        problems.push("checklists: two of the three roles share a checklist".to_string());
    }

    let mut summary = ROLES
        .iter()
        .map(|role| {
            format!(
                "{role}({})",
                section_of(&sections, role).map(Vec::len).unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if problems.is_empty() {
        summary.push_str("; three distinct");
    }
    (problems, summary)
}

/// `check_checklists`: `(problems, summary)` for `docs/journal-review.md`.
fn check_checklists(root: &Path) -> (Vec<String>, String) {
    let path = root.join("docs").join("journal-review.md");
    if !path.is_file() {
        return (vec![format!("checklists: {} is missing", path.display())], String::new());
    }
    checklists_of(&pytext::read_text(&path))
}

/// `first_fenced_block`: the `(tag, lines)` of the comment's opening block.
fn first_fenced_block(text: &str) -> Option<(String, Vec<String>)> {
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_string)
        .collect();
    let start = lines.iter().position(|line| !line.trim().is_empty())?;
    if !pytext::strip(&lines[start]).starts_with("```") {
        return None;
    }
    let opening = pytext::strip(&lines[start]);
    let tag = opening.get(3..).unwrap_or("").trim().to_string();
    let mut body = Vec::new();
    for line in lines.iter().skip(start + 1) {
        if pytext::strip(line).starts_with("```") {
            return Some((tag, body));
        }
        body.push(line.clone());
    }
    None
}

/// One editorial verdict: the winning comment for a role.
#[derive(Clone)]
pub struct Verdict {
    pub verdict: String,
    pub notes: i64,
    pub index: usize,
}

/// `parse_editorial`: `(roles, problems, superseded)`.
pub fn parse_editorial(bodies: &[String]) -> (Vec<(String, Verdict)>, Vec<String>, Vec<String>) {
    let mut roles: Vec<(String, Verdict)> = Vec::new();
    let mut problems = Vec::new();
    let mut superseded = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        let Some((tag, lines)) = first_fenced_block(body) else {
            continue;
        };
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in &lines {
            if let Some(found) = field_re().captures(pytext::strip(line)) {
                fields.push((found[1].to_lowercase(), found[2].trim().to_string()));
            }
        }
        let field = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        let role_text = field("role").to_lowercase();
        let named: Vec<&str> = ROLES.iter().copied().filter(|role| role_text.contains(role)).collect();
        let label = format!("comment {}", index + 1);
        if named.len() > 1 {
            problems.push(format!(
                "{label}: names two roles ({}); one comment carries one role",
                named.join(", ")
            ));
            continue;
        }
        let Some(role) = named.first() else {
            continue;
        };
        if tag != EDITORIAL_FENCE {
            problems.push(format!(
                "{label}: role {role} is fenced `{}`, not `{EDITORIAL_FENCE}`",
                if tag.is_empty() { "(bare)".to_string() } else { tag.clone() }
            ));
            continue;
        }
        let verdict = field("verdict").trim().to_lowercase();
        let notes = field("notes").trim().to_string();
        if !VERDICTS.contains(&verdict.as_str()) {
            problems.push(format!(
                "{label}: {role} has no valid `verdict:` (got {})",
                pytext::py_repr(&field("verdict"))
            ));
            continue;
        }
        if !integer_re().is_match(&notes) {
            problems.push(format!(
                "{label}: {role} has no integer `notes:` (got {})",
                pytext::py_repr(&field("notes"))
            ));
            continue;
        }
        if let Some((_, previous)) = roles.iter().find(|(name, _)| name == role) {
            superseded.push(format!(
                "comment {}: {role} {} superseded by {label} (the last comment per role wins)",
                previous.index, previous.verdict
            ));
        }
        roles.retain(|(name, _)| name != role);
        roles.push((
            role.to_string(),
            Verdict {
                verdict,
                notes: notes.parse().unwrap_or(0),
                index: index + 1,
            },
        ));
    }
    (roles, problems, superseded)
}

/// `gate_state`: `(missing roles, roles whose verdict closes the gate)`.
pub fn gate_state(roles: &[(String, Verdict)]) -> (Vec<String>, Vec<String>) {
    let missing = ROLES
        .iter()
        .filter(|role| !roles.iter().any(|(name, _)| name == *role))
        .map(|role| role.to_string())
        .collect();
    let closed = ROLES
        .iter()
        .filter(|role| {
            roles
                .iter()
                .any(|(name, verdict)| name == *role && verdict.verdict == "request-changes")
        })
        .map(|role| role.to_string())
        .collect();
    (missing, closed)
}

/// `check_entry`: the writer's ASK questions and the style form.
pub fn check_entry(text: &str, label: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (lineno, line) in pytext::splitlines(text).iter().enumerate() {
        if ask_re().is_match(line) {
            problems.push(format!("{label}:{}: unresolved `<!-- ASK: -->`", lineno + 1));
        }
    }
    let lowered = text.to_lowercase();
    for heading in REQUIRED_HEADINGS {
        if !lowered.contains(&heading.to_lowercase()) {
            problems.push(format!("{label}: missing {heading}"));
        }
    }
    problems
}

/// `added_lines`: the `+` side of a unified diff, without the file headers.
pub fn added_lines(diff_text: &str) -> String {
    let mut lines = Vec::new();
    for line in diff_text.replace("\r\n", "\n").split('\n') {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            lines.push(rest.to_string());
        }
    }
    lines.join("\n")
}

/// The `b/<path>` side of a `diff --git a/<path> b/<path>` header, if any.
fn diff_header_path(rest: &str) -> Option<String> {
    rest.rsplit_once(" b/")
        .map(|(_, path)| path.to_string())
        .or_else(|| {
            rest.rsplit(' ')
                .next()
                .and_then(|token| token.strip_prefix("b/").map(str::to_string))
        })
}

/// `diff_markdown_files`: the markdown paths a unified diff names, and the
/// subset it *creates* (`new file mode`), both in diff order. A diff that names
/// markdown but creates none is a **revision**: its `+` lines are the changed
/// lines, not the entry, so the entry's unchanged headings never appear there.
pub fn diff_markdown_files(diff_text: &str) -> (Vec<String>, Vec<String>) {
    let mut named: Vec<String> = Vec::new();
    let mut created: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut is_new = false;
    let flush =
        |path: Option<String>, is_new: bool, named: &mut Vec<String>, created: &mut Vec<String>| {
            if let Some(path) = path {
                if path.to_lowercase().ends_with(".md") {
                    if is_new {
                        created.push(path.clone());
                    }
                    named.push(path);
                }
            }
        };
    for line in diff_text.replace("\r\n", "\n").split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(current.take(), is_new, &mut named, &mut created);
            current = diff_header_path(rest);
            is_new = false;
            continue;
        }
        if line.starts_with("new file mode") {
            is_new = true;
        }
    }
    flush(current, is_new, &mut named, &mut created);
    (named, created)
}

/// `is_entry_shape`: the two shapes an entry takes — a figure directory's
/// `README.md` in this repository, and a `_posts/*.md` entry in the journal
/// site.
fn is_entry_shape(path: &str) -> bool {
    let path = path.replace('\\', "/");
    (path.starts_with("docs/figures/")
        && path.ends_with("/README.md")
        && path.matches('/').count() == 3)
        || path.starts_with("_posts/")
}

/// `entry_path_of`: which of a PR's changed files is the entry. `Err` names the
/// candidates when it cannot tell, because the remedy (`--entry <path>`) needs
/// the path.
fn entry_path_of(changed: &[String]) -> Result<String, String> {
    let markdown: Vec<String> = changed
        .iter()
        .filter(|path| path.to_lowercase().ends_with(".md"))
        .cloned()
        .collect();
    let shaped: Vec<String> = markdown
        .iter()
        .filter(|path| is_entry_shape(path))
        .cloned()
        .collect();
    if shaped.len() == 1 {
        return Ok(shaped[0].clone());
    }
    if markdown.len() == 1 {
        return Ok(markdown[0].clone());
    }
    Err(format!(
        "the PR changes {} markdown file(s) ({}); cannot tell which is the entry — pass --entry <path>. A revision PR's entry already exists on the base, so the diff shows only its changed lines and never its headings",
        markdown.len(),
        if markdown.is_empty() {
            "none".to_string()
        } else {
            markdown.join(", ")
        }
    ))
}

/// `figure_refs`: every `src="…"` and markdown image reference, without
/// duplicates.
pub fn figure_refs(text: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    for found in src_re().captures_iter(text) {
        refs.push(found[1].to_string());
    }
    for found in markdown_image_re().captures_iter(text) {
        refs.push(found[1].to_string());
    }
    let mut unique: Vec<String> = Vec::new();
    for reference in refs {
        if !unique.contains(&reference) {
            unique.push(reference);
        }
    }
    unique
}

/// `figure_urls`: the absolute figure URLs among them.
pub fn figure_urls(text: &str) -> Vec<String> {
    figure_refs(text)
        .into_iter()
        .filter(|reference| reference.starts_with("http://") || reference.starts_with("https://"))
        .collect()
}

/// `relative_figure_problems`: relative figure references, refused in a `--url`
/// run as the `docs/journal-review.md` style 4 failure they are.
pub fn relative_figure_problems(text: &str) -> Vec<String> {
    figure_refs(text)
        .into_iter()
        .filter(|reference| {
            !(reference.starts_with("http://") || reference.starts_with("https://"))
        })
        .map(|reference| {
            format!(
                "figure URL is relative; `docs/journal-review.md` style 4 requires an absolute URL: {reference}"
            )
        })
        .collect()
}

/// `(stdout, None)` or `(None, error)` from `gh`.
fn run_gh(args: &[String]) -> Result<String, String> {
    let out = Command::new("gh").args(args).output();
    let out = match out {
        Ok(out) => out,
        Err(error) => return Err(format!("gh {} could not run: {error}", args.join(" "))),
    };
    if !out.status.success() {
        let detail = if !out.stderr.is_empty() { &out.stderr } else { &out.stdout };
        let detail = String::from_utf8_lossy(detail).trim().to_string();
        return Err(format!(
            "gh {} exited {}: {detail}",
            args.join(" "),
            out.status.code().unwrap_or(-1)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn default_repo() -> Option<String> {
    run_gh(&[
        "repo".to_string(),
        "view".to_string(),
        "--json".to_string(),
        "nameWithOwner".to_string(),
        "--jq".to_string(),
        ".nameWithOwner".to_string(),
    ])
    .ok()
    .map(|stdout| stdout.trim().to_string())
}

fn fetch_comments(repo: &str, pr: i64) -> Result<Vec<String>, String> {
    let stdout = run_gh(&[
        "pr".to_string(),
        "view".to_string(),
        pr.to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--json".to_string(),
        "comments".to_string(),
    ])?;
    let payload: Value = serde_json::from_str(&stdout)
        .map_err(|error| format!("gh pr view --json comments did not return JSON: {error}"))?;
    Ok(payload
        .get("comments")
        .and_then(Value::as_array)
        .map(|comments| {
            comments
                .iter()
                .map(|comment| {
                    comment
                        .get("body")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default())
}

/// `fetch_pr_entry`: the entry text at the PR's head commit, its label, and the
/// note naming that source. The entry leg is a *content* check, and for a
/// revision PR the entry already exists on the base, so its source is the file
/// at `headRefOid` — never the diff's added lines, which hold only the changed
/// lines and would report the entry's unchanged headings as missing.
fn fetch_pr_entry(repo: &str, pr: i64) -> Result<(String, String, String), String> {
    let meta = run_gh(&[
        "pr".to_string(),
        "view".to_string(),
        pr.to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--json".to_string(),
        "files,headRefOid".to_string(),
    ])?;
    let payload: Value = serde_json::from_str(&meta).map_err(|error| {
        format!("gh pr view --json files,headRefOid did not return JSON: {error}")
    })?;
    let head = payload
        .get("headRefOid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if head.is_empty() {
        return Err(format!("PR {pr} has no head commit (headRefOid)"));
    }
    let changed: Vec<String> = payload
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| file.get("path").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let path = entry_path_of(&changed)?;
    let text = run_gh(&[
        "api".to_string(),
        "-H".to_string(),
        "Accept: application/vnd.github.raw".to_string(),
        format!("repos/{repo}/contents/{path}?ref={head}"),
    ])?;
    let short: String = head.chars().take(12).collect();
    let note = format!(
        "entry source: {path} at the PR head {short} (the entry as it exists at the head, not the diff's added lines)"
    );
    Ok((text, path, note))
}

/// `fetch_status`: the HTTP status of a live URL, or `None` when it cannot be
/// read at all.
fn fetch_status(url: &str, timeout: u64) -> Option<u16> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("journal-gate")
        .timeout(Duration::from_secs(timeout))
        .build()
        .ok()?;
    client.get(url).send().ok().map(|response| response.status().as_u16())
}

/// The closing lines and exit code — `finish`, with the legs it actually read.
pub fn finish(
    out: &mut dyn Write,
    err: &mut dyn Write,
    problems: &[String],
    roles_checked: bool,
    entry_checked: bool,
) -> i32 {
    if !problems.is_empty() {
        for problem in problems {
            writeln!(err, "journal-gate: {problem}").ok();
        }
        writeln!(
            out,
            "journal-gate: FAIL - {} problem(s); the gate is not open",
            problems.len()
        )
        .ok();
        return 1;
    }
    if !roles_checked {
        let subject = if entry_checked { "entry" } else { "checklists" };
        writeln!(out, "journal-gate: {subject} OK (roles not checked: pass --pr <n>)").ok();
        writeln!(
            out,
            "journal-gate: FAIL - the roles leg was not checked; `journal-gate: OK` needs an entry PR (--pr <n>, or --comments FILE with --pr <n>)"
        )
        .ok();
        return 1;
    }
    if !entry_checked {
        writeln!(out, "journal-gate: roles OK (entry not checked)").ok();
        writeln!(
            out,
            "journal-gate: FAIL - the entry leg was not checked; `journal-gate: OK` needs the entry text (--entry <file>, or --diff <file> for an entry-creating diff, or --pr <n> to read the entry at the PR's head)"
        )
        .ok();
        return 1;
    }
    writeln!(out, "journal-gate: OK").ok();
    0
}

fn usage_error(err: &mut dyn Write, message: &str) -> i32 {
    writeln!(err, "usage: qualia-gates journal [--repo OWNER/NAME] [--pr N] [--comments FILE] [--diff FILE] [--entry FILE] [--url URL] [--self-test]").ok();
    writeln!(err, "error: {message}").ok();
    2
}

/// The gate, writing to the two streams it was given (the self-test drives this
/// directly). Returns the exit code: 0 open, 1 closed, 2 a usage error.
pub fn drive(
    args: &JournalArgs,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    if args.comments.is_some() && args.pr.is_none() {
        return usage_error(
            err,
            "--comments needs --pr <n>: the JSON is that entry PR's comment stream, and without it the roles leg cannot be checked",
        );
    }
    if args.comments.is_some() && args.diff.is_none() && args.entry.is_none() {
        return usage_error(
            err,
            "--comments needs an explicit entry source (--entry <file>, or --diff <file> for an entry-creating diff): the JSON is the comment stream only, so without one the entry leg would go unchecked and an entry carrying an unresolved `<!-- ASK: -->` could pass",
        );
    }

    let mut problems: Vec<String> = Vec::new();
    let mut entry_text: Option<String> = None;
    let mut entry_label = "entry".to_string();
    let mut roles_checked = false;
    let mut repo = args.repo.clone();

    // The checklists are static and cheap: every run checks them.
    let (check_problems, summary) = check_checklists(&git::repo_root());
    let check_problems_empty = check_problems.is_empty();
    problems.extend(check_problems);
    if check_problems_empty {
        writeln!(out, "journal-gate: checklists OK - {summary}").ok();
    }

    if args.pr.is_some() || args.entry.is_some() {
        let mut bodies: Option<Vec<String>> = None;
        if let Some(path) = &args.comments {
            match std::fs::read_to_string(path) {
                Ok(text) => match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Array(items))
                        if items.iter().all(|item| item.is_string()) =>
                    {
                        bodies = Some(
                            items
                                .iter()
                                .map(|item| item.as_str().unwrap_or("").to_string())
                                .collect(),
                        );
                    }
                    _ => {
                        problems.push(format!(
                            "comments: {path} is not a JSON array of comment bodies"
                        ));
                    }
                },
                Err(error) => {
                    problems.push(format!("comments: {path} could not be read: {error}"));
                }
            }
        } else if args.pr.is_some() {
            if repo.is_none() {
                repo = default_repo();
            }
            match &repo {
                None => problems
                    .push("roles: no --repo given and no default repository resolved".to_string()),
                Some(repo) => match fetch_comments(repo, args.pr.expect("checked")) {
                    Ok(fetched) => bodies = Some(fetched),
                    Err(error) => {
                        problems.push(format!("roles: {error}"));
                    }
                },
            }
        }

        let (roles, role_problems, superseded) =
            parse_editorial(bodies.as_deref().unwrap_or(&[]));
        let role_problems_empty = role_problems.is_empty();
        problems.extend(role_problems);
        for note in &superseded {
            writeln!(out, "journal-gate: {note}").ok();
        }
        roles_checked = bodies.is_some();
        let (missing, closed) = gate_state(&roles);
        if roles_checked && role_problems_empty {
            if !missing.is_empty() {
                problems.push(format!(
                    "roles: no {} comment on the entry PR",
                    missing.join("/")
                ));
            } else if !closed.is_empty() {
                problems.push(format!(
                    "gate closed by {}; the author merges only when all three approve",
                    closed.join(", ")
                ));
            } else {
                let verdicts: Vec<String> = ROLES
                    .iter()
                    .map(|role| {
                        let verdict = roles
                            .iter()
                            .find(|(name, _)| name == role)
                            .map(|(_, verdict)| verdict)
                            .expect("all three present");
                        format!("{role}={}({})", verdict.verdict, verdict.notes)
                    })
                    .collect();
                writeln!(out, "journal-gate: roles OK - {}", verdicts.join(", ")).ok();
            }
        }

        if let Some(entry) = &args.entry {
            match std::fs::read_to_string(entry) {
                Ok(text) => {
                    entry_text = Some(text);
                    entry_label = entry.clone();
                }
                Err(error) => problems.push(format!("entry: {entry} could not be read: {error}")),
            }
        } else if let Some(path) = &args.diff {
            match std::fs::read_to_string(path) {
                Ok(text) => {
                    let (named, created) = diff_markdown_files(&text);
                    if !named.is_empty() && created.len() != named.len() {
                        let changed: Vec<String> = named
                            .iter()
                            .filter(|file| !created.contains(*file))
                            .cloned()
                            .collect();
                        problems.push(format!(
                            "entry: {path} is a revision diff — it changes {} ({}) and creates {} ({}); the entry's headings live in the changed file, so the diff cannot be the source of the heading check — pass --entry <path> with the entry at the PR head instead",
                            changed.len(),
                            if changed.is_empty() { "none".to_string() } else { changed.join(", ") },
                            created.len(),
                            if created.is_empty() { "none".to_string() } else { created.join(", ") },
                        ));
                    } else {
                        entry_text = Some(added_lines(&text));
                        entry_label = created.first().cloned().unwrap_or_else(|| path.clone());
                    }
                }
                Err(error) => problems.push(format!("entry: {path} could not be read: {error}")),
            }
        } else if args.pr.is_some() && repo.is_some() {
            match fetch_pr_entry(&repo.clone().unwrap_or_default(), args.pr.expect("checked")) {
                Ok((text, path, note)) => {
                    writeln!(out, "journal-gate: {note}").ok();
                    entry_text = Some(text);
                    entry_label = path;
                }
                Err(error) => problems.push(format!("entry: {error}")),
            }
        }
        if let Some(text) = &entry_text {
            problems.extend(check_entry(text, &entry_label));
        }
    }

    if let Some(url) = &args.url {
        let status = fetch_status(url, 30);
        match status {
            Some(200) => {
                writeln!(out, "journal-gate: live URL 200 - {url}").ok();
            }
            Some(status) => problems.push(format!("live URL returned {status}: {url}")),
            None => problems.push(format!("live URL could not be read: {url}")),
        }
        if let Some(text) = &entry_text {
            problems.extend(relative_figure_problems(text));
            for figure in figure_urls(text) {
                match fetch_status(&figure, 30) {
                    Some(200) => {
                        writeln!(out, "journal-gate: figure 200 - {figure}").ok();
                    }
                    Some(status) => problems.push(format!("figure returned {status}: {figure}")),
                    None => problems.push(format!("figure could not be read: {figure}")),
                }
            }
        }
    }

    finish(out, err, &problems, roles_checked, entry_text.is_some())
}

/// The gate. 0 open, 1 closed, 2 a usage error.
pub fn run(args: &JournalArgs) -> i32 {
    if args.self_test {
        return self_test();
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = drive(args, &mut out, &mut err);
    let _ = std::io::stdout().write_all(&out);
    let _ = std::io::stderr().write_all(&err);
    code
}

// --- the gate's own fixtures ---------------------------------------------

fn editorial_comment(role: &str, verdict: &str, notes: i64) -> String {
    format!(
        "```braid-review\nrole: {role}\nverdict: {verdict}\nnotes: {notes}\n```\n\n- [x] the checklist, completed\n"
    )
}

const GOOD_ENTRY: &str = "---\ntitle: \"The public record\"\ndate: 2026-09-11\n---\n\n**The claim.** One paragraph with the numbers inline.\n\n## What we tried\n\nA paragraph.\n\n## Evidence\n\n| Quantity | Value |\n| --- | ---: |\n| Edges | 929,735 |\n\n## What this does not establish\n\nA paragraph.\n";

const GOOD_CHECKLISTS: &str = "# Journal review checklists\n\n## accuracy\n\n1. one\n2. two\n3. three\n\n## teaching\n\n1. alpha\n2. beta\n\n## style\n\n1. short paragraphs\n2. valid front matter\n";

/// A `JournalArgs` with everything unset, for the fixture runs.
fn fixture_args() -> JournalArgs {
    JournalArgs {
        repo: None,
        pr: None,
        comments: None,
        diff: None,
        entry: None,
        url: None,
        self_test: false,
    }
}

fn run_gate(args: &JournalArgs) -> (i32, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = drive(args, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

fn run_finish(problems: &[String], roles_checked: bool, entry_checked: bool) -> (i32, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = finish(&mut out, &mut err, problems, roles_checked, entry_checked);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

fn fixture_file(scratch: &Path, name: &str, text: &str) -> String {
    let path = scratch.join(name);
    std::fs::write(&path, text).expect("fixture file");
    path.to_string_lossy().to_string()
}

pub fn self_test() -> i32 {
    let mut failures: Vec<String> = Vec::new();
    let mut ran = 0usize;
    let mut check = |name: &str, condition: bool| {
        ran += 1;
        if !condition {
            failures.push(name.to_string());
        }
    };

    let bodies: Vec<String> = ROLES
        .iter()
        .map(|role| editorial_comment(role, "approve", 0))
        .collect();
    let (roles, problems, _) = parse_editorial(&bodies);
    check(
        "three approvals parse",
        problems.is_empty() && roles.len() == ROLES.len(),
    );
    let state = gate_state(&roles);
    check("three approvals open the gate", state.0.is_empty() && state.1.is_empty());

    let mut roles_closed = roles.clone();
    if let Some((_, verdict)) = roles_closed.iter_mut().find(|(name, _)| name == "teaching") {
        verdict.verdict = "request-changes".to_string();
    }
    let closed_state = gate_state(&roles_closed);
    check(
        "a request-changes closes the gate",
        closed_state.0.is_empty() && closed_state.1 == vec!["teaching".to_string()],
    );

    let (roles, _, _) = parse_editorial(&[
        editorial_comment("accuracy", "approve", 0),
        editorial_comment("style", "approve", 0),
    ]);
    check("a missing role is missing", gate_state(&roles).0 == vec!["teaching".to_string()]);

    let (roles, problems, superseded) = parse_editorial(&[
        editorial_comment("accuracy", "request-changes", 2),
        editorial_comment("accuracy", "approve", 0),
    ]);
    check(
        "the last comment per role wins",
        problems.is_empty()
            && roles
                .iter()
                .find(|(name, _)| name == "accuracy")
                .is_some_and(|(_, verdict)| verdict.verdict == "approve"),
    );
    check(
        "an earlier comment is reported superseded",
        problems.is_empty() && superseded.len() == 1 && superseded[0].contains("superseded"),
    );

    let (_, problems, _) = parse_editorial(&[
        "```braid-review\nrole: accuracy|teaching\nverdict: approve\nnotes: 0\n```\n".to_string(),
    ]);
    check(
        "a fence naming two roles is a problem",
        problems.iter().any(|problem| problem.contains("two roles")),
    );

    let (_, problems, _) =
        parse_editorial(&["```braid-review\nrole: accuracy\nnotes: 0\n```\n".to_string()]);
    check(
        "a missing verdict is a problem",
        problems.iter().any(|problem| problem.contains("verdict")),
    );

    let (_, problems, _) = parse_editorial(&[
        "```braid-review\nrole: style\nverdict: approve\nnotes: many\n```\n".to_string(),
    ]);
    check(
        "a non-integer notes is a problem",
        problems.iter().any(|problem| problem.contains("notes")),
    );

    let braid = "```braid\nagent: X\nbranch: b\nstate: review\nnext: n\nblocked_on: none\nevidence: none\n```\n";
    let (roles, problems, _) = parse_editorial(&[braid.to_string()]);
    check(
        "a breadcrumb is ignored, not an error",
        problems.is_empty() && roles.is_empty(),
    );

    let (_, problems, _) = parse_editorial(&[
        "```text\nrole: accuracy\nverdict: approve\nnotes: 0\n```\n".to_string(),
    ]);
    check(
        "the wrong fence tag is a problem",
        problems.iter().any(|problem| problem.contains("braid-review")),
    );

    check("a good entry passes", check_entry(GOOD_ENTRY, "entry").is_empty());
    check(
        "an unresolved ASK is a problem",
        check_entry(&format!("{GOOD_ENTRY}<!-- ASK: is this right? -->\n"), "entry")
            .iter()
            .any(|problem| problem.contains("ASK")),
    );
    check(
        "a missing heading is a problem",
        check_entry(&GOOD_ENTRY.replace("## Evidence", "## Notes"), "entry")
            .iter()
            .any(|problem| problem.contains("## Evidence")),
    );

    let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@\n-old\n+new\n";
    check("added_lines keeps only additions", added_lines(diff) == "new");
    let revision = "diff --git a/docs/figures/f/README.md b/docs/figures/f/README.md\n--- a/docs/figures/f/README.md\n+++ b/docs/figures/f/README.md\n@@\n-context\n+changed\n";
    let (named, created) = diff_markdown_files(revision);
    check(
        "a revision diff names markdown and creates none",
        named == vec!["docs/figures/f/README.md"] && created.is_empty(),
    );
    let adding = "diff --git a/docs/figures/f/README.md b/docs/figures/f/README.md\nnew file mode 100644\n--- /dev/null\n+++ b/docs/figures/f/README.md\n@@\n+## Evidence\n";
    check(
        "an entry-creating diff creates its markdown",
        diff_markdown_files(adding).1 == vec!["docs/figures/f/README.md"],
    );
    let mixed = "diff --git a/docs/figures/new/README.md b/docs/figures/new/README.md\nnew file mode 100644\n--- /dev/null\n+++ b/docs/figures/new/README.md\n@@\n+## Evidence\n\ndiff --git a/docs/figures/f/README.md b/docs/figures/f/README.md\n--- a/docs/figures/f/README.md\n+++ b/docs/figures/f/README.md\n@@\n-context\n+changed\n";
    let (named, created) = diff_markdown_files(mixed);
    check(
        "a diff that also changes a markdown file is not an entry source",
        named.len() == 2 && created.len() == 1,
    );
    check(
        "the entry is the figure README among the PR's markdown",
        entry_path_of(&[
            "README.md".to_string(),
            "docs/figures/f/README.md".to_string(),
            "docs/figures/f/plot.png".to_string(),
        ])
        .ok()
        .as_deref()
            == Some("docs/figures/f/README.md"),
    );
    check(
        "two unshaped markdown files are ambiguous",
        entry_path_of(&["a.md".to_string(), "b.md".to_string()]).is_err(),
    );
    check(
        "figure_urls finds absolute figures",
        figure_urls("<img src=\"/rel.png\">\n![a](https://e/f.svg)\n") == vec!["https://e/f.svg"],
    );
    check(
        "a relative figure is a style failure",
        relative_figure_problems("<img src=\"figs/a.svg\">\n")
            .iter()
            .any(|problem| problem.contains("absolute URL")),
    );
    check(
        "an absolute figure passes the style item",
        relative_figure_problems("<img src=\"https://e/f.svg\">\n").is_empty(),
    );

    let (problems, summary) = checklists_of(GOOD_CHECKLISTS);
    check("three distinct checklists pass", problems.is_empty());
    check("the summary counts the three jobs", summary.matches('(').count() == 3);

    let shared = GOOD_CHECKLISTS.replace("1. alpha\n2. beta", "1. one\n2. two\n3. three");
    check(
        "a shared checklist is a problem",
        checklists_of(&shared)
            .0
            .iter()
            .any(|problem| problem.contains("share")),
    );

    check(
        "a missing section is a problem",
        checklists_of("# x\n\n## accuracy\n\n1. one\n")
            .0
            .iter()
            .any(|problem| problem.contains("teaching")),
    );

    // F1: the OK contract, driven end to end on temp fixtures. No network, no `gh`.
    let scratch = tempfile::TempDir::new().expect("scratch dir");
    let scratch = scratch.path();
    let entry = fixture_file(scratch, "entry.md", GOOD_ENTRY);
    let entry_diff = fixture_file(
        scratch,
        "entry.diff",
        &format!(
            "{}",
            GOOD_ENTRY.lines().map(|line| format!("+{line}\n")).collect::<String>()
        ),
    );
    let ok_comments = fixture_file(
        scratch,
        "comments-ok.json",
        &serde_json::to_string(
            &ROLES
                .iter()
                .map(|role| editorial_comment(role, "approve", 0))
                .collect::<Vec<_>>(),
        )
        .expect("comments json"),
    );
    let reread_open = fixture_file(
        scratch,
        "comments-reread-open.json",
        &serde_json::to_string(&vec![
            editorial_comment("accuracy", "approve", 0),
            editorial_comment("teaching", "request-changes", 2),
            editorial_comment("style", "approve", 0),
            editorial_comment("teaching", "approve", 0),
        ])
        .expect("comments json"),
    );
    let reread_closed = fixture_file(
        scratch,
        "comments-reread-closed.json",
        &serde_json::to_string(&vec![
            editorial_comment("accuracy", "approve", 0),
            editorial_comment("teaching", "approve", 0),
            editorial_comment("style", "approve", 0),
            editorial_comment("teaching", "request-changes", 3),
        ])
        .expect("comments json"),
    );

    let (code, out, _) = run_gate(&fixture_args());
    check(
        "a bare run does not open the gate",
        code == 1 && out.contains("roles not checked") && !out.contains("\njournal-gate: OK\n"),
    );

    let entry_only = JournalArgs { entry: Some(entry.clone()), ..fixture_args() };
    let (code, out, _) = run_gate(&entry_only);
    check(
        "an entry-only run does not open the gate",
        code == 1 && out.contains("entry OK (roles not checked") && !out.contains("\njournal-gate: OK\n"),
    );

    let comments_only = JournalArgs { comments: Some(ok_comments.clone()), ..fixture_args() };
    let (code, _, _) = run_gate(&comments_only);
    check("--comments without --pr is a usage error", code == 2);

    let comments_with_pr = JournalArgs {
        pr: Some(37),
        comments: Some(ok_comments.clone()),
        ..fixture_args()
    };
    let (code, _, err) = run_gate(&comments_with_pr);
    check(
        "--comments without an entry source is a usage error",
        code == 2 && err.contains("entry source"),
    );

    let (code, out, _) = run_finish(&[], true, true);
    check("both legs checked open the gate", code == 0 && out.contains("journal-gate: OK"));

    let (code, out, _) = run_finish(&[], true, false);
    check(
        "a roles run that never read the entry does not print OK",
        code == 1 && out.contains("entry not checked") && !out.contains("\njournal-gate: OK\n"),
    );

    let both = JournalArgs {
        pr: Some(37),
        comments: Some(ok_comments),
        diff: Some(entry_diff.clone()),
        ..fixture_args()
    };
    let (code, out, _) = run_gate(&both);
    check("roles and entry open the gate", code == 0 && out.contains("journal-gate: OK"));

    let reread = JournalArgs {
        pr: Some(37),
        comments: Some(reread_open),
        diff: Some(entry_diff.clone()),
        ..fixture_args()
    };
    let (code, out, _) = run_gate(&reread);
    check(
        "a later approve opens the gate on the re-read",
        code == 0 && out.contains("superseded by comment 4"),
    );

    let closed = JournalArgs {
        pr: Some(37),
        comments: Some(reread_closed),
        diff: Some(entry_diff),
        ..fixture_args()
    };
    let (code, _, err) = run_gate(&closed);
    check(
        "a later request-changes closes the gate",
        code == 1 && err.contains("gate closed by teaching"),
    );

    if !failures.is_empty() {
        for name in &failures {
            eprintln!("journal-gate: self-test FAIL - {name}");
        }
        println!("journal-gate: self-test FAIL - {} check(s)", failures.len());
        return 1;
    }
    println!("journal-gate: self-test OK - {ran} checks");
    0
}
