#!/usr/bin/env python3
"""Figures for the journal entry `the-agent-in-the-loop`.

    mission-lifecycle.svg   a chart, two panels: `open_missions` through the
                            braid's own fold, and the belief pace gate's wait /
                            stale decision
    strand-flow.svg         a diagram: the strands, the one write edge, `observe`,
                            the state they report into, and the durable copies
                            the braid does not keep

Data: `loop-state.json` — the event sequences two committed tests fold, and the
pace gate's two constants and two stale publications its tests assert. The fold
and the constants are re-read from `crates/braid/src/lib.rs`,
`runners/map/src/main.rs` and the tests before anything is drawn.

Run:  py -3.13 docs/figures/the-agent-in-the-loop/make_figures.py
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BLUE,
    GREEN,
    INK,
    LAVENDER,
    MUTED,
    RULE,
    SAND,
    arrow,
    box,
    save,
    use_style,
)

use_style()

RECORDING = json.loads((HERE / "loop-state.json").read_text(encoding="utf-8"))

# The colours the two series and their annotations keep in both figures.
CRATE_COLOUR = GREEN
EDGE_COLOUR = BLUE


def fold(events) -> list:
    """The braid's own count: opened adds one, closed subtracts one, never below zero.

    `crates/braid/src/lib.rs`'s `observe` counts the mission events it folds and
    saturates the close, so a close for a mission the braid never saw opened is
    still a fact the strand reported but cannot wrap the count.
    """
    count = 0
    series = []
    for event in events:
        if event.startswith("mission_opened"):
            count += 1
        elif event.startswith("mission_closed"):
            count = max(0, count - 1)
        series.append(count)
    return series


def check_recording() -> None:
    """Re-fold every recorded sequence and re-read the gate's constants."""
    for entry in RECORDING["lifecycle"]:
        folded = fold(entry["events"])
        if folded != entry["open_missions"]:
            raise SystemExit(
                "figure: %s folds to %r, the recording says %r"
                % (entry["series"], folded, entry["open_missions"])
            )

    source = (REPO / "runners" / "map" / "src" / "main.rs").read_text(encoding="utf-8")
    pace = int(re.search(r"DEFAULT_BELIEF_PACE_MS: u64 = (\d+)", source).group(1))
    windows = int(re.search(r"BELIEF_PACE_STALE_WINDOWS: u64 = (\d+)", source).group(1))
    constants = RECORDING["pace"]["constants"]
    if (pace, windows) != (constants["pace_ms"], constants["stale_windows"]):
        raise SystemExit(
            "figure: runners/map says %d ms x %d, the recording says %r"
            % (pace, windows, constants)
        )
    tests = (REPO / "runners" / "map" / "tests" / "runtime.rs").read_text(encoding="utf-8")
    for name in (
        "publishes_when_belief_lag_exceeded",
        "waits_while_belief_is_inside_the_stale_window",
    ):
        if name not in tests:
            raise SystemExit(f"figure: runners/map/tests/runtime.rs no longer holds {name}")


def lifecycle_chart() -> None:
    fig, (left, right) = plt.subplots(1, 2, figsize=(11.0, 4.9))

    # Left: the fold, one step line per committed sequence.
    crate = RECORDING["lifecycle"][0]
    edge = RECORDING["lifecycle"][1]
    labelled = set()
    for entry, colour, label in (
        (crate, CRATE_COLOUR, "crates/braid's test — two opened, two closed, then a stray close"),
        (edge, EDGE_COLOUR, "runners/agent's edge — one opened, one closed"),
    ):
        values = [0] + entry["open_missions"]
        xs = list(range(len(values)))
        left.plot(xs, values, drawstyle="steps-post", color=colour, linewidth=1.8, label=label)
        left.plot(xs, values, "o", color=colour, markersize=4.5)
        for x, value in zip(xs, values):
            if (x, value) in labelled:
                continue
            labelled.add((x, value))
            left.text(x, value + 0.09, str(value), ha="center", va="bottom", color=colour, fontsize=9)
    left.set_xticks(range(len(crate["open_missions"]) + 1))
    left.set_xticklabels(["start"] + [f"e{i}" for i in range(1, len(crate["open_missions"]) + 1)])
    left.set_ylim(0, 2.55)
    left.set_yticks([0, 1, 2])
    left.set_ylabel("open_missions in the folded state", fontsize=9.5)
    left.set_title(
        "The fold: `open_missions` counts the events,\nand a stray close does not wrap it",
        fontsize=10.4,
        color=INK,
        pad=10,
        linespacing=1.4,
    )
    left.legend(frameon=False, fontsize=8.2, labelcolor=INK, loc="upper left")
    left.text(
        0.0,
        -0.16,
        "`e1`..`e5` are each series' own events, in order:\n"
        "the crate's test folds open m1, open m2, close m1, close m2\n"
        "and a stray close; the edge's test folds open then close.\n"
        "Every point drawn is a value the test asserts.",
        transform=left.transAxes,
        fontsize=8.2,
        color=MUTED,
        linespacing=1.6,
        va="top",
    )

    # Right: the pace gate, in units of the pace window.
    pace = RECORDING["pace"]
    windows = pace["constants"]["stale_windows"]
    right.add_patch(Rectangle((0.05, 0.0), windows, 1.0, facecolor=GREEN, alpha=0.10, edgecolor="none"))
    right.add_patch(
        Rectangle((windows, 0.0), 200.0, 1.0, facecolor=SAND, alpha=0.10, edgecolor="none")
    )
    right.axvline(1.0, color=RULE, linewidth=1.1)
    right.axvline(windows, color=SAND, linewidth=1.4)
    right.text(0.85, 0.985, "the window (1×)", color=MUTED, ha="right", va="top", fontsize=8.4)
    right.text(
        7.6,
        0.985,
        f"stale window ({windows}×)",
        color=SAND,
        ha="right",
        va="top",
        fontsize=8.4,
    )
    right.text(
        0.02,
        0.05,
        "hold — publish while the newest\nbelief commit is inside the window",
        transform=right.transAxes,
        color=GREEN,
        ha="left",
        va="bottom",
        fontsize=8.4,
        linespacing=1.6,
    )
    right.text(
        8.6,
        0.05,
        "stale — publish anyway,\nand log it",
        color=SAND,
        ha="left",
        va="bottom",
        fontsize=8.4,
        linespacing=1.6,
    )
    for entry, y, colour in zip(pace["observations"], (0.62, 0.34), (SAND, INK)):
        low = entry["lag_ms_low"] / entry["pace_ms"]
        high = max(entry["lag_ms_high"], entry["lag_ms_low"]) / entry["pace_ms"]
        right.plot([low, high], [y, y], color=colour, linewidth=3.0, solid_capstyle="butt")
        right.plot([low], [y], "|", color=colour, markersize=12)
        span = f"{low:.0f} windows" if low == high else f"{low:.0f}–{high:.0f} windows"
        ms = f"{entry['lag_ms_low']:,}" if low == high else f"{entry['lag_ms_low']:,}–{entry['lag_ms_high']:,}"
        right.text(
            low * 0.9,
            y,
            f"{entry['pace_ms']} ms → {ms} ms ({span})",
            color=colour,
            ha="right",
            va="center",
            fontsize=8.4,
        )
    right.set_xscale("log")
    right.set_xlim(0.1, 100.0)
    right.set_ylim(0.0, 1.0)
    right.set_yticks([])
    right.set_xlabel("belief lag, in units of QUALIA_BELIEF_PACE_MS (log)", fontsize=9.5)
    right.set_title(
        "The pace gate: navigation slows, it does not deadlock", fontsize=10.4, color=INK, pad=10
    )
    right.text(
        0.0,
        -0.16,
        "Both observations are `runners/map` tests:\n"
        "a 10 s-old tick under the 250 ms default publishes\n"
        "stale; a 300 ms tick under 200 ms waits to 1,600 ms.",
        transform=right.transAxes,
        fontsize=8.2,
        color=MUTED,
        linespacing=1.6,
        va="top",
    )
    for ax in (left, right):
        ax.spines["top"].set_visible(False)
        ax.spines["right"].set_visible(False)
        ax.tick_params(length=0)
    fig.subplots_adjust(left=0.075, right=0.985, top=0.80, bottom=0.30, wspace=0.20)
    save(fig, HERE, "mission-lifecycle")


def strand_flow() -> None:
    fig, ax = plt.subplots(figsize=(11.5, 8.2))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.012, 0.978, "Every strand reports into one place", fontsize=13, color=INK)
    ax.text(
        0.012,
        0.948,
        "Three owners report — one in this process, two across the edge; the braid stores nothing, and `observe` is the only mutator.",
        fontsize=9,
        color=MUTED,
    )

    owners = (
        (
            "mission broker",
            "opens and closes a mission by\ncalling `observe` directly: it\nruns in this process.",
            GREEN,
        ),
        (
            "evidence recorder",
            "another process: it hands over\n`EvidenceSealed` with the\nsegment's digest, over the edge.",
            BLUE,
        ),
        (
            "JEPA runtime",
            "another process: it hands over\n`PromotionAccepted` /\n`PromotionRolledBack` on a move.",
            LAVENDER,
        ),
    )
    for index, (title, text, colour) in enumerate(owners):
        x = 0.012 + index * 0.328
        box(ax, x, 0.775, 0.310, 0.160, text, colour, title=title, fontsize=8.8)
    arrow(ax, (0.167, 0.775), (0.167, 0.712), GREEN)
    arrow(ax, (0.495, 0.775), (0.495, 0.732), BLUE)
    arrow(ax, (0.823, 0.775), (0.823, 0.732), LAVENDER)

    box(
        ax,
        0.030,
        0.585,
        0.300,
        0.125,
        "the sole mutator of `BraidState`",
        GREEN,
        title="observe(state, event)",
        fontsize=9.0,
    )
    box(
        ax,
        0.360,
        0.575,
        0.630,
        0.160,
        "frozen `BraidEvent` in, the `GET /braid` view out; a failed dispatch is\n"
        "503. Not deduplicated: the count is the events folded, so a re-delivered\n"
        "`mission_opened` counts twice.",
        INK,
        title="POST /braid — the write edge the two other-process strands cross",
        fontsize=9.0,
    )
    arrow(ax, (0.360, 0.6475), (0.332, 0.6475), GREEN)
    arrow(ax, (0.167, 0.585), (0.167, 0.400), GREEN, label="the fold")

    box(
        ax,
        0.360,
        0.405,
        0.630,
        0.145,
        "recovery's `Quarantined` is not a strand report: the edge refuses it with 400,\n"
        "and the dispatch (renaming `*.partial` files aside) stays in the crate's fold,\n"
        "over a local root — no strand, and no state moved.",
        SAND,
        title="the variant the edge refuses",
        fontsize=8.0,
    )
    arrow(ax, (0.675, 0.405), (0.720, 0.392), SAND, dashed=True)
    box(
        ax,
        0.060,
        0.225,
        0.420,
        0.170,
        "schema_version · generation · session_id\nopen_missions · last_promotion_ns ·\nlast_quarantine_ns — the fold, rebuildable\nby replaying the stream",
        SAND,
        title="BraidState (the agent's own copy)",
        fontsize=8.6,
    )
    box(
        ax,
        0.550,
        0.225,
        0.420,
        0.170,
        "MCAP: `quarantine_partials` moves the partials aside\nregistry: `route()` writes a rollback's reason, a handle the\nfold does not hold; a sealed segment is its own record.\nThe braid keeps none of them.",
        BLUE,
        title="the durable copies — where the records live",
        fontsize=8.6,
    )
    box(
        ax,
        0.060,
        0.035,
        0.910,
        0.125,
        "the console's Mission view, the TUI's Braid line and the operator page read it; "
        "`BeliefClock::decision_latency_ns()`\nreads the arena's own stamps instead — it is a view over the region, not a field of this state",
        LAVENDER,
        title="GET /braid — the read every strand and page polls",
        fontsize=8.8,
    )
    arrow(ax, (0.270, 0.225), (0.270, 0.165), SAND)
    arrow(ax, (0.730, 0.225), (0.730, 0.165), BLUE)

    fig.text(
        0.012,
        0.012,
        "Sources: crates/braid/src/lib.rs, crates/braid/src/rules.rs, runners/agent/src/{braid.rs,mission_control.rs},\n"
        "runners/agent/tests/{braid.rs,mission_broker.rs}, apps/qualia-console/tests/fixtures/braid-state.json.",
        fontsize=8.4,
        color=MUTED,
        linespacing=1.7,
    )
    fig.subplots_adjust(left=0.01, right=0.99, top=0.99, bottom=0.075)
    save(fig, HERE, "strand-flow")


def main() -> int:
    check_recording()
    lifecycle_chart()
    strand_flow()
    crate = RECORDING["lifecycle"][0]
    edge = RECORDING["lifecycle"][1]
    pace = RECORDING["pace"]
    print(
        f"figures: crate fold {crate['open_missions']}, edge fold {edge['open_missions']}, "
        f"pace {pace['constants']['pace_ms']} ms × {pace['constants']['stale_windows']} "
        f"(stale at {pace['constants']['pace_ms'] * pace['constants']['stale_windows']} ms), "
        "wrote mission-lifecycle, strand-flow"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
