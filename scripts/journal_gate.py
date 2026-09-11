#!/usr/bin/env python3
"""The journal publish gate (step 37b; see docs/journal-pipeline.md).

An entry is produced by agents and reviewed before it goes live, and every role's
output is a comment on the entry's PR, so this gate reads the PR: the comments are
the process, not an agent's memory. It checks three legs:

  checklists  `docs/journal-review.md` holds three distinct checklists, one
              `## accuracy`, `## teaching`, `## style` section each. This is the
              ticket's test written first: "docs/journal-review.md holds three
              distinct checklists".
  roles       the entry PR carries exactly one editorial comment per role, each
              beginning with the `braid-review` fenced block that
              `docs/journal-review.md` fixes (`role:`, `verdict:`, `notes:`), and
              no `<!-- ASK: -->` question survives in the entry. The ticket's
              Command counts the `role:` blocks with `gh pr view <n> --comments`;
              that display form aborts in this repository on the deprecated
              `projectCards` GraphQL field, so the same comment stream is read as
              JSON here.
  gate        publish happens only when all three verdicts are `approve`: a
              `request-changes` closes the gate and names the role. With `--url`,
              the live entry and every absolute figure URL in the entry must
              return 200.

Usage:

    python scripts/journal_gate.py                          # the checklists alone
    python scripts/journal_gate.py --pr <n> --repo superposition/superposition.github.io
    python scripts/journal_gate.py --pr <n> --repo OWNER/NAME --url <live-url>
    python scripts/journal_gate.py --pr <n> --repo OWNER/NAME --comments comments.json --diff entry.diff
    python scripts/journal_gate.py --entry _posts/2026-09-11-the-public-record.md

`--comments` is the JSON array `gh pr view <n> --repo R --json comments --jq
'[.comments[].body]'` prints, and `--diff` is `gh pr diff <n> --repo R`; both
replace the network calls for a dry run or a test. `--entry` checks one local
markdown file instead of a PR. `--self-test` runs the built-in fixtures and needs
no network.

Exit code 0 and a final `journal-gate: OK` when every leg holds and the gate is
open; 1 with the failing leg named otherwise.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

ROLES = ("accuracy", "teaching", "style")
EDITORIAL_FENCE = "braid-review"
VERDICTS = ("approve", "request-changes")
REQUIRED_HEADINGS = (
    "**The claim.**",
    "## What we tried",
    "## Evidence",
    "## What this does not establish",
)
ASK_RE = re.compile(r"<!--\s*ASK:")
NUMBERED_RE = re.compile(r"^\s*\d+\.\s+(.*\S)\s*$")
HEADING_RE = re.compile(r"^##\s+(.*\S)\s*$")
FIELD_RE = re.compile(r"^(role|verdict|notes)\s*:\s*(.*)$", re.IGNORECASE)
SRC_RE = re.compile(r'src\s*=\s*"([^"]+)"')
MARKDOWN_IMAGE_RE = re.compile(r"!\[[^\]]*\]\((\S+?)\)")


def repo_root():
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


def parse_checklists(text):
    """Map each `## <role>` section to its numbered checklist items."""
    sections = {}
    current = None
    for line in text.replace("\r\n", "\n").split("\n"):
        heading = HEADING_RE.match(line)
        if heading:
            current = heading.group(1).strip().lower()
            sections.setdefault(current, [])
            continue
        if current is not None:
            item = NUMBERED_RE.match(line)
            if item:
                sections[current].append(item.group(1))
    return sections


def check_checklists(path):
    """Return (problems, summary) for `docs/journal-review.md`, read from path."""
    if not os.path.isfile(path):
        return ["checklists: %s is missing" % path], ""
    with open(path, encoding="utf-8") as handle:
        return _checklists_of(handle.read())


def _checklists_of(text):
    """Return (problems, summary) for the checklists in `text`."""
    sections = parse_checklists(text)

    problems = []
    for role in ROLES:
        if role not in sections:
            problems.append("checklists: no `## %s` section" % role)
        elif not sections[role]:
            problems.append("checklists: `## %s` lists no items" % role)
    extra = sorted(set(sections) - set(ROLES))
    if extra:
        problems.append(
            "checklists: unexpected section(s) %s; there are exactly three jobs"
            % ", ".join("`## %s`" % name for name in extra)
        )
    items = [tuple(sections.get(role, [])) for role in ROLES]
    if all(items) and len(set(items)) != len(items):
        problems.append("checklists: two of the three roles share a checklist")

    summary = ", ".join("%s(%d)" % (role, len(sections.get(role, []))) for role in ROLES)
    if not problems:
        summary += "; three distinct"
    return problems, summary


def first_fenced_block(text):
    """The (tag, lines) of the comment's opening fenced block, or None."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    start = None
    for index, line in enumerate(lines):
        if line.strip():
            start = index
            break
    if start is None or not lines[start].lstrip().startswith("```"):
        return None
    tag = lines[start].strip()[3:].strip()
    body = []
    for line in lines[start + 1 :]:
        if line.strip().startswith("```"):
            return tag, body
        body.append(line)
    return None


def parse_editorial(bodies):
    """Return (roles, problems): the editorial verdicts among the PR comments.

    A comment whose opening fenced block is a `braid` breadcrumb or a code-review
    block carries no editorial role and is ignored. A comment whose block names
    an editorial role must be the `braid-review` form, well formed, and the only
    comment for that role.
    """
    roles = {}
    problems = []
    for index, body in enumerate(bodies):
        block = first_fenced_block(body)
        if block is None:
            continue
        tag, lines = block
        fields = {}
        for line in lines:
            match = FIELD_RE.match(line.strip())
            if match:
                fields[match.group(1).lower()] = match.group(2).strip()
        role = fields.get("role", "").split("|")[0].strip().lower()
        if role not in ROLES:
            continue
        label = "comment %d" % (index + 1)
        if tag != EDITORIAL_FENCE:
            problems.append(
                "%s: role %s is fenced `%s`, not `%s`"
                % (label, role, tag or "(bare)", EDITORIAL_FENCE)
            )
            continue
        if role in roles:
            problems.append("%s: a second %s comment; exactly one per role" % (label, role))
            continue
        verdict = fields.get("verdict", "").strip().lower()
        notes = fields.get("notes", "").strip()
        if verdict not in VERDICTS:
            problems.append(
                "%s: %s has no valid `verdict:` (got %r)"
                % (label, role, fields.get("verdict", ""))
            )
            continue
        if not re.match(r"^\d+$", notes):
            problems.append(
                "%s: %s has no integer `notes:` (got %r)"
                % (label, role, fields.get("notes", ""))
            )
            continue
        roles[role] = {"verdict": verdict, "notes": int(notes)}
    return roles, problems


def gate_state(roles):
    """(missing roles, roles whose verdict closes the gate)."""
    missing = [role for role in ROLES if role not in roles]
    closed = [
        role
        for role in ROLES
        if role in roles and roles[role]["verdict"] == "request-changes"
    ]
    return missing, closed


def check_entry(text, label):
    """The writer's ASK questions and the style form, checked on the entry text."""
    problems = []
    for lineno, line in enumerate(text.splitlines(), 1):
        if ASK_RE.search(line):
            problems.append("%s:%d: unresolved `<!-- ASK: -->`" % (label, lineno))
    lowered = text.lower()
    for heading in REQUIRED_HEADINGS:
        if heading.lower() not in lowered:
            problems.append("%s: missing %s" % (label, heading))
    return problems


def added_lines(diff_text):
    """The `+` side of a unified diff, without the file headers."""
    lines = []
    for line in diff_text.replace("\r\n", "\n").split("\n"):
        if line.startswith("+++") or line.startswith("---"):
            continue
        if line.startswith("+"):
            lines.append(line[1:])
    return "\n".join(lines)


def figure_urls(text):
    """Absolute figure URLs: `src="…"` attributes and markdown images."""
    seen = []
    for match in SRC_RE.finditer(text):
        seen.append(match.group(1))
    for match in MARKDOWN_IMAGE_RE.finditer(text):
        seen.append(match.group(1))
    urls = []
    for url in seen:
        if url.startswith(("http://", "https://")) and url not in urls:
            urls.append(url)
    return urls


def run_gh(args):
    """(stdout, None) or (None, error)."""
    try:
        proc = subprocess.run(["gh"] + args, capture_output=True, text=True, timeout=180)
    except (OSError, subprocess.SubprocessError) as error:
        return None, "gh %s could not run: %s" % (" ".join(args), error)
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "").strip()
        return None, "gh %s exited %d: %s" % (" ".join(args), proc.returncode, detail)
    return proc.stdout, None


def default_repo():
    stdout, error = run_gh(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
    return None if error else stdout.strip()


def fetch_comments(repo, pr):
    stdout, error = run_gh(
        ["pr", "view", str(pr), "--repo", repo, "--json", "comments"]
    )
    if error:
        return None, error
    try:
        payload = json.loads(stdout)
    except ValueError as error:
        return None, "gh pr view --json comments did not return JSON: %s" % error
    return [comment.get("body", "") for comment in payload.get("comments", [])], None


def fetch_status(url, timeout=30):
    request = urllib.request.Request(url, headers={"User-Agent": "journal-gate"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.getcode()
    except urllib.error.HTTPError as error:
        return error.code
    except Exception:  # URLError, timeout, TLS, DNS
        return None


def editorial_comment(role, verdict, notes):
    return (
        "```braid-review\nrole: %s\nverdict: %s\nnotes: %d\n```\n\n"
        "- [x] the checklist, completed\n" % (role, verdict, notes)
    )


GOOD_ENTRY = (
    "---\ntitle: \"The public record\"\ndate: 2026-09-11\n---\n\n"
    "**The claim.** One paragraph with the numbers inline.\n\n"
    "## What we tried\n\nA paragraph.\n\n"
    "## Evidence\n\n| Quantity | Value |\n| --- | ---: |\n| Edges | 929,735 |\n\n"
    "## What this does not establish\n\nA paragraph.\n"
)

GOOD_CHECKLISTS = (
    "# Journal review checklists\n\n"
    "## accuracy\n\n1. one\n2. two\n3. three\n\n"
    "## teaching\n\n1. alpha\n2. beta\n\n"
    "## style\n\n1. short paragraphs\n2. valid front matter\n"
)


def self_test():
    """Fixtures for the parsers and the gate; no network, no `gh`."""
    failures = []
    ran = []

    def check(name, condition):
        ran.append(name)
        if not condition:
            failures.append(name)

    roles, problems = parse_editorial([editorial_comment(role, "approve", 0) for role in ROLES])
    check("three approvals parse", not problems and set(roles) == set(ROLES))
    check("three approvals open the gate", gate_state(roles) == ([], []))

    roles, problems = parse_editorial([editorial_comment(role, "approve", 0) for role in ROLES])
    roles["teaching"]["verdict"] = "request-changes"
    check("a request-changes closes the gate", gate_state(roles) == ([], ["teaching"]))

    roles, problems = parse_editorial(
        [editorial_comment("accuracy", "approve", 0), editorial_comment("style", "approve", 0)]
    )
    check("a missing role is missing", gate_state(roles)[0] == ["teaching"])

    _, problems = parse_editorial(
        [editorial_comment(role, "approve", 0) for role in ROLES]
        + [editorial_comment("accuracy", "approve", 0)]
    )
    check("a duplicate role is a problem", any("second accuracy" in p for p in problems))

    _, problems = parse_editorial(["```braid-review\nrole: accuracy\nnotes: 0\n```\n"])
    check("a missing verdict is a problem", any("verdict" in p for p in problems))

    _, problems = parse_editorial(["```braid-review\nrole: style\nverdict: approve\nnotes: many\n```\n"])
    check("a non-integer notes is a problem", any("notes" in p for p in problems))

    braid = "```braid\nagent: X\nbranch: b\nstate: review\nnext: n\nblocked_on: none\nevidence: none\n```\n"
    roles, problems = parse_editorial([braid])
    check("a breadcrumb is ignored, not an error", not problems and not roles)

    _, problems = parse_editorial(["```text\nrole: accuracy\nverdict: approve\nnotes: 0\n```\n"])
    check("the wrong fence tag is a problem", any("braid-review" in p for p in problems))

    check("a good entry passes", check_entry(GOOD_ENTRY, "entry") == [])
    check(
        "an unresolved ASK is a problem",
        any("ASK" in p for p in check_entry(GOOD_ENTRY + "<!-- ASK: is this right? -->\n", "entry")),
    )
    check(
        "a missing heading is a problem",
        any(
            "## Evidence" in p
            for p in check_entry(GOOD_ENTRY.replace("## Evidence", "## Notes"), "entry")
        ),
    )

    diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@\n-old\n+new\n"
    check("added_lines keeps only additions", added_lines(diff) == "new")
    check(
        "figure_urls finds absolute figures",
        figure_urls('<img src="/rel.png">\n![a](https://e/f.svg)\n') == ["https://e/f.svg"],
    )

    problems, summary = _checklists_of(GOOD_CHECKLISTS)
    check("three distinct checklists pass", not problems)
    check("the summary counts the three jobs", summary.count("(") == 3)

    shared = GOOD_CHECKLISTS.replace("1. alpha\n2. beta", "1. one\n2. two\n3. three")
    problems, _ = _checklists_of(shared)
    check("a shared checklist is a problem", any("share" in p for p in problems))

    problems, _ = _checklists_of("# x\n\n## accuracy\n\n1. one\n")
    check("a missing section is a problem", any("teaching" in p for p in problems))

    if failures:
        for name in failures:
            print("journal-gate: self-test FAIL - %s" % name, file=sys.stderr)
        print("journal-gate: self-test FAIL - %d check(s)" % len(failures))
        return 1
    print("journal-gate: self-test OK - %d checks" % len(ran))
    return 0


def main(argv):
    parser = argparse.ArgumentParser(
        prog="journal_gate.py", description="The journal publish gate (docs/journal-pipeline.md)."
    )
    parser.add_argument("--repo", default=None, help="OWNER/NAME of the repository that holds the entry PR")
    parser.add_argument("--pr", type=int, default=None, help="entry PR number")
    parser.add_argument("--comments", default=None, help="JSON array of comment bodies, instead of the PR")
    parser.add_argument("--diff", default=None, help="unified diff of the entry PR, instead of the PR")
    parser.add_argument("--entry", default=None, help="a local entry markdown file, instead of a PR")
    parser.add_argument("--url", default=None, help="live entry URL; its figures must return 200 too")
    parser.add_argument("--self-test", action="store_true", dest="self_test", help="run the built-in fixtures")
    args = parser.parse_args(argv[1:])

    if args.self_test:
        return self_test()

    problems = []
    entry_text = None
    entry_label = "entry"
    repo = args.repo

    # The checklists are static and cheap: every run checks them.
    check_problems, summary = check_checklists(
        os.path.join(repo_root(), "docs", "journal-review.md")
    )
    problems += check_problems
    if not check_problems:
        print("journal-gate: checklists OK - %s" % summary)

    if args.pr is not None or args.entry is not None:
        bodies = None
        if args.comments is not None:
            with open(args.comments, encoding="utf-8") as handle:
                bodies = json.load(handle)
            if not isinstance(bodies, list) or not all(isinstance(b, str) for b in bodies):
                problems.append("comments: %s is not a JSON array of comment bodies" % args.comments)
                bodies = None
        elif args.pr is not None:
            if not repo:
                repo = default_repo()
            if not repo:
                problems.append("roles: no --repo given and no default repository resolved")
            else:
                bodies, error = fetch_comments(repo, args.pr)
                if error:
                    problems.append("roles: %s" % error)
                    bodies = None

        roles, role_problems = parse_editorial(bodies or [])
        problems += role_problems
        missing, closed = gate_state(roles)
        if bodies is not None and not role_problems:
            if missing:
                problems.append("roles: no %s comment on the entry PR" % "/".join(missing))
            elif closed:
                problems.append(
                    "gate closed by %s; the author merges only when all three approve"
                    % ", ".join(closed)
                )
            else:
                print(
                    "journal-gate: roles OK - %s"
                    % ", ".join(
                        "%s=%s(%d)" % (role, roles[role]["verdict"], roles[role]["notes"])
                        for role in ROLES
                    )
                )

        if args.entry is not None:
            with open(args.entry, encoding="utf-8") as handle:
                entry_text = handle.read()
            entry_label = args.entry
        else:
            diff_text = None
            if args.diff is not None:
                with open(args.diff, encoding="utf-8") as handle:
                    diff_text = handle.read()
            elif args.pr is not None and repo:
                diff_text, error = run_gh(["pr", "diff", str(args.pr), "--repo", repo])
                if error:
                    problems.append("entry: %s" % error)
                    diff_text = None
            if diff_text is not None:
                entry_text = added_lines(diff_text)
        if entry_text is not None:
            problems += check_entry(entry_text, entry_label)

    if args.url:
        status = fetch_status(args.url)
        if status != 200:
            problems.append("live URL returned %s: %s" % (status, args.url))
        else:
            print("journal-gate: live URL 200 - %s" % args.url)
        if entry_text is not None:
            for url in figure_urls(entry_text):
                status = fetch_status(url)
                if status != 200:
                    problems.append("figure returned %s: %s" % (status, url))
                else:
                    print("journal-gate: figure 200 - %s" % url)

    if problems:
        for problem in problems:
            print("journal-gate: %s" % problem, file=sys.stderr)
        print("journal-gate: FAIL - %d problem(s); the gate is not open" % len(problems))
        return 1
    print("journal-gate: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
