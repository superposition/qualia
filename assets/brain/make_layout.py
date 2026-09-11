#!/usr/bin/env python3
"""Derive the deterministic 3D layout of a connectome prior.

The prior the runtime loads is a ``qualia-connectome-prior`` artifact: ``manifest.json``
records ``type_count``/``edge_count`` and a SHA-256 per section, and ``graph.bin`` holds the
little-endian ``rowptr`` (u64), ``cols`` (u32) and ``weights`` (u32) sections back to back.
Nothing in the artifact is spatial, so the nodes get a layout derived from the graph itself
— the normalized graph Laplacian's three lowest non-trivial eigenvectors (a spectral
embedding), which is a deterministic function of the CSR.

Determinism is the contract:

* directed rows are symmetrised as ``A = W + Wᵀ`` so an edge contributes to both endpoints;
* the eigenproblem is solved densely below ``DENSE_LIMIT`` nodes and with a fixed Krylov
  start vector (``seed``) above it, so the same prior always gives the same vectors;
* each eigenvector's sign is fixed by making its largest-magnitude component positive
  (lowest index breaks a tie), because an eigenvector and its negation are both valid;
* coordinates are rounded to ``PRECISION`` decimals before they are written.

A different prior therefore produces a different layout, and ``layout.json`` carries the
prior's digests so a stale layout is detectable.

Usage::

    python assets/brain/make_layout.py --prior assets/brain/prior --out assets/brain/layout.json
    python assets/brain/make_layout.py --prior <dir> --out <path> --check   # non-zero if stale
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import struct
import sys

SCHEMA = "qualia.connectome-prior.v1"
LAYOUT_SCHEMA = "qualia.brain-layout.v1"
ALGORITHM = "spectral-laplacian-v1"
SEED = 20260911
PRECISION = 6
DENSE_LIMIT = 512

MANIFEST_FILE = "manifest.json"
GRAPH_FILE = "graph.bin"


class LayoutError(Exception):
    """The prior could not be read or does not describe a layout."""


def read_prior(directory: pathlib.Path) -> tuple[dict, list[int], list[int], list[int], str]:
    manifest_path = directory / MANIFEST_FILE
    graph_path = directory / GRAPH_FILE
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise LayoutError(f"{manifest_path}: {error}") from error
    if manifest.get("schema") != SCHEMA:
        raise LayoutError(f"{manifest_path}: schema {manifest.get('schema')!r} is not {SCHEMA}")
    type_count = int(manifest["type_count"])
    edge_count = int(manifest["edge_count"])

    graph = graph_path.read_bytes()
    graph_sha = hashlib.sha256(graph).hexdigest()
    expected = (type_count + 1) * 8 + edge_count * 4 * 2
    if len(graph) != expected:
        raise LayoutError(
            f"{graph_path}: {len(graph)} bytes do not match {type_count} types and "
            f"{edge_count} edges (expected {expected})"
        )

    (rowptr,) = [list(struct.unpack_from(f"<{type_count + 1}Q", graph, 0))]
    cols_offset = (type_count + 1) * 8
    cols = list(struct.unpack_from(f"<{edge_count}I", graph, cols_offset))
    weights = list(struct.unpack_from(f"<{edge_count}I", graph, cols_offset + edge_count * 4))

    if rowptr[0] != 0 or rowptr[-1] != edge_count or any(
        rowptr[i] > rowptr[i + 1] for i in range(type_count)
    ):
        raise LayoutError(f"{graph_path}: CSR rowptr is not monotone from 0 to {edge_count}")
    if any(column >= type_count for column in cols):
        raise LayoutError(f"{graph_path}: a column names a type outside 0..{type_count}")

    return manifest, rowptr, cols, weights, graph_sha


def adjacency(type_count: int, rowptr: list[int], cols: list[int], weights: list[int]):
    """The symmetrised weighted adjacency ``A = W + Wᵀ`` as a dense or sparse matrix."""
    try:
        import numpy as np
    except ImportError as error:  # pragma: no cover - environment failure, not input
        raise LayoutError(f"numpy is required: {error}") from error

    rows: list[int] = []
    columns: list[int] = []
    values: list[float] = []
    for source in range(type_count):
        for edge in range(rowptr[source], rowptr[source + 1]):
            rows.append(source)
            columns.append(cols[edge])
            values.append(float(weights[edge]))

    if type_count <= DENSE_LIMIT:
        dense = np.zeros((type_count, type_count), dtype=np.float64)
        for row, column, value in zip(rows, columns, values):
            dense[row, column] += value
        return dense + dense.T

    try:
        import scipy.sparse as sparse
    except ImportError as error:  # pragma: no cover - environment failure, not input
        raise LayoutError(f"scipy is required above {DENSE_LIMIT} nodes: {error}") from error

    directed = sparse.csr_matrix(
        (values, (rows, columns)), shape=(type_count, type_count), dtype=np.float64
    )
    return (directed + directed.T).tocsr()


def spectral_positions(
    type_count: int, rowptr: list[int], cols: list[int], weights: list[int]
):
    """The three lowest non-trivial eigenvectors of the normalized Laplacian."""
    import numpy as np

    adjacency_matrix = adjacency(type_count, rowptr, cols, weights)
    if type_count < 2:
        return np.zeros((type_count, 3), dtype=np.float64)

    if type_count <= DENSE_LIMIT:
        degrees = np.asarray(adjacency_matrix.sum(axis=1)).ravel()
    else:
        degrees = np.asarray(adjacency_matrix.sum(axis=1)).ravel()
    inverse_sqrt = np.zeros_like(degrees)
    np.divide(1.0, np.sqrt(degrees), out=inverse_sqrt, where=degrees > 0)
    scale = np.outer(inverse_sqrt, inverse_sqrt)

    if type_count <= DENSE_LIMIT:
        normalized = np.eye(type_count) - scale * adjacency_matrix
        _, vectors = np.linalg.eigh(normalized)
        spectrum = vectors[:, 1 : min(4, type_count)]
    else:
        import scipy.sparse as sparse
        from scipy.sparse.linalg import eigsh

        normalized = sparse.eye(type_count, format="csr") - sparse.csr_matrix(scale) @ adjacency_matrix
        start = np.linspace(1.0, 2.0, type_count)
        start /= np.linalg.norm(start)
        _, vectors = eigsh(normalized, k=min(4, type_count - 1), which="SA", v0=start)
        spectrum = vectors[:, 1:]

    positions = np.zeros((type_count, 3), dtype=np.float64)
    for axis in range(min(3, spectrum.shape[1])):
        column = spectrum[:, axis]
        pivot = int(np.argmax(np.abs(column)))
        if column[pivot] < 0.0:
            column = -column
        positions[:, axis] = column

    extent = float(np.abs(positions).max())
    if extent > 0.0:
        positions /= extent
    return positions


def layout(prior_dir: pathlib.Path) -> dict:
    manifest, rowptr, cols, weights, graph_sha = read_prior(prior_dir)
    type_count = int(manifest["type_count"])
    positions = spectral_positions(type_count, rowptr, cols, weights)
    nodes = [
        [round(float(value), PRECISION) for value in positions[index]]
        for index in range(type_count)
    ]
    return {
        "schema": LAYOUT_SCHEMA,
        "algorithm": ALGORITHM,
        "seed": SEED,
        "prior": {
            "schema": manifest["schema"],
            "source_sha256": manifest.get("source_sha256", ""),
            "graph_sha256": graph_sha,
            "rowptr_sha256": manifest.get("rowptr_sha256", ""),
            "cols_sha256": manifest.get("cols_sha256", ""),
            "weights_sha256": manifest.get("weights_sha256", ""),
            "type_count": type_count,
            "edge_count": int(manifest["edge_count"]),
        },
        "nodes": nodes,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prior", required=True, type=pathlib.Path)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare --out with the freshly derived layout instead of writing it",
    )
    args = parser.parse_args(argv)

    try:
        derived = layout(args.prior)
    except LayoutError as error:
        print(f"make_layout: {error}", file=sys.stderr)
        return 1

    text = json.dumps(derived, indent=2, sort_keys=True) + "\n"
    if args.check:
        try:
            committed = args.out.read_text(encoding="utf-8")
        except OSError as error:
            print(f"make_layout: {args.out}: {error}", file=sys.stderr)
            return 1
        if committed != text:
            print(f"make_layout: {args.out} is stale against {args.prior}", file=sys.stderr)
            return 1
        print(
            f"layout: OK ({derived['prior']['type_count']} nodes, "
            f"{derived['prior']['edge_count']} edges)"
        )
        return 0

    args.out.write_text(text, encoding="utf-8")
    print(
        f"layout: {derived['prior']['type_count']} nodes, "
        f"{derived['prior']['edge_count']} edges -> {args.out}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
