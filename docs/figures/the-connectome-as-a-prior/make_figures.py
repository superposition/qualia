#!/usr/bin/env python3
"""Figures for the journal entry `the-connectome-as-a-prior`.

    csr-matrix.svg   a chart: the committed fixture's 12 segment rows summed
                     into the 9 nonzero (pre_type, post_type) pairs of the
                     type-level CSR, as a weight matrix

Data: `crates/connectome-prior/tests/fixtures/mini-type-edges.csv`, read
directly — the same committed rows the builder's oracle aggregates.

Run:  python make_figures.py
"""

from __future__ import annotations

import csv
import pathlib
import sys

import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BG,
    BLUE,
    GREEN,
    INK,
    MUTED,
    PANEL,
    RULE,
    save,
    use_style,
)

use_style()

EDGES = REPO / "crates" / "connectome-prior" / "tests" / "fixtures" / "mini-type-edges.csv"


def read_pairs() -> tuple[list[str], dict[tuple[str, str], int]]:
    """The fixture's rows as (types, summed weight per (pre, post) pair)."""
    totals: dict[tuple[str, str], int] = {}
    types: set[str] = set()
    with EDGES.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            types.add(row["pre_type"])
            types.add(row["post_type"])
            key = (row["pre_type"], row["post_type"])
            totals[key] = totals.get(key, 0) + int(row["weight"])
    return sorted(types), totals


def blend(first: str, second: str, t: float) -> tuple[float, float, float]:
    """A linear blend of two hex colours, for the weight ramp."""
    channels = []
    for index in (1, 3, 5):
        a = int(first[index : index + 2], 16) / 255.0
        b = int(second[index : index + 2], 16) / 255.0
        channels.append(a * (1.0 - t) + b * t)
    return tuple(channels)


def csr_matrix() -> None:
    types, totals = read_pairs()
    count = len(types)
    limit = max(totals.values())
    fig, ax = plt.subplots(figsize=(7.4, 6.2))

    for row, pre in enumerate(types):
        for column, post in enumerate(types):
            weight = totals.get((pre, post))
            y = count - 1 - row
            if weight is None:
                ax.add_patch(
                    Rectangle((column, y), 1, 1, facecolor=PANEL, edgecolor=RULE, linewidth=0.8)
                )
                continue
            ax.add_patch(
                Rectangle(
                    (column, y),
                    1,
                    1,
                    facecolor=blend(BLUE, GREEN, weight / limit),
                    edgecolor=RULE,
                    linewidth=0.8,
                )
            )
            ax.text(
                column + 0.5,
                y + 0.5,
                str(weight),
                color=BG,
                ha="center",
                va="center",
                fontsize=11.5,
                fontweight="bold",
            )

    ax.set_xlim(0, count)
    ax.set_ylim(0, count)
    ax.set_aspect("equal")
    ax.set_xticks([index + 0.5 for index in range(count)])
    ax.set_xticklabels(types, rotation=35, ha="right")
    ax.set_yticks([index + 0.5 for index in range(count)])
    ax.set_yticklabels(list(reversed(types)))
    ax.set_xlabel("post_type")
    ax.set_ylabel("pre_type")
    ax.tick_params(length=0)
    for spine in ax.spines.values():
        spine.set_visible(False)
    ax.set_title(
        "12 segment rows -> 9 nonzero pairs; fill is the pair's summed weight",
        color=MUTED,
        fontsize=9.0,
        loc="left",
    )
    fig.tight_layout()
    save(fig, HERE, "csr-matrix")


if __name__ == "__main__":
    csr_matrix()
    print("wrote csr-matrix.svg/.png")
