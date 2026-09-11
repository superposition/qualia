#!/usr/bin/env python3
"""Build the `the-licence-and-the-snapshot` turntable GLB with Blender, headless.

Step 43's GLB for the journal entry `the-licence-and-the-snapshot`: the entry's
extruded mark plus the entry's element, with a 60-frame rotation baked as an
animation.

    blender --background --python docs/figures/the-licence-and-the-snapshot/make_turntable.py

The element is three pillars per run in `provenance.json` — tracked files, files
that also exist in the reference checkout, and files byte-identical to it — the
same six numbers the entry's provenance chart draws, each height proportional to
its count. The mark itself is the committed `assets/mark/psi.glb`. Writes
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

PROVENANCE_PATH = HERE / "provenance.json"
GLB_PATH = HERE / "turntable.glb"

# The provenance chart's series and colours (`make_figures.py`).
SERIES = (
    ("tracked", "tracked_files", "#4a5468"),
    ("compared", "compared", "#93caff"),
    ("identical", "identical", "#e0a08a"),
)


def main() -> int:
    runs = json.loads(PROVENANCE_PATH.read_text(encoding="utf-8"))["runs"]
    items = [
        (
            f"{run['commit']}-{name}",
            colour,
            int(run[key]),
            f"{run['commit']} {name}",
        )
        for run in runs
        for name, key, colour in SERIES
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
