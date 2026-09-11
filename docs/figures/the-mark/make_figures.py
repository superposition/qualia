#!/usr/bin/env python3
"""Figures for the journal entry `the-mark`.

    mark-geometry.svg   a chart: each part of the psi monogram's geometry source
                        as length and extent in viewBox units
    mark-anatomy.svg    a diagram: the same four parts stroked at the geometry
                        source's proportions, with the inset frame and the labels

Data: `assets/mark/psi.json`, the committed geometry source `assets/mark/
render_mark.py` writes `psi.svg` from and `assets/mark/build_mark.py` extrudes.
Every number in the chart is computed from that file; nothing is typed in.

Run:  py -3.13 make_figures.py
"""

from __future__ import annotations

import json
import pathlib
import sys

import matplotlib.pyplot as plt
from matplotlib.patches import Polygon, Rectangle

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BG,
    GREEN,
    INK,
    MUTED,
    RULE,
    save,
    use_style,
)

use_style()

SPEC_PATH = REPO / "assets" / "mark" / "psi.json"

# The house palette name a part's `colour` key selects.
PART_COLOURS = {"ink": INK, "accent": GREEN}


def viewbox_points(spec: dict) -> list[tuple[str, str, list[tuple[float, float]]]]:
    """Each part's centre line as `psi.svg` draws it: `v' = inset + span v`."""
    viewbox = float(spec["viewbox"])
    inset = float(spec["inset"])
    span = viewbox - 2.0 * inset
    return [
        (
            part["name"],
            part["colour"],
            [(inset + span * float(x), inset + span * float(y)) for x, y in part["points"]],
        )
        for part in spec["parts"]
    ]


def centre_line_length(points: list[tuple[float, float]]) -> float:
    """Length of the polyline, in viewBox units. A closed part counts its close."""
    total = 0.0
    for (x0, y0), (x1, y1) in zip(points, points[1:]):
        total += ((x1 - x0) ** 2 + (y1 - y0) ** 2) ** 0.5
    if len(points) > 2:
        (x0, y0), (x1, y1) = points[-1], points[0]
        total += ((x1 - x0) ** 2 + (y1 - y0) ** 2) ** 0.5
    return total


def extents(points: list[tuple[float, float]]) -> tuple[float, float]:
    xs = [x for x, _ in points]
    ys = [y for _, y in points]
    return max(xs) - min(xs), max(ys) - min(ys)


def stroke_quads(points: list[tuple[float, float]], half: float):
    """The part's stroke as filled quads along its segments, square ends.

    `psi.svg` draws every part as a closed, mitre-joined stroke of width
    `stroke`; with a single colour per part the mitre joins are the overlap of
    these quads, so the drawn outline matches the committed SVG's proportions.
    """
    closed = len(points) > 2
    segments = list(zip(points, points[1:]))
    if closed:
        segments.append((points[-1], points[0]))
    for (x0, y0), (x1, y1) in segments:
        dx, dy = x1 - x0, y1 - y0
        length = (dx * dx + dy * dy) ** 0.5
        if length == 0.0:
            continue
        ux, uy = dx / length, dy / length
        nx, ny = -uy, ux
        a = (x0 - ux * half, y0 - uy * half)
        b = (x1 + ux * half, y1 + uy * half)
        yield [
            (a[0] + nx * half, a[1] + ny * half),
            (b[0] + nx * half, b[1] + ny * half),
            (b[0] - nx * half, b[1] - ny * half),
            (a[0] - nx * half, a[1] - ny * half),
        ]


def geometry_chart() -> None:
    spec = json.loads(SPEC_PATH.read_text(encoding="utf-8"))
    parts = viewbox_points(spec)
    names = [name for name, _, _ in parts]
    lengths = [centre_line_length(points) for _, _, points in parts]
    spans = [extents(points) for _, _, points in parts]
    colours = [PART_COLOURS[colour] for _, colour, _ in parts]

    fig, (left, right) = plt.subplots(1, 2, figsize=(9.6, 4.2))
    fig.suptitle(
        "psi.json — the geometry source, in viewBox units",
        color=INK,
        fontsize=12,
        y=0.98,
    )

    for ax, title in ((left, "centre-line length"), (right, "extent")):
        ax.set_title(title, color=INK, fontsize=10, pad=8)
        ax.set_axisbelow(True)
        ax.grid(axis="y", color=RULE, linewidth=0.7, alpha=0.7)
        for spine in ax.spines.values():
            spine.set_color(RULE)

    left.bar(names, lengths, color=colours, width=0.62)
    for index, value in enumerate(lengths):
        left.text(index, value + 1.2, f"{value:.1f}", ha="center", color=INK, fontsize=8.5)
    left.set_ylabel("units", color=MUTED, fontsize=9)

    width = 0.34
    positions = range(len(names))
    right.bar([p - width / 2 for p in positions], [s[0] for s in spans], width=width, color=colours, label="x-extent")
    right.bar([p + width / 2 for p in positions], [s[1] for s in spans], width=width, color=BG, edgecolor=colours, linewidth=1.4, label="y-extent")
    for index, (sx, sy) in enumerate(spans):
        right.text(index - width / 2, sx + 1.2, f"{sx:.1f}", ha="center", color=INK, fontsize=8.5)
        right.text(index + width / 2, sy + 1.2, f"{sy:.1f}", ha="center", color=MUTED, fontsize=8.5)
    right.set_ylabel("units", color=MUTED, fontsize=9)
    right.legend(frameon=False, labelcolor=INK, fontsize=8.5, loc="upper left")

    for ax in (left, right):
        ax.tick_params(colors=MUTED, labelsize=8.5)
        ax.set_ylim(0, max(lengths + [max(s) for s in spans]) * 1.28)
    for ax in (left, right):
        ax.set_xticks(list(positions))
        ax.set_xticklabels(names, color=MUTED, fontsize=8.5)

    fig.tight_layout(rect=(0, 0, 1, 0.93))
    save(fig, HERE, "mark-geometry")


def anatomy_diagram() -> None:
    spec = json.loads(SPEC_PATH.read_text(encoding="utf-8"))
    viewbox = float(spec["viewbox"])
    inset = float(spec["inset"])
    half = 0.5 * float(spec["stroke"]) * (viewbox - 2.0 * inset)
    parts = viewbox_points(spec)
    lengths = {name: centre_line_length(points) for name, _, points in parts}

    fig, ax = plt.subplots(figsize=(6.4, 6.4))
    ax.set_xlim(-4, viewbox + 4)
    ax.set_ylim(-4, viewbox + 4)
    ax.set_aspect("equal")
    ax.axis("off")
    fig.suptitle("psi.json — four parts, one geometry source", color=INK, fontsize=12, y=0.97)

    ax.add_patch(
        Rectangle(
            (inset, inset),
            viewbox - 2 * inset,
            viewbox - 2 * inset,
            fill=False,
            edgecolor=RULE,
            linestyle=(0, (5, 4)),
            linewidth=1.0,
        )
    )
    ax.text(inset + 1.0, inset + 1.0, f"inset {inset:g}", color=MUTED, fontsize=8.5)

    for name, colour, points in parts:
        face = PART_COLOURS[colour]
        for quad in stroke_quads(points, half):
            ax.add_patch(Polygon(quad, closed=True, facecolor=face, edgecolor="none"))

    # One label per part, in the house box vocabulary, naming its length.
    labels = {
        "stem": (viewbox / 2 + 8.0, viewbox * 0.58, "left"),
        "left-arm": (viewbox * 0.06, viewbox * 0.02, "left"),
        "right-arm": (viewbox * 0.94, viewbox * 0.02, "right"),
        "descender": (viewbox / 2 + 8.0, viewbox * 0.84, "left"),
    }
    for name, colour, _ in parts:
        x, y, align = labels[name]
        ax.text(
            x,
            y,
            f"{name}\n{lengths[name]:.1f} units",
            color=PART_COLOURS[colour],
            fontsize=8.5,
            ha=align,
            va="center",
            linespacing=1.5,
            bbox={"boxstyle": "round,pad=0.4", "facecolor": "#161a24", "edgecolor": PART_COLOURS[colour], "linewidth": 1.1},
        )

    ax.text(viewbox / 2, -3.0, f"viewBox 0 0 {viewbox:g} {viewbox:g} · stroke {spec['stroke']:g}", color=MUTED, fontsize=8.5, ha="center")

    fig.tight_layout(rect=(0, 0, 1, 0.94))
    save(fig, HERE, "mark-anatomy")


def main() -> None:
    geometry_chart()
    anatomy_diagram()
    print("figures: the-mark — mark-geometry, mark-anatomy")


if __name__ == "__main__":
    main()
