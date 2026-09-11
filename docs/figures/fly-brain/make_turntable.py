#!/usr/bin/env python3
"""Build the fly-brain turntable GLB with Blender, headless and deterministic.

The T43 pattern: one GLB per journal entry, the layout's nodes and edges plus a
60-frame rotation baked as an animation, so the entry can embed one asset with
`<model-viewer … auto-rotate>` and no player. `assets/mark/build_mark.py` is the
precedent for the pipeline — Blender is driven from the command line, nothing is
stamped into the file, and the geometry is a pure function of committed inputs.

    blender --background --python docs/figures/fly-brain/make_turntable.py

Reads `assets/brain/layout.json`, `assets/brain/prior/` and `firing-sample.json`
(the recording `apps/qualia-console/examples/brain_evidence.rs` writes); writes
`turntable.glb` beside this script.

Large priors are capped: the turntable is a figure, and a million-sphere GLB is
not one. The cap and the counts actually written are printed.
"""

from __future__ import annotations

import json
import math
import pathlib
import struct
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[2]
LAYOUT_PATH = REPO / "assets" / "brain" / "layout.json"
PRIOR_DIR = REPO / "assets" / "brain" / "prior"
SAMPLE_PATH = HERE / "firing-sample.json"
GLB_PATH = HERE / "turntable.glb"

FRAMES = 60
MAX_NODES = 512
MAX_EDGES = 2048

NODE_BASE_RADIUS = 0.018
NODE_RATE_RADIUS = 0.045
EDGE_RADIUS = 0.004

# The mark's palette, sRGB.
INK = "#edf0f5"
ACCENT = "#91dbba"
MUTED = "#5c6673"


def srgb_to_linear(value: float) -> float:
    return ((value + 0.055) / 1.055) ** 2.4


def hex_to_linear(text: str) -> tuple[float, float, float]:
    digits = text.lstrip("#")
    return tuple(srgb_to_linear(int(digits[i : i + 2], 16) / 255.0) for i in (0, 2, 4))


def mix(a: tuple[float, float, float], b: tuple[float, float, float], t: float):
    t = min(max(t, 0.0), 1.0)
    return tuple(a[i] * (1.0 - t) + b[i] * t for i in range(3))


def read_edges():
    manifest = json.loads((PRIOR_DIR / "manifest.json").read_text(encoding="utf-8"))
    type_count = int(manifest["type_count"])
    edge_count = int(manifest["edge_count"])
    graph = (PRIOR_DIR / "graph.bin").read_bytes()
    rowptr = list(struct.unpack_from(f"<{type_count + 1}Q", graph, 0))
    offset = (type_count + 1) * 8
    cols = list(struct.unpack_from(f"<{edge_count}I", graph, offset))
    weights = list(struct.unpack_from(f"<{edge_count}I", graph, offset + edge_count * 4))
    edges = []
    for source in range(type_count):
        for edge in range(rowptr[source], rowptr[source + 1]):
            edges.append((source, cols[edge], weights[edge]))
    return type_count, edge_count, edges


def main() -> int:
    import bpy
    from mathutils import Vector

    layout = json.loads(LAYOUT_PATH.read_text(encoding="utf-8"))
    sample = json.loads(SAMPLE_PATH.read_text(encoding="utf-8"))
    type_count, edge_count, edges = read_edges()
    positions = layout["nodes"]
    rates = sample["rates"]
    peak = max((abs(rate) for rate in rates), default=0.0) or 1.0
    max_weight = max((weight for _, _, weight in edges), default=1) or 1

    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.frame_start = 1
    scene.frame_end = FRAMES

    muted = hex_to_linear(MUTED)
    accent = hex_to_linear(ACCENT)
    ink = hex_to_linear(INK)

    parent = bpy.data.objects.new("turntable", None)
    scene.collection.objects.link(parent)

    def material(name: str, colour, strength: float):
        mat = bpy.data.materials.new(name)
        mat.use_nodes = True
        principled = mat.node_tree.nodes["Principled BSDF"]
        principled.inputs["Base Color"].default_value = (*colour, 1.0)
        principled.inputs["Roughness"].default_value = 0.45
        principled.inputs["Emission Color"].default_value = (*colour, 1.0)
        principled.inputs["Emission Strength"].default_value = strength
        return mat

    parts = []
    nodes_written = 0
    for index in range(min(type_count, MAX_NODES)):
        rate = abs(rates[index]) / peak if index < len(rates) else 0.0
        intensity = min(max(rate, 0.0), 1.0)
        location = positions[index]
        bpy.ops.mesh.primitive_uv_sphere_add(
            segments=12,
            ring_count=8,
            radius=NODE_BASE_RADIUS + NODE_RATE_RADIUS * intensity,
            location=Vector(location),
        )
        sphere = bpy.context.active_object
        sphere.name = f"node-{index}"
        sphere.data.materials.append(material(f"node-{index}-mat", mix(muted, accent, intensity), 1.5 + 4.0 * intensity))
        parts.append(sphere)
        nodes_written += 1

    edges_written = 0
    stride = max(len(edges) // MAX_EDGES, 1)
    edge_material = material("edge-mat", muted, 0.4)
    for edge_index, (source, destination, weight) in enumerate(edges):
        if edge_index % stride != 0 or edges_written >= MAX_EDGES:
            continue
        start = Vector(positions[source])
        end = Vector(positions[destination])
        direction = end - start
        length = direction.length
        if length <= 0.0:
            continue
        flux = min(weight * abs(rates[source]) / (peak * max_weight), 1.0) if source < len(rates) else 0.0
        material_for_edge = material(
            f"edge-{edge_index}-mat", mix(muted, accent, flux), 0.4 + 3.0 * flux
        )
        bpy.ops.mesh.primitive_cylinder_add(
            vertices=6,
            radius=EDGE_RADIUS * (1.0 + 2.0 * flux),
            depth=length,
            location=(start + end) / 2.0,
        )
        cylinder = bpy.context.active_object
        cylinder.name = f"edge-{edge_index}"
        cylinder.rotation_mode = "QUATERNION"
        cylinder.rotation_quaternion = Vector((0.0, 0.0, 1.0)).rotation_difference(direction)
        cylinder.data.materials.append(material_for_edge)
        parts.append(cylinder)
        edges_written += 1

    # One mesh rather than hundreds of objects: the figure is a turntable, and a
    # single body with one material slot per node and per edge is what a viewer
    # wants to draw. (The exporter is not byte-stable either way; see the README.)
    bpy.ops.object.select_all(action="DESELECT")
    for part in parts:
        part.select_set(True)
    bpy.context.view_layer.objects.active = parts[0]
    bpy.ops.object.join()
    body = bpy.context.active_object
    body.name = "brain"
    body.parent = parent

    # The baked turntable: one full turn over FRAMES, linear so it loops evenly.
    parent.rotation_mode = "XYZ"
    for frame in range(1, FRAMES + 1):
        parent.rotation_euler = (0.0, 0.0, math.tau * (frame - 1) / FRAMES)
        parent.keyframe_insert(data_path="rotation_euler", index=2, frame=frame)
    for fcurve in parent.animation_data.action.fcurves:
        for keyframe in fcurve.keyframe_points:
            keyframe.interpolation = "LINEAR"

    bpy.ops.export_scene.gltf(
        filepath=str(GLB_PATH),
        export_format="GLB",
        export_animations=True,
        export_apply=True,
        export_yup=True,
    )
    if not GLB_PATH.exists():
        raise SystemExit(f"turntable: Blender wrote no {GLB_PATH}")

    print(
        f"turntable: {nodes_written} nodes, {edges_written} edges, "
        f"{FRAMES} frames -> {GLB_PATH} ({GLB_PATH.stat().st_size} bytes); "
        f"prior {type_count} types / {edge_count} edges, peak rate {peak:.4f}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
