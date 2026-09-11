#!/usr/bin/env python3
"""Generate ``assets/mark/psi.svg`` from ``assets/mark/psi.json``.

``psi.json`` is the single source of truth for the psi monogram: four closed
polygons (stem, left arm, right arm, descender) in a 0..1 unit square, each
tagged with a palette colour.  This script maps the unit square into the SVG's
``viewbox`` frame with the configured ``inset`` and strokes every polygon with
``stroke`` unit-square units, so the flat mark and the Blender mesh built from
it cannot drift.

Usage::

    python assets/mark/render_mark.py            # (re)write the SVG
    python assets/mark/render_mark.py --check    # exit 1 if the SVG is stale

``--check`` regenerates the SVG in memory and compares it byte-for-byte with
the committed file, so a clean tree exits 0 and any edit to ``psi.json`` that
has not been rendered exits non-zero.
"""

from __future__ import annotations

import argparse
import json
import sys
from itertools import zip_longest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC_PATH = HERE / "psi.json"
SVG_PATH = HERE / "psi.svg"

EXPECTED_PARTS = ("stem", "left-arm", "right-arm", "descender")


class SpecError(Exception):
    """``psi.json`` does not describe a renderable mark."""


def num(value: float) -> str:
    """Format a coordinate deterministically, without trailing zeros."""
    text = "{:.4f}".format(value).rstrip("0").rstrip(".")
    if text in ("", "-", "-0"):
        text = "0"
    return text


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise SpecError(message)


def load_spec(path: Path = SPEC_PATH) -> dict:
    """Read and validate ``psi.json``."""
    try:
        raw = json.loads(path.read_bytes().decode("utf-8"))
    except FileNotFoundError:
        raise SpecError("{} is missing".format(path))
    except ValueError as exc:
        raise SpecError("{} is not valid JSON: {}".format(path, exc))

    _require(isinstance(raw, dict), "the spec must be a JSON object")
    viewbox = raw.get("viewbox")
    inset = raw.get("inset")
    stroke = raw.get("stroke")
    palette = raw.get("palette")
    parts = raw.get("parts")

    _require(isinstance(viewbox, (int, float)) and viewbox > 0,
             "viewbox must be a positive number")
    _require(isinstance(inset, (int, float)) and inset >= 0,
             "inset must be a non-negative number")
    _require(2 * inset < viewbox, "inset must leave room for the geometry")
    _require(isinstance(stroke, (int, float)) and stroke > 0,
             "stroke must be a positive number")
    _require(isinstance(palette, dict) and palette,
             "palette must map colour names to hex values")
    for name, value in palette.items():
        _require(isinstance(value, str) and value.startswith("#") and len(value) == 7,
                 "palette colour {!r} must be a #rrggbb string".format(name))

    _require(isinstance(parts, list) and len(parts) == len(EXPECTED_PARTS),
             "the mark is exactly {} parts, found {}".format(
                 len(EXPECTED_PARTS), len(parts) if isinstance(parts, list) else "none"))
    names = [part.get("name") for part in parts]
    _require(names == list(EXPECTED_PARTS),
             "parts must be named {}, in that order; found {}".format(
                 ", ".join(EXPECTED_PARTS), names))

    for part in parts:
        colour = part.get("colour")
        _require(colour in palette,
                 "part {!r} has colour {!r}, which is not in the palette".format(
                     part.get("name"), colour))
        points = part.get("points")
        _require(isinstance(points, list) and len(points) >= 2,
                 "part {!r} needs at least two points".format(part.get("name")))
        for point in points:
            _require(isinstance(point, list) and len(point) == 2,
                     "part {!r} has a malformed point {!r}".format(part.get("name"), point))
            for value in point:
                _require(isinstance(value, (int, float)) and not isinstance(value, bool),
                         "part {!r} has a non-numeric coordinate {!r}".format(
                             part.get("name"), value))
                _require(0.0 <= value <= 1.0,
                         "part {!r} has coordinate {} outside the 0..1 unit square".format(
                             part.get("name"), value))
        _require(points[0] != points[-1] or len(points) > 2,
                 "part {!r} must not repeat its first point; the path is closed with Z".format(
                     part.get("name")))

    return raw


def render(spec: dict) -> str:
    """Return the SVG document for a validated spec."""
    viewbox = float(spec["viewbox"])
    inset = float(spec["inset"])
    scale = viewbox - 2.0 * inset
    stroke_width = float(spec["stroke"]) * scale
    palette = spec["palette"]

    def view(value: float) -> str:
        return num(inset + scale * value)

    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        "<!-- Generated by assets/mark/render_mark.py from assets/mark/psi.json.",
        "     Do not edit by hand: run `python assets/mark/render_mark.py` instead. -->",
        "<!-- Unit square mapped to the viewBox as v' = {} + {} v.".format(
            num(inset), num(scale)),
        "     Stroke {} unit-square units = {} viewBox units.".format(
            num(float(spec["stroke"])), num(stroke_width)),
        "     Each part is a closed polygon stroked in its palette colour",
        "     ({} = ink, {} = accent). -->".format(
            palette["ink"], palette["accent"]),
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {} {}" width="{}" height="{}">'.format(
            num(viewbox), num(viewbox), num(viewbox), num(viewbox)),
        "  <title>psi monogram</title>",
    ]

    for part in spec["parts"]:
        commands = ["{} {} {}".format("M" if index == 0 else "L",
                                      view(x), view(y))
                    for index, (x, y) in enumerate(part["points"])]
        commands.append("Z")
        lines.append(
            '  <path d="{}" fill="none" stroke="{}" stroke-width="{}" '
            'stroke-linecap="square" stroke-linejoin="miter" stroke-miterlimit="4"/>'.format(
                " ".join(commands), palette[part["colour"]], num(stroke_width)))

    lines.append("</svg>")
    return "\n".join(lines) + "\n"


def _report_diff(committed: str, rendered: str) -> None:
    committed_lines = committed.splitlines()
    rendered_lines = rendered.splitlines()
    shown = 0
    for index, (old, new) in enumerate(zip_longest(committed_lines, rendered_lines)):
        if old == new:
            continue
        print("render_mark: line {}: committed {!r} != generated {!r}".format(
            index + 1, old, new), file=sys.stderr)
        shown += 1
        if shown == 5:
            print("render_mark: ... further differences suppressed", file=sys.stderr)
            break
    if shown == 0 and len(committed) != len(rendered):
        print("render_mark: byte counts differ ({} committed, {} generated)".format(
            len(committed.encode("utf-8")), len(rendered.encode("utf-8"))),
            file=sys.stderr)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="Render the psi monogram SVG from psi.json.")
    parser.add_argument(
        "--check", action="store_true",
        help="do not write; exit non-zero if the committed SVG differs")
    args = parser.parse_args(argv)

    try:
        spec = load_spec()
    except SpecError as exc:
        print("render_mark: invalid psi.json: {}".format(exc), file=sys.stderr)
        return 2

    rendered = render(spec)

    if args.check:
        if not SVG_PATH.exists():
            print("render_mark: {} is missing; run the script without --check".format(
                SVG_PATH.name), file=sys.stderr)
            return 1
        committed = SVG_PATH.read_bytes().decode("utf-8")
        if committed == rendered:
            print("render_mark: {} is up to date ({} bytes, {} paths)".format(
                SVG_PATH.name, len(rendered.encode("utf-8")), len(spec["parts"])))
            return 0
        print("render_mark: {} is stale; regenerate it".format(SVG_PATH.name),
              file=sys.stderr)
        _report_diff(committed, rendered)
        return 1

    with open(SVG_PATH, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(rendered)
    print("render_mark: wrote {} ({} bytes, {} paths)".format(
        SVG_PATH.name, len(rendered.encode("utf-8")), len(spec["parts"])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
