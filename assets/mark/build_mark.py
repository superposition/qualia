#!/usr/bin/env python3
"""Build the psi mark's 3D assets with Blender, headless and deterministic.

    blender --background --factory-startup --python assets/mark/build_mark.py

Every path is resolved from this file's directory, so the command works from any
working directory. The geometry is the committed `psi.svg` — the same paths the
sites inline (D-007) — imported with Blender's built-in SVG importer; `psi.json`
supplies the stroke width, the palette and the part names, and the imported
centre lines are checked against it so the flat mark and the mesh cannot drift.

Blender's SVG importer reads fills and ignores `stroke`, and `psi.svg` draws each
part as a closed, mitre-joined stroke, so what the importer produces is a
zero-width centre line: converting it straight to mesh and extruding would give
nothing to render. The imported centre lines are therefore stroked here as the
closed polygons the SVG draws — join and mitre-limit rules, no caps — and that
outline is what is extruded along `+Z` by 0.12 and bevelled by 0.01 in two
segments.

Outputs, written in this order:

    psi.glb                          the extruded mark, two Principled materials
    psi-hero.png  1920x1080          the 45-degree hero, transparent film
    psi-hero.webp 1920x1080          the same frame, WebP
    psi-512.png / psi-180.png / psi-32.png
                                     the flat camera at `ortho_scale = 1.0`

The flat camera looks down `-Z` at the un-extruded face with `ortho_scale` fixed
at 1.0, so the whole 100-unit viewBox maps to the frame and the 32-pixel icon
stays legible. Blender is the only rasteriser: nothing here shells out to
`inkscape`, `rsvg-convert` or ImageMagick.

Every value is fixed and the file names are fixed, so two runs write
byte-identical files; prove it by hashing `psi.glb` and `psi-180.png` before and
after a second run.

`--entry <slug>` renders one journal entry's hero instead:

    blender --background --factory-startup --python assets/mark/build_mark.py \
        -- --entry the-connectome-as-a-prior

It writes `docs/figures/<slug>/hero.png` and `hero.webp` (1920x1080, WebP
quality 82): the mark and one geometric element that encodes the entry's
subject, on the same fixed 45-degree camera. The element's numbers are read from
the committed source `entries.json` names for the slug — the connectome
fixture's CSV, the ladder's committed recording, the console's `VIEWS` table — so
the picture can only say what the data says.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import pathlib
import re
import sys

import bmesh
import bpy
from bpy_extras.object_utils import world_to_camera_view
from mathutils import Quaternion, Vector

HERE = pathlib.Path(__file__).resolve().parent
SPEC_PATH = HERE / "psi.json"
SVG_PATH = HERE / "psi.svg"

GLB_PATH = HERE / "psi.glb"
HERO_PNG_PATH = HERE / "psi-hero.png"
HERO_WEBP_PATH = HERE / "psi-hero.webp"
ICON_PATHS = (
    (512, HERE / "psi-512.png"),
    (180, HERE / "psi-180.png"),
    (32, HERE / "psi-32.png"),
)

SCHEMA = "qualia.psi-mark.v1"

# psi.json's geometry is stroked, and psi.svg carries each part as a closed
# subpath (`Z`) with `stroke-linejoin="miter"` and `stroke-miterlimit="4"`; a
# closed subpath joins at its ends rather than capping them. Blender's SVG
# importer only reads fills, so the strokes are expanded here as closed polygons
# with the same join and miter-limit rules and the result is what gets extruded.
MITER_LIMIT = 4.0

# One viewBox unit. The viewBox is 100 units across, so 100 units == one Blender
# unit and the flat camera's `ortho_scale` of 1.0 frames exactly the viewBox.
UNIT = 1.0 / 100.0

DEPTH = 0.12
BEVEL = 0.01
BEVEL_SEGMENTS = 2

ROUGHNESS = 0.35
METALLIC = 0.0

HERO_RESOLUTION = (1920, 1080)
HERO_ORTHO_SCALE = 1.35
HERO_ELEVATION = 45.0
HERO_AZIMUTH = 35.0
HERO_DISTANCE = 4.0

# Calibrated so the flat camera renders the mark at its palette colours: the
# ink's top face comes out #edf0f5 and the descender's #91dbba.
KEY_ENERGY = 100.0
FILL_ENERGY = 28.0
RIM_ENERGY = 72.0

FLAT_ORTHO_SCALE = 1.0
FLAT_DISTANCE = 4.0

# The entry heroes (step 42). The element is built in the mark's own units — the
# viewBox is 100 units across, so the mark is 0.84 units wide — and placed beside
# the mark in the camera's image plane, where it cannot be occluded by it. Every
# value below is fixed, and the camera keeps its fixed elevation and azimuth; the
# ortho scale is solved from the composition's own silhouette, so the frame is a
# deterministic function of the committed geometry.
REPO_ROOT = HERE.parents[1]
ENTRIES_PATH = HERE / "entries.json"
ENTRIES_SCHEMA = "qualia.mark-entries.v1"
FIGURES_DIR = REPO_ROOT / "docs" / "figures"
COMPOSITION_MARGIN = 0.66
ELEMENT_OFFSET = (0.46, 0.46, -0.05)

# csr-grid: one block per nonzero (pre_type, post_type) pair on a 5x5 lattice.
GRID_PITCH = 0.26
GRID_BLOCK = 0.205
GRID_BASE_DEPTH = 0.03
GRID_MIN_HEIGHT = 0.07
GRID_MAX_HEIGHT = 0.36

# rings: the four ladder bands, drawn at their thresholds times RING_UNIT.
RING_UNIT = 0.05
RING_GAP = 0.5
RING_SEGMENTS = 128
RING_DEPTH = 0.09

# tiles: the console's five views, staggered like the five floating windows.
TILE_WIDTH = 0.44
TILE_HEIGHT = 0.30
TILE_STEP = 0.15
TILE_DEPTH = 0.07
TILE_RISE = 0.012
SCREEN_INSET = 0.05
SCREEN_DEPTH = 0.02

WEBP_QUALITY = 82
PNG_COMPRESSION = 15

# Blender stamps the date, the render time and the scene into a PNG's metadata,
# which would make two runs differ; only the pixels are wanted.
STAMP_FLAGS = (
    "use_stamp_date",
    "use_stamp_time",
    "use_stamp_render_time",
    "use_stamp_frame",
    "use_stamp_frame_range",
    "use_stamp_camera",
    "use_stamp_lens",
    "use_stamp_scene",
    "use_stamp_note",
    "use_stamp_marker",
    "use_stamp_filename",
    "use_stamp_sequencer_strip",
    "use_stamp_memory",
    "use_stamp_hostname",
    "use_stamp_labels",
)

# The importer stores its own float32 coordinates; psi.json is what the flat
# mark is drawn from, so agreement is checked generously in viewBox units.
SVG_TOLERANCE = 0.05


class BuildError(Exception):
    """The mark could not be built."""


def srgb_to_linear(value: float) -> float:
    """sRGB channel in 0..1 to the linear value Blender's Principled wants."""
    if value <= 0.04045:
        return value / 12.92
    return ((value + 0.055) / 1.055) ** 2.4


def hex_to_linear(text: str) -> tuple[float, float, float]:
    digits = text.lstrip("#")
    if len(digits) != 6:
        raise BuildError(f"not a six-digit colour: {text!r}")
    return tuple(srgb_to_linear(int(digits[i : i + 2], 16) / 255.0) for i in (0, 2, 4))


def read_spec(path: pathlib.Path = SPEC_PATH) -> dict:
    """Read and validate `psi.json`."""
    spec = json.loads(path.read_text(encoding="utf-8"))
    if spec.get("schema") != SCHEMA:
        raise BuildError(f"{path.name}: schema is {spec.get('schema')!r}, expected {SCHEMA!r}")
    for key in ("viewbox", "inset", "stroke"):
        if not isinstance(spec.get(key), (int, float)):
            raise BuildError(f"{path.name}: {key} must be a number")
    palette = spec.get("palette") or {}
    parts = spec.get("parts") or []
    if not parts:
        raise BuildError(f"{path.name}: no parts")
    seen = set()
    for part in parts:
        colour = part.get("colour")
        if colour not in palette:
            raise BuildError(f"{path.name}: part {part.get('name')!r} has unknown colour {colour!r}")
        if part.get("name") in seen:
            raise BuildError(f"{path.name}: duplicate part {part.get('name')!r}")
        seen.add(part.get("name"))
        points = part.get("points") or []
        if len(points) < 2:
            raise BuildError(f"{path.name}: part {part.get('name')!r} needs at least two points")
        for point in points:
            if len(point) != 2:
                raise BuildError(f"{path.name}: part {part.get('name')!r} has a malformed point")
    return spec


def viewbox_points(spec: dict) -> list[list[tuple[float, float]]]:
    """The parts' centre lines as psi.svg draws them: `v' = inset + span v`."""
    viewbox = float(spec["viewbox"])
    inset = float(spec["inset"])
    span = viewbox - 2.0 * inset
    return [
        [(inset + span * float(x), inset + span * float(y)) for x, y in part["points"]]
        for part in spec["parts"]
    ]


def stroke_half_width(spec: dict) -> float:
    """Half the stroke width in viewBox units."""
    viewbox = float(spec["viewbox"])
    inset = float(spec["inset"])
    return 0.5 * float(spec["stroke"]) * (viewbox - 2.0 * inset)


def import_centre_lines(spec: dict) -> list[tuple[list[tuple[float, float]], str]]:
    """Import `psi.svg` and return each part's centre line in viewBox units.

    Blender's importer works in its own units, so the imported points are fitted
    back onto the viewBox the committed SVG draws, and every part is then checked
    against `psi.json`.
    """
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.preferences.addon_enable(module="io_curve_svg")
    bpy.ops.import_curve.svg(filepath=str(SVG_PATH))

    curves = sorted(
        (obj for obj in bpy.context.scene.objects if obj.type == "CURVE"),
        key=lambda obj: obj.name,
    )
    if len(curves) != len(spec["parts"]):
        raise BuildError(f"{SVG_PATH.name}: imported {len(curves)} paths, expected {len(spec['parts'])}")

    raw = []
    for obj in curves:
        if len(obj.data.splines) != 1:
            raise BuildError(f"{SVG_PATH.name}: {obj.name} has {len(obj.data.splines)} splines")
        spline = obj.data.splines[0]
        raw.append([(point.co[0], point.co[1]) for point in spline.bezier_points])
        bpy.data.objects.remove(obj, do_unlink=True)

    expected = viewbox_points(spec)
    imported = [point for points in raw for point in points]
    wanted = [point for points in expected for point in points]
    scale_x = (_span(wanted, 0)) / _span(imported, 0)
    scale_y = (_span(wanted, 1)) / _span(imported, 1)
    x0 = min(point[0] for point in wanted)
    y1 = max(point[1] for point in wanted)
    ix0 = min(point[0] for point in imported)
    iy0 = min(point[1] for point in imported)

    lines = []
    for part, points in zip(spec["parts"], raw):
        mapped = [(x0 + (x - ix0) * scale_x, y1 - (y - iy0) * scale_y) for x, y in points]
        lines.append((mapped, part["colour"]))

    for part, want, (mapped, _) in zip(spec["parts"], expected, lines):
        if len(mapped) != len(want):
            raise BuildError(f"{SVG_PATH.name}: {part['name']} has {len(mapped)} points, expected {len(want)}")
        for (x, y), (wx, wy) in zip(mapped, want):
            if math.dist((x, y), (wx, wy)) > SVG_TOLERANCE:
                raise BuildError(f"{SVG_PATH.name}: {part['name']} centre line differs from psi.json")
    return lines


def _span(points: list[tuple[float, float]], axis: int) -> float:
    values = [point[axis] for point in points]
    return max(values) - min(values)


def to_blender(points: list[tuple[float, float]], origin: tuple[float, float]) -> list[tuple[float, float]]:
    """ViewBox coordinates to Blender: centred, scaled by UNIT, `y` flipped up."""
    cx, cy = origin
    return [((x - cx) * UNIT, (cy - y) * UNIT) for x, y in points]


def unit(vector: tuple[float, float]) -> tuple[float, float]:
    length = math.hypot(vector[0], vector[1])
    if length == 0.0:
        raise BuildError("zero-length segment in psi.json")
    return (vector[0] / length, vector[1] / length)


def _intersection(
    point_a: tuple[float, float],
    direction_a: tuple[float, float],
    point_b: tuple[float, float],
    direction_b: tuple[float, float],
) -> tuple[float, float] | None:
    denominator = direction_a[0] * direction_b[1] - direction_a[1] * direction_b[0]
    if abs(denominator) < 1e-12:
        return None
    delta = (point_b[0] - point_a[0], point_b[1] - point_a[1])
    distance = (delta[0] * direction_b[1] - delta[1] * direction_b[0]) / denominator
    return (point_a[0] + distance * direction_a[0], point_a[1] + distance * direction_a[1])


def _wound(polygon: list[tuple[float, float]]) -> list[tuple[float, float]]:
    area = 0.0
    for index, (x, y) in enumerate(polygon):
        nx, ny = polygon[(index + 1) % len(polygon)]
        area += x * ny - nx * y
    return polygon if area >= 0.0 else polygon[::-1]


def stroke_pieces(points: list[tuple[float, float]], half: float) -> list[list[tuple[float, float]]]:
    """Simple convex pieces whose union is the stroked centre line.

    The centre line is the closed polygon `psi.svg` draws, so the closing segment
    is a band like the rest and the start and end vertices join like the interior
    ones: a closed subpath has joins at its ends, not caps. Every piece is convex,
    so the union Blender's exact boolean builds stays clean even where the mark's
    arms turn more sharply than the stroke is wide.
    """
    pieces = []
    count = len(points)
    for index in range(count):
        start, end = points[index], points[(index + 1) % count]
        tangent = unit((end[0] - start[0], end[1] - start[1]))
        normal = (-tangent[1], tangent[0])
        pieces.append(
            [
                (start[0] + normal[0] * half, start[1] + normal[1] * half),
                (end[0] + normal[0] * half, end[1] + normal[1] * half),
                (end[0] - normal[0] * half, end[1] - normal[1] * half),
                (start[0] - normal[0] * half, start[1] - normal[1] * half),
            ]
        )
    for index in range(count):
        piece = _join_piece(points[index - 1], points[index], points[(index + 1) % count], half)
        if piece is not None:
            pieces.append(piece)
    return [_wound(piece) for piece in pieces]


def _join_piece(
    previous: tuple[float, float],
    vertex: tuple[float, float],
    following: tuple[float, float],
    half: float,
) -> list[tuple[float, float]] | None:
    """The mitre, or bevel, that fills the outside of an interior corner."""
    incoming = unit((vertex[0] - previous[0], vertex[1] - previous[1]))
    outgoing = unit((following[0] - vertex[0], following[1] - vertex[1]))
    cross = incoming[0] * outgoing[1] - incoming[1] * outgoing[0]
    if abs(cross) < 1e-9:
        return None
    normal_in = (-incoming[1], incoming[0])
    normal_out = (-outgoing[1], outgoing[0])
    side = -1.0 if cross > 0.0 else 1.0  # the outer side of the turn
    first = (vertex[0] + side * normal_in[0] * half, vertex[1] + side * normal_in[1] * half)
    second = (vertex[0] + side * normal_out[0] * half, vertex[1] + side * normal_out[1] * half)
    mitre = _intersection(first, incoming, second, outgoing)
    if mitre is not None and math.dist(mitre, vertex) <= MITER_LIMIT * half + 1e-9:
        return [vertex, first, mitre, second]
    return [vertex, first, second]  # the mitre limit chops it: a bevel join


def polygon_solid(name: str, polygon: list[tuple[float, float]], depth: float, z: float = 0.0):
    """A prism: the polygon at `z`, extruded along `+Z` by `depth`."""
    mesh = bpy.data.meshes.new(name)
    builder = bmesh.new()
    try:
        face = builder.faces.new([builder.verts.new((x, y, z)) for x, y in polygon])
        grown = bmesh.ops.extrude_face_region(builder, geom=[face])
        bmesh.ops.translate(
            builder,
            verts=[element for element in grown["geom"] if isinstance(element, bmesh.types.BMVert)],
            vec=(0.0, 0.0, depth),
        )
        bmesh.ops.recalc_face_normals(builder, faces=builder.faces[:])
        builder.to_mesh(mesh)
    finally:
        builder.free()
    mesh.validate()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def union_pieces(name: str, pieces: list[list[tuple[float, float]]], depth: float):
    """Boolean-union the stroke pieces into one manifold solid."""
    objects = [polygon_solid(f"{name}-piece-{index:02d}", piece, depth) for index, piece in enumerate(pieces)]
    target = objects[0]
    operands = bpy.data.collections.new(f"{name}-pieces")
    bpy.context.scene.collection.children.link(operands)
    for obj in objects[1:]:
        bpy.context.scene.collection.objects.unlink(obj)
        operands.objects.link(obj)

    modifier = target.modifiers.new("union", "BOOLEAN")
    modifier.operation = "UNION"
    modifier.solver = "EXACT"
    modifier.operand_type = "COLLECTION"
    modifier.collection = operands
    bpy.context.view_layer.objects.active = target
    target.select_set(True)
    bpy.ops.object.modifier_apply(modifier=modifier.name)

    for obj in objects[1:]:
        mesh = obj.data
        bpy.data.objects.remove(obj, do_unlink=True)
        bpy.data.meshes.remove(mesh)
    bpy.data.collections.remove(operands)
    target.name = name
    target.data.name = name
    return target


def canonical_elements(obj) -> tuple[list[tuple[float, float, float]], list[tuple[int, ...]]]:
    """The object's mesh as (vertices, faces) in a canonical order.

    Blender's exact boolean is threaded, so it emits the same solid with a
    different element order between runs. Sorting the faces by their own
    coordinates — each loop turned to start at the smallest of its rotations,
    which keeps the winding and so the shading — makes everything read out of the
    mesh reproducible: the bevel's clamping decisions and the exported GLB.
    """
    coordinates = [tuple(vertex.co) for vertex in obj.data.vertices]
    loops = []
    for polygon in obj.data.polygons:
        loop = tuple(polygon.vertices)
        sequence = [coordinates[corner] for corner in loop]
        start = min(range(len(loop)), key=lambda turn: sequence[turn:] + sequence[:turn])
        loops.append(loop[start:] + loop[:start])
    loops.sort(key=lambda loop: tuple(coordinates[i] for i in loop))

    vertices: list[tuple[float, float, float]] = []
    faces: list[tuple[int, ...]] = []
    remap: dict[int, int] = {}
    for loop in loops:
        for corner in loop:
            if corner not in remap:
                remap[corner] = len(remap)
                vertices.append(coordinates[corner])
        faces.append(tuple(remap[corner] for corner in loop))
    return vertices, faces


def canonicalise(obj) -> None:
    """Rebuild the mesh's elements in canonical order before anything walks them.

    `bevel_edges` clamps overlapping offsets in the order it meets them, so its
    result is only reproducible if the mesh it is handed is.
    """
    vertices, faces = canonical_elements(obj)
    old = obj.data
    name = old.name
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    obj.data = mesh
    bpy.data.meshes.remove(old)
    mesh.name = name


def bevel_edges(obj, offset: float, segments: int) -> None:
    builder = bmesh.new()
    try:
        builder.from_mesh(obj.data)
        bmesh.ops.bevel(
            builder,
            geom=list(builder.verts) + list(builder.edges),
            offset=offset,
            offset_type="OFFSET",
            segments=segments,
            profile=0.5,
            affect="EDGES",
            clamp_overlap=True,
            material=-1,
        )
        builder.to_mesh(obj.data)
    finally:
        builder.free()
    obj.data.validate()


def principled(name: str, colour: str, roughness: float, metallic: float):
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    shader = material.node_tree.nodes["Principled BSDF"]
    shader.inputs["Base Color"].default_value = (*hex_to_linear(colour), 1.0)
    shader.inputs["Roughness"].default_value = roughness
    shader.inputs["Metallic"].default_value = metallic
    return material


def combine(name: str, groups: list[tuple[object, int]], materials: list[object]):
    """One mesh object carrying both materials, in a canonical element order."""
    vertices: list[tuple[float, float, float]] = []
    faces: list[tuple[int, ...]] = []
    materials_per_face: list[int] = []
    for obj, index in groups:
        group_vertices, group_faces = canonical_elements(obj)
        base = len(vertices)
        vertices.extend(group_vertices)
        for loop in group_faces:
            faces.append(tuple(base + corner for corner in loop))
            materials_per_face.append(index)

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    for material in materials:
        mesh.materials.append(material)
    for polygon, index in zip(mesh.polygons, materials_per_face):
        polygon.material_index = index

    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    for group_obj, _ in groups:
        group_mesh = group_obj.data
        bpy.data.objects.remove(group_obj, do_unlink=True)
        bpy.data.meshes.remove(group_mesh)
    return obj


def bounds(obj) -> tuple[Vector, Vector]:
    corners = [obj.matrix_world @ vertex.co for vertex in obj.data.vertices]
    low = Vector((min(c.x for c in corners), min(c.y for c in corners), min(c.z for c in corners)))
    high = Vector((max(c.x for c in corners), max(c.y for c in corners), max(c.z for c in corners)))
    return low, high


def add_area_light(name: str, energy: float, size: float, location: Vector, aim: Vector) -> None:
    light = bpy.data.lights.new(name, "AREA")
    light.energy = energy
    light.shape = "SQUARE"
    light.size = size
    obj = bpy.data.objects.new(name, light)
    obj.location = location
    obj.rotation_euler = (aim - location).to_track_quat("-Z", "Y").to_euler()
    bpy.context.scene.collection.objects.link(obj)


def three_point_rig(centre: Vector) -> None:
    """Key, fill and rim, all aimed at the mark."""
    add_area_light("key", KEY_ENERGY, 1.8, centre + Vector((-1.7, -1.9, 2.1)), centre)
    add_area_light("fill", FILL_ENERGY, 2.6, centre + Vector((2.1, -1.5, 0.3)), centre)
    add_area_light("rim", RIM_ENERGY, 1.2, centre + Vector((0.5, 2.4, 1.7)), centre)


def add_camera(name: str, location: Vector, rotation, ortho_scale: float):
    camera = bpy.data.cameras.new(name)
    camera.type = "ORTHO"
    camera.ortho_scale = ortho_scale
    camera.clip_start = 0.01
    camera.clip_end = 100.0
    obj = bpy.data.objects.new(name, camera)
    obj.location = location
    obj.rotation_mode = "QUATERNION"
    obj.rotation_quaternion = rotation
    bpy.context.scene.collection.objects.link(obj)
    return obj


def hero_camera(scene, mark, centre: Vector):
    """A 45-degree orthographic view, framed on the mark's projected silhouette.

    Looking down at 45 degrees, the mark's silhouette is not centred on the
    bounding-box centre — the arms lean towards the camera and the descender
    away from it — so the camera is slid in its own image plane until the
    silhouette's bounding box sits in the middle of the frame.
    """
    elevation = math.radians(HERO_ELEVATION)
    azimuth = math.radians(HERO_AZIMUTH)
    direction = Vector(
        (
            math.sin(azimuth) * math.cos(elevation),
            -math.cos(azimuth) * math.cos(elevation),
            math.sin(elevation),
        )
    )
    location = centre + direction * HERO_DISTANCE
    rotation = (centre - location).to_track_quat("-Z", "Y")
    camera = add_camera("hero-camera", location, rotation, HERO_ORTHO_SCALE)

    scene.render.resolution_x, scene.render.resolution_y = HERO_RESOLUTION
    bpy.context.view_layer.update()
    projected = [world_to_camera_view(scene, camera, vertex.co) for vertex in mark.data.vertices]
    width = HERO_ORTHO_SCALE
    height = HERO_ORTHO_SCALE * HERO_RESOLUTION[1] / HERO_RESOLUTION[0]
    offset = (
        (min(c.x for c in projected) + max(c.x for c in projected)) / 2.0 - 0.5,
        (min(c.y for c in projected) + max(c.y for c in projected)) / 2.0 - 0.5,
    )
    camera.location = location + rotation.to_matrix() @ Vector((offset[0] * width, offset[1] * height, 0.0))
    return camera


def flat_camera():
    """Down `-Z`, `ortho_scale` fixed at 1.0 so the viewBox fills the frame."""
    return add_camera(
        "flat-camera",
        Vector((0.0, 0.0, FLAT_DISTANCE)),
        Quaternion(),
        FLAT_ORTHO_SCALE,
    )


def entry_spec(path: pathlib.Path = ENTRIES_PATH) -> dict:
    """Read `entries.json`: the element each journal entry's hero renders."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise BuildError(f"{path.name}: {error}") from error
    if document.get("schema") != ENTRIES_SCHEMA:
        raise BuildError(
            f"{path.name}: schema is {document.get('schema')!r}, expected {ENTRIES_SCHEMA!r}"
        )
    entries = document.get("entries") or []
    if not entries:
        raise BuildError(f"{path.name}: no entries")
    for entry in entries:
        for key in ("slug", "element", "encodes", "data"):
            if not entry.get(key):
                raise BuildError(f"{path.name}: an entry has no {key!r}")
    return document


def element_colour_hex(name: str, spec: dict, palette: dict) -> str:
    """A colour name's hex: psi.json owns the mark's, entries.json the element's."""
    if name in spec["palette"]:
        return spec["palette"][name]
    if name in palette:
        return palette[name]
    raise BuildError(f"unknown colour name {name!r}")


def read_csr_pairs(path: pathlib.Path) -> tuple[list[str], list[tuple[str, str, int]]]:
    """The committed fixture as (types, pairs): the rows summed per type pair."""
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))
    if not rows:
        raise BuildError(f"{path.name}: no edge rows")
    totals: dict[tuple[str, str], int] = {}
    types: set[str] = set()
    for row in rows:
        pre, post, weight = row["pre_type"], row["post_type"], int(row["weight"])
        types.add(pre)
        types.add(post)
        totals[(pre, post)] = totals.get((pre, post), 0) + weight
    return sorted(types), [(pre, post, totals[(pre, post)]) for pre, post in sorted(totals)]


def read_ladder(path: pathlib.Path) -> list[float]:
    """The ladder recording's thresholds, as the plan fixes them."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise BuildError(f"{path.name}: {error}") from error
    thresholds = [float(value) for value in document.get("thresholds") or []]
    if len(thresholds) < 2:
        raise BuildError(f"{path.name}: needs at least two thresholds")
    if any(later <= earlier for earlier, later in zip(thresholds, thresholds[1:])):
        raise BuildError(f"{path.name}: thresholds are not increasing")
    return thresholds


def read_views(path: pathlib.Path) -> list[str]:
    """The console's `VIEWS` table, in the order the console declares it."""
    text = path.read_text(encoding="utf-8")
    marker = "pub const VIEWS"
    if marker not in text:
        raise BuildError(f"{path.name}: no `{marker}` table")
    block = text.split(marker, 1)[1].split("];", 1)[0]
    names = [name.lower() for name in re.findall(r"View::([A-Za-z0-9_]+)", block)]
    if not names:
        raise BuildError(f"{path.name}: `{marker}` names no views")
    return names


def rectangle(x: float, y: float, width: float, height: float) -> list[tuple[float, float]]:
    return [(x, y), (x + width, y), (x + width, y + height), (x, y + height)]


def translate_solid(obj, offset: tuple[float, float, float]) -> None:
    """Bake an offset into the mesh: `combine` reads local coordinates only."""
    builder = bmesh.new()
    try:
        builder.from_mesh(obj.data)
        bmesh.ops.translate(builder, verts=builder.verts[:], vec=offset)
        builder.to_mesh(obj.data)
    finally:
        builder.free()
    obj.data.validate()


def ring_solid(name: str, inner_radius: float, outer_radius: float, depth: float, z: float = 0.0):
    """A flat annulus: a quad strip between two circles, extruded along `+Z`."""
    mesh = bpy.data.meshes.new(name)
    builder = bmesh.new()
    try:
        angles = [2.0 * math.pi * index / RING_SEGMENTS for index in range(RING_SEGMENTS)]
        inner = [
            builder.verts.new((inner_radius * math.cos(angle), inner_radius * math.sin(angle), z))
            for angle in angles
        ]
        outer = [
            builder.verts.new((outer_radius * math.cos(angle), outer_radius * math.sin(angle), z))
            for angle in angles
        ]
        faces = []
        for index in range(RING_SEGMENTS):
            following = (index + 1) % RING_SEGMENTS
            faces.append(
                builder.faces.new((outer[index], outer[following], inner[following], inner[index]))
            )
        grown = bmesh.ops.extrude_face_region(builder, geom=faces)
        bmesh.ops.translate(
            builder,
            verts=[element for element in grown["geom"] if isinstance(element, bmesh.types.BMVert)],
            vec=(0.0, 0.0, depth),
        )
        bmesh.ops.recalc_face_normals(builder, faces=builder.faces[:])
        builder.to_mesh(mesh)
    finally:
        builder.free()
    mesh.validate()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def csr_grid(path: pathlib.Path) -> list[tuple[object, str]]:
    """The fixture's type-level CSR as raised blocks on a square lattice."""
    types, pairs = read_csr_pairs(path)
    index = {name: position for position, name in enumerate(types)}
    span = (len(types) - 1) * GRID_PITCH
    limit = max(weight for _, _, weight in pairs)
    margin = span + GRID_BLOCK + 0.06
    solids = [
        (polygon_solid("csr-base", rectangle(-margin / 2, -margin / 2, margin, margin), GRID_BASE_DEPTH), "slate")
    ]
    for pre, post, weight in pairs:
        x = -span / 2 + index[post] * GRID_PITCH - GRID_BLOCK / 2
        y = span / 2 - index[pre] * GRID_PITCH - GRID_BLOCK / 2
        height = GRID_MIN_HEIGHT + (GRID_MAX_HEIGHT - GRID_MIN_HEIGHT) * weight / limit
        solids.append(
            (
                polygon_solid(
                    f"csr-{pre}-{post}",
                    rectangle(x, y, GRID_BLOCK, GRID_BLOCK),
                    height,
                    z=GRID_BASE_DEPTH,
                ),
                "blue",
            )
        )
    return solids


def ladder_rings(path: pathlib.Path) -> list[tuple[object, str]]:
    """The ladder's four bands: the three thresholds and the open band above them."""
    thresholds = read_ladder(path)
    boundaries = [0.0] + thresholds + [thresholds[-1] + (thresholds[-1] - thresholds[-2])]
    colours = ("green", "lavender", "blue", "sand")
    solids = []
    for band in range(len(boundaries) - 1):
        inner = (boundaries[band] + RING_GAP) * RING_UNIT
        outer = (boundaries[band + 1] - RING_GAP) * RING_UNIT
        solids.append((ring_solid(f"ring-{band}", inner, outer, RING_DEPTH), colours[band % len(colours)]))
    return solids


def console_tiles(path: pathlib.Path) -> list[tuple[object, str]]:
    """The console's views as staggered, extruded tiles, in `VIEWS` order."""
    views = read_views(path)
    start = -TILE_STEP * (len(views) - 1) / 2.0
    solids = []
    for position, view in enumerate(views):
        x = start + position * TILE_STEP
        y = start + position * TILE_STEP
        z = position * TILE_RISE
        solids.append(
            (
                polygon_solid(
                    f"tile-{view}",
                    rectangle(x - TILE_WIDTH / 2, y - TILE_HEIGHT / 2, TILE_WIDTH, TILE_HEIGHT),
                    TILE_DEPTH,
                    z=z,
                ),
                "blue",
            )
        )
        solids.append(
            (
                polygon_solid(
                    f"screen-{view}",
                    rectangle(
                        x - TILE_WIDTH / 2 + SCREEN_INSET,
                        y - TILE_HEIGHT / 2 + SCREEN_INSET,
                        TILE_WIDTH - 2 * SCREEN_INSET,
                        TILE_HEIGHT - 2 * SCREEN_INSET,
                    ),
                    SCREEN_DEPTH,
                    z=z + TILE_DEPTH,
                ),
                "green",
            )
        )
    return solids


ELEMENTS = {
    "csr-grid": csr_grid,
    "rings": ladder_rings,
    "tiles": console_tiles,
}


def element_solids(entry: dict) -> list[tuple[object, str]]:
    """The entry's committed element, placed beside the mark in the image plane."""
    builder = ELEMENTS.get(entry["element"])
    if builder is None:
        raise BuildError(f"{ENTRIES_PATH.name}: unknown element {entry['element']!r}")
    path = REPO_ROOT / entry["data"]
    if not path.is_file():
        raise BuildError(f"{entry['slug']}: {entry['data']} is not committed")
    solids = builder(path)
    for obj, _ in solids:
        translate_solid(obj, ELEMENT_OFFSET)
    return solids


def composition_camera(scene, subject, centre: Vector):
    """The fixed 45-degree hero camera, framed on mark and element together.

    The elevation and azimuth are the hero's own; only `ortho_scale` and the
    in-plane slide are solved, from the composition's projected silhouette, so a
    larger element is framed rather than cropped. `world_to_camera_view` returns
    fractions of the frame, so the scale that makes the silhouette span
    `COMPOSITION_MARGIN` is the current scale times the span over the margin.
    """
    elevation = math.radians(HERO_ELEVATION)
    azimuth = math.radians(HERO_AZIMUTH)
    direction = Vector(
        (
            math.sin(azimuth) * math.cos(elevation),
            -math.cos(azimuth) * math.cos(elevation),
            math.sin(elevation),
        )
    )
    location = centre + direction * HERO_DISTANCE
    rotation = (centre - location).to_track_quat("-Z", "Y")
    camera = add_camera("hero-camera", location, rotation, HERO_ORTHO_SCALE)

    scene.render.resolution_x, scene.render.resolution_y = HERO_RESOLUTION
    bpy.context.view_layer.update()
    projected = [world_to_camera_view(scene, camera, vertex.co) for vertex in subject.data.vertices]
    span = max(
        max(c.x for c in projected) - min(c.x for c in projected),
        max(c.y for c in projected) - min(c.y for c in projected),
    )
    camera.data.ortho_scale *= span / COMPOSITION_MARGIN
    bpy.context.view_layer.update()

    projected = [world_to_camera_view(scene, camera, vertex.co) for vertex in subject.data.vertices]
    width = camera.data.ortho_scale
    height = width * HERO_RESOLUTION[1] / HERO_RESOLUTION[0]
    offset = (
        (min(c.x for c in projected) + max(c.x for c in projected)) / 2.0 - 0.5,
        (min(c.y for c in projected) + max(c.y for c in projected)) / 2.0 - 0.5,
    )
    camera.location = location + rotation.to_matrix() @ Vector((offset[0] * width, offset[1] * height, 0.0))
    return camera


def configure_render(
    scene,
    transparent: bool = True,
    compression: int = PNG_COMPRESSION,
    dither: float | None = None,
    samples: int | None = None,
) -> None:
    scene.render.engine = "BLENDER_EEVEE_NEXT"
    scene.render.film_transparent = transparent
    if samples is not None:
        scene.eevee.taa_render_samples = samples
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.render.image_settings.color_depth = "8"
    scene.render.image_settings.compression = compression
    if dither is not None:
        scene.render.dither_intensity = dither
    for flag in STAMP_FLAGS:
        setattr(scene.render, flag, False)
    # The mark's colours are the site's palette, so render them straight: the
    # default AgX transform would desaturate the accent green.
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"
    scene.view_settings.exposure = 0.0
    scene.view_settings.gamma = 1.0


def world_background(scene, colour: tuple[float, float, float]) -> None:
    world = bpy.data.worlds.new("psi-world")
    world.use_nodes = True
    background = world.node_tree.nodes["Background"]
    background.inputs["Color"].default_value = (*colour, 1.0)
    background.inputs["Strength"].default_value = 1.0
    scene.world = world


def export_glb(path: pathlib.Path) -> None:
    bpy.ops.export_scene.gltf(
        filepath=str(path),
        export_format="GLB",
        use_selection=False,
        export_apply=True,
        export_yup=True,
        export_texcoords=False,
        export_normals=True,
        export_materials="EXPORT",
        export_cameras=False,
        export_lights=False,
        export_animations=False,
        export_extras=False,
    )
    if not path.is_file():
        raise BuildError(f"the glTF exporter wrote no {path.name}")


def render(
    scene,
    camera,
    path: pathlib.Path,
    resolution: tuple[int, int],
    file_format: str,
    quality: int | None = None,
) -> None:
    scene.camera = camera
    scene.render.resolution_x = resolution[0]
    scene.render.resolution_y = resolution[1]
    scene.render.image_settings.file_format = file_format
    scene.render.image_settings.color_mode = "RGBA"
    if quality is not None:
        scene.render.image_settings.quality = quality
    scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)
    if not path.is_file():
        raise BuildError(f"the renderer wrote no {path.name}")


def mark_solids(spec: dict) -> tuple[list[object], list[str]]:
    """The mark as one solid per palette colour, in psi.svg's paint order.

    psi.svg paints the parts in order, so the descender covers the end of the
    stem; building the accent solid after the ink one keeps that order in the
    combined mesh.
    """
    lines = import_centre_lines(spec)
    half = stroke_half_width(spec) * UNIT
    viewbox = float(spec["viewbox"])
    origin = (viewbox / 2.0, viewbox / 2.0)

    colours = []
    for _, colour in lines:
        if colour not in colours:
            colours.append(colour)

    solids = []
    for colour in colours:
        parts = [to_blender(points, origin) for points, part_colour in lines if part_colour == colour]
        pieces = [piece for points in parts for piece in stroke_pieces(points, half)]
        solid = union_pieces(f"mark-{colour}", pieces, DEPTH)
        canonicalise(solid)
        bevel_edges(solid, BEVEL, BEVEL_SEGMENTS)
        solids.append(solid)
    return solids, colours


def build() -> None:
    spec = read_spec()
    solids, colours = mark_solids(spec)
    groups = list(zip(solids, range(len(colours))))
    materials = [principled(name, spec["palette"][name], ROUGHNESS, METALLIC) for name in colours]
    mark = combine("psi-mark", groups, materials)

    low, high = bounds(mark)
    centre = (low + high) / 2.0

    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    configure_render(scene)
    world_background(scene, (0.020, 0.022, 0.028))
    three_point_rig(centre)
    hero = hero_camera(scene, mark, centre)
    flat = flat_camera()

    export_glb(GLB_PATH)
    render(scene, hero, HERO_PNG_PATH, HERO_RESOLUTION, "PNG")
    render(scene, hero, HERO_WEBP_PATH, HERO_RESOLUTION, "WEBP", WEBP_QUALITY)
    for size, path in ICON_PATHS:
        render(scene, flat, path, (size, size), "PNG")


def build_entry(slug: str) -> list[pathlib.Path]:
    """Render one entry's hero: the mark plus the element `entries.json` names."""
    document = entry_spec()
    entry = next((item for item in document["entries"] if item["slug"] == slug), None)
    if entry is None:
        known = ", ".join(item["slug"] for item in document["entries"])
        raise BuildError(f"unknown entry {slug!r}; known entries: {known}")

    spec = read_spec()
    solids, colours = mark_solids(spec)
    pieces = element_solids(entry)

    names = list(colours)
    for _, colour in pieces:
        if colour not in names:
            names.append(colour)
    groups = [(obj, names.index(colour)) for obj, colour in zip(solids, colours)]
    groups += [(obj, names.index(colour)) for obj, colour in pieces]
    materials = [
        principled(name, element_colour_hex(name, spec, document["palette"]), ROUGHNESS, METALLIC)
        for name in names
    ]
    subject = combine("psi-mark", groups, materials)

    low, high = bounds(subject)
    centre = (low + high) / 2.0

    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    # The marks' rasters keep the transparent film step 40 fixes; a hero is a
    # page image, so it renders onto the world background instead. Dither off and
    # maximum PNG compression keep the 1920x1080 file inside the figure
    # convention's 400 KiB budget: dithering noise is what a PNG cannot compress.
    configure_render(scene, transparent=False, compression=100, dither=0.0, samples=512)
    world_background(scene, (0.020, 0.022, 0.028))
    three_point_rig(centre)
    camera = composition_camera(scene, subject, centre)

    directory = FIGURES_DIR / slug
    directory.mkdir(parents=True, exist_ok=True)
    paths = [directory / "hero.png", directory / "hero.webp"]
    render(scene, camera, paths[0], HERO_RESOLUTION, "PNG")
    render(scene, camera, paths[1], HERO_RESOLUTION, "WEBP", WEBP_QUALITY)
    return paths


def script_args(argv: list[str] | None) -> list[str]:
    """Blender owns `sys.argv`; a script's own flags follow `--`."""
    if argv is not None:
        return argv
    if "--" in sys.argv:
        return sys.argv[sys.argv.index("--") + 1 :]
    return []


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Build the psi mark's GLB and rasters.")
    parser.add_argument(
        "--entry",
        metavar="SLUG",
        help="render docs/figures/<slug>/hero.png and hero.webp: the mark plus the entry's element",
    )
    args = parser.parse_args(script_args(argv))
    try:
        if args.entry:
            paths: tuple = tuple(build_entry(args.entry))
        else:
            build()
            paths = (GLB_PATH, HERO_PNG_PATH, HERO_WEBP_PATH) + tuple(path for _, path in ICON_PATHS)
    except BuildError as error:
        print(f"build_mark: {error}", file=sys.stderr)
        return 1
    for path in paths:
        print(f"build_mark: wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
