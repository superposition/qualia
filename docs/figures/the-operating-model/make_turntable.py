#!/usr/bin/env python3
"""Build the `the-operating-model` turntable GLB with Blender, headless.

Step 43's GLB for the journal entry `the-operating-model`: the entry's extruded
mark plus the entry's element, with a 60-frame rotation baked as an animation.

    blender --background --python docs/figures/the-operating-model/make_turntable.py

The element is one pillar per `status:*` label in `ledger-snapshot.json` — the
same five counts the entry's ledger chart draws — its height proportional to
the count. The mark itself is the committed `assets/mark/psi.glb`. Writes
`turntable.glb` beside this script.

Blender is invoked by path because it is not on `PATH`.
"""

from __future__ import annotations

import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from _turntable import MARK_GLB, build_element  # noqa: E402

SNAPSHOT_PATH = HERE / "ledger-snapshot.json"
GLB_PATH = HERE / "turntable.glb"

# The ledger chart's order and colours (`make_figures.py`).
STATUSES = (
    ("status:ready", "#93caff"),
    ("status:claimed", "#c9b2ff"),
    ("status:done", "#91dbba"),
    ("status:review", "#e0a08a"),
    ("status:blocked", "#8b93a7"),
)


def main() -> int:
    status = json.loads(SNAPSHOT_PATH.read_text(encoding="utf-8"))["status"]
    items = [
        (label.split(":", 1)[1], colour, int(status[label]), label)
        for label, colour in STATUSES
    ]
    result = build_element(MARK_GLB, items, GLB_PATH)
    print(
        f"turntable: mark mesh + {result['pillars']} pillars, {result['frames']} frames -> "
        f"{GLB_PATH} ({GLB_PATH.stat().st_size} bytes); "
        + ", ".join(f"{label} {value}" for _n, _c, value, label in items)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
