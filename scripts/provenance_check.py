#!/usr/bin/env python3
"""Clean-room provenance check (see docs/decisions.md D-001).

No file in this repository may be byte-identical to a file in the private
engine. This walks every tracked file, finds the same relative path under the
reference root, and fails if the bytes match.

    python scripts/provenance_check.py [reference-root]

Default reference root: $QUALIA_PRIVATE_ROOT, else the first of C:/qualia and
/c/qualia that exists. Files with no counterpart in the reference (this
repository's own work) are skipped.
"""

from __future__ import annotations

import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def default_reference() -> str | None:
    env = os.environ.get("QUALIA_PRIVATE_ROOT")
    if env:
        return env
    for candidate in ("C:/qualia", "/c/qualia"):
        if os.path.isdir(candidate):
            return candidate
    return None


def tracked_files() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return [line for line in out.stdout.splitlines() if line]


def main(argv: list[str]) -> int:
    reference = argv[1] if len(argv) > 1 else default_reference()
    if not reference or not os.path.isdir(reference):
        print(f"provenance: reference root {reference!r} is not a directory", file=sys.stderr)
        return 2

    checked = 0
    identical = 0
    for rel in tracked_files():
        other = os.path.join(reference, rel)
        if not os.path.isfile(other):
            continue
        checked += 1
        with open(os.path.join(ROOT, rel), "rb") as a, open(other, "rb") as b:
            if a.read() == b.read():
                print(f"IDENTICAL: {rel}")
                identical += 1

    print(
        f"provenance: compared {checked} file(s) that also exist in the reference; "
        f"{identical} identical"
    )
    if identical:
        print(
            "provenance: FAIL - every line in this repository must be authored here",
            file=sys.stderr,
        )
        return 1
    print("provenance: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
