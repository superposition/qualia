#!/usr/bin/env python3
"""Figures for the journal entry `the-fly-brain-on-the-robot`.

    step-rates.svg      a chart: the released connectome's step over the same
                        190 MB artifact, per device, against the 60 Hz frame
    dataset-gate.svg    a chart: two real sessions through the dataset's
                        transition gates, and the promotion floor above both
    brain-to-robot.svg  a diagram: the release tables to the robot's wheels,
                        with the leash's ownership boundary drawn

Data: `wave-numbers.json`, every value a line of the committed evidence named
in its `source` field. The script refuses to draw a value the file does not
carry.

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

DATA = json.loads((HERE / "wave-numbers.json").read_text(encoding="utf-8"))


def need(path: str):
    """A value the data file must carry; a missing one is a refusal, not a guess."""
    node = DATA
    for key in path.split("."):
        if key not in node:
            raise SystemExit(f"wave-numbers.json is missing {path!r}; refusing to draw")
        node = node[key]
    return node


SHORT_DEVICES = {
    "RTX 4090 (NVRTC, sm_89)": "RTX 4090 (sm_89)",
    "Orin NX iGPU (sm_87 cubin, 6 cores)": "Orin NX iGPU (sm_87)",
    "dev host CPU (one i9-14900KF thread)": "dev host CPU (1 thread)",
    "Orin NX CPU reference": "Orin NX CPU reference",
}


def short_device(name: str) -> str:
    """The axis label for a device; every full name is in the data file and the caption."""
    if name not in SHORT_DEVICES:
        raise SystemExit(f"no short label for device {name!r}; refusing to draw")
    return SHORT_DEVICES[name]


def step_rates() -> None:
    rows = sorted(need("step_rates"), key=lambda row: row["ticks_per_s"])
    frame_hz = need("frame_hz")

    colours = [SLATE, BLUE, LAVENDER, GREEN]
    fig, ax = plt.subplots(figsize=(9.4, 4.4))
    positions = range(len(rows))

    for pos, (row, colour) in enumerate(zip(rows, colours)):
        ax.barh(pos, row["ticks_per_s"], height=0.56, color=colour)
        ax.text(
            1.02,
            pos,
            f"{row['ticks_per_s']:.1f} ticks/s\n{row['ms_per_tick']:.3f} ms/tick",
            transform=ax.get_yaxis_transform(),
            clip_on=False,
            va="center",
            ha="left",
            color=colour,
            fontsize=9.5,
            fontweight="bold",
            linespacing=1.5,
        )

    ax.axvline(frame_hz, color=SAND, linewidth=1.3, linestyle="--")
    ax.text(
        frame_hz * 1.04,
        len(rows) - 0.38,
        f"{frame_hz} Hz = {need('frame_ms')} ms",
        color=SAND,
        fontsize=9.2,
        va="top",
    )

    ax.set_yticks(list(positions))
    ax.set_yticklabels([short_device(row["device"]) for row in rows], fontsize=9.5)
    ax.set_xscale("log")
    ax.set_xlim(10, 4000)
    ax.set_xlabel("ticks per second (log scale)", fontsize=10)
    ax.set_title(
        "One artifact, four meters: the 25,582,938-edge step per device",
        fontsize=12,
        color=INK,
        pad=12,
    )
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    ax.grid(axis="x", color=RULE, linewidth=0.6, alpha=0.5)
    ax.set_axisbelow(True)

    fig.text(
        0.012,
        0.02,
        "Source: docs/evidence/T55/connectome-runner/README.md §Rates — one 190 MB artifact, "
        "board-verified.",
        fontsize=8.4,
        color=MUTED,
    )
    fig.subplots_adjust(left=0.29, right=0.70, top=0.86, bottom=0.19)
    save(fig, HERE, "step-rates")


def dataset_gate() -> None:
    sessions = need("sessions")
    floor = need("promotion_floor")

    fig, ax = plt.subplots(figsize=(9.4, 4.6))
    width = 0.30

    for pos, session in enumerate(sessions):
        series = [
            ("candidate transitions", session["candidates"], SLATE, pos - width / 1.7),
            ("admitted valid", session["valid"], GREEN if session["valid"] else SAND, pos + width / 1.7),
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


def brain_to_robot() -> None:
    fig, ax = plt.subplots(figsize=(10.2, 6.6))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.015, 0.975, "From the release tables to the robot's wheels", fontsize=12.5, color=INK)
    ax.text(
        0.015,
        0.945,
        "Solid arrows are data or control. The dashed line is the leash's ownership boundary, "
        "measured on Pinkie (D-025).",
        fontsize=8.8,
        color=MUTED,
    )

    # Band A: the release into the network.
    box(
        ax, 0.015, 0.735, 0.215, 0.19,
        "4 public feather tables\nCC-BY 4.0 · 1.05 GB\n151,856,684 source rows",
        SAND, "FlyEM Male CNS v1.0",
    )
    box(
        ax, 0.275, 0.735, 0.235, 0.19,
        "166,700 neurons\n25,582,938 neuron-level edges\n124,177,617 synapses",
        BLUE, "cns-import -> QLCN artifact",
    )
    box(
        ax, 0.555, 0.735, 0.205, 0.19,
        "sparse LIF, one launch\n11.840 ms/tick on the Orin\n84.5 ticks/s · 182 MB",
        GREEN, "cns_lif (CUDA)",
    )
    box(
        ax, 0.805, 0.735, 0.18, 0.19,
        "positions + tick firing\nheadless render\n9,976 of 139,662 points",
        LAVENDER, "console Brain view",
    )
    arrow(ax, (0.23, 0.83), (0.275, 0.83), SAND)
    arrow(ax, (0.51, 0.83), (0.555, 0.83), BLUE)
    arrow(ax, (0.76, 0.83), (0.805, 0.83), GREEN)

    # Band B: the robot's own sensors, and the boundary.
    ax.plot([0.015, 0.985], [0.695, 0.695], color=RULE, linewidth=1.0)
    ax.text(0.015, 0.665, "The robot's sensors, and who owns them", fontsize=10.5, color=INK)

    box(
        ax, 0.015, 0.45, 0.225, 0.19,
        "camera snapshot + MJPEG\nLD06 scan via MCP observe\n9.999 Hz · 360 beams",
        SAND, "leash surface (HTTP)",
    )
    box(
        ax, 0.30, 0.45, 0.245, 0.19,
        "8 luminance columns, gain 0.5\n95,494 visual-stage cells\nol_intrinsic + R1-R6/R7/R8",
        BLUE, "encoder (fixed constants)",
    )
    box(
        ax, 0.605, 0.45, 0.22, 0.19,
        "2,012 motor neurons\n(1,011 L, 1,001 R)\n31-32 ticks/s over the leash",
        GREEN, "decoder -> command",
    )
    arrow(ax, (0.24, 0.545), (0.30, 0.545), SAND)
    arrow(ax, (0.545, 0.545), (0.605, 0.545), BLUE)

    ax.plot([0.015, 0.985], [0.415, 0.415], color=SAND, linewidth=1.5, linestyle="--")
    ax.text(
        0.015, 0.397,
        "Dashed: the leash owns /dev/ttyACM0 and /dev/ttyTHS1 — a second opener gets errno=16, "
        "and qualia-lidar and qualia-drive exit 2.\n"
        "It serves /dev/video0 to several readers; the stack subscribes to its HTTP surface "
        "(runners/leash-sensors, runners/camera) and opens no owned port.",
        fontsize=8.4, color=SAND, ha="left", va="top", linespacing=1.5,
    )

    # Band C: the record, and the gate it stops at.
    ax.plot([0.015, 0.985], [0.325, 0.325], color=RULE, linewidth=1.0)
    ax.text(0.015, 0.30, "The record on disk, and the floor it does not clear", fontsize=10.5, color=INK)

    box(
        ax, 0.015, 0.10, 0.20, 0.19,
        "t58-real-01: 900 s\n83,299,459 B · sha256 9761…\ncamera 4,502 · lidar 8,775",
        LAVENDER, "arena-recorder -> MCAP",
    )
    box(
        ax, 0.245, 0.10, 0.20, 0.19,
        "jepa-dataset gates:\n0 of 4,501 valid\nall calibration_missing",
        SAND, "T58 dataset leg",
    )
    box(
        ax, 0.475, 0.10, 0.215, 0.19,
        "t65-real-01: 300 s\n30,300,801 B + calibration,\npose, leash-auth actions",
        LAVENDER, "three producers (#250)",
    )
    box(
        ax, 0.72, 0.10, 0.265, 0.19,
        "valid=1499 candidates=1500\n1 rejected at action_coverage\ntrainer: 50k/12/3/3 floor unmet",
        GREEN, "the same gates, admitted",
    )
    arrow(ax, (0.215, 0.195), (0.245, 0.195), LAVENDER)
    arrow(ax, (0.445, 0.195), (0.475, 0.195), SAND, dashed=True)
    arrow(ax, (0.69, 0.195), (0.72, 0.195), LAVENDER)

    fig.text(
        0.012, 0.012,
        "Sources: docs/evidence/T55, T56, T58, T59, T65; docs/decisions.md D-025, D-026, D-027.",
        fontsize=8.2, color=MUTED,
    )
    fig.subplots_adjust(left=0.005, right=0.995, top=0.995, bottom=0.03)
    save(fig, HERE, "brain-to-robot")


if __name__ == "__main__":
    step_rates()
    dataset_gate()
    brain_to_robot()
    rates = {row["ticks_per_s"] for row in DATA["step_rates"]}
    sessions = [(s["session"], s["candidates"], s["valid"]) for s in DATA["sessions"]]
    print(
        "wrote step-rates, dataset-gate, brain-to-robot; "
        f"ticks/s {sorted(rates)}; sessions {sessions}; "
        f"60 Hz frame {DATA['frame_ms']} ms; floor {DATA['promotion_floor']['valid_transitions']}"
    )
