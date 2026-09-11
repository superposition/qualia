#!/usr/bin/env python3
"""Figures for the journal entry `the-front-end-rebuilt-from-lessons`.

    lesson-sources.svg   a chart: how many of the console's decisions each of
                         the five front ends the lessons document read informs

Data: `docs/frontend-lessons.md`, read directly — its source table and the
closing "what this means for apps/qualia-console" table, whose "Comes from"
column names the sources each decision owes itself to.

Run:  python make_figures.py
"""

from __future__ import annotations

import pathlib
import re
import sys

import matplotlib.pyplot as plt

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    GREEN,
    INK,
    MUTED,
    RULE,
    SAND,
    save,
    use_style,
)

use_style()

LESSONS = REPO / "docs" / "frontend-lessons.md"


def source_names(text: str) -> dict[int, str]:
    names: dict[int, str] = {}
    for line in text.splitlines():
        match = re.match(r"^\|\s*(\d)\s*\|\s*([^|]+?)\s*\|", line)
        if match:
            names[int(match.group(1))] = match.group(2)
    return names


def decisions(text: str) -> tuple[dict[int, int], int]:
    """Decisions per source, and the decision count: the closing table's rows."""
    counts: dict[int, int] = {number: 0 for number in source_names(text)}
    lines = text.splitlines()
    start = next((index for index, line in enumerate(lines) if "Comes from" in line), None)
    if start is None:
        raise SystemExit("frontend-lessons.md: no `Comes from` table")
    rows = 0
    for line in lines[start + 2 :]:
        if not line.startswith("|"):
            break
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 2:
            continue
        rows += 1
        for number in (int(digit) for digit in re.findall(r"\d+", cells[1])):
            if number in counts:
                counts[number] += 1
    return counts, rows


def lesson_sources() -> None:
    text = LESSONS.read_text(encoding="utf-8")
    names = source_names(text)
    counts, rows = decisions(text)
    order = sorted(counts)
    labels = [names[number] for number in order]
    values = [counts[number] for number in order]
    peak = max(values)
    colours = [SAND if value == peak else GREEN for value in values]

    fig, ax = plt.subplots(figsize=(9.8, 4.4))
    positions = list(range(len(order)))
    ax.barh(positions, values, height=0.56, color=colours, edgecolor=RULE, linewidth=0.8)
    for position, value in zip(positions, values):
        ax.text(value + 0.12, position, str(value), color=INK, va="center", fontsize=10.0)
    ax.set_yticks(positions)
    ax.set_yticklabels(labels)
    ax.invert_yaxis()
    ax.set_xlim(0.0, peak + 1.2)
    ax.set_xticks(list(range(0, peak + 1)))
    ax.set_xlabel("decisions in docs/frontend-lessons.md that name this source")
    ax.tick_params(length=0)
    ax.grid(axis="x", color=RULE, linewidth=0.6, alpha=0.6)
    ax.set_axisbelow(True)
    for spine in ax.spines.values():
        spine.set_visible(False)
    fig.text(
        0.30,
        0.04,
        f"{len(names)} front ends read; {rows} console decisions distilled,\n"
        f"{sum(values)} source references (a decision may name several sources).",
        color=MUTED,
        fontsize=8.6,
        ha="left",
        va="bottom",
    )
    fig.subplots_adjust(left=0.30, right=0.97, top=0.95, bottom=0.24)
    save(fig, HERE, "lesson-sources")


if __name__ == "__main__":
    lesson_sources()
    print("wrote lesson-sources.svg/.png")
