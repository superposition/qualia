#!/usr/bin/env python3
"""Figures for the journal entry `the-fly-in-the-belief-matrices`.

    coupling-normalisation.svg   a chart, two panels: the factor `couple`
                                 applies per type, for the tests' fixture and
                                 for the committed artifact
    new-kernels.svg              a chart: the T16 capture's per-kernel totals,
                                 with the three kernels this epic added
    prior-path.svg               a diagram: the connectome artifact's path into
                                 the belief slots, behind an off-by-default flag

Data: `coupling-strength.json` (the two graph fixtures and the capture), read
against the committed artifacts they name — `assets/brain/prior/graph.bin` and
`docs/evidence/T16/three-kernels/kernels.csv` — and re-decoded here, so the
figures cannot draw a factor the committed data does not.

Run:  py -3.13 docs/figures/the-fly-in-the-belief-matrices/make_figures.py
"""

from __future__ import annotations

import csv
import hashlib
import json
import pathlib
import struct
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

RECORDING = json.loads((HERE / "coupling-strength.json").read_text(encoding="utf-8"))


def couple_factors(type_count: int, rowptr, cols, weights):
    """The crate's own arithmetic: in-strength per type, normalised by the peak.

    `CouplingPrior::couple` sums, for every edge, the weight into its column
    type, divides each type's sum by the largest, and multiplies the mapped
    belief slot by that factor. The returned `applied` is the sum of the
    factors, which is the total the operator log prints and the entry quotes.
    """
    in_strength = [0] * type_count
    for column, weight in zip(cols, weights):
        in_strength[column] += weight
    peak = max(in_strength, default=0)
    if peak == 0:
        return in_strength, 0, [0.0] * type_count, 0.0
    factors = [strength / peak for strength in in_strength]
    return in_strength, peak, factors, sum(factors)


def decode_artifact() -> tuple:
    """Decode `assets/brain/prior/graph.bin`, verifying the manifest's digests.

    This is the load path `CouplingPrior::load` takes: three little-endian
    sections named by `manifest.json`, each checked against its recorded
    SHA-256 before a value is read. A figure drawn from an artifact whose
    digests do not hold would be a figure of something else, so this refuses.
    """
    prior_dir = REPO / "assets" / "brain" / "prior"
    manifest = json.loads((prior_dir / "manifest.json").read_text(encoding="utf-8"))
    blob = (prior_dir / "graph.bin").read_bytes()
    type_count = manifest["type_count"]
    edge_count = manifest["edge_count"]
    rowptr_len = (type_count + 1) * 8
    cols_len = edge_count * 4
    if len(blob) != rowptr_len + 2 * cols_len:
        raise SystemExit(f"figure: graph.bin is {len(blob)} bytes, not the manifest's shape")
    sections = (
        ("rowptr", manifest["rowptr_sha256"], blob[:rowptr_len]),
        ("cols", manifest["cols_sha256"], blob[rowptr_len : rowptr_len + cols_len]),
        ("weights", manifest["weights_sha256"], blob[rowptr_len + cols_len :]),
    )
    for name, recorded, section in sections:
        if hashlib.sha256(section).hexdigest() != recorded:
            raise SystemExit(f"figure: graph.bin's {name} section does not match its digest")
    rowptr = struct.unpack(f"<{type_count + 1}Q", sections[0][2])
    cols = struct.unpack(f"<{edge_count}I", sections[1][2])
    weights = struct.unpack(f"<{edge_count}I", sections[2][2])
    return type_count, edge_count, rowptr, cols, weights


def check_recording() -> tuple:
    """Recompute both graphs and refuse a recording they do not support."""
    fixture = RECORDING["fixture"]
    fixture_factors = couple_factors(
        len(fixture["rowptr"]) - 1, fixture["rowptr"], fixture["cols"], fixture["weights"]
    )
    if abs(fixture_factors[3] - fixture["applied"]) > 1e-9:
        raise SystemExit(
            "figure: the fixture recording says %r, the graph folds to %r"
            % (fixture["applied"], fixture_factors[3])
        )

    type_count, edge_count, rowptr, cols, weights = decode_artifact()
    artifact_factors = couple_factors(type_count, rowptr, cols, weights)
    artifact = RECORDING["artifact"]
    if abs(artifact_factors[3] - artifact["applied"]) > 1e-9:
        raise SystemExit(
            "figure: the artifact recording says %r, graph.bin folds to %r"
            % (artifact["applied"], artifact_factors[3])
        )
    if abs(artifact_factors[3] / type_count - artifact["mean_per_type"]) > 1e-9:
        raise SystemExit("figure: the artifact's mean applied weight does not match its recording")
    return (fixture_factors, (type_count, edge_count, artifact_factors))


def read_capture() -> dict:
    """Per-kernel totals from the committed T16 capture, checked against it."""
    capture = RECORDING["capture"]
    rows = list(csv.DictReader((REPO / capture["path"]).read_text(encoding="utf-8").splitlines()))
    totals: dict = {}
    for row in rows:
        totals[row["kernel_name"]] = totals.get(row["kernel_name"], 0.0) + float(row["duration_us"])
    total = sum(totals.values())
    if len(rows) != capture["launches"] or len(totals) != capture["kernels"]:
        raise SystemExit("figure: the capture's launches or kernels differ from the recording")
    if abs(total - capture["total_us"]) > 1e-6:
        raise SystemExit(
            "figure: the capture recording says %r µs, the CSV sums to %r"
            % (capture["total_us"], total)
        )
    return totals


def coupling_chart(fixture_factors, artifact) -> None:
    type_count, edge_count, (in_strength, peak, factors, applied) = artifact

    fig, (left, right) = plt.subplots(1, 2, figsize=(10.4, 4.7))
    panels = (
        (
            left,
            fixture_factors,
            "the tests' fixture — 3 types, 4 edges\nin-strength 4, 8, 2",
        ),
        (
            right,
            (in_strength, peak, factors, applied),
            f"the committed artifact — {type_count} types, {edge_count} edges\n"
            f"in-strength {', '.join(str(value) for value in in_strength)}",
        ),
    )
    for ax, (strengths, _, panel_factors, panel_applied), title in panels:
        positions = list(range(len(panel_factors)))
        colours = [BLUE if factor == 1.0 else GREEN for factor in panel_factors]
        ax.bar(positions, panel_factors, 0.55, color=colours)
        for position, factor, strength in zip(positions, panel_factors, strengths):
            ax.text(
                position,
                factor + 0.03,
                f"{factor:.3f}".rstrip("0").rstrip("."),
                ha="center",
                va="bottom",
                color=INK,
                fontsize=9.5,
                fontweight="bold",
            )
            ax.text(
                position,
                0.02,
                f"w={strength}",
                ha="center",
                va="bottom",
                color=SLATE,
                fontsize=8.4,
            )
        ax.set_xticks(positions)
        ax.set_xticklabels([f"type {index}" for index in positions], fontsize=9.5)
        ax.set_ylim(0.0, 1.22)
        ax.set_yticks([0.0, 0.5, 1.0])
        ax.set_title(title, fontsize=10.5, color=INK, pad=10, linespacing=1.6)
        ax.text(
            0.98,
            0.96,
            f"applied = Σ factors = {panel_applied:.4g}",
            transform=ax.transAxes,
            ha="right",
            va="top",
            fontsize=9.4,
            color=SAND,
        )
        ax.spines["top"].set_visible(False)
        ax.spines["right"].set_visible(False)
        ax.tick_params(length=0)
    left.set_ylabel("coupling factor applied to the mapped slot", fontsize=9.5)
    left.text(
        0.0,
        -0.16,
        "`w` is the summed weight of the edges\n"
        "ending on the type. The blue bar is the peak\n"
        "type, which couples at unit weight.",
        transform=left.transAxes,
        fontsize=8.4,
        color=MUTED,
        linespacing=1.6,
        va="top",
    )
    right.text(
        0.0,
        -0.16,
        "The artifact's mean applied weight (2.7879 / 5\n"
        "= 0.5576) is the `uncertainty_weight`\n"
        "`runners/explore` sends the planner.",
        transform=right.transAxes,
        fontsize=8.4,
        color=MUTED,
        linespacing=1.6,
        va="top",
    )
    fig.subplots_adjust(left=0.075, right=0.985, top=0.78, bottom=0.30, wspace=0.22)
    save(fig, HERE, "coupling-normalisation")


def kernels_chart(totals: dict) -> None:
    capture = RECORDING["capture"]
    new_kernels = set(capture["new_kernels"])
    ordered = sorted(totals.items(), key=lambda item: item[1])
    names = [name for name, _ in ordered]
    values = [value for _, value in ordered]
    colours = [GREEN if name in new_kernels else SLATE for name in names]

    fig, ax = plt.subplots(figsize=(9.6, 4.6))
    positions = list(range(len(names)))
    ax.barh(positions, values, 0.55, left=1e-3, color=colours)
    for position, value in zip(positions, values):
        shown = f"{value:,.0f}" if value >= 100 else f"{value:.3g}"
        ax.text(
            value * 1.2, position, f"{shown} µs", va="center", ha="left", color=INK, fontsize=9.4
        )
    ax.set_xscale("log")
    ax.set_xlim(1e-3, 3e4)
    ax.set_xticks([1e-3, 1e-2, 1e-1, 1.0, 10.0, 100.0, 1000.0, 10000.0])
    ax.set_xticklabels(["0.001", "0.01", "0.1", "1", "10", "100", "1,000", "10,000"])
    ax.set_yticks(positions)
    ax.set_yticklabels(names, fontsize=9.6)
    ax.set_xlabel("kernel time in the capture, µs per kernel (log)", fontsize=9.5)
    ax.set_title(
        "The T16 capture on the RTX 4090: 16 launches over 8 kernels",
        fontsize=12,
        color=INK,
        pad=10,
    )
    ax.text(
        0.99,
        0.06,
        "\n".join(
            [
                f"three new kernels (green) = {sum(totals[name] for name in new_kernels):.3f} µs",
                f"of {capture['total_us']:.3f} µs",
                f"baseline: {capture['baseline']['launches']} launches / "
                f"{capture['baseline']['kernels']} kernels, {capture['baseline']['total_us']:.3f} µs",
                "duration second; the 4090 is shared",
            ]
        ),
        transform=ax.transAxes,
        ha="right",
        va="bottom",
        fontsize=8.6,
        color=MUTED,
        linespacing=1.5,
    )
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    ax.tick_params(length=0)
    fig.subplots_adjust(left=0.155, right=0.975, top=0.86, bottom=0.20)
    save(fig, HERE, "new-kernels")


def prior_path() -> None:
    fig, ax = plt.subplots(figsize=(10.6, 8.0))
    ax.set_xlim(0, 1)
    ax.set_ylim(0, 1)
    ax.axis("off")

    ax.text(0.012, 0.975, "The fly's path into the belief slots", fontsize=13, color=INK)
    ax.text(
        0.012,
        0.944,
        "One stage a row; the column on the right is what the stage refuses, or does not do.",
        fontsize=9,
        color=MUTED,
    )

    stages = (
        ("Male CNS v1.0 (CC-BY-4.0): read for its graph; no file is copied", BLUE, "dataset"),
        (
            "assets/brain/prior: graph.bin + manifest.json — 5 types, 9 edges",
            LAVENDER,
            "artifact (committed)",
        ),
        (
            "three SHA-256 sections and the length the counts imply, checked before decode",
            SAND,
            "CouplingPrior::load",
        ),
        (
            "in-strength(type) / peak → factor in (0, 1]; the strongest type couples at 1.0",
            BLUE,
            "normalise",
        ),
        (
            "slot t ×= factor(type t), host-side; returns Σ factors = 1.75 on the fixture",
            GREEN,
            "couple(belief, slots)",
        ),
        (
            "the slots the panel reads: STATE_DIM 1024 × 8 layers, coupling applied last",
            LAVENDER,
            "belief matrices",
        ),
    )
    top, height, gap = 0.895, 0.115, 0.034
    centres = []
    for index, (text, colour, title) in enumerate(stages):
        y = top - index * (height + gap) - height
        box(ax, 0.030, y, 0.560, height, text, colour, title=title, fontsize=9.4)
        centres.append(y + height / 2)
        if index:
            arrow(ax, (0.310, y + height + gap), (0.310, y + height), MUTED)

    notes = (
        (
            1,
            "Built by crates/connectome-prior\nfrom the dataset's graph; the\nartifact is committed, not rebuilt.",
            GREEN,
        ),
        (
            2,
            "Edited or truncated data is refused\nhere, before a value is decoded:\nthe load fails closed.",
            SAND,
        ),
        (
            3,
            "flag `fly-prior` is off by default —\n`couple_prior` returns Ok(0.0) and\nthe loop has no branch for it.",
            MUTED,
        ),
        (
            4,
            "Host-side data (D-001), so no kernel\nis launched. An unmappable type or\nslot is refused, never half-applied.",
            INK,
        ),
    )
    for index, text, colour in notes:
        centre = centres[index]
        box(ax, 0.620, centre - 0.075, 0.365, 0.150, text, colour, fontsize=7.6)
        arrow(ax, (0.620, centre), (0.590, centre), colour, dashed=True)

    fig.text(
        0.012,
        0.022,
        "crates/fly-circuit (feature `sim`, also off by default) is a separate invented rate model over the same graph: it publishes\n"
        "FlySimPayload, 65,984 bytes, under SHM_VERSION 3, and carries no coupling gain. Neither path reaches a motor.\n"
        "Sources: crates/jepa/src/prior.rs, crates/jepa/tests/prior.rs, crates/cuda/src/{lib.rs,cuda_impl.rs}, crates/metal/src/lib.rs,\n"
        "assets/brain/prior/{manifest.json,graph.bin}, runners/explore/src/main.rs, docs/decisions.md (D-001).",
        fontsize=8.4,
        color=MUTED,
        linespacing=1.7,
    )
    fig.subplots_adjust(left=0.01, right=0.99, top=0.99, bottom=0.105)
    save(fig, HERE, "prior-path")


def main() -> int:
    fixture_factors, artifact = check_recording()
    totals = read_capture()
    coupling_chart(fixture_factors, artifact)
    kernels_chart(totals)
    prior_path()
    type_count, edge_count, (in_strength, peak, factors, applied) = artifact
    print(
        f"figures: fixture applied {fixture_factors[3]:.4g} (types {len(fixture_factors[2])}), "
        f"artifact {type_count} types / {edge_count} edges, in-strength {in_strength}, peak {peak}, "
        f"applied {applied:.6g}, mean {applied / type_count:.6g}; "
        f"T16 {sum(totals.values()):.3f} µs over {len(totals)} kernels; "
        "wrote coupling-normalisation, new-kernels, prior-path"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
