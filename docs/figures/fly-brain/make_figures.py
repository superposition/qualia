#!/usr/bin/env python3
"""Figures for the journal entry `fly-brain` (T51).

    brain-layout.svg    a render: the prior in its committed spectral layout
    brain-firing.svg    a render: node intensity from the belief slots' activity
                        (per layer), edge pulses from weight × rate[source]
    brain-matrices.svg  a chart: the per-layer weight matrix and belief vector
                        as heatmaps, over a short time axis with the braid's
                        promotion markers

Data: `assets/brain/layout.json` and `assets/brain/prior/` (the committed prior
and its layout), and `firing-sample.json` — one recording taken through the
console's own sampling path by `apps/qualia-console/examples/brain_evidence.rs`,
which steps the fly rate model (`crates/fly-circuit`) with a synthetic drive and
publishes into a shared region, so the numbers here are the ones the view reads,
not a hand-drawn sketch. The node intensity is the belief slots' activity, which
in this recording is the example's evolving belief; live it is the runners'. The
edge pulses are the published fly rate vector, which stays flat until the fly
drive is wired (T30/T31).

Run:  python make_figures.py
"""

from __future__ import annotations

import json
import pathlib
import struct
import sys

import matplotlib.pyplot as plt
import numpy as np
from matplotlib.colors import LinearSegmentedColormap

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))

from _house import (  # noqa: E402
    BG,
    GREEN,
    INK,
    MUTED,
    PANEL,
    RULE,
    SAND,
    SLATE,
    save,
    use_style,
)

LAYOUT_PATH = REPO / "assets" / "brain" / "layout.json"
PRIOR_DIR = REPO / "assets" / "brain" / "prior"
SAMPLE_PATH = HERE / "firing-sample.json"

MANIFEST_FILE = "manifest.json"
GRAPH_FILE = "graph.bin"

# The console's node-intensity source (`views/brain/mod.rs::node_intensity`):
# type `t` reads layer `t % LAYERS`' belief mean at index `t / LAYERS`. These
# mirror `qualia_types::NUM_LAYERS` and `STATE_DIM`.
LAYERS = 8
BELIEF_DIM = 1024


def node_intensity(sample: dict, type_count: int) -> np.ndarray:
    """The scene's per-node intensity, from the belief slots (per layer)."""
    layers = {int(layer["layer"]): layer for layer in sample["layers"]}
    intensity = np.zeros(type_count, dtype=float)
    for node in range(type_count):
        reading = layers.get(node % LAYERS)
        if reading is None:
            continue
        belief = reading["belief"]
        index = (node // LAYERS) % BELIEF_DIM
        if index < len(belief):
            intensity[node] = abs(float(belief[index]))
    return intensity

# The console's own default camera, so the figure is the panel's view.
YAW = -0.6
PITCH = 0.5

# The console's ramp, on the console's dark canvas: idle is near the background,
# firing is the accent. Negative values (a belief mean can be signed) run to the
# sand end, so a sign change is visible rather than clipped to a bright zero.
FIRING_RAMP = LinearSegmentedColormap.from_list("brain-ramp", [BG, "#5c6673", GREEN])
SIGNED_RAMP = LinearSegmentedColormap.from_list("brain-signed", [SAND, BG, GREEN])


def read_prior() -> tuple[int, int, list[int], list[int], list[int]]:
    manifest = json.loads((PRIOR_DIR / MANIFEST_FILE).read_text(encoding="utf-8"))
    type_count = int(manifest["type_count"])
    edge_count = int(manifest["edge_count"])
    graph = (PRIOR_DIR / GRAPH_FILE).read_bytes()
    rowptr = list(struct.unpack_from(f"<{type_count + 1}Q", graph, 0))
    cols_offset = (type_count + 1) * 8
    cols = list(struct.unpack_from(f"<{edge_count}I", graph, cols_offset))
    weights = list(struct.unpack_from(f"<{edge_count}I", graph, cols_offset + edge_count * 4))
    return type_count, edge_count, rowptr, cols, weights


def edges_of(rowptr: list[int], cols: list[int], weights: list[int]):
    for source in range(len(rowptr) - 1):
        for edge in range(rowptr[source], rowptr[source + 1]):
            yield source, cols[edge], weights[edge]


def project(positions: np.ndarray) -> np.ndarray:
    """The console's orthographic camera, so the figure matches the panel."""
    sin_yaw, cos_yaw = np.sin(YAW), np.cos(YAW)
    x = positions[:, 0] * cos_yaw + positions[:, 2] * sin_yaw
    z = -positions[:, 0] * sin_yaw + positions[:, 2] * cos_yaw
    sin_pitch, cos_pitch = np.sin(PITCH), np.cos(PITCH)
    y = positions[:, 1] * cos_pitch - z * sin_pitch
    return np.stack([x, y], axis=1)


def heat(ax, values: np.ndarray, title: str) -> None:
    ax.set_title(title, color=INK, fontsize=9.0, fontfamily="DejaVu Sans Mono")
    scale = max(float(np.abs(values).max()), 1e-9)
    ax.imshow(values, cmap=SIGNED_RAMP, vmin=-scale, vmax=scale, interpolation="nearest", aspect="equal")
    ax.set_xticks([])
    ax.set_yticks([])
    for spine in ax.spines.values():
        spine.set_color(RULE)


def figure_layout(layout: dict, type_count: int, edge_count: int, rowptr, cols, weights) -> None:
    positions = np.asarray(layout["nodes"], dtype=float)
    screen = project(positions)
    degree = np.zeros(type_count, dtype=float)
    for source, destination, _ in edges_of(rowptr, cols, weights):
        degree[source] += 1.0
        degree[destination] += 1.0
    peak = max(degree.max(), 1.0)

    fig, ax = plt.subplots(figsize=(7.2, 4.8), dpi=110)
    fig.patch.set_facecolor(BG)
    ax.set_facecolor(PANEL)
    for source, destination, _ in edges_of(rowptr, cols, weights):
        ax.plot(
            [screen[source, 0], screen[destination, 0]],
            [screen[source, 1], screen[destination, 1]],
            color=SLATE,
            linewidth=0.8,
            zorder=1,
        )
    colours = [GREEN if value > 0 else MUTED for value in degree]
    sizes = 40.0 + 180.0 * (degree / peak)
    ax.scatter(screen[:, 0], screen[:, 1], c=colours, s=sizes, zorder=2, edgecolors=BG)
    for index in range(type_count):
        ax.annotate(
            str(index),
            (screen[index, 0], screen[index, 1]),
            color=INK,
            fontsize=8.0,
            fontfamily="DejaVu Sans Mono",
            xytext=(6, 4),
            textcoords="offset points",
        )
    ax.set_title(
        f"the prior in its committed layout — {type_count} types, {edge_count} edges "
        f"({layout['algorithm']}, seed {layout['seed']})",
        color=INK,
        fontsize=10.0,
        fontfamily="DejaVu Sans Mono",
    )
    ax.set_aspect("equal")
    ax.set_axis_off()
    save(fig, HERE, "brain-layout")


def figure_firing(layout: dict, sample: dict, rowptr, cols, weights) -> None:
    positions = np.asarray(layout["nodes"], dtype=float)
    screen = project(positions)
    rates = np.asarray(sample["rates"], dtype=float)
    node_count = min(len(positions), len(rates))
    # The edge pulses read the published fly rate vector; the nodes read the
    # belief slots. Two sources, two scales — the console's split exactly.
    rate_peak = max(float(np.abs(rates[:node_count]).max()), 1e-9)
    intensity = node_intensity(sample, len(positions))[:node_count]
    intensity_peak = max(float(intensity.max()), 1e-9)
    intensity = intensity / intensity_peak
    max_weight = max(max(weights), 1)

    fig, ax = plt.subplots(figsize=(7.2, 4.8), dpi=110)
    fig.patch.set_facecolor(BG)
    ax.set_facecolor(PANEL)
    for source, destination, weight in edges_of(rowptr, cols, weights):
        if source >= node_count or destination >= node_count:
            continue
        flux = min(weight * abs(rates[source]) / (rate_peak * max_weight), 1.0)
        ax.plot(
            [screen[source, 0], screen[destination, 0]],
            [screen[source, 1], screen[destination, 1]],
            color=(0.36 + 0.20 * flux, 0.42 + 0.44 * flux, 0.46 + 0.27 * flux),
            linewidth=0.8 + 3.2 * flux,
            zorder=1,
        )
    ax.scatter(
        screen[:node_count, 0],
        screen[:node_count, 1],
        c=intensity,
        cmap=FIRING_RAMP,
        vmin=0.0,
        vmax=1.0,
        s=60.0 + 420.0 * intensity,
        zorder=2,
        edgecolors=BG,
    )
    for index in range(node_count):
        ax.annotate(
            f"{index}  {intensity[index]:.3f}",
            (screen[index, 0], screen[index, 1]),
            color=INK,
            fontsize=8.0,
            fontfamily="DejaVu Sans Mono",
            xytext=(8, 5),
            textcoords="offset points",
        )
    ax.set_title(
        f"firing — nodes lit by belief activity (layer = type mod {LAYERS}); "
        f"edges pulse by weight x rate[source], peak {rate_peak:.3f}",
        color=INK,
        fontsize=10.0,
        fontfamily="DejaVu Sans Mono",
    )
    ax.set_aspect("equal")
    ax.set_axis_off()
    save(fig, HERE, "brain-firing")


def figure_matrices(sample: dict, history: list[dict], markers: list[dict]) -> None:
    layers = {layer["layer"]: layer for layer in sample["layers"]}
    layer = layers.get(0) or next(iter(layers.values()))
    dim = int(len(layer["weight"]) ** 0.5)
    weight = np.asarray(layer["weight"], dtype=float).reshape(dim, dim)
    belief = np.asarray(layer["belief"], dtype=float).reshape(dim, dim)

    fig = plt.figure(figsize=(7.2, 4.6), dpi=110)
    fig.patch.set_facecolor(BG)
    grid = fig.add_gridspec(2, 2, height_ratios=[3.0, 1.0], hspace=0.35, wspace=0.12)

    heat(fig.add_subplot(grid[0, 0]), weight, f"layer {layer['layer']} weight matrix (decimated)")
    heat(fig.add_subplot(grid[0, 1]), belief, f"layer {layer['layer']} belief mean (decimated)")

    ax = fig.add_subplot(grid[1, :])
    ax.set_facecolor(PANEL)
    layer_history = [point for point in history if point["layer"] == layer["layer"]]
    residuals = np.asarray([point["residual_norm"] for point in layer_history], dtype=float)
    peak = max(float(residuals.max()) if residuals.size else 0.0, 1e-9)
    ax.bar(
        np.arange(len(residuals)),
        residuals / peak,
        color=GREEN,
        width=0.8,
    )
    times = [point["timestamp_ns"] for point in layer_history]
    if times:
        first, last = min(times), max(times)
        span = max(last - first, 1)
        for marker in markers:
            stamp = marker["timestamp_ns"]
            if stamp < first or stamp > last:
                continue
            index = (stamp - first) / span * max(len(layer_history) - 1, 1)
            colour = GREEN if marker["kind"] == "PromotionAccepted" else (
                MUTED if marker["kind"] == "CouplingScale" else SAND
            )  # SAND: a partials quarantine
            ax.axvline(index, color=colour, linewidth=1.2, ymax=1.0)
            ax.annotate(
                marker["label"],
                (index, 1.02),
                color=colour,
                fontsize=7.5,
                fontfamily="DejaVu Sans Mono",
                rotation=90,
                va="bottom",
            )
    ax.set_title(
        f"layer {layer['layer']} residual over {len(layer_history)} frames",
        color=INK,
        fontsize=9.0,
        fontfamily="DejaVu Sans Mono",
    )
    ax.set_xticks([])
    ax.set_yticks([])
    for spine in ax.spines.values():
        spine.set_color(RULE)

    save(fig, HERE, "brain-matrices")


def main() -> int:
    use_style()
    layout = json.loads(LAYOUT_PATH.read_text(encoding="utf-8"))
    sample = json.loads(SAMPLE_PATH.read_text(encoding="utf-8"))
    type_count, edge_count, rowptr, cols, weights = read_prior()

    figure_layout(layout, type_count, edge_count, rowptr, cols, weights)
    figure_firing(layout, sample, rowptr, cols, weights)
    figure_matrices(sample, sample["history"], sample["markers"])

    intensity = node_intensity(sample, type_count)
    print(
        f"figures: {type_count} nodes, {edge_count} edges, "
        f"belief node peak {max(intensity, default=0.0):.4f}, "
        f"peak rate {max(abs(rate) for rate in sample['rates']):.4f}, "
        f"{len(sample['layers'])} layers, {len(sample['history'])} history frames, "
        f"{len(sample['markers'])} markers"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
