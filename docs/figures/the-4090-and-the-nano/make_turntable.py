#!/usr/bin/env python3
"""Build the `the-4090-and-the-nano` turntable GLB with Blender, headless.

Step 43's GLB for the journal entry `the-4090-and-the-nano`: the entry's
extruded mark plus the entry's element, with a 60-frame rotation baked as an
animation.

    "C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
        --python docs/figures/the-4090-and-the-nano/make_turntable.py

The element is one pillar per kernel in the board's committed capture — the
same five `kernels.csv` rows the entry's chart draws — its height proportional
to that kernel's mean time on the Orin NX. Two pillars carry the shape the entry
states: `cognition_update` and `belief_update` are 99.6 % of the board's kernel
time, and the three kernels beside them are stubs. Writes `turntable.glb` beside
this script.

Blender is invoked by path because it is not on `PATH`.
"""

from __future__ import annotations

import csv
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _turntable import MARK_GLB, build_element  # noqa: E402

BOARD_CSV = REPO / "docs" / "evidence" / "T50" / "pinkie-kernels" / "kernels.csv"
GLB_PATH = HERE / "turntable.glb"

# One colour a kernel, the house palette; the two tall pillars take the warm and
# cool accents so the pair that is 99.6 % of the time is the pair you see.
KERNELS = (
    ("cognition_update", "#e0a08a"),
    ("belief_update", "#93caff"),
    ("costmap_stats", "#91dbba"),
    ("cognition_patch", "#c9b2ff"),
    ("add_one", "#8b93a7"),
)


def board_means(run: str = "base") -> dict:
    """Mean time per launch, in µs, for each kernel of the board's own capture."""
    totals: dict = {}
    for row in csv.DictReader(BOARD_CSV.read_text(encoding="utf-8").splitlines()):
        if row["run"] != run:
            continue
        entry = totals.setdefault(row["kernel"], {"launches": 0, "total_ns": 0})
        entry["launches"] += 1
        entry["total_ns"] += int(row["duration_ns"])
    return {name: entry["total_ns"] / entry["launches"] / 1000.0 for name, entry in totals.items()}


def main() -> int:
    means = board_means()
    if set(means) != {name for name, _ in KERNELS}:
        raise SystemExit(f"turntable: the capture holds {sorted(means)}, not the five kernels")
    items = [
        (name, colour, means[name], f"{name} {means[name]:,.0f} µs")
        for name, colour in KERNELS
    ]
    result = build_element(MARK_GLB, items, GLB_PATH)
    print(
        f"turntable: mark mesh + {result['pillars']} pillars, {result['frames']} frames -> "
        f"{GLB_PATH} ({GLB_PATH.stat().st_size} bytes); "
        + ", ".join(f"{label}" for _n, _c, _v, label in items)
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
