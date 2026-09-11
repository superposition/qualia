#!/usr/bin/env python3
"""Figures for the journal entry `the-ladder`.

    ladder-bands.svg   a chart: the healing ladder's escalation as four bands
                       over squared Mahalanobis distance, separated at the
                       plan's 3, 5 and 8, with the measured anchors marked

Data: `ladder.json`, the recording of the plan's escalation table (epic #11,
step 35) and of the values this repository measures (`crates/braid`).

Run:  python make_figures.py
"""

from __future__ import annotations

import json
import pathlib
import sys

import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BLUE,
    GREEN,
    INK,
    LAVENDER,
    MUTED,
    SAND,
    save,
    use_style,
)

use_style()

COLOURS = (GREEN, LAVENDER, BLUE, SAND)


def ladder_bands() -> None:
    document = json.loads((HERE / "ladder.json").read_text(encoding="utf-8"))
    thresholds = [float(value) for value in document["thresholds"]]
    upper = thresholds[-1] + (thresholds[-1] - thresholds[-2])
    edges = [0.0] + thresholds + [upper]
    actions = [step["action"] for step in document["steps"]]
    measured = {row["quantity"]: row for row in document["measured"]}

    fig, ax = plt.subplots(figsize=(9.8, 4.6))
    for band in range(len(edges) - 1):
        ax.add_patch(
            Rectangle(
                (edges[band], 0.0),
                edges[band + 1] - edges[band],
                1.0,
                facecolor=COLOURS[band],
                alpha=0.20,
                edgecolor=COLOURS[band],
                linewidth=1.2,
            )
        )
        ax.text(
            (edges[band] + edges[band + 1]) / 2.0,
            0.5,
            actions[band],
            color=INK,
            ha="center",
            va="center",
            fontsize=10.0,
        )

    for threshold in thresholds:
        ax.axvline(threshold, color=INK, linewidth=1.0, linestyle=(0, (4, 3)))
        ax.text(threshold, 1.035, f"{threshold:g}", color=INK, ha="center", va="bottom", fontsize=10.0)

    identity = float(measured["identity-prediction drift"]["value"])
    ax.plot([identity], [1.0], marker="o", color=INK, markersize=5.0, clip_on=False)
    ax.text(
        identity + 0.12,
        1.0,
        "identity-prediction drift 0.0 (measured)",
        color=INK,
        fontsize=8.6,
        ha="left",
        va="center",
    )

    ax.set_xlim(0.0, upper)
    ax.set_xticks([0.0] + thresholds)
    ax.set_xticklabels(["0"] + [f"{threshold:g}" for threshold in thresholds])
    ax.set_ylim(0.0, 1.0)
    ax.set_yticks([])
    ax.set_xlabel("squared Mahalanobis distance from the predictor's mean")
    ax.tick_params(length=0)
    for spine in ax.spines.values():
        spine.set_visible(False)
    fig.text(
        0.02,
        0.04,
        "3, 5, 8 and the 10 s hold are the plan's specification (epic #11, step 35): "
        "no run has measured a normal drift to compare against.",
        color=MUTED,
        fontsize=8.6,
        ha="left",
        va="bottom",
    )
    fig.subplots_adjust(left=0.03, right=0.98, top=0.92, bottom=0.28)
    save(fig, HERE, "ladder-bands")


if __name__ == "__main__":
    ladder_bands()
    print("wrote ladder-bands.svg/.png")
