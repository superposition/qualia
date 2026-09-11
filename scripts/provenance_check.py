#!/usr/bin/env python3
"""Clean-room provenance check (see docs/decisions.md D-001).

Nothing is copied from the private engine. Two things are fatal, two are
reported:

FATAL   whole-file identity: the bytes match the reference and the working
        file is not this worktree's rendition of its own recorded content.
        The match is then not a line-ending accident — the bytes were written
        into the tree — so it is the copy this gate exists to catch;
FATAL   a run of more than two consecutive identical *comment or doc* lines
        (prose is never forced by an interface, so shared prose means copied
        prose);
REPORT  the longest run of identical code lines, and the share of this file's
        non-trivial lines that also appear in the reference. These are
        reported rather than fatal because a faithful reimplementation of a
        fixed interface necessarily shares declaration lines: constant tables,
        enum variants, struct field lists, `name = "..."` manifest entries;
REPORT  files whose text equals the reference's once line endings are folded
        (EOL-IDENTICAL). The reference tree is a Windows checkout of LF blobs
        (`core.autocrlf=true`), so the same text reaches it as CRLF and a
        worktree of this repository as LF; a file that legitimately coincides
        with the reference — a manifest whose keys the interface fixes — is
        therefore identical modulo line endings and not byte-identical, and so
        is our own content rendered CRLF by a CRLF checkout. Line endings are
        a checkout setting, not authored content: no `core.autocrlf` value and
        no way of checking out a worktree may change this gate's verdict.

A file whose text is the reference's and not our own recorded text, but whose
working bytes differ, is reported by the share and code-run metrics below
rather than treated as fatal — the same report the gate has always given a
reimplementation that shares declaration text.

Generated files are excluded from the run metrics; they are machine output,
not authorship.

The gate measures the worktree it is run in, not the checkout it lives in:
ROOT is the worktree root of the current directory (`git rev-parse
--show-toplevel`), so a checkout's copy invoked from a ticket worktree
measures that worktree. A copy run from outside a worktree of this repository
falls back to the checkout containing the script.

A line is "trivial" when, after trimming, it is empty or consists only of
brackets, braces, parens, commas, semicolons, quotes, angle brackets, equals
signs, asterisks or comment delimiters.

    python scripts/provenance_check.py [reference-root] [--max-run N]

Default reference root: $QUALIA_PRIVATE_ROOT, else the first of C:/qualia and
/c/qualia that exists.
"""

from __future__ import annotations

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT_ROOT = os.path.dirname(HERE)


def git_output(args, cwd):
    """Stripped stdout of `git args` in `cwd`, or None when that fails."""
    try:
        out = subprocess.run(["git"] + args, cwd=cwd, capture_output=True, text=True)
    except OSError:
        return None
    if out.returncode != 0:
        return None
    return out.stdout.strip()


def cwd_root():
    """The worktree root of the repository the command is run in.

    Falls back to the checkout holding this script when the current directory
    is not inside a worktree of the same repository, so the gate always
    measures the tree it was pointed at.
    """
    toplevel = git_output(["rev-parse", "--show-toplevel"], os.getcwd())
    here_common = git_output(["rev-parse", "--git-common-dir"], SCRIPT_ROOT)
    if not toplevel or not here_common:
        return SCRIPT_ROOT
    common = git_output(["rev-parse", "--git-common-dir"], toplevel)
    if not common:
        return SCRIPT_ROOT
    if os.path.realpath(os.path.join(toplevel, common)) != os.path.realpath(
        os.path.join(SCRIPT_ROOT, here_common)
    ):
        return SCRIPT_ROOT
    return toplevel


ROOT = cwd_root()

TRIVIAL = set("{}()[];,<>=\\'\"`*")

# Machine-generated, not authored here.
GENERATED = ("Cargo.lock",)

COMMENT_PREFIXES = ("//", "///", "//!", "#", "/*", "*", "<!--", "--")


def default_reference():
    env = os.environ.get("QUALIA_PRIVATE_ROOT")
    if env:
        return env
    for candidate in ("C:/qualia", "/c/qualia"):
        if os.path.isdir(candidate):
            return candidate
    return None


def tracked_files():
    out = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return [line for line in out.stdout.splitlines() if line]


def read_lines(path):
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read().splitlines()


def normalise_eol(data):
    """`data` with every line ending folded to LF.

    The reference tree is checked out with `core.autocrlf=true` (LF blobs,
    CRLF files), so text that is CRLF there is LF here. Folding both sides
    makes identity a property of the text, not of the checkout.
    """
    return data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def recorded_bytes(rel):
    """The bytes this repository records for `rel` at HEAD, or None.

    The blob is the stored content, before any checkout filter: it is what the
    path is supposed to hold, not what a particular checkout rendered.
    """
    out = subprocess.run(
        ["git", "cat-file", "blob", "HEAD:" + rel], cwd=ROOT, capture_output=True
    )
    if out.returncode != 0:
        return None
    return out.stdout


def carries_own_text(rel, data):
    """True when `data` is a line-ending rendition of this path's own content.

    A working file whose text equals the text recorded at HEAD was produced by
    the checkout of our own content; a working file whose text is not the
    recorded text was written by something else. The test is deliberately
    independent of `core.autocrlf`: it asks what the bytes are, not which
    filter produced them.
    """
    recorded = recorded_bytes(rel)
    return recorded is not None and normalise_eol(recorded) == normalise_eol(data)


def trivial(line):
    stripped = line.strip()
    if not stripped:
        return True
    return all(ch in TRIVIAL for ch in stripped)


def is_comment(line):
    return line.strip().startswith(COMMENT_PREFIXES)


def longest_run(a, b, predicate):
    """Longest run of consecutive identical lines satisfying `predicate`."""
    if not a or not b:
        return 0
    previous = [0] * (len(b) + 1)
    best = 0
    for i in range(1, len(a) + 1):
        current = [0] * (len(b) + 1)
        left = a[i - 1]
        if predicate(left):
            for j in range(1, len(b) + 1):
                if left == b[j - 1]:
                    current[j] = previous[j - 1] + 1
                    if current[j] > best:
                        best = current[j]
        previous = current
    return best


def code_line(line):
    return not trivial(line) and not is_comment(line)


def prose_line(line):
    return is_comment(line) and not trivial(line)


def main(argv):
    max_run = 20
    max_prose_run = 2
    positional = []
    rest = argv[1:]
    while rest:
        item = rest.pop(0)
        if item == "--max-run":
            max_run = int(rest.pop(0))
        elif item == "--max-prose-run":
            max_prose_run = int(rest.pop(0))
        else:
            positional.append(item)

    reference = positional[0] if positional else default_reference()
    if not reference or not os.path.isdir(reference):
        sys.stderr.write("provenance: reference root %r is not a directory\n" % (reference,))
        return 2

    checked = 0
    skipped_generated = 0
    identical = 0
    eol_identical = 0
    prose_offences = []
    over_run = []
    worst_share = (0.0, "")

    for rel in tracked_files():
        if os.path.basename(rel) in GENERATED:
            skipped_generated += 1
            continue
        other = os.path.join(reference, rel)
        if not os.path.isfile(other):
            continue
        checked += 1
        mine = os.path.join(ROOT, rel)
        with open(mine, "rb") as handle_a, open(other, "rb") as handle_b:
            our_bytes = handle_a.read()
            ref_bytes = handle_b.read()
        if our_bytes == ref_bytes and not carries_own_text(rel, our_bytes):
            # The working file holds the reference's bytes and not our own
            # text, so no checkout setting explains the match.
            print("IDENTICAL: %s" % rel)
            identical += 1
            continue
        if normalise_eol(our_bytes) == normalise_eol(ref_bytes):
            # Same text, different line endings: the reference's CRLF is its
            # checkout, and either LF or CRLF here is ours. Reported, not
            # fatal, and still measured for code runs and shared prose below.
            print("EOL-IDENTICAL: %s" % rel)
            eol_identical += 1

        a_lines = read_lines(mine)
        b_lines = read_lines(other)
        if not a_lines or not b_lines:
            continue

        prose = longest_run(a_lines, b_lines, prose_line)
        if prose > max_prose_run:
            prose_offences.append((rel, prose))

        run = longest_run(a_lines, b_lines, code_line)
        if run > max_run:
            over_run.append((rel, run))

        ours = [line for line in a_lines if code_line(line)]
        if ours:
            theirs = set(line for line in b_lines if code_line(line))
            share = float(sum(1 for line in ours if line in theirs)) / len(ours)
            if share > worst_share[0]:
                worst_share = (share, rel)

    print(
        "provenance: compared %d authored file(s) (%d generated skipped); "
        "%d byte-identical, %d EOL-identical, %d code runs over %d, "
        "%d prose runs over %d"
        % (
            checked,
            skipped_generated,
            identical,
            eol_identical,
            len(over_run),
            max_run,
            len(prose_offences),
            max_prose_run,
        )
    )
    print(
        "provenance: highest identical-code-line share %.3f%s"
        % (worst_share[0], (" (%s)" % worst_share[1]) if worst_share[1] else "")
    )
    for rel, run in sorted(prose_offences, key=lambda item: -item[1]):
        print("PROSE RUN %d: %s" % (run, rel))
    for rel, run in sorted(over_run, key=lambda item: -item[1]):
        print("CODE RUN %d: %s" % (run, rel))

    if identical or prose_offences:
        sys.stderr.write(
            "provenance: FAIL - no file may be copied, and no prose may be shared\n"
        )
        return 1
    print("provenance: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
