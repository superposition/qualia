#!/usr/bin/env python3
"""Figures for the journal entry `the-licence-and-the-clean-room-snapshot`.

    provenance-check.svg   a chart: what this tree and the private reference
                           have in common, and how much of it is identical
    licence-flow.svg       a diagram: the clean-room boundary, the three
                           attribution sources fanning into NOTICE, and the CI
                           gate that keeps them there

Data: `provenance.json`, recorded from this repository's own
`scripts/provenance_check.py` in two runs (the commit that introduced the check,
and the head of `main` when the entry was written).

Run:  python make_figures.py
"""

from __future__ import annotations

import json
import pathlib
import sys

import matplotlib.pyplot as plt

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BLUE,
    GREEN,
    INK,
    LAVENDER,
    MUTED,
    RULE,
    SAND,
    SLATE,
    arrow,
    box,
    save,
    use_style,
)

use_style()


def provenance_chart() -> None:
    runs = json.loads((HERE / "provenance.json").read_text())["runs"]

    fig, ax = plt.subplots(figsize=(9.0, 3.8))
    width = 0.24

    for pos, run in enumerate(runs):
        series = [
            ("tracked files", run["tracked_files"], SLATE, pos - width),
            ("also in the reference", run["compared"], BLUE, pos),
            ("byte-identical", run["identical"], SAND, pos + width),
        ]
        for label, value, colour, x in series:
            ax.bar(x, value, width, color=colour, label=label if pos == 0 else None)
            ax.text(
                x,
                value + 4,
                str(value),
                ha="center",
                va="bottom",
                color=colour,
                fontsize=11,
                fontweight="bold",
            )

    ax.axhline(0, color=RULE, linewidth=1)
    ax.set_xticks([0, 1])
    ax.set_xticklabels(
        [f"{run['commit']}\n{run['tracked_files']} tracked files" for run in runs],
        fontsize=10,
    )
    ax.set_ylim(0, 205)
    ax.set_ylabel("files", fontsize=10)
    ax.set_title(
        "The clean-room check: what it compared, and what it found identical",
        fontsize=12,
        color=INK,
        pad=12,
    )
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    ax.legend(
        frameon=False,
        fontsize=10,
        labelcolor=INK,
        ncol=3,
        loc="upper center",
        bbox_to_anchor=(0.5, -0.16),
    )

    fig.text(
        0.012,
        0.02,
        "Source: scripts/provenance_check.py, exit 0 at both commits; counts in provenance.json.",
        fontsize=9,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.075, right=0.985, top=0.84, bottom=0.30)
    save(fig, HERE, "provenance-check")


def licence_flow() -> None:
    fig, ax = plt.subplots(figsize=(9.6, 5.2))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.015, 0.965, "What may cross, and what the licence must carry", fontsize=12.5, color=INK)
    ax.text(
        0.015,
        0.925,
        "Solid arrows are files and commands. The dashed line is the clean-room boundary.",
        fontsize=9,
        color=MUTED,
    )

    # The boundary, labelled along its own length.
    ax.plot([0.345, 0.345], [0.05, 0.90], color=SAND, linewidth=1.6, linestyle="--")
    ax.text(
        0.356,
        0.47,
        "clean-room boundary: no file is copied",
        fontsize=8.6,
        color=SAND,
        rotation=90,
        ha="left",
        va="center",
    )

    # Top row: the two trees and the proof.
    box(
        ax,
        0.02,
        0.66,
        0.30,
        0.20,
        "read for interfaces only:\ncrate names, public signatures,\nwire contracts, SHM layout",
        MUTED,
        title="private engine checkout",
    )
    box(
        ax,
        0.42,
        0.66,
        0.28,
        0.20,
        'every line authored here\n(license = "Apache-2.0")',
        GREEN,
        title="public tree",
    )
    box(
        ax,
        0.76,
        0.66,
        0.22,
        0.20,
        "compared 110 files,\nbyte-identical 0,\nexit 0",
        BLUE,
        title="provenance_check.py",
    )
    arrow(ax, (0.32, 0.76), (0.42, 0.76), MUTED, label="interfaces only")
    arrow(ax, (0.70, 0.76), (0.76, 0.76), BLUE)

    # Bottom-left: the three attribution sources.
    box(ax, 0.02, 0.455, 0.30, 0.135, "the workspace root was\nrelicensed: MIT -> Apache-2.0", LAVENDER)
    box(ax, 0.02, 0.285, 0.30, 0.135, "Leash over HTTP (MIT)\ngithub.com/specdog/leash", GREEN)
    box(ax, 0.02, 0.115, 0.30, 0.135, "Male CNS connectome (CC-BY 4.0)\nmale-cns.janelia.org", BLUE)

    # Bottom-middle: what they must produce.
    box(
        ax,
        0.44,
        0.20,
        0.26,
        0.28,
        "three paragraphs, in order:\n\nCopyright 2026 Superposition LLC\nLeash project (MIT)\nMale CNS dataset (CC-BY 4.0)",
        SAND,
        title="NOTICE",
    )
    box(
        ax,
        0.76,
        0.20,
        0.22,
        0.28,
        "runs on every pull request\nand every push to main:\nNOTICE and LICENSE\nnon-empty, and both\nattribution strings\npresent",
        LAVENDER,
        title="notice-check.yml",
    )

    for y in (0.5225, 0.3525, 0.1825):
        arrow(ax, (0.32, y), (0.44, 0.34), SAND)
    arrow(ax, (0.70, 0.34), (0.76, 0.34), LAVENDER)

    fig.text(
        0.012,
        0.015,
        "Sources: LICENSE, NOTICE, .github/workflows/notice-check.yml, Cargo.toml.",
        fontsize=9,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.01, right=0.99, top=0.99, bottom=0.05)
    save(fig, HERE, "licence-flow")


if __name__ == "__main__":
    provenance_chart()
    licence_flow()
    print("wrote provenance-check.svg/.png and licence-flow.svg/.png")
