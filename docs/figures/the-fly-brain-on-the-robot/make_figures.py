#!/usr/bin/env python3
"""Figures for the journal entry `the-fly-brain-on-the-robot`.

    drive-commands.svg  a chart: what the connectome's read-out commanded, per
                        closed-loop run, read straight from each run's trace
    dataset-gate.svg    a chart: two real sessions through the dataset's
                        transition gates, and the promotion floor above both

The driving loop is not drawn here: it is `driving-loop.mmd`, the Mermaid
source the entry's `mermaid` block copies (rule 1's Mermaid form). This
generator rewrites the two charts.

Data: `wave-numbers.json` carries the charts' numbers, each a line of the
committed evidence named in its `source` field; a chart value the file does not
carry is a refusal, not a guess. The command counts are not copied here — the
generator re-counts each run's committed trace CSV and refuses when the file and
the recording disagree.

Run:  python make_figures.py
"""

from __future__ import annotations

import csv
import json
import pathlib
import sys

import matplotlib.pyplot as plt

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BLUE,
    GREEN,
    INK,
    MUTED,
    RULE,
    SAND,
    SLATE,
    save,
    use_style,
)

use_style()

DATA = json.loads((HERE / "wave-numbers.json").read_text(encoding="utf-8"))


def need(path: str):
    """A value the data file must carry; a missing one is a refusal, not a guess."""
    node = DATA
    for key in path.split("."):
        if key not in node:
            raise SystemExit(f"wave-numbers.json is missing {path!r}; refusing to draw")
        node = node[key]
    return node


def command_counts(entry: dict) -> tuple[int, int, int]:
    """Re-count one run's committed trace: (left, right, hold).

    The recording carries the counts; the trace carries the commands. A file
    that disagrees with the recording is a refusal, not a rounding difference.
    """
    rel = DATA["sources"][entry["source"]]
    path = REPO / rel
    if not path.is_file():
        raise SystemExit(f"trace {rel} is missing; refusing to draw")
    rows = [
        row
        for row in csv.DictReader(line for line in path.read_text().splitlines() if not line.startswith("#"))
    ]
    counts = {"-1": 0, "1": 0, "0": 0}
    for row in rows:
        counts[row["command"]] += 1
    found = (counts["-1"], counts["1"], counts["0"])
    if found != (entry["left"], entry["right"], entry["hold"]):
        raise SystemExit(
            f"{rel} holds L={found[0]} R={found[1]} hold={found[2]}, "
            f"the recording says L={entry['left']} R={entry['right']} hold={entry['hold']}"
        )
    if len(rows) != entry["ticks"]:
        raise SystemExit(f"{rel} holds {len(rows)} trace rows, the recording says {entry['ticks']} ticks")
    return found


def drive_commands() -> None:
    runs = need("command_runs")
    decoder = need("command_rule")

    fig, ax = plt.subplots(figsize=(11.4, 4.4))
    height = 0.5
    for pos, entry in enumerate(reversed(runs)):
        left, right, hold = command_counts(entry)
        segments = [
            ("left", left, GREEN),
            ("right", right, BLUE),
            ("hold", hold, SLATE),
        ]
        start = 0
        for label, value, colour in segments:
            if value == 0:
                continue
            ax.barh(pos, value, height, left=start, color=colour, label=label if pos == 0 else None)
            ax.text(
                start + value / 2,
                pos,
                f"{value}",
                ha="center",
                va="center",
                color="#101217",
                fontsize=10,
                fontweight="bold",
            )
            start += value
        ax.text(
            entry["ticks"] + 6,
            pos,
            f"{entry['non_hold']} of {entry['ticks']} ticks commanded\n"
            f"{entry['ticks_per_s']:.1f} ticks/s · {entry['seconds']:.3f} s",
            va="center",
            ha="left",
            color=MUTED,
            fontsize=9,
            linespacing=1.5,
        )

    ax.set_yticks(range(len(runs)))
    # The bars are drawn bottom-up over `reversed(runs)`, so the labels must be
    # taken in that same order; labelling from `runs` would put the host run's
    # bar under the first board run's name.
    ax.set_yticklabels([entry["run"] for entry in reversed(runs)], fontsize=9.8)
    ax.invert_yaxis()
    ax.set_xlim(0, 440)
    ax.set_xticks([0, 100, 200, 300])
    ax.set_xlabel("ticks of one closed-loop run", fontsize=10)
    ax.set_title(
        "What the connectome commanded: "
        f"{decoder['decoder_neurons']:,} descending/motor neurons, sign of R - L",
        fontsize=12,
        color=INK,
        pad=12,
    )
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    ax.grid(axis="x", color=RULE, linewidth=0.6, alpha=0.5)
    ax.set_axisbelow(True)
    ax.legend(
        frameon=False,
        fontsize=9.8,
        labelcolor=INK,
        ncol=3,
        loc="upper center",
        bbox_to_anchor=(0.5, -0.26),
    )
    fig.text(
        0.012,
        0.02,
        "Source: each run's committed trace in docs/evidence/T55/connectome-runner/ "
        "(host-loop-gpu-trace.csv, board-loop-trace.csv, board-loop-trace-rerun.csv), re-counted here.",
        fontsize=8.3,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.25, right=0.70, top=0.85, bottom=0.31)
    save(fig, HERE, "drive-commands")


def dataset_gate() -> None:
    sessions = need("sessions")
    floor = need("promotion_floor")

    fig, ax = plt.subplots(figsize=(9.4, 4.6))
    width = 0.30

    for pos, session in enumerate(sessions):
        series = [
            ("candidate transitions", session["candidates"], SLATE, pos - width / 1.7),
            ("admitted valid", session["valid"], GREEN, pos + width / 1.7),
        ]
        for label, value, colour, x in series:
            ax.bar(x, value, width, color=colour, label=label if pos == 0 else None)
            ax.text(
                x,
                value + 90,
                f"{value:,}",
                ha="center",
                va="bottom",
                color=colour,
                fontsize=11,
                fontweight="bold",
            )

    ax.annotate(
        f"promotion floor: {floor['valid_transitions']:,} valid · {floor['sessions']} sessions · "
        f"{floor['environments']} environments · {floor['conditions']} conditions\n"
        f"({floor['shortfall_x']}x the whole accepted session; off this chart)",
        xy=(0.5, 1.0),
        xycoords="axes fraction",
        xytext=(0, -2),
        textcoords="offset points",
        ha="center",
        va="top",
        color=SAND,
        fontsize=9.2,
    )

    ax.set_xticks([0, 1])
    ax.set_xticklabels(
        [
            f"{session['session']}\n{session['seconds']} s · {session['mcap_bytes']:,} B\n"
            f"{session['valid']:,} of {session['candidates']:,} valid"
            for session in sessions
        ],
        fontsize=9.4,
    )
    ax.set_ylim(0, 5400)
    ax.set_yticks([0, 1000, 2000, 3000, 4000, 5000])
    ax.set_ylabel("transitions", fontsize=10)
    ax.set_title(
        "Two real sessions through the dataset's transition gates",
        fontsize=12,
        color=INK,
        pad=12,
    )
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    ax.legend(
        frameon=False,
        fontsize=9.8,
        labelcolor=INK,
        ncol=2,
        loc="upper center",
        bbox_to_anchor=(0.5, -0.36),
    )
    fig.text(
        0.012,
        0.02,
        "Source: docs/evidence/T58/real-sessions/README.md and docs/evidence/T65/calibration/README.md "
        "(the dataset binary's own audit lines); the floor is docs/decisions.md D-027.",
        fontsize=8.4,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.085, right=0.985, top=0.80, bottom=0.335)
    save(fig, HERE, "dataset-gate")


if __name__ == "__main__":
    drive_commands()
    dataset_gate()
    runs = [
        (
            entry["run"],
            (entry["left"], entry["right"], entry["hold"]),
            entry["ticks_per_s"],
        )
        for entry in DATA["command_runs"]
    ]
    print(f"wrote drive-commands, dataset-gate; runs {runs}")
