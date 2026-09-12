//! The figure convention gate (step 38), ported from `scripts/figures_check.py`.
//!
//! Every journal entry's figures live under `docs/figures/<entry-slug>/`, drawn
//! with the one house style from committed data by a committed generator. The
//! gate reads that tree and checks the convention: the house palette, the entry
//! READMEs and their figure tables, the picture rules and the 400 KiB budget,
//! the journal template, the review checklists, and the repo README's journal
//! table. The port keeps each leg's wording and the summary line
//! `figures: OK (<n> entries, <n> figures, 400 KiB budget)` byte for byte.

use crate::pytext;
use clap::Args;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

const FIGURES_REL: &str = "docs/figures";
const HOUSE_REL: &str = "docs/figures/_house.py";
const TEMPLATE_REL: &str = "docs/journal-template.md";
const REVIEW_REL: &str = "docs/journal-review.md";
const README_REL: &str = "README.md";

/// The six palette colours the plan fixes, by the name `_house.py` gives them.
const PALETTE: [(&str, &str); 6] = [
    ("BG", "101217"),
    ("INK", "edf0f5"),
    ("GREEN", "91dbba"),
    ("LAVENDER", "c9b2ff"),
    ("BLUE", "93caff"),
    ("SAND", "e0a08a"),
];
const KINDS: [&str; 5] = ["chart", "diagram", "render", "3d asset", "data"];
const CHART_KIND: &str = "chart";
const DRAWING_KINDS: [&str; 2] = ["diagram", "render"];
const FIGURE_EXTS: [&str; 5] = [".svg", ".png", ".webp", ".glb", ".gltf"];
const BUDGET_BYTES: u64 = 400 * 1024;

const CLAIM: &str = "**The claim.**";
const TEMPLATE_H2S: [&str; 4] = [
    "What we tried",
    "What we changed",
    "Evidence",
    "What this does not establish",
];
const REVIEW_SECTIONS: [&str; 3] = ["accuracy", "teaching", "style"];
const REQUIRED_ENTRIES: [&str; 8] = [
    "the licence and the snapshot",
    "the connectome as a prior",
    "the fly in the belief matrices",
    "the front end, rebuilt from lessons",
    "the agent in the loop",
    "the ladder",
    "the mark",
    "the 4090 and the Nano",
];

fn slug_re() -> Regex {
    Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("slug regex")
}
fn table_header_re() -> Regex {
    Regex::new(r"^\|\s*File\s*\|\s*Kind\s*\|").expect("figure table header regex")
}
fn backtick_re() -> Regex {
    Regex::new(r"`([^`]+)`").expect("backtick regex")
}
fn journal_url_re() -> Regex {
    Regex::new(r"https://superposition\.github\.io/journal/([A-Za-z0-9._-]+)/").expect("journal url regex")
}
fn h2_re() -> Regex {
    Regex::new(r"^##\s+(.*\S)\s*$").expect("h2 regex")
}
fn issue_link_re() -> Regex {
    Regex::new(r"https://github\.com/superposition/qualia/issues/(\d+)").expect("issue link regex")
}
fn journal_header_re() -> Regex {
    Regex::new(r"^\|\s*Entry\s*\|\s*Epic\s*\|").expect("journal header regex")
}

#[derive(Args)]
pub struct FiguresArgs {
    /// The tree to check (default: the worktree you run it in)
    #[arg(long)]
    pub root: Option<String>,

    /// Run the checker's own fixtures; no repository needed
    #[arg(long = "self-test")]
    pub self_test: bool,
}

/// `text.replace("\r\n","\n").split("\n")`, empty trailing line and all.
fn lines_of(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n").split('\n').map(str::to_string).collect()
}

/// Data rows of the first table whose header matches — `table_rows`.
fn table_rows(text: &str, header: &Regex, start: usize) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in lines_of(text).iter().skip(start) {
        let stripped = pytext::strip(line);
        if !in_table {
            if header.is_match(stripped) {
                in_table = true;
            }
            continue;
        }
        if !stripped.starts_with('|') {
            break;
        }
        let cells: Vec<String> = stripped
            .trim_matches('|')
            .split('|')
            .map(|cell| pytext::strip(cell).to_string())
            .collect();
        if cells
            .iter()
            .all(|cell| cell.chars().all(|c| matches!(c, '-' | ':' | ' ')))
        {
            continue;
        }
        rows.push(cells);
    }
    rows
}

fn check_house(root: &Path, problems: &mut Vec<String>) {
    let path = root.join(HOUSE_REL);
    if !path.is_file() {
        problems.push(format!("{HOUSE_REL}: missing"));
        return;
    }
    let text = pytext::read_text(&path).to_lowercase();
    for (name, colour) in PALETTE {
        if !text.contains(colour) {
            problems.push(format!(
                "{HOUSE_REL}: palette colour {name} ({colour}) is not defined"
            ));
        }
    }
}

fn check_entry(root: &Path, name: &str, problems: &mut Vec<String>) -> usize {
    let rel = format!("{FIGURES_REL}/{name}");
    if !slug_re().is_match(name) {
        problems.push(format!("{rel}: directory name is not a lowercase slug"));
    }

    let directory = root.join(&rel);
    let readme = directory.join("README.md");
    if !readme.is_file() {
        problems.push(format!("{rel}: missing README.md"));
        return 0;
    }
    let text = pytext::read_text(&readme);
    if !text.trim_start_matches(pytext::is_space).starts_with("# ") {
        problems.push(format!("{rel}/README.md: no `# Title` line"));
    }

    let rows = table_rows(&text, &table_header_re(), 0);
    if rows.is_empty() {
        problems.push(format!("{rel}/README.md: no `| File | Kind |` figure table"));
        return 0;
    }

    let mut kinds: Vec<String> = Vec::new();
    for cells in &rows {
        if cells.len() < 2 {
            problems.push(format!(
                "{rel}/README.md: table row with no kind: {}",
                pytext::py_list_repr_str(cells)
            ));
            continue;
        }
        let Some(found) = backtick_re().captures(&cells[0]) else {
            problems.push(format!(
                "{rel}/README.md: file cell is not a backticked path: {}",
                cells[0]
            ));
            continue;
        };
        let filename = found[1].to_string();
        let kind = cells[1].split(',').next().unwrap_or("").trim().to_lowercase();
        kinds.push(kind.clone());
        if !KINDS.contains(&kind.as_str()) {
            problems.push(format!("{rel}/README.md: unknown kind `{}`", cells[1]));
        }
        if !directory.join(&filename).is_file() {
            problems.push(format!("{rel}/README.md: declared file {filename} is missing"));
        }
        if filename.ends_with(".svg") && cells[0].contains(".png") {
            let companion = format!("{}.png", filename.trim_end_matches(".svg"));
            if !directory.join(&companion).is_file() {
                problems.push(format!(
                    "{rel}/README.md: {filename} has no `{companion}` beside it"
                ));
            }
        }
    }

    if !kinds.iter().any(|kind| kind == CHART_KIND) {
        problems.push(format!("{rel}: no chart (rule 1: one chart per entry)"));
    }
    if !kinds.iter().any(|kind| DRAWING_KINDS.contains(&kind.as_str())) {
        problems.push(format!(
            "{rel}: no diagram or render (rule 1: one drawing per entry)"
        ));
    }

    for slug in journal_url_re().captures_iter(&text).map(|found| found[1].to_string()) {
        if slug != name {
            problems.push(format!(
                "{rel}/README.md: quotes the journal URL of `{slug}`, not `{name}`"
            ));
        }
    }

    let mut assets: Vec<String> = fs::read_dir(&directory)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|entry| {
                    Path::new(entry)
                        .extension()
                        .map(|ext| {
                            let dotted = format!(".{}", ext.to_string_lossy().to_lowercase());
                            FIGURE_EXTS.contains(&dotted.as_str())
                        })
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    assets.sort();

    if assets.iter().any(|asset| asset.ends_with(".svg")) {
        let generator = directory.join("make_figures.py");
        if !generator.is_file() {
            problems.push(format!("{rel}: SVG figures but no make_figures.py"));
        } else {
            let source = pytext::read_text(&generator);
            if !source.contains("from _house") && !source.contains("import _house") {
                problems.push(format!(
                    "{rel}/make_figures.py: does not import the house style from _house"
                ));
            }
        }
    }
    for asset in &assets {
        let path = directory.join(asset);
        let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if size == 0 {
            problems.push(format!("{rel}/{asset}: empty"));
        } else if size > BUDGET_BYTES {
            problems.push(format!(
                "{rel}/{asset}: {size} bytes over the {BUDGET_BYTES}-byte budget"
            ));
        }
        if asset.ends_with(".svg") && !pytext::read_text(&path).to_lowercase().contains("101217") {
            problems.push(format!("{rel}/{asset}: no house background #101217"));
        }
    }
    assets.len()
}

fn check_template(root: &Path, problems: &mut Vec<String>) {
    let path = root.join(TEMPLATE_REL);
    if !path.is_file() {
        problems.push(format!("{TEMPLATE_REL}: missing"));
        return;
    }
    let text = pytext::read_text(&path);
    let lines = lines_of(&text);
    let headings: Vec<String> = lines
        .iter()
        .filter_map(|line| h2_re().captures(line).map(|found| found[1].to_string()))
        .collect();
    if headings != TEMPLATE_H2S {
        let expected = pytext::py_list_repr_lit(&TEMPLATE_H2S);
        problems.push(format!(
            "{TEMPLATE_REL}: headings are {}, not {expected}",
            pytext::py_list_repr_str(&headings)
        ));
    }
    let claim_lines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(CLAIM))
        .map(|(index, _)| index)
        .collect();
    if claim_lines.is_empty() {
        problems.push(format!("{TEMPLATE_REL}: no `{CLAIM}` line"));
        return;
    }
    let first_heading = lines
        .iter()
        .position(|line| h2_re().is_match(line))
        .unwrap_or(lines.len());
    if claim_lines[0] > first_heading {
        problems.push(format!(
            "{TEMPLATE_REL}: `{CLAIM}` is not before the first section"
        ));
    }
}

fn check_review(root: &Path, problems: &mut Vec<String>) {
    let path = root.join(REVIEW_REL);
    if !path.is_file() {
        problems.push(format!("{REVIEW_REL}: missing"));
        return;
    }
    let text = pytext::read_text(&path);
    let headings: Vec<String> = lines_of(&text)
        .iter()
        .filter_map(|line| h2_re().captures(line).map(|found| found[1].to_lowercase()))
        .collect();
    if headings != REVIEW_SECTIONS {
        let expected = pytext::py_list_repr_lit(&REVIEW_SECTIONS);
        problems.push(format!(
            "{REVIEW_REL}: sections are {}, not {expected}",
            pytext::py_list_repr_str(&headings)
        ));
    }
}

fn check_journal(root: &Path, problems: &mut Vec<String>) {
    let path = root.join(README_REL);
    if !path.is_file() {
        problems.push(format!("{README_REL}: missing"));
        return;
    }
    let text = pytext::read_text(&path);
    let lines = lines_of(&text);
    let Some(start) = lines.iter().position(|line| pytext::strip(line) == "## Journal") else {
        problems.push(format!("{README_REL}: no `## Journal` section"));
        return;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.starts_with("## "))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let section = lines[start..end].join("\n");
    let rows = table_rows(&section, &journal_header_re(), 0);
    if rows.is_empty() {
        problems.push(format!("{README_REL}: `## Journal` has no `| Entry | Epic |` table"));
        return;
    }
    let mut entries: Vec<String> = Vec::new();
    let mut lows: Vec<i64> = Vec::new();
    for cells in &rows {
        if cells.len() < 2 {
            problems.push(format!(
                "{README_REL}: journal row with no epic: {}",
                pytext::py_list_repr_str(cells)
            ));
            continue;
        }
        entries.push(cells[0].clone());
        let numbers: Vec<i64> = issue_link_re()
            .captures_iter(&cells[1])
            .map(|found| found[1].parse().unwrap_or(0))
            .collect();
        if numbers.is_empty() {
            problems.push(format!(
                "{README_REL}: journal row `{}` has no absolute epic link",
                cells[0]
            ));
            continue;
        }
        lows.push(*numbers.iter().min().expect("non-empty"));
    }
    let mut sorted = lows.clone();
    sorted.sort_unstable();
    let mut unique = lows.clone();
    unique.sort_unstable();
    unique.dedup();
    if lows != sorted || unique.len() != lows.len() {
        problems.push(format!("{README_REL}: journal rows are not in epic order: {lows:?}"));
    }
    let named = entries.join(" ").to_lowercase();
    for entry in REQUIRED_ENTRIES {
        if !named.contains(&entry.to_lowercase()) {
            problems.push(format!("{README_REL}: journal entry `{entry}` is not listed"));
        }
    }
}

/// Every problem in the tree at `root`; an empty list is the convention held.
fn check(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    check_house(root, &mut problems);
    let figures = root.join(FIGURES_REL);
    if !figures.is_dir() {
        problems.push(format!("{FIGURES_REL}: missing"));
        return problems;
    }
    let mut entries = 0usize;
    let mut names: Vec<String> = fs::read_dir(&figures)
        .map(|dir| dir.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    names.sort();
    for name in names {
        if name.starts_with('_') || name.starts_with('.') || name == "__pycache__" {
            continue;
        }
        if figures.join(&name).is_dir() {
            entries += 1;
            check_entry(root, &name, &mut problems);
        }
    }
    if entries == 0 {
        problems.push(format!("{FIGURES_REL}: no entry directories"));
    }
    check_template(root, &mut problems);
    check_review(root, &mut problems);
    check_journal(root, &mut problems);
    problems
}

fn count_figures(root: &Path) -> usize {
    let figures = root.join(FIGURES_REL);
    let mut names: Vec<String> = fs::read_dir(&figures)
        .map(|dir| dir.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    names.sort();
    let mut total = 0usize;
    for name in names {
        let directory = figures.join(&name);
        if name.starts_with('_') || name.starts_with('.') || !directory.is_dir() {
            continue;
        }
        total += fs::read_dir(&directory)
            .map(|dir| {
                dir.flatten()
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .map(|ext| {
                                let dotted = format!(".{}", ext.to_string_lossy().to_lowercase());
                                FIGURE_EXTS.contains(&dotted.as_str())
                            })
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0);
    }
    total
}

fn report(root: &Path) -> i32 {
    let problems = check(root);
    for problem in &problems {
        eprintln!("figures: FAIL - {problem}");
    }
    if !problems.is_empty() {
        return 1;
    }
    let figures = root.join(FIGURES_REL);
    let entries = fs::read_dir(&figures)
        .map(|dir| {
            dir.flatten()
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    !name.starts_with('_')
                        && !name.starts_with('.')
                        && entry.path().is_dir()
                })
                .count()
        })
        .unwrap_or(0);
    println!(
        "figures: OK ({entries} entries, {} figures, {} KiB budget)",
        count_figures(root),
        BUDGET_BYTES / 1024
    );
    0
}

// --- the checker's own fixtures ------------------------------------------

const GOOD_README: &str = r#"# Figures - fx

Figures for the journal entry [fx](https://superposition.github.io/journal/fx/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `a.svg` (+ `.png`) | chart | One bar per run. |
| `b.svg` (+ `.png`) | diagram | The boundary. |

## Regenerating

```console
$ python make_figures.py
```
"#;

const GOOD_TEMPLATE: &str = r#"# Journal entry template

**The claim.** One paragraph with the numbers inline.

## What we tried

## What we changed

## Evidence

## What this does not establish
"#;

const GOOD_REVIEW: &str = r#"# Journal review checklists

## accuracy

1. One item.

## teaching

1. One item.

## style

1. One item.
"#;

const GOOD_JOURNAL: &str = r#"# Repo

## Journal

| Entry | Epic |
| --- | --- |
| the licence and the snapshot | [EPIC-01](https://github.com/superposition/qualia/issues/1) |
| the connectome as a prior | [EPIC-02](https://github.com/superposition/qualia/issues/2) |
| the fly in the belief matrices | [EPIC-03](https://github.com/superposition/qualia/issues/3) |
| the agent in the loop | [EPIC-07](https://github.com/superposition/qualia/issues/7) |
| the front end, rebuilt from lessons | [EPIC-09](https://github.com/superposition/qualia/issues/10) |
| the ladder | [EPIC-08C](https://github.com/superposition/qualia/issues/11) |
| the mark | [EPIC-11](https://github.com/superposition/qualia/issues/13) |
| the 4090 and the Nano | [EPIC-10](https://github.com/superposition/qualia/issues/15) |

## Licence
"#;

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(path, text).expect("fixture file");
}

fn make_good_tree(root: &Path) {
    let house: Vec<String> = PALETTE
        .iter()
        .map(|(name, colour)| format!("{name} = \"#{colour}\""))
        .collect();
    write_file(&root.join(HOUSE_REL), &format!("{}\n", house.join("\n")));
    write_file(
        &root.join(FIGURES_REL).join("fx").join("make_figures.py"),
        "from _house import save\n",
    );
    write_file(&root.join(FIGURES_REL).join("fx").join("README.md"), GOOD_README);
    for name in ["a", "b"] {
        write_file(
            &root.join(FIGURES_REL).join("fx").join(format!("{name}.svg")),
            "<svg fill=\"#101217\"></svg>\n",
        );
        write_file(&root.join(FIGURES_REL).join("fx").join(format!("{name}.png")), "PNG\n");
    }
    write_file(&root.join(TEMPLATE_REL), GOOD_TEMPLATE);
    write_file(&root.join(REVIEW_REL), GOOD_REVIEW);
    write_file(&root.join(README_REL), GOOD_JOURNAL);
}

fn mutate(path: &Path, old: &str, new: &str) {
    let text = pytext::read_text(path);
    assert!(text.contains(old), "fixture does not contain {old:?}");
    write_file(path, &text.replacen(old, new, 1));
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("fixture copy");
    for entry in fs::read_dir(from).expect("fixture dir").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("fixture file copy");
        }
    }
}

pub fn self_test() -> i32 {
    let scratch = tempfile::TempDir::new().expect("scratch dir");
    let good = scratch.path().join("good");
    make_good_tree(&good);
    let good_problems = check(&good);
    if !good_problems.is_empty() {
        eprintln!("figures: self-test FAIL - the good fixture does not hold");
        for problem in good_problems {
            eprintln!("figures: {problem}");
        }
        return 1;
    }

    let mut cases: Vec<(&str, Box<dyn Fn(&Path)>)> = Vec::new();
    cases.push((
        "no chart",
        Box::new(|root: &Path| {
            mutate(
                &root.join(FIGURES_REL).join("fx").join("README.md"),
                "| `a.svg` (+ `.png`) | chart | One bar per run. |",
                "| `a.svg` (+ `.png`) | diagram | One bar per run. |",
            )
        }),
    ));
    cases.push((
        "no diagram or render",
        Box::new(|root: &Path| {
            mutate(
                &root.join(FIGURES_REL).join("fx").join("README.md"),
                "| `b.svg` (+ `.png`) | diagram | The boundary. |",
                "| `b.svg` (+ `.png`) | data | The boundary. |",
            )
        }),
    ));
    cases.push((
        "declared file missing",
        Box::new(|root: &Path| {
            fs::remove_file(root.join(FIGURES_REL).join("fx").join("b.png")).expect("remove b.png")
        }),
    ));
    cases.push((
        "empty figure",
        Box::new(|root: &Path| {
            write_file(&root.join(FIGURES_REL).join("fx").join("b.png"), "")
        }),
    ));
    cases.push((
        "over budget",
        Box::new(|root: &Path| {
            write_file(
                &root.join(FIGURES_REL).join("fx").join("b.png"),
                &"x".repeat(BUDGET_BYTES as usize + 1),
            )
        }),
    ));
    cases.push((
        "wrong background",
        Box::new(|root: &Path| {
            write_file(
                &root.join(FIGURES_REL).join("fx").join("b.svg"),
                "<svg fill=\"#ffffff\"></svg>\n",
            )
        }),
    ));
    cases.push((
        "generator does not import the house style",
        Box::new(|root: &Path| {
            write_file(
                &root.join(FIGURES_REL).join("fx").join("make_figures.py"),
                "import matplotlib\n",
            )
        }),
    ));
    cases.push((
        "unknown kind",
        Box::new(|root: &Path| {
            mutate(
                &root.join(FIGURES_REL).join("fx").join("README.md"),
                "| `a.svg` (+ `.png`) | chart | One bar per run. |",
                "| `a.svg` (+ `.png`) | pie | One bar per run. |",
            )
        }),
    ));
    cases.push((
        "journal URL for another slug",
        Box::new(|root: &Path| {
            mutate(
                &root.join(FIGURES_REL).join("fx").join("README.md"),
                "https://superposition.github.io/journal/fx/",
                "https://superposition.github.io/journal/other/",
            )
        }),
    ));
    cases.push((
        "missing palette colour",
        Box::new(|root: &Path| mutate(&root.join(HOUSE_REL), "SAND = \"#e0a08a\"", "SAND = \"#ffffff\"")),
    ));
    cases.push((
        "template heading added",
        Box::new(|root: &Path| {
            mutate(&root.join(TEMPLATE_REL), "## Evidence", "## Numbers\n\n## Evidence")
        }),
    ));
    cases.push((
        "template claim after the sections",
        Box::new(|root: &Path| {
            let late = format!(
                "{}\n**The claim.** Late.\n",
                GOOD_TEMPLATE.replace("**The claim.** One paragraph with the numbers inline.\n\n", "")
            );
            write_file(&root.join(TEMPLATE_REL), &late)
        }),
    ));
    cases.push((
        "review section missing",
        Box::new(|root: &Path| mutate(&root.join(REVIEW_REL), "## teaching", "## Teaching notes")),
    ));
    cases.push((
        "journal out of epic order",
        Box::new(|root: &Path| {
            write_file(
                &root.join(README_REL),
                &GOOD_JOURNAL.replace(
                    "| the connectome as a prior | [EPIC-02](https://github.com/superposition/qualia/issues/2) |",
                    "| the connectome as a prior | [EPIC-04](https://github.com/superposition/qualia/issues/4) |",
                ),
            )
        }),
    ));
    cases.push((
        "required entry missing",
        Box::new(|root: &Path| {
            write_file(
                &root.join(README_REL),
                &GOOD_JOURNAL.replace(
                    "| the ladder | [EPIC-08C](https://github.com/superposition/qualia/issues/11) |\n",
                    "",
                ),
            )
        }),
    ));
    cases.push((
        "epic link is relative",
        Box::new(|root: &Path| {
            mutate(
                &root.join(README_REL),
                "[EPIC-01](https://github.com/superposition/qualia/issues/1)",
                "[EPIC-01](issues/1)",
            )
        }),
    ));
    cases.push((
        "no entry directories",
        Box::new(|root: &Path| {
            fs::remove_dir_all(root.join(FIGURES_REL).join("fx")).expect("remove fx")
        }),
    ));

    for (index, (label, apply)) in cases.iter().enumerate() {
        let root = scratch.path().join(format!("case{index:02}"));
        copy_tree(&good, &root);
        apply(&root);
        if check(&root).is_empty() {
            eprintln!("figures: self-test FAIL - {label} passed");
            return 1;
        }
    }
    println!("figures: self-test OK ({} mutations)", cases.len());
    0
}

/// The gate. 0 holds, 1 a leg failed, 2 a usage error.
pub fn run(args: &FiguresArgs) -> i32 {
    if args.self_test {
        return self_test();
    }
    let root = match &args.root {
        Some(dir) => std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(dir),
        None => crate::git::repo_root(),
    };
    if !root.is_dir() {
        eprintln!("usage: qualia-gates figures [--root DIR] [--self-test]");
        eprintln!("figures: usage error - no such directory: {}", root.display());
        return 2;
    }
    report(&root)
}
