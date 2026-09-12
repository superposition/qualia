#!/usr/bin/env python3
"""Figures for the journal entry `the-4090-and-the-nano`.

    kernel-durations.svg   a chart, two panels: the same 13 launches' per-kernel
                           time on the RTX 4090 and on the Orin NX, and the
                           board's residency against its issued work per cycle
    deploy-path.svg        a diagram: host archive -> ship -> board build -> PTX
                           ceiling -> run and capture, with the dead ends

Data: the two committed captures — `docs/evidence/baseline-2026-09-11/` (the
RTX 4090 under nsys) and `docs/evidence/T50/pinkie-kernels/` (the Orin NX under
ncu). Both CSVs are read directly, one row per CUDA launch, and the suite totals
the README quotes are summed from them here.

Run:  py -3.13 docs/figures/the-4090-and-the-nano/make_figures.py
"""

from __future__ import annotations

import csv
import pathlib
import sys

import matplotlib.pyplot as plt

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BLUE,
    GREEN,
    INK,
    LAVENDER,
    MUTED,
    SAND,
    SLATE,
    arrow,
    box,
    save,
    use_style,
)

use_style()

HOST_CSV = REPO / "docs" / "evidence" / "baseline-2026-09-11" / "kernels.csv"
BOARD_CSV = REPO / "docs" / "evidence" / "T50" / "pinkie-kernels" / "kernels.csv"
HOST_TOTAL_US = 4784.753
BOARD_TOTAL_NS = 39182_176

HOST_COLOUR = BLUE
BOARD_COLOUR = SAND


def read_rows(path: pathlib.Path) -> list:
    return list(csv.DictReader(path.read_text(encoding="utf-8").splitlines()))


def host_kernels() -> dict:
    """Per-kernel launch count and mean duration, from the 4090's nsys capture."""
    kernels: dict = {}
    for row in read_rows(HOST_CSV):
        name = row["kernel_name"]
        entry = kernels.setdefault(name, {"launches": 0, "total_us": 0.0})
        entry["launches"] += 1
        entry["total_us"] += float(row["duration_us"])
    for entry in kernels.values():
        entry["mean_us"] = entry["total_us"] / entry["launches"]
    return kernels


def board_kernels(run: str = "base") -> dict:
    """Per-kernel launch count, mean duration and the two counters, from the board.

    Only the board's own `base` rows (ncu's default clock control) are read; the
    `--clock-control none` rows agree with them to 0.001 % in total and are the
    second run the T50 README reports rather than a second figure.
    """
    kernels: dict = {}
    for row in read_rows(BOARD_CSV):
        if row["run"] != run:
            continue
        name = row["kernel"]
        entry = kernels.setdefault(
            name, {"launches": 0, "total_ns": 0, "warps": 0.0, "throughput": 0.0}
        )
        entry["launches"] += 1
        entry["total_ns"] += int(row["duration_ns"])
        entry["warps"] += float(row["warps_active_pct_of_peak"])
        entry["throughput"] += float(row["sm_throughput_pct_of_peak"])
    for entry in kernels.values():
        entry["mean_us"] = entry["total_ns"] / entry["launches"] / 1000.0
        entry["warps"] /= entry["launches"]
        entry["throughput"] /= entry["launches"]
    return kernels


def check_totals(host: dict, board: dict) -> tuple:
    host_total = sum(entry["total_us"] for entry in host.values())
    board_total = sum(entry["total_ns"] for entry in board.values())
    if abs(host_total - HOST_TOTAL_US) > 1e-6:
        raise SystemExit(f"figure: the 4090 capture sums to {host_total}, not {HOST_TOTAL_US}")
    if board_total != BOARD_TOTAL_NS:
        raise SystemExit(f"figure: the board capture sums to {board_total}, not {BOARD_TOTAL_NS}")
    return host_total, board_total


def duration_chart() -> None:
    host = host_kernels()
    board = board_kernels()
    host_total, board_total = check_totals(host, board)

    names = sorted(board, key=lambda name: -board[name]["mean_us"])
    fig, (left, right) = plt.subplots(1, 2, figsize=(12.0, 4.9))

    positions = list(range(len(names)))
    height = 0.34
    for index, name in enumerate(names):
        for entry, offset, colour, label in (
            (host[name], height / 2, HOST_COLOUR, "RTX 4090 (nsys)"),
            (board[name], -height / 2, BOARD_COLOUR, "Orin NX (ncu)"),
        ):
            left.barh(
                index + offset,
                entry["mean_us"],
                height,
                left=1e-2,
                color=colour,
                label=label if index == 0 else None,
            )
            value = entry["mean_us"]
            shown = f"{value:,.0f}" if value >= 100 else f"{value:.3g}"
            left.text(
                value * 1.2,
                index + offset,
                f"{shown} µs",
                va="center",
                ha="left",
                color=colour,
                fontsize=8.4,
            )
    left.set_xscale("log")
    left.set_xlim(1e-2, 2e5)
    left.set_xticks([1e-2, 1e-1, 1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0])
    left.set_xticklabels(["0.01", "0.1", "1", "10", "100", "1,000", "10,000", "100,000"])
    left.set_yticks(positions)
    left.set_yticklabels(
        [f"{name} ×{host[name]['launches']}" for name in names], fontsize=9.4
    )
    left.set_xlabel("mean time per launch, µs (log)", fontsize=9.5)
    left.set_title(
        f"13 launches: {host_total:g} µs → {board_total / 1000:,.2f} µs",
        fontsize=10.6,
        color=INK,
        pad=10,
    )
    left.legend(frameon=False, fontsize=8.6, labelcolor=INK, loc="lower right")
    left.text(
        0.0,
        -0.16,
        "Each pair is one kernel's mean over its launches (the ×N on\n"
        "the label), summed from `docs/evidence/baseline-2026-09-11/\n"
        "kernels.csv` and `docs/evidence/T50/pinkie-kernels/kernels.csv`:\n"
        "4,784.753 µs and 39,182.176 µs, a ratio of 8.19×.",
        transform=left.transAxes,
        fontsize=8.2,
        color=MUTED,
        linespacing=1.6,
        va="top",
    )

    points: dict = {}
    for name in names:
        entry = board[name]
        key = (round(entry["warps"], 2), round(entry["throughput"], 2))
        points.setdefault(key, []).append(name)
    offsets = {
        "belief_update": 1.35,
        "cognition_update": 0.68,
        "costmap_stats": 1.35,
    }
    for (warps, throughput), group in points.items():
        right.plot(warps, throughput, "o", color=BOARD_COLOUR, markersize=7)
        right_aligned = warps > 40.0
        name = group[0]
        y = throughput * offsets.get(name, 1.5)
        right.text(
            warps - 1.6 if right_aligned else warps + 1.6,
            y,
            f"{', '.join(sorted(group))}\n{warps:.2f}% / {throughput:.2f}%",
            color=INK,
            fontsize=8.4,
            ha="right" if right_aligned else "left",
            va="center" if right_aligned else "bottom",
            linespacing=1.5,
        )
    belief = board["belief_update"]
    costmap = board["costmap_stats"]
    right.plot(
        [belief["warps"], costmap["warps"]],
        [belief["throughput"], costmap["throughput"]],
        color=GREEN,
        linewidth=1.2,
        linestyle="--",
    )
    right.annotate(
        "11× the issued work per cycle\nat almost the same residency",
        xy=((belief["warps"] + costmap["warps"]) / 2, 5.0),
        xytext=(1.5, 6.0),
        color=GREEN,
        fontsize=8.6,
        ha="left",
        va="center",
        arrowprops={"arrowstyle": "-|>", "color": GREEN, "linewidth": 1.2},
    )
    right.set_yscale("log")
    right.set_ylim(0.08, 60.0)
    right.set_xlim(0.0, 85.0)
    right.set_xticks([0, 10, 20, 30, 40, 50, 60, 70, 80])
    right.set_yticks([0.1, 1.0, 10.0])
    right.set_yticklabels(["0.1", "1", "10"])
    right.set_xlabel("warps active, % of peak sustained active", fontsize=9.5)
    right.set_ylabel("SM throughput, % of peak sustained elapsed (log)", fontsize=9.5)
    right.set_title("Residency is not the constraint; the loop is", fontsize=10.6, color=INK, pad=10)
    right.text(
        0.0,
        -0.16,
        "Counters from the same `kernels.csv` rows; each label\n"
        "reads warps active % / SM throughput %. Equal residency,\n"
        "11× the issued work per cycle.",
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
    fig.subplots_adjust(left=0.145, right=0.985, top=0.80, bottom=0.30, wspace=0.28)
    save(fig, HERE, "kernel-durations")


def deploy_path() -> None:
    fig, ax = plt.subplots(figsize=(11.4, 8.6))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.012, 0.978, "Getting the kernels onto the board", fontsize=13, color=INK)
    ax.text(
        0.012,
        0.948,
        "Stage and ship, because the board has no DNS and the host builds the target with cargo-zigbuild; a stage a row, notes on the right.",
        fontsize=9,
        color=MUTED,
    )

    stages = (
        ("host: archive a clean tree", "git archive HEAD | gzip > qualia.tar.gz", BLUE),
        ("host: ship it", "scp -i ~/.ssh/qualia_jetson_ed25519 → jetson@192.168.55.1", BLUE),
        (
            "board: unpack",
            "tar xzf; workspace `members` trimmed to cuda/types/shm; Cargo.lock relocked offline",
            LAVENDER,
        ),
        (
            "board: build natively",
            "cargo test -p qualia-cuda --features cuda --test gpu --no-run --release -j3 --offline",
            LAVENDER,
        ),
        (
            "board: the PTX ceiling, and the fix",
            "driver 540.4 JITs PTX ≤ 8.5, NVRTC 12.9 emits 8.8; the toolkit's compat libcuda loads it",
            SAND,
        ),
        (
            "board: run and capture",
            "5 passed / 0 failed; sudo ncu --section LaunchStats --section Occupancy → kernels.csv",
            GREEN,
        ),
    )
    top, height, gap = 0.885, 0.115, 0.030
    centres = []
    for index, (title, text, colour) in enumerate(stages):
        y = top - index * (height + gap) - height
        box(ax, 0.030, y, 0.585, height, text, colour, title=title, fontsize=9.2)
        centres.append(y + height / 2)
        if index:
            arrow(ax, (0.320, y + height + gap), (0.320, y + height), MUTED)

    notes = (
        (0, "Stage and ship: the aarch64 build runs here\n(cargo-zigbuild, or cross over Docker),\nso the board only has to run it.", SAND),
        (2, "No DNS on the board: git clone and\ncrates.io are unreachable, and cargo\nresolves every member even for one -p.", SAND),
        (3, "The board's registry cache is partial\nand older than main's lock, so cargo\nre-locks offline from what it has.", MUTED),
        (4, "Fallback, checked and not needed: a\ncubin from the board's own nvcc\n-arch=sm_87 -cubin loads on the driver.", BLUE),
        (5, "The board has no nsys and no mage, so\nthe capture is manual; Tegra exposes no\nDRAM counter, so dram__bytes.sum is n/a.", GREEN),
    )
    for index, text, colour in notes:
        centre = centres[index]
        box(ax, 0.640, centre - 0.075, 0.345, 0.150, text, colour, fontsize=7.6)
        arrow(ax, (0.640, centre), (0.615, centre), colour, dashed=True)

    fig.text(
        0.012,
        0.016,
        "Sources: docs/evidence/T50/pinkie-kernels/README.md (the manual capture and its two ncu runs), docs/decisions.md\n"
        "(D-010 Pinkie, D-012 the board is the profiling target, D-016/D-018 the stage-and-ship recipe).",
        fontsize=8.4,
        color=MUTED,
        linespacing=1.7,
    )
    fig.subplots_adjust(left=0.01, right=0.99, top=0.99, bottom=0.07)
    save(fig, HERE, "deploy-path")


def main() -> int:
    host = host_kernels()
    board = board_kernels()
    host_total, board_total = check_totals(host, board)
    duration_chart()
    deploy_path()
    ratio = board_total / 1000 / host_total
    print(
        f"figures: 4090 {len(host)} kernels / {sum(e['launches'] for e in host.values())} launches / "
        f"{host_total:.3f} µs; board {len(board)} kernels / "
        f"{sum(e['launches'] for e in board.values())} launches / {board_total / 1000:.3f} µs; "
        f"ratio {ratio:.2f}×; wrote kernel-durations, deploy-path"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
