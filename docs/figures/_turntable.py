#!/usr/bin/env python3
"""The shared turntable pipeline for the journal figures (step 43, T43).

One GLB per journal entry, the entry's extruded mark plus the element that
encodes its subject, with a 60-frame rotation baked as an animation, so the
entry embeds one asset with `<model-viewer … auto-rotate>` and no player. This
module is the pipeline the per-entry `make_turntable.py` scripts stand on, the
way they stand on `_house.py` for the flat figures; `assets/mark/build_mark.py`
(steps 40) is the pipeline's precedent and `docs/figures/fly-brain/` (T51) its
first use.

The mark is imported from `assets/mark/psi.glb` — the committed asset the entry
and its hero render already stand on — not rebuilt here, so the mark a
turntable draws cannot drift from the mark the site inlines. `build_mark.py`
extrudes along `+Z`, so the imported mark is stood up: its face into `XZ`, its
thickness along `Y`, and the whole scene rotates about `Z`.

The element is a ring of pillars, one per datum of the entry's own committed
data, height proportional to the datum's value. The per-entry script decides
what a pillar means and what its height is; this module only builds it.

Blender is the only tool this module needs, and it is invoked by path:

    blender --background --python docs/figures/<entry>/make_turntable.py
"""

from __future__ import annotations

import math
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
MARK_GLB = HERE.parents[1] / "assets" / "mark" / "psi.glb"

FRAMES = 60
PILLAR_WIDTH = 0.15
ELEMENT_RADIUS = 0.82
MIN_HEIGHT = 0.10
MAX_HEIGHT = 0.60


def srgb_to_linear(value: float) -> float:
    return ((value + 0.055) / 1.055) ** 2.4


def hex_to_linear(text: str) -> tuple[float, float, float]:
    digits = text.lstrip("#")
    return tuple(srgb_to_linear(int(digits[i : i + 2], 16) / 255.0) for i in (0, 2, 4))


def pillar_height(value: float, peak: float) -> float:
    """A datum's pillar height; a zero datum gets no pillar at all.

    A zero rendered as a minimum-height block would read as a small count, the
    opposite of what the datum says, so a zero value contributes nothing to the
    element and the run log names it.
    """
    if value <= 0.0 or peak <= 0.0:
        return 0.0
    return MIN_HEIGHT + (MAX_HEIGHT - MIN_HEIGHT) * (value / peak)


def build_element(mark_path: pathlib.Path, items, out_path: pathlib.Path, frames: int = FRAMES) -> dict:
    """Build the turntable and write it to `out_path`.

    `items` is one `(name, colour, value, label)` per pillar: `name` and
    `colour` label the mesh and its material, `value` scales the height against
    the largest value, and `label` is the value's caption for the run log.
    Returns the counts actually written.
    """
    import bpy

    peak = max((value for _, _, value, _ in items), default=1.0)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.frame_start = 1
    scene.frame_end = frames

    bpy.ops.import_scene.gltf(filepath=str(mark_path))
    imported = [obj for obj in scene.objects if obj.type == "MESH"]
    if not imported:
        raise SystemExit(f"turntable: {mark_path} imported no mesh")
    for obj in imported:
        obj.rotation_mode = "XYZ"
        obj.rotation_euler = (math.radians(90.0), 0.0, 0.0)
        obj.location = (0.0, 0.0, 0.0)

    def material(name: str, colour, strength: float):
        mat = bpy.data.materials.new(name)
        mat.use_nodes = True
        principled = mat.node_tree.nodes["Principled BSDF"]
        principled.inputs["Base Color"].default_value = (*colour, 1.0)
        principled.inputs["Roughness"].default_value = 0.35
        principled.inputs["Emission Color"].default_value = (*colour, 1.0)
        principled.inputs["Emission Strength"].default_value = strength
        return mat

    step = math.tau / len(items)
    pillars = []
    for index, (name, colour, value, _label) in enumerate(items):
        height = pillar_height(value, peak)
        if height <= 0.0:
            continue  # a zero datum leaves no pillar
        angle = step * index
        bpy.ops.mesh.primitive_cube_add(
            size=1.0,
            location=(
                ELEMENT_RADIUS * math.sin(angle),
                -ELEMENT_RADIUS * math.cos(angle),
                height / 2.0,
            ),
        )
        pillar = bpy.context.active_object
        pillar.name = name
        pillar.scale = (PILLAR_WIDTH, PILLAR_WIDTH, height)
        pillar.rotation_mode = "XYZ"
        pillar.rotation_euler = (0.0, 0.0, angle)
        pillar.data.materials.append(material(f"{name}-mat", hex_to_linear(colour), 2.0))
        pillars.append(pillar)

    # One full turn over `frames`, linear so it loops evenly.
    parent = bpy.data.objects.new("turntable", None)
    scene.collection.objects.link(parent)
    for obj in [*imported, *pillars]:
        if obj.parent is None:
            obj.parent = parent
    parent.rotation_mode = "XYZ"
    for frame in range(1, frames + 1):
        parent.rotation_euler = (0.0, 0.0, math.tau * (frame - 1) / frames)
        parent.keyframe_insert(data_path="rotation_euler", index=2, frame=frame)
    for fcurve in parent.animation_data.action.fcurves:
        for keyframe in fcurve.keyframe_points:
            keyframe.interpolation = "LINEAR"

    bpy.ops.export_scene.gltf(
        filepath=str(out_path),
        export_format="GLB",
        export_animations=True,
        export_apply=True,
        export_yup=True,
    )
    if not out_path.exists():
        raise SystemExit(f"turntable: Blender wrote no {out_path}")

    return {"meshes": len(imported), "pillars": len(pillars), "frames": frames, "peak": peak}
