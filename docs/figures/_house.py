#!/usr/bin/env python3
"""The house style for the journal figures: one palette, one box/arrow vocabulary.

Every figure under `docs/figures/<entry-slug>/` is drawn with these helpers, so
the entries cannot drift apart in colour or line weight. The palette is the
site's: background `#101217`, ink `#edf0f5`, accents `#91dbba`, `#c9b2ff`,
`#93caff`, `#e0a08a`.
"""

from __future__ import annotations

import pathlib

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyArrowPatch, FancyBboxPatch

BG = "#101217"
PANEL = "#0c0f15"
BOX = "#161a24"
INK = "#edf0f5"
GREEN = "#91dbba"
LAVENDER = "#c9b2ff"
BLUE = "#93caff"
SAND = "#e0a08a"
RULE = "#2a3140"
MUTED = "#8b93a7"
SLATE = "#4a5468"

MONO = ["DejaVu Sans Mono", "Consolas", "monospace"]

STYLE = {
    "font.family": "monospace",
    "font.monospace": MONO,
    "svg.hashsalt": "qualia-journal",
    "figure.facecolor": BG,
    "savefig.facecolor": BG,
    "savefig.transparent": False,
    "text.color": INK,
    "axes.labelcolor": INK,
    "axes.facecolor": PANEL,
    "axes.edgecolor": RULE,
    "xtick.color": MUTED,
    "ytick.color": MUTED,
}

# Metadata is suppressed so two runs of the same script produce identical bytes
# (with the same matplotlib version).
META = {"Date": None, "Creator": None}


def use_style() -> None:
    plt.rcParams.update(STYLE)


def save(fig, directory: pathlib.Path, name: str) -> None:
    """Write `<name>.svg` and `<name>.png` beside the script."""
    for ext in ("svg", "png"):
        fig.savefig(directory / f"{name}.{ext}", format=ext, metadata=META, dpi=110)
    plt.close(fig)


def box(ax, x, y, w, h, text, edge, title=None, fontsize=9.0) -> None:
    ax.add_patch(
        FancyBboxPatch(
            (x, y),
            w,
            h,
            boxstyle="round,pad=0.012,rounding_size=0.02",
            linewidth=1.2,
            edgecolor=edge,
            facecolor=BOX,
        )
    )
    if title:
        ax.text(
            x + 0.014,
            y + h - 0.032,
            title,
            fontsize=fontsize,
            color=edge,
            va="top",
            fontweight="bold",
        )
        ax.text(
            x + 0.014,
            y + h - 0.085,
            text,
            fontsize=fontsize - 0.7,
            color=INK,
            va="top",
            linespacing=1.55,
        )
    else:
        ax.text(
            x + 0.014,
            y + h / 2,
            text,
            fontsize=fontsize - 0.7,
            color=INK,
            va="center",
            linespacing=1.55,
        )


def arrow(ax, start, end, colour, label=None, label_offset=(0.0, 0.024), dashed=False) -> None:
    ax.add_patch(
        FancyArrowPatch(
            start,
            end,
            arrowstyle="-|>",
            mutation_scale=11,
            linewidth=1.3,
            color=colour,
            linestyle="--" if dashed else "-",
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
