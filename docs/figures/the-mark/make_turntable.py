#!/usr/bin/env python3
"""Build the `the-mark` turntable GLB with Blender, headless.

Step 43's GLB for the journal entry `the-mark`: the entry's extruded mark plus
the entry's element, with a 60-frame rotation baked as an animation.

    blender --background --python docs/figures/the-mark/make_turntable.py

The element is one pillar per part of `assets/mark/psi.json`, the geometry
source the entry is about, its height the part's centre-line length in viewBox
units — the same number `make_figures.py` charts. The mark itself is the
committed `assets/mark/psi.glb`. Writes `turntable.glb` beside this script.

Blender is invoked by path because it is not on `PATH`.
"""

from __future__ import annotations

import json
import math
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _turntable import MARK_GLB, build_element  # noqa: E402

SPEC_PATH = REPO / "assets" / "mark" / "psi.json"
GLB_PATH = HERE / "turntable.glb"

PART_COLOURS = {"ink": "#edf0f5", "accent": "#91dbba"}


def centre_line_length(points: list[list[float]], viewbox: float, inset: float) -> float:
    """The part's centre-line length in viewBox units, as `make_figures.py` reports."""
    span = viewbox - 2.0 * inset
    mapped = [(inset + span * float(x), inset + span * float(y)) for x, y in points]
    total = 0.0
    for (x0, y0), (x1, y1) in zip(mapped, mapped[1:]):
        total += math.hypot(x1 - x0, y1 - y0)
    if len(mapped) > 2:
        (x0, y0), (x1, y1) = mapped[-1], mapped[0]
        total += math.hypot(x1 - x0, y1 - y0)
    return total


def main() -> int:
    spec = json.loads(SPEC_PATH.read_text(encoding="utf-8"))
    viewbox = float(spec["viewbox"])
    inset = float(spec["inset"])
    items = [
        (
            f"part-{part['name']}",
            PART_COLOURS[part["colour"]],
            centre_line_length(part["points"], viewbox, inset),
            part["name"],
        )
        for part in spec["parts"]
    ]
    result = build_element(MARK_GLB, items, GLB_PATH)
    print(
        f"turntable: mark mesh + {result['pillars']} pillars, {result['frames']} frames -> "
        f"{GLB_PATH} ({GLB_PATH.stat().st_size} bytes); "
        + ", ".join(f"{label} {value:.1f}" for _n, _c, value, label in items)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
