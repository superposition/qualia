# `assets/brain` — the fly brain's layout and its prior

The console's Brain view (`apps/qualia-console`, the sixth view) draws the connectome graph the
belief layers couple. The graph is an artifact built by `crates/connectome-prior`; the *layout* the
graph is drawn in is computed here, offline, once, and committed, because a force or spectral layout
is a function of the graph and has no business running while the robot is driving.

| Path | What it is |
| --- | --- |
| `prior/` | The reference prior artifact: `graph.bin` (rowptr/cols/weights), `manifest.json` (per-section SHA-256), `attribution.json` (CC-BY). |
| `layout.json` | One 3D position per prior type, plus the digests of the prior it was derived from. |
| `make_prior.py` | Regenerates the Arrow IPC inputs the prior builder reads, from the crate's committed CSV fixtures. |
| `make_layout.py` | Derives `layout.json` from a prior directory; `--check` fails if the committed layout is stale. |

## The prior

The deployment's prior is whatever `QUALIA_FLY_PRIOR_PATH` names: the runtime
(`runners/jepa-runtime`) loads the same directory with `qualia-fly-circuit`, and the console loads it
with the same schema so the graph on screen is the graph being coupled. The reference artifact
committed here is built by the real builder from the crate's hand-authored fixtures
(`crates/connectome-prior/tests/fixtures/`), so the whole chain — fixtures to CSR to layout to figure
— is reproducible in the repository without the full Male CNS dataset, which is external
(<https://male-cns.janelia.org>, CC-BY 4.0, and not vendored).

The committed reference prior is **5 types and 9 edges** (its `manifest.json` carries the exact
counts and the per-section digests). It is deliberately small: it is the same prior the crate's own
tests exercise, and a graph that fits in 120 bytes of `graph.bin` keeps the layout, the figures and
the console snapshot cheap and deterministic. Point `make_layout.py` at the Male CNS prior to get a
layout at the fly's real scale; the algorithm does not care about the size.

## The layout

`layout.json` is a **seeded spectral embedding** of the prior:

1. read `manifest.json` and `graph.bin` and check the CSR invariants (monotone `rowptr`, columns
   inside the type range, byte length implied by the manifest);
2. symmetrise the directed rows as `A = W + Wᵀ`;
3. form the normalized Laplacian `L = I − D^(−1/2) A D^(−1/2)`;
4. take the three lowest non-trivial eigenvectors — dense `numpy.linalg.eigh` at up to 512 types,
   `scipy.sparse.linalg.eigsh` with a fixed Krylov start above that;
5. fix each eigenvector's sign (make its largest-magnitude component positive, lowest index breaking
   a tie), scale the three axes into `[-1, 1]`, and round to 6 decimals.

It is deterministic: the same prior always gives the same file, and a different prior gives a
different one. `layout.json` records the prior's `source_sha256`, `graph_sha256`, `type_count` and
`edge_count`; the console refuses to draw a graph with a layout that does not match the prior it
loaded, rather than place nodes at meaningless coordinates.

* `algorithm`: `spectral-laplacian-v1`
* `seed`: `20260911` (the fixed Krylov start vector / any future restart)

## Regenerating

The Feather inputs the builder reads are synthesised from the committed CSV fixtures (the builder's
arrow build carries no lz4 codec, so the IPC files are written uncompressed):

```console
$ py -3.13 assets/brain/make_prior.py --out /tmp/brain-inputs
$ cargo run -p qualia-connectome-prior -j 2 -- \
      --edges /tmp/brain-inputs/weights.feather \
      --annotations /tmp/brain-inputs/annotations.feather \
      --out assets/brain/prior
prior: 5 types, 9 edges -> assets/brain/prior
$ py -3.13 assets/brain/make_layout.py --prior assets/brain/prior --out assets/brain/layout.json
layout: 5 nodes, 9 edges -> assets/brain/layout.json
```

To check the committed layout rather than rewrite it:

```console
$ py -3.13 assets/brain/make_layout.py --prior assets/brain/prior --out assets/brain/layout.json --check
layout: OK (5 nodes, 9 edges)
```

For a deployment's own prior:

```console
$ py -3.13 assets/brain/make_layout.py --prior "$QUALIA_FLY_PRIOR_PATH" --out /tmp/layout.json
```

The builder refuses to overwrite an artifact whose `source_sha256` already matches
(`PriorError::AlreadyBuilt`, exit non-zero); remove `assets/brain/prior` first to rebuild it. Its
`manifest.json` carries a `created_at_ms` stamp, so the manifest bytes change per build while
`graph.bin` — and therefore the layout — do not.
