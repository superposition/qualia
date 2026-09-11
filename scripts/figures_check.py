#!/usr/bin/env python3
"""The figure convention check (step 38; see `docs/figures/README.md`).

Every journal entry's figures are committed under `docs/figures/<entry-slug>/`,
drawn with the one house style (`docs/figures/_house.py`) from committed data by
a committed generator. This gate reads that tree and checks the convention:

  house     `docs/figures/_house.py` exists and defines the six palette
            colours the plan fixes: background `#101217`, ink `#edf0f5`,
            accents `#91dbba`, `#c9b2ff`, `#93caff`, `#e0a08a`.
  entries   every directory under `docs/figures/` other than `_house.py` is one
            entry, its name is a lowercase slug, it has a README whose first
            table is the figure list (`| File | Kind | ...`), every file the
            table names exists, every `.svg` row's `(+ `.png`)` companion
            exists, and every kind begins with one of `chart`, `diagram`,
            `render`, `3D asset`, `data`.
  pictures  each entry declares at least one `chart` and at least one of
            `diagram` or `render` (step 38's rule: a human learner needs the
            picture as well as the number); every figure asset is non-empty and
            no larger than 400 KiB; every committed `.svg` carries the house
            background `#101217`; a directory with an SVG has a
            `make_figures.py` that imports `_house`; and any journal URL a
            README quotes names that directory's own slug.
  template  `docs/journal-template.md` has exactly the headings `**The
            claim.**`, `## What we tried`, `## What we changed`, `## Evidence`,
            `## What this does not establish`, in that order.
  review    `docs/journal-review.md` holds one `## accuracy`, `## teaching` and
            `## style` section. The publish gate reads the same three
            checklists from it (`scripts/journal_gate.py`, step 37b), so a
            checklist item changes in exactly one file.
  journal   the repo `README.md`'s `## Journal` table names the entries in epic
            order with absolute epic links, and the plan's required entries are
            all there.

The gate is offline and deterministic. Live URLs — the entry and each figure
returning 200 — are the publish gate's `--url` leg, not this one's.

Usage:

    python scripts/figures_check.py
    python scripts/figures_check.py --root DIR
    python scripts/figures_check.py --self-test

`--root DIR` checks that tree instead of the worktree the script is run in.
`--self-test` builds its own fixtures in a temporary directory — a good tree,
then one mutation per rule — and needs no repository and no network.

Exit codes: 0 when every leg holds and `figures: OK` is printed; 1 when a leg
fails, the failing paths named on stderr; 2 for a usage error.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

FIGURES_REL = os.path.join("docs", "figures")
HOUSE_REL = os.path.join(FIGURES_REL, "_house.py")
TEMPLATE_REL = os.path.join("docs", "journal-template.md")
REVIEW_REL = os.path.join("docs", "journal-review.md")
README_REL = "README.md"

# The six palette colours the plan fixes, by the name `_house.py` gives them.
PALETTE = {
    "BG": "101217",
    "INK": "edf0f5",
    "GREEN": "91dbba",
    "LAVENDER": "c9b2ff",
    "BLUE": "93caff",
    "SAND": "e0a08a",
}
KINDS = ("chart", "diagram", "render", "3d asset", "data")
CHART_KIND = "chart"
DRAWING_KINDS = ("diagram", "render")
FIGURE_EXTS = (".svg", ".png", ".webp", ".glb", ".gltf")
BUDGET_BYTES = 400 * 1024

SLUG_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
TABLE_HEADER_RE = re.compile(r"^\|\s*File\s*\|\s*Kind\s*\|")
BACKTICK_RE = re.compile(r"`([^`]+)`")
JOURNAL_URL_RE = re.compile(
    r"https://superposition\.github\.io/journal/([A-Za-z0-9._-]+)/"
)
H2_RE = re.compile(r"^##\s+(.*\S)\s*$")
CLAIM = "**The claim.**"
TEMPLATE_H2S = [
    "What we tried",
    "What we changed",
    "Evidence",
    "What this does not establish",
]
REVIEW_SECTIONS = ["accuracy", "teaching", "style"]
ISSUE_LINK_RE = re.compile(r"https://github\.com/superposition/qualia/issues/(\d+)")
JOURNAL_HEADER_RE = re.compile(r"^\|\s*Entry\s*\|\s*Epic\s*\|")
REQUIRED_ENTRIES = (
    "the licence and the snapshot",
    "the connectome as a prior",
    "the fly in the belief matrices",
    "the front end, rebuilt from lessons",
    "the agent in the loop",
    "the ladder",
    "the mark",
    "the 4090 and the Nano",
)


def repo_root() -> str:
    """The worktree root of the current directory, like provenance_check.py."""
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if proc.returncode == 0 and proc.stdout.strip():
            return proc.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        pass
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def read_text(path: str) -> str:
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def lines_of(text: str):
    return text.replace("\r\n", "\n").split("\n")


def table_rows(text: str, header_re: re.Pattern, start: int = 0):
    """Data rows of the first table whose header matches `header_re`.

    A row is a list of cell strings; the `| --- |` separator is skipped. The
    scan starts at line `start` and stops at the first non-table line.
    """
    rows = []
    in_table = False
    for line in lines_of(text)[start:]:
        stripped = line.strip()
        if not in_table:
            if header_re.match(stripped):
                in_table = True
            continue
        if not stripped.startswith("|"):
            break
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if all(set(cell) <= set("-: ") for cell in cells):
            continue
        rows.append(cells)
    return rows


def check_house(root: str, problems: list) -> None:
    path = os.path.join(root, HOUSE_REL)
    if not os.path.isfile(path):
        problems.append("%s: missing" % HOUSE_REL)
        return
    text = read_text(path).lower()
    for name, colour in PALETTE.items():
        if colour not in text:
            problems.append(
                "%s: palette colour %s (%s) is not defined" % (HOUSE_REL, name, colour)
            )


def check_entry(root: str, name: str, problems: list) -> int:
    """Check one `docs/figures/<name>/`; return its figure-asset count."""
    rel = os.path.join(FIGURES_REL, name)
    if not SLUG_RE.match(name):
        problems.append("%s: directory name is not a lowercase slug" % rel)

    readme = os.path.join(root, rel, "README.md")
    if not os.path.isfile(readme):
        problems.append("%s: missing README.md" % rel)
        return 0
    text = read_text(readme)
    if not text.lstrip().startswith("# "):
        problems.append("%s/README.md: no `# Title` line" % rel)

    rows = table_rows(text, TABLE_HEADER_RE)
    if not rows:
        problems.append("%s/README.md: no `| File | Kind |` figure table" % rel)
        return 0

    kinds = set()
    for cells in rows:
        if len(cells) < 2:
            problems.append("%s/README.md: table row with no kind: %s" % (rel, cells))
            continue
        match = BACKTICK_RE.search(cells[0])
        if not match:
            problems.append(
                "%s/README.md: file cell is not a backticked path: %s" % (rel, cells[0])
            )
            continue
        filename = match.group(1)
        kind = cells[1].split(",")[0].strip().lower()
        kinds.add(kind)
        if kind not in KINDS:
            problems.append("%s/README.md: unknown kind `%s`" % (rel, cells[1]))
        if not os.path.isfile(os.path.join(root, rel, filename)):
            problems.append("%s/README.md: declared file %s is missing" % (rel, filename))
        if filename.endswith(".svg") and ".png" in cells[0]:
            companion = filename[: -len(".svg")] + ".png"
            if not os.path.isfile(os.path.join(root, rel, companion)):
                problems.append(
                    "%s/README.md: %s has no `%s` beside it" % (rel, filename, companion)
                )

    if CHART_KIND not in kinds:
        problems.append("%s: no chart (rule 1: one chart per entry)" % rel)
    if not kinds & set(DRAWING_KINDS):
        problems.append(
            "%s: no diagram or render (rule 1: one drawing per entry)" % rel
        )

    for slug in JOURNAL_URL_RE.findall(text):
        if slug != name:
            problems.append(
                "%s/README.md: quotes the journal URL of `%s`, not `%s`"
                % (rel, slug, name)
            )

    directory = os.path.join(root, rel)
    assets = sorted(
        entry
        for entry in os.listdir(directory)
        if os.path.splitext(entry)[1].lower() in FIGURE_EXTS
    )
    if any(asset.endswith(".svg") for asset in assets):
        generator = os.path.join(directory, "make_figures.py")
        if not os.path.isfile(generator):
            problems.append("%s: SVG figures but no make_figures.py" % rel)
        else:
            source = read_text(generator)
            if "from _house" not in source and "import _house" not in source:
                problems.append(
                    "%s/make_figures.py: does not import the house style from _house"
                    % rel
                )
    for asset in assets:
        path = os.path.join(directory, asset)
        size = os.path.getsize(path)
        if size == 0:
            problems.append("%s/%s: empty" % (rel, asset))
        elif size > BUDGET_BYTES:
            problems.append(
                "%s/%s: %d bytes over the %d-byte budget"
                % (rel, asset, size, BUDGET_BYTES)
            )
        if asset.endswith(".svg") and PALETTE["BG"] not in read_text(path).lower():
            problems.append(
                "%s/%s: no house background #%s" % (rel, asset, PALETTE["BG"])
            )
    return len(assets)


def check_template(root: str, problems: list) -> None:
    path = os.path.join(root, TEMPLATE_REL)
    if not os.path.isfile(path):
        problems.append("%s: missing" % TEMPLATE_REL)
        return
    text = read_text(path)
    headings = [match.group(1) for match in map(H2_RE.match, lines_of(text)) if match]
    if headings != TEMPLATE_H2S:
        problems.append(
            "%s: headings are %s, not %s"
            % (TEMPLATE_REL, headings, TEMPLATE_H2S)
        )
    claim_lines = [i for i, line in enumerate(lines_of(text)) if CLAIM in line]
    if not claim_lines:
        problems.append("%s: no `%s` line" % (TEMPLATE_REL, CLAIM))
        return
    first_heading = next(
        (
            i
            for i, line in enumerate(lines_of(text))
            if H2_RE.match(line)
        ),
        len(lines_of(text)),
    )
    if claim_lines[0] > first_heading:
        problems.append("%s: `%s` is not before the first section" % (TEMPLATE_REL, CLAIM))


def check_review(root: str, problems: list) -> None:
    path = os.path.join(root, REVIEW_REL)
    if not os.path.isfile(path):
        problems.append("%s: missing" % REVIEW_REL)
        return
    headings = [
        match.group(1).lower()
        for match in map(H2_RE.match, lines_of(read_text(path)))
        if match
    ]
    if headings != REVIEW_SECTIONS:
        problems.append(
            "%s: sections are %s, not %s" % (REVIEW_REL, headings, REVIEW_SECTIONS)
        )


def check_journal(root: str, problems: list) -> None:
    path = os.path.join(root, README_REL)
    if not os.path.isfile(path):
        problems.append("%s: missing" % README_REL)
        return
    text = read_text(path)
    lines = lines_of(text)
    start = next((i for i, line in enumerate(lines) if line.strip() == "## Journal"), None)
    if start is None:
        problems.append("%s: no `## Journal` section" % README_REL)
        return
    end = next(
        (i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")),
        len(lines),
    )
    rows = table_rows("\n".join(lines[start:end]), JOURNAL_HEADER_RE)
    if not rows:
        problems.append("%s: `## Journal` has no `| Entry | Epic |` table" % README_REL)
        return
    entries = []
    lows = []
    for cells in rows:
        if len(cells) < 2:
            problems.append("%s: journal row with no epic: %s" % (README_REL, cells))
            continue
        entries.append(cells[0])
        numbers = [int(number) for number in ISSUE_LINK_RE.findall(cells[1])]
        if not numbers:
            problems.append(
                "%s: journal row `%s` has no absolute epic link" % (README_REL, cells[0])
            )
            continue
        lows.append(min(numbers))
    if lows != sorted(lows) or len(set(lows)) != len(lows):
        problems.append(
            "%s: journal rows are not in epic order: %s" % (README_REL, lows)
        )
    named = " ".join(entries).lower()
    for entry in REQUIRED_ENTRIES:
        if entry.lower() not in named:
            problems.append("%s: journal entry `%s` is not listed" % (README_REL, entry))


def check(root: str) -> list:
    """Every problem in the tree at `root`; an empty list is the convention held."""
    problems: list = []
    check_house(root, problems)
    figures = os.path.join(root, FIGURES_REL)
    if not os.path.isdir(figures):
        problems.append("%s: missing" % FIGURES_REL)
        return problems
    entries = 0
    for name in sorted(os.listdir(figures)):
        if name.startswith("_") or name.startswith(".") or name == "__pycache__":
            continue
        if os.path.isdir(os.path.join(figures, name)):
            entries += 1
            check_entry(root, name, problems)
    if not entries:
        problems.append("%s: no entry directories" % FIGURES_REL)
    check_template(root, problems)
    check_review(root, problems)
    check_journal(root, problems)
    return problems


def count_figures(root: str) -> int:
    figures = os.path.join(root, FIGURES_REL)
    total = 0
    for name in sorted(os.listdir(figures)):
        directory = os.path.join(figures, name)
        if name.startswith("_") or name.startswith(".") or not os.path.isdir(directory):
            continue
        total += sum(
            1
            for entry in os.listdir(directory)
            if os.path.splitext(entry)[1].lower() in FIGURE_EXTS
        )
    return total


def report(root: str) -> int:
    problems = check(root)
    for problem in problems:
        sys.stderr.write("figures: FAIL - %s\n" % problem)
    if problems:
        return 1
    figures = os.path.join(root, FIGURES_REL)
    entries = sum(
        1
        for name in os.listdir(figures)
        if not name.startswith("_")
        and not name.startswith(".")
        and os.path.isdir(os.path.join(figures, name))
    )
    print(
        "figures: OK (%d entries, %d figures, %d KiB budget)"
        % (entries, count_figures(root), BUDGET_BYTES // 1024)
    )
    return 0


# --- the checker's own fixtures -------------------------------------------------

GOOD_README = """# Figures - fx

Figures for the journal entry [fx](https://superposition.github.io/journal/fx/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `a.svg` (+ `.png`) | chart | One bar per run. |
| `b.svg` (+ `.png`) | diagram | The boundary. |

## Regenerating

```console
$ python make_figures.py
```
"""

GOOD_TEMPLATE = """# Journal entry template

**The claim.** One paragraph with the numbers inline.

## What we tried

## What we changed

## Evidence

## What this does not establish
"""

GOOD_REVIEW = """# Journal review checklists

## accuracy

1. One item.

## teaching

1. One item.

## style

1. One item.
"""

GOOD_JOURNAL = """# Repo

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
"""


def write_file(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def make_good_tree(root: str) -> None:
    write_file(
        os.path.join(root, HOUSE_REL),
        "\n".join(
            "%s = \"#%s\"" % (name, colour) for name, colour in PALETTE.items()
        ),
    )
    write_file(
        os.path.join(root, FIGURES_REL, "fx", "make_figures.py"),
        "from _house import save\n",
    )
    write_file(os.path.join(root, FIGURES_REL, "fx", "README.md"), GOOD_README)
    for name in ("a", "b"):
        write_file(
            os.path.join(root, FIGURES_REL, "fx", name + ".svg"),
            '<svg fill="#%s"></svg>\n' % PALETTE["BG"],
        )
        write_file(
            os.path.join(root, FIGURES_REL, "fx", name + ".png"), "PNG\n"
        )
    write_file(os.path.join(root, TEMPLATE_REL), GOOD_TEMPLATE)
    write_file(os.path.join(root, REVIEW_REL), GOOD_REVIEW)
    write_file(os.path.join(root, README_REL), GOOD_JOURNAL)


def mutate(path: str, old: str, new: str) -> None:
    text = read_text(path)
    if old not in text:
        raise AssertionError("fixture does not contain %r" % old)
    write_file(path, text.replace(old, new, 1))


def self_test() -> int:
    cases = []
    with tempfile.TemporaryDirectory(prefix="figures-check-") as tmp:
        make_good_tree(os.path.join(tmp, "good"))
        if check(os.path.join(tmp, "good")):
            sys.stderr.write("figures: self-test FAIL - the good fixture does not hold\n")
            for problem in check(os.path.join(tmp, "good")):
                sys.stderr.write("figures: %s\n" % problem)
            return 1
        broken = lambda label, fn: cases.append((label, fn))
        broken(
            "no chart",
            lambda root: mutate(
                os.path.join(root, FIGURES_REL, "fx", "README.md"),
                "| `a.svg` (+ `.png`) | chart | One bar per run. |",
                "| `a.svg` (+ `.png`) | diagram | One bar per run. |",
            ),
        )
        broken(
            "no diagram or render",
            lambda root: mutate(
                os.path.join(root, FIGURES_REL, "fx", "README.md"),
                "| `b.svg` (+ `.png`) | diagram | The boundary. |",
                "| `b.svg` (+ `.png`) | data | The boundary. |",
            ),
        )
        broken(
            "declared file missing",
            lambda root: os.remove(
                os.path.join(root, FIGURES_REL, "fx", "b.png")
            ),
        )
        broken(
            "empty figure",
            lambda root: write_file(
                os.path.join(root, FIGURES_REL, "fx", "b.png"), ""
            ),
        )
        broken(
            "over budget",
            lambda root: write_file(
                os.path.join(root, FIGURES_REL, "fx", "b.png"),
                "x" * (BUDGET_BYTES + 1),
            ),
        )
        broken(
            "wrong background",
            lambda root: write_file(
                os.path.join(root, FIGURES_REL, "fx", "b.svg"),
                '<svg fill="#ffffff"></svg>\n',
            ),
        )
        broken(
            "generator does not import the house style",
            lambda root: write_file(
                os.path.join(root, FIGURES_REL, "fx", "make_figures.py"),
                "import matplotlib\n",
            ),
        )
        broken(
            "unknown kind",
            lambda root: mutate(
                os.path.join(root, FIGURES_REL, "fx", "README.md"),
                "| `a.svg` (+ `.png`) | chart | One bar per run. |",
                "| `a.svg` (+ `.png`) | pie | One bar per run. |",
            ),
        )
        broken(
            "journal URL for another slug",
            lambda root: mutate(
                os.path.join(root, FIGURES_REL, "fx", "README.md"),
                "https://superposition.github.io/journal/fx/",
                "https://superposition.github.io/journal/other/",
            ),
        )
        broken(
            "missing palette colour",
            lambda root: mutate(
                os.path.join(root, HOUSE_REL), 'SAND = "#e0a08a"', 'SAND = "#ffffff"'
            ),
        )
        broken(
            "template heading added",
            lambda root: mutate(
                os.path.join(root, TEMPLATE_REL),
                "## Evidence",
                "## Numbers\n\n## Evidence",
            ),
        )
        broken(
            "template claim after the sections",
            lambda root: write_file(
                os.path.join(root, TEMPLATE_REL),
                GOOD_TEMPLATE.replace("**The claim.** One paragraph with the numbers inline.\n\n", "")
                + "\n**The claim.** Late.\n",
            ),
        )
        broken(
            "review section missing",
            lambda root: mutate(
                os.path.join(root, REVIEW_REL), "## teaching", "## Teaching notes"
            ),
        )
        broken(
            "journal out of epic order",
            lambda root: write_file(
                os.path.join(root, README_REL),
                GOOD_JOURNAL.replace(
                    "| the connectome as a prior | [EPIC-02](https://github.com/superposition/qualia/issues/2) |",
                    "| the connectome as a prior | [EPIC-04](https://github.com/superposition/qualia/issues/4) |",
                ),
            ),
        )
        broken(
            "required entry missing",
            lambda root: write_file(
                os.path.join(root, README_REL),
                GOOD_JOURNAL.replace(
                    "| the ladder | [EPIC-08C](https://github.com/superposition/qualia/issues/11) |\n",
                    "",
                ),
            ),
        )
        broken(
            "epic link is relative",
            lambda root: mutate(
                os.path.join(root, README_REL),
                "[EPIC-01](https://github.com/superposition/qualia/issues/1)",
                "[EPIC-01](issues/1)",
            ),
        )
        broken(
            "no entry directories",
            lambda root: shutil.rmtree(os.path.join(root, FIGURES_REL, "fx")),
        )

        for index, (label, apply) in enumerate(cases):
            root = os.path.join(tmp, "case%02d" % index)
            shutil.copytree(os.path.join(tmp, "good"), root)
            apply(root)
            if not check(root):
                sys.stderr.write("figures: self-test FAIL - %s passed\n" % label)
                return 1
        print("figures: self-test OK (%d mutations)" % len(cases))
    return 0


def main(argv) -> int:
    parser = argparse.ArgumentParser(
        description="The figure convention check (step 38)."
    )
    parser.add_argument(
        "--root",
        default=None,
        help="the tree to check (default: the worktree you run it in)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the checker's own fixtures; no repository needed",
    )
    args = parser.parse_args(argv[1:])
    if args.self_test:
        return self_test()
    root = os.path.abspath(args.root) if args.root else repo_root()
    if not os.path.isdir(root):
        parser.print_usage(sys.stderr)
        sys.stderr.write("figures: usage error - no such directory: %s\n" % root)
        return 2
    return report(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
