#!/usr/bin/env python3
"""Install the psi mark's favicon files into a site checkout and verify them.

``assets/mark/psi.json`` -> ``assets/mark/render_mark.py`` -> ``assets/mark/psi.svg``
is the single source of truth for the mark (D-007).  The two Jekyll sites that
wear it -- ``superposition.github.io`` and ``superposition/mage`` -- keep their
own copy of the rendered mark under ``assets/mark/``, so this script carries the
committed files into a checkout and proves the copies cannot drift:

* ``--site <checkout>`` copies ``assets/mark/psi.svg`` and the 180x180
  ``assets/mark/psi-180.png`` Apple touch icon into ``<checkout>/assets/mark/``.
  For a site published from a subdirectory (``superposition/mage``), pass that
  directory: ``--site <mage>/docs``.
* ``--check`` copies nothing: it compares the installed files byte-for-byte with
  the committed assets, parses the installed SVG and requires the four
  ``<path>`` elements of the mark, and reads the PNG header of the installed
  touch icon to require 180x180.
* ``--print`` emits the inline ``<svg class="mark">`` wordmark block the sites'
  ``_layouts/default.html`` carries, rendered from ``psi.json`` so the pasted
  geometry cannot drift from the committed favicon.

Usage::

    python assets/mark/install_mark.py --site ../superposition.github.io
    python assets/mark/install_mark.py --site ../superposition.github.io --check
    python assets/mark/install_mark.py --site ../mage/docs
    python assets/mark/install_mark.py --print
"""

from __future__ import annotations

import argparse
import struct
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import render_mark

HERE = Path(__file__).resolve().parent

SVG_NAME = "psi.svg"
TOUCH_ICON_NAME = "psi-180.png"
ASSETS_DIR = ("assets", "mark")
TOUCH_ICON_SIZE = 180
PATH_COUNT = len(render_mark.EXPECTED_PARTS)

SVG_NS = "{http://www.w3.org/2000/svg}"


class InstallError(Exception):
    """The site cannot be installed into as described."""


def inline_mark(spec: dict) -> str:
    """Return the header ``<svg class="mark">`` block for a validated spec.

    The ``<path>`` elements are taken verbatim from ``render_mark.render``, the
    same function that writes ``psi.svg``: the SVG element is retagged with the
    site's ``mark`` class and the enclosing ``viewbox`` frame.
    """
    viewbox = render_mark.num(float(spec["viewbox"]))
    paths = _rendered_paths(spec)
    lines = ['<svg class="mark" viewBox="0 0 {0} {0}" aria-hidden="true">'.format(viewbox)]
    lines.extend("  " + path for path in paths)
    lines.append("</svg>")
    return "\n".join(lines)


def _rendered_paths(spec: dict) -> list:
    paths = [line.strip() for line in render_mark.render(spec).splitlines()
             if line.strip().startswith("<path ")]
    if len(paths) != PATH_COUNT:
        raise InstallError("render_mark.render produced {} <path> elements, expected {}".format(
            len(paths), PATH_COUNT))
    return paths


def _compare(installed: Path, source: Path, problems: list) -> None:
    if not installed.exists():
        problems.append("{} is missing; run the script without --check".format(installed))
        return
    if installed.read_bytes() == source.read_bytes():
        return
    problems.append("{} differs from {}".format(installed, source))


def _check_svg(installed: Path, spec: dict, problems: list) -> None:
    """The installed SVG parses and draws the mark's four paths in order."""
    try:
        root = ET.parse(str(installed)).getroot()
    except ET.ParseError as exc:
        problems.append("{} is not well-formed XML: {}".format(installed, exc))
        return
    if root.tag != SVG_NS + "svg":
        problems.append("{} is <{}>, not an <svg>".format(installed, root.tag))
        return
    viewbox = "0 0 {0} {0}".format(render_mark.num(float(spec["viewbox"])))
    if root.get("viewBox") != viewbox:
        problems.append("{} has viewBox {!r}, expected {!r}".format(
            installed, root.get("viewBox"), viewbox))
    found = [path.get("d") for path in root.iter(SVG_NS + "path")]
    expected = [line.split('d="', 1)[1].split('"', 1)[0] for line in _rendered_paths(spec)]
    if found != expected:
        problems.append("{} draws {} paths that do not match psi.json".format(
            installed, len(found)))


def _check_png(installed: Path, problems: list) -> None:
    """The installed touch icon is a PNG whose IHDR is 180x180."""
    header = installed.read_bytes()[:24]
    if len(header) < 24 or header[:8] != b"\x89PNG\r\n\x1a\n":
        problems.append("{} is not a PNG".format(installed))
        return
    width, height = struct.unpack(">II", header[16:24])
    if (width, height) != (TOUCH_ICON_SIZE, TOUCH_ICON_SIZE):
        problems.append("{} is {}x{}, expected {}x{}".format(
            installed, width, height, TOUCH_ICON_SIZE, TOUCH_ICON_SIZE))


def install(site: Path, check: bool) -> int:
    if not site.is_dir():
        print("install_mark: {} is not a directory".format(site), file=sys.stderr)
        return 2

    try:
        spec = render_mark.load_spec()
        inline = inline_mark(spec)
    except render_mark.SpecError as exc:
        print("install_mark: invalid psi.json: {}".format(exc), file=sys.stderr)
        return 2
    except InstallError as exc:
        print("install_mark: {}".format(exc), file=sys.stderr)
        return 2

    sources = [(SVG_NAME, HERE / SVG_NAME), (TOUCH_ICON_NAME, HERE / TOUCH_ICON_NAME)]
    if not check:
        missing = [source for _, source in sources if not source.exists()]
        if missing:
            print("install_mark: {} is missing; run "
                  "assets/mark/render_mark.py first".format(missing[0]), file=sys.stderr)
            return 2

    assets = site.joinpath(*ASSETS_DIR)
    if not check:
        assets.mkdir(parents=True, exist_ok=True)
        for name, source in sources:
            assets.joinpath(name).write_bytes(source.read_bytes())

    problems = []
    for name, source in sources:
        _compare(assets / name, source, problems)
    if assets.joinpath(SVG_NAME).exists():
        _check_svg(assets / SVG_NAME, spec, problems)
    if assets.joinpath(TOUCH_ICON_NAME).exists():
        _check_png(assets / TOUCH_ICON_NAME, problems)

    if problems:
        for problem in problems:
            print("install_mark: {}".format(problem), file=sys.stderr)
        return 1

    print("install_mark: {} {} ({} paths, {} assets)".format(
        "up to date:" if check else "wrote", site,
        len(inline.splitlines()) - 2, len(sources)))
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="Install the psi mark's favicon files into a site checkout.")
    parser.add_argument(
        "--site", type=Path,
        help="the site checkout (or its published directory) to install into")
    parser.add_argument(
        "--check", action="store_true",
        help="copy nothing; exit non-zero if the site differs from the assets")
    parser.add_argument(
        "--print", dest="print_only", action="store_true",
        help="print the inline wordmark block and exit")
    args = parser.parse_args(argv)

    try:
        spec = render_mark.load_spec()
    except render_mark.SpecError as exc:
        print("install_mark: invalid psi.json: {}".format(exc), file=sys.stderr)
        return 2

    if args.print_only:
        try:
            print(inline_mark(spec))
        except InstallError as exc:
            print("install_mark: {}".format(exc), file=sys.stderr)
            return 2
        return 0

    if args.site is None:
        parser.error("--site is required unless --print is given")
    return install(args.site.resolve(), args.check)


if __name__ == "__main__":
    sys.exit(main())
