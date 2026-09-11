#!/usr/bin/env python3
"""Figures for the journal entry `the-licence-and-the-clean-room-snapshot`.

Two figures, both written from `provenance.json` (the recorded output of the
repository's own `scripts/provenance_check.py`) and from the committed licence
files:

    provenance-check.svg   a chart: what the private reference and this tree
                           have in common, and how much of it is identical
    licence-flow.svg       a diagram: the clean-room boundary, the three
                           attribution sources fanning into NOTICE, and the CI
                           gate that keeps them there

Run:  python make_figures.py
The SVG output is deterministic for a given matplotlib version: metadata dates
are suppressed and a hash salt is fixed, so two runs produce the same bytes.
"""

from __future__ import annotations

import json
import pathlib

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyArrowPatch, FancyBboxPatch

HERE = pathlib.Path(__file__).resolve().parent

BG = "#101217"
INK = "#edf0f5"
GREEN = "#91dbba"
LAVENDER = "#c9b2ff"
BLUE = "#93caff"
SAND = "#e0a08a"
RULE = "#2a3140"
MUTED = "#8b93a7"
SLATE = "#4a5468"

MONO = ["DejaVu Sans Mono", "Consolas", "monospace"]

plt.rcParams.update(
    {
        "font.family": "monospace",
        "font.monospace": MONO,
        "svg.hashsalt": "qualia-journal",
        "figure.facecolor": BG,
        "savefig.facecolor": BG,
        "text.color": INK,
        "axes.labelcolor": INK,
        "xtick.color": MUTED,
        "ytick.color": MUTED,
        "axes.edgecolor": RULE,
    }
)

META = {"Date": None, "Creator": None}


def save(fig, name: str) -> None:
    for ext in ("svg", "png"):
        fig.savefig(HERE / f"{name}.{ext}", format=ext, metadata=META, dpi=110)
    plt.close(fig)


def provenance_chart() -> None:
    runs = json.loads((HERE / "provenance.json").read_text())["runs"]

    fig, ax = plt.subplots(figsize=(9.0, 3.6))
    positions = range(len(runs))
    width = 0.24

    for pos, run in zip(positions, runs):
        series = [
            ("tracked files", run["tracked_files"], SLATE, pos - width),
            ("also in the reference", run["compared"], BLUE, pos),
            ("byte-identical", run["identical"], SAND, pos + width),
        ]
        for label, value, colour, x in series:
            ax.bar(x, value, width, color=colour, label=label if pos == 0 else None)
            ax.text(
                x,
                value + 3,
                str(value),
                ha="center",
                va="bottom",
                color=colour,
                fontsize=11,
                fontweight="bold",
            )

    ax.axhline(0, color=RULE, linewidth=1)
    ax.set_xticks(list(positions))
    ax.set_xticklabels(
        [f"{run['commit']}\n{run['tracked_files']} tracked files" for run in runs],
        fontsize=10,
    )
    ax.set_ylim(0, 190)
    ax.set_ylabel("files", fontsize=10)
    ax.set_title(
        "The clean-room check: 110 files share a path with the private reference, 0 share a byte",
        fontsize=12,
        color=INK,
        pad=14,
    )
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    ax.legend(frameon=False, fontsize=10, labelcolor=INK, ncol=3, loc="upper left")

    fig.text(
        0.012,
        0.015,
        "Source: scripts/provenance_check.py, exit 0 at both commits (provenance.json).",
        fontsize=9,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.065, right=0.985, top=0.83, bottom=0.24)
    save(fig, "provenance-check")


def box(ax, x, y, w, h, text, edge, title=None, fontsize=9.0):
    ax.add_patch(
        FancyBboxPatch(
            (x, y),
            w,
            h,
            boxstyle="round,pad=0.012,rounding_size=0.02",
            linewidth=1.2,
            edgecolor=edge,
            facecolor="#161a24",
        )
    )
    if title:
        ax.text(x + 0.012, y + h - 0.035, title, fontsize=fontsize, color=edge, va="top", fontweight="bold")
        ax.text(x + 0.012, y + h - 0.085, text, fontsize=fontsize - 0.7, color=INK, va="top", linespacing=1.5)
    else:
        ax.text(x + 0.012, y + h / 2, text, fontsize=fontsize - 0.7, color=INK, va="center", linespacing=1.5)


def arrow(ax, start, end, colour, style="-", dashed=False, label=None, label_offset=(0.0, 0.022)):
    ax.add_patch(
        FancyArrowPatch(
            start,
            end,
            arrowstyle="-|>",
            mutation_scale=11,
            linewidth=1.3,
            color=colour,
            linestyle="--" if dashed else style,
            shrinkA=1,
            shrinkB=1,
        )
    )
    if label:
        ax.text(
            (start[0] + end[0]) / 2 + label_offset[0],
            (start[1] + end[1]) / 2 + label_offset[1],
            label,
            fontsize=8.2,
            color=colour,
            ha="center",
            va="center",
            linespacing=1.4,
        )


def licence_flow() -> None:
    fig, ax = plt.subplots(figsize=(9.6, 5.0))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.015, 0.955, "What may cross, and what the licence must carry", fontsize=12.5, color=INK)
    ax.text(
        0.015,
        0.915,
        "Solid arrows are files and commands; the dashed line is the clean-room boundary.",
        fontsize=9,
        color=MUTED,
    )

    # Private side.
    box(ax, 0.02, 0.62, 0.28, 0.22, "read for interfaces only:\ncrate names, public\nsignatures, wire contracts,\nSHM layout", MUTED, title="private engine checkout")

    # The boundary.
    ax.plot([0.345, 0.345], [0.10, 0.88], color=SAND, linewidth=1.6, linestyle="--")
    ax.text(
        0.352,
        0.86,
        "clean-room boundary\nno file is copied",
        fontsize=8.6,
        color=SAND,
        va="top",
        linespacing=1.5,
    )

    # Public side.
    box(ax, 0.40, 0.62, 0.27, 0.22, "every line authored here\n(license = \"Apache-2.0\")", GREEN, title="public tree")

    # The proof.
    box(
        ax,
        0.72,
        0.62,
        0.26,
        0.22,
        "compared 104 files\nbyte-identical 0\nexit 0",
        BLUE,
        title="provenance_check.py",
    )
    arrow(ax, (0.30, 0.73), (0.40, 0.73), MUTED, label="interfaces")
    arrow(ax, (0.67, 0.73), (0.72, 0.73), BLUE)

    # Attribution sources.
    box(ax, 0.02, 0.30, 0.24, 0.17, "the workspace root\nwas relicensed:\nMIT -> Apache-2.0", LAVENDER)
    box(ax, 0.02, 0.10, 0.24, 0.17, "Leash, reached over HTTP\n(MIT)\nhttps://github.com/specdog/leash", GREEN)
    box(ax, 0.29, 0.10, 0.24, 0.17, "Male CNS connectome\n(CC-BY 4.0)\nhttps://male-cns.janelia.org", BLUE)

    box(ax, 0.40, 0.18, 0.29, 0.24, "three paragraphs, in order:\n\nCopyright 2026 Superposition LLC\nLeash project (MIT)\nMale CNS dataset (CC-BY 4.0)", SAND, title="NOTICE")

    box(ax, 0.72, 0.18, 0.26, 0.24, "runs on every pull request\nand every push to main:\nNOTICE and LICENSE\nnon-empty; the two\nattribution strings present", LAVENDER, title="notice-check.yml")

    for y in (0.385, 0.185):
        arrow(ax, (0.26, y), (0.40, 0.30), SAND)
    arrow(ax, (0.53, 0.10), (0.53, 0.18), SAND)
    arrow(ax, (0.69, 0.30), (0.72, 0.30), LAVENDER)

    fig.text(0.012, 0.015, "Sources: LICENSE, NOTICE, .github/workflows/notice-check.yml, Cargo.toml.", fontsize=9, color=MUTED)
    fig.subplots_adjust(left=0.01, right=0.99, top=0.99, bottom=0.05)
    save(fig, "licence-flow")


if __name__ == "__main__":
    provenance_chart()
    licence_flow()
    print("wrote provenance-check.svg/.png and licence-flow.svg/.png")
