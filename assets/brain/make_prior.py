#!/usr/bin/env python3
"""Turn the committed connectome fixtures into the Arrow IPC inputs the prior builder reads.

`crates/connectome-prior` reads two Arrow IPC (Feather) tables:

* the segment weights table, columns ``body_pre`` (int64), ``body_post`` (int64) and
  ``weight`` (uint32);
* the body annotation table, columns ``bodyId`` (int64), ``type``, ``superclass``,
  ``side`` and ``dimorphism`` (all utf8).

The repository's fixtures are hand-authored CSV — ``crates/connectome-prior/tests/fixtures/``
— because a human has to read them, and the builder's own tests synthesise Feather from
that CSV on every run. This script does the same for the prior this directory commits, so
the committed prior is reproducible from the CSV without a binary fixture nobody can read.

The two CSVs use type names on both sides of an edge; the weights table is segment-level,
so each type name is resolved to the first body id that carries it, exactly as
``crates/connectome-prior/tests/prior.rs`` does.

Usage::

    python assets/brain/make_prior.py --out <dir>

Writes ``weights.feather`` and ``annotations.feather`` into ``--out`` and prints the two
paths, one per line, so the documented regeneration command can pass them to the builder.
"""

from __future__ import annotations

import argparse
import csv
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
FIXTURES = REPO / "crates" / "connectome-prior" / "tests" / "fixtures"

EDGES_CSV = FIXTURES / "mini-type-edges.csv"
ANNOTATIONS_CSV = FIXTURES / "mini-body-annotations.csv"

EDGES_OUT = "weights.feather"
ANNOTATIONS_OUT = "annotations.feather"


class InputError(Exception):
    """A fixture does not carry the columns the builder reads."""


def read_rows(path: pathlib.Path, header: tuple[str, ...]) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None or tuple(reader.fieldnames) != header:
            raise InputError(
                f"{path.name}: expected header {header}, found {reader.fieldnames}"
            )
        return [dict(row) for row in reader]


def first_body_id_by_type(annotations: list[dict[str, str]]) -> dict[str, int]:
    """The first body id listed for each type, the same resolution the crate's tests use."""
    resolved: dict[str, int] = {}
    for row in annotations:
        resolved.setdefault(row["type"], int(row["bodyId"]))
    return resolved


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args(argv)

    try:
        import pyarrow as pa
        import pyarrow.feather as feather
    except ImportError as error:  # pragma: no cover - environment failure, not input
        print(f"make_prior: pyarrow is required: {error}", file=sys.stderr)
        return 1

    annotations = read_rows(
        ANNOTATIONS_CSV,
        ("bodyId", "type", "superclass", "side", "dimorphism"),
    )
    edges = read_rows(EDGES_CSV, ("pre_type", "post_type", "weight"))

    by_type = first_body_id_by_type(annotations)

    body_pre: list[int] = []
    body_post: list[int] = []
    weight: list[int] = []
    for row in edges:
        for side in ("pre_type", "post_type"):
            if row[side] not in by_type:
                raise InputError(f"{EDGES_CSV.name}: {row[side]!r} has no annotation")
        body_pre.append(by_type[row["pre_type"]])
        body_post.append(by_type[row["post_type"]])
        weight.append(int(row["weight"]))

    args.out.mkdir(parents=True, exist_ok=True)
    edges_table = pa.table(
        {
            "body_pre": pa.array(body_pre, type=pa.int64()),
            "body_post": pa.array(body_post, type=pa.int64()),
            "weight": pa.array(weight, type=pa.uint32()),
        }
    )
    annotations_table = pa.table(
        {
            "bodyId": pa.array([int(row["bodyId"]) for row in annotations], type=pa.int64()),
            "type": pa.array([row["type"] for row in annotations], type=pa.string()),
            "superclass": pa.array([row["superclass"] for row in annotations], type=pa.string()),
            "side": pa.array([row["side"] for row in annotations], type=pa.string()),
            "dimorphism": pa.array(
                [row["dimorphism"] for row in annotations], type=pa.string()
            ),
        }
    )

    edges_path = args.out / EDGES_OUT
    annotations_path = args.out / ANNOTATIONS_OUT
    # The builder's arrow build carries no lz4/zstd codec, so the IPC file must be
    # uncompressed: the default lz4 framing fails to read with
    # "lz4 IPC decompression requires the lz4 feature".
    feather.write_feather(edges_table, edges_path, compression="uncompressed")
    feather.write_feather(annotations_table, annotations_path, compression="uncompressed")

    print(edges_path)
    print(annotations_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
