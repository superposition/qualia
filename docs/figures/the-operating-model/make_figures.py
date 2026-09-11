#!/usr/bin/env python3
"""Figures for the journal entry `the-operating-model`.

    ledger-status.svg   a chart: the 95 ticket issues by status label, and how
                        many of them the committed dependency diagram draws
    operating-loop.svg  a diagram: the claim/work/handoff loop and the two edges
                        that keep it resumable — the blocked branch and the
                        agent-dies branch

Data: `ledger-snapshot.json`, one timestamped reading of the tracker taken with
`gh issue list --repo superposition/qualia --label ticket --state all`.

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

COLOURS = {
    "status:ready": BLUE,
    "status:claimed": LAVENDER,
    "status:review": SAND,
    "status:done": GREEN,
    "status:blocked": MUTED,
}


def ledger_chart() -> None:
    snap = json.loads((HERE / "ledger-snapshot.json").read_text())
    status = snap["status"]
    coverage = snap["diagram_coverage"]

    fig, (left, right) = plt.subplots(1, 2, figsize=(9.6, 4.4), gridspec_kw={"width_ratios": [1.7, 1.0]})

    order = ["status:ready", "status:claimed", "status:done", "status:review", "status:blocked"]
    labels = [name.split(":")[1] for name in order]
    values = [status[name] for name in order]
    colours = [COLOURS[name] for name in order]

    bars = left.barh(range(len(order)), values, color=colours, height=0.62)
    left.set_yticks(range(len(order)))
    left.set_yticklabels(labels, fontsize=11, color=INK)
    left.invert_yaxis()
    left.set_xlim(0, 66)
    left.set_xlabel("ticket issues", fontsize=10)
    for bar, value in zip(bars, values):
        left.text(
            value + 1.2,
            bar.get_y() + bar.get_height() / 2,
            str(value),
            va="center",
            fontsize=11,
            fontweight="bold",
            color=bar.get_facecolor(),
        )
    left.set_title("95 ticket issues, by status label", fontsize=12, color=INK, pad=12)
    for spine in ("top", "right"):
        left.spines[spine].set_visible(False)
    left.text(
        0.0,
        -0.235,
        f"{snap['closed_but_not_status_done']} closed tickets still carry "
        "status:review or status:claimed.",
        transform=left.transAxes,
        fontsize=9.5,
        color=SAND,
    )

    drawn = [
        ("in the issues", coverage["ids_in_ticket_issues"], BLUE),
        ("in epics.mmd", coverage["ids_in_diagram"], SLATE),
        ("missing", len(coverage["missing_from_diagram"]), SAND),
    ]
    for pos, (name, value, colour) in enumerate(drawn):
        right.bar(pos, value, 0.5, color=colour)
        right.text(pos, value + 2, str(value), ha="center", va="bottom", fontsize=11, fontweight="bold", color=colour)
    right.set_xticks(range(len(drawn)))
    right.set_xticklabels([name for name, _, _ in drawn], fontsize=10, color=INK)
    right.set_ylim(0, 112)
    right.set_title("Tickets the diagram draws", fontsize=12, color=INK, pad=12)
    for spine in ("top", "right"):
        right.spines[spine].set_visible(False)
    right.text(
        0.0,
        -0.235,
        "missing: "
        + ", ".join(coverage["missing_from_diagram"])
        + f"\n(drawn once, at {coverage['last_commit']})",
        transform=right.transAxes,
        fontsize=9.5,
        color=SAND,
    )

    fig.text(
        0.012,
        0.02,
        f"One reading of a live tracker at {snap['measured_at_utc']}; agents were claiming tickets while it was taken.",
        fontsize=9,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.085, right=0.985, top=0.84, bottom=0.26, wspace=0.28)
    save(fig, HERE, "ledger-status")


def operating_loop() -> None:
    fig, ax = plt.subplots(figsize=(9.6, 5.2))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.015, 0.965, "The operating loop, and the two edges that make it resumable", fontsize=12.5, color=INK)
    ax.text(
        0.015,
        0.925,
        "Every box is a comment on an issue or a pull request; nothing is held in an agent's memory.",
        fontsize=9,
        color=MUTED,
    )

    # Row 1, left to right: the happy path.
    box(ax, 0.02, 0.71, 0.20, 0.13, "read the epic\nissue", BLUE)
    box(ax, 0.25, 0.71, 0.20, 0.13, "list\nstatus:ready", BLUE)
    box(ax, 0.48, 0.71, 0.22, 0.13, "claim: assignee +\nbraid comment +\nstatus:claimed", LAVENDER)
    box(ax, 0.73, 0.71, 0.25, 0.13, "work: tests first,\none ticket, one owner", GREEN)
    arrow(ax, (0.22, 0.775), (0.25, 0.775), MUTED)
    arrow(ax, (0.45, 0.775), (0.48, 0.775), MUTED)
    arrow(ax, (0.70, 0.775), (0.73, 0.775), MUTED)

    # Row 2, right to left: evidence, review, merge.
    box(ax, 0.73, 0.47, 0.25, 0.13, "open a PR: braid block,\nthen Closes #<ticket>", GREEN)
    box(ax, 0.48, 0.47, 0.22, 0.13, "comment the\nbreadcrumb: state review", LAVENDER)
    box(ax, 0.20, 0.47, 0.24, 0.13, "merge: status:done;\nthe epic keeps its\nJournal line", SAND)
    arrow(ax, (0.855, 0.71), (0.855, 0.60), MUTED)
    arrow(ax, (0.73, 0.535), (0.70, 0.535), MUTED)
    arrow(ax, (0.48, 0.535), (0.44, 0.535), MUTED)
    # A failed review goes back to the work, not forward.
    arrow(
        ax,
        (0.66, 0.60),
        (0.76, 0.71),
        SAND,
        dashed=True,
        label="request-changes",
        label_offset=(-0.02, -0.035),
    )

    # Row 3: the two branches that keep the loop resumable.
    box(
        ax,
        0.02,
        0.245,
        0.30,
        0.135,
        "status:blocked + blocked_on;\nthe resume label only when the\nblocker may never return",
        SAND,
    )
    box(
        ax,
        0.44,
        0.245,
        0.54,
        0.135,
        "the agent dies mid-task, leaving one braid block;\na later agent relabels the ticket resume, reads the\nlast block and continues where it stopped",
        MUTED,
    )
    arrow(ax, (0.30, 0.47), (0.17, 0.38), SAND, label="blocked", label_offset=(-0.09, 0.03))
    arrow(ax, (0.59, 0.47), (0.71, 0.38), MUTED, dashed=True, label="agent dies", label_offset=(0.0, 0.035))

    box(
        ax,
        0.09,
        0.015,
        0.82,
        0.155,
        "gh issue list --label ticket --state open \\\n"
        "  --json number,title,labels,updatedAt \\\n"
        "  | jq -r '.[] | select([.labels[].name] | index(\"status:ready\")) | .number'",
        BLUE,
        title="the resume command, run from a cold shell",
        fontsize=8.4,
    )
    arrow(
        ax,
        (0.71, 0.245),
        (0.71, 0.17),
        BLUE,
        dashed=True,
        label="the ledger is the state",
        label_offset=(0.145, 0.0),
    )

    fig.text(
        0.015,
        0.893,
        "Sources: docs/agents.md, docs/waves.md, docs/architecture/{agents,states}.mmd.",
        fontsize=8.6,
        color=MUTED,
        ha="left",
    )
    fig.subplots_adjust(left=0.01, right=0.99, top=0.99, bottom=0.02)
    save(fig, HERE, "operating-loop")


if __name__ == "__main__":
    ledger_chart()
    operating_loop()
    print("wrote ledger-status.svg/.png and operating-loop.svg/.png")
