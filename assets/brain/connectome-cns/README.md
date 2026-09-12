# `assets/brain/connectome-cns` — the released male-CNS connectome, positions

The console's Brain view (`apps/qualia-console`) draws this: one point per placed soma of the
released male-CNS connectome, coloured by cell type, with the runner's firing set drawn over it. The
artifact is built by the importer in `crates/connectome-cns` (T55 / #236) from the public release and
committed here so the console needs no environment variable and a reader can reproduce the cloud from
the tree.

| Path | Bytes | SHA-256 |
| --- | --- | --- |
| `positions.bin` | 3,334,012 | `016a285b01756f16b44219fa84fe2a6d83f6276c22e01f24f5d73a17c41afa6e` |
| `types.txt` | 92,587 | `545ec649b861b3cdeb41f597471b0061494007db9f25c592f2a43bbcf317024c` |
| `nodes.txt` | 5,841,627 | `52e7c6a88217b5f47f6a5c7979353fea946e91eaedab9676f876fba41c8c2f84` |
| `manifest.json` | 2,890 | the importer's own manifest: counts, per-section digests, source digests, attribution |
| `attribution.json` | 172 | CC-BY-4.0, the release's citation |

## The layout

`positions.bin` is magic `QLCB`, a `u16` version, a `u16` reserved word, a `u32` node count, then the
SoA sections `id u32[N] | x f32[N] | y f32[N] | z f32[N] | cell_type u32[N]` — the reader is
`crates/connectome-stream` (`qualia-connectome-view verify --positions <this directory>` re-reads it
and checks every section digest against `manifest.json`). **Row `k` is CSR node `k`**, in the order
`weights.bin`'s rows are in, so a firing id from `spikes.bin` indexes a row directly and no lookup
table sits between them. `id` is the released body id; `cell_type` indexes `types.txt`, one interned
label per line (line 0 is the empty label). `nodes.txt` is the same mapping for a human: body id,
type, superclass, side and instance name, one line per node. Of the 166,700 released neurons,
**164,506** carry a `type` in the annotation table; the other 2,194 index the empty label.

| | |
| --- | --- |
| nodes | 166,700 (the CSR node count) |
| placed | 139,662 rows carry a `somaLocation` |
| unplaced | 27,038 rows have `x = y = z = NaN` |
| cell types | 11,752 interned labels — 11,751 distinct non-null `type` values in the annotation table, plus the empty label the importer interns for the 2,194 neurons that have no type |
| coordinates | the dataset's raw 8 nm EM voxel units, unrescaled and uncentred; the runtime centres and scales them into the scene's unit frame, nothing is written back |
| source table | `body-annotations-male-cns-v1.0-minconf-0.5.feather`, 211,577 bodies |

An unplaced row is a node the annotations place nowhere. It keeps its id and its cell type, the
viewer leaves it out of the cloud rather than drawing it at a made-up coordinate, and the panel
reports the placed count beside the node count. **The cloud therefore shows the somata we have, not
every neuron** — 139,662 of the 211,577 bodies in the annotation table.

The network these nodes belong to, on the importer's measurement: **166,700 neurons, 25,582,938
neuron-level edges, 124,177,617 synapses** (Σ weight), +94,542,746 excitatory / −26,402,637
inhibitory / 3,232,234 unknown. The release's 151,856,684 weight rows and Σweight 311,833,243 are the
segment-resolution totals and are not the neuron-level number.

## Rebuilding it

```console
$ cargo run -p qualia-connectome-cns -j 2 -- import \
      --weights           flat-connectome/connectome-weights-male-cns-v1.0-minconf-0.5.feather \
      --annotations       flat-connectome/body-annotations-male-cns-v1.0-minconf-0.5.feather \
      --neurotransmitters flat-connectome/body-neurotransmitters-male-cns-v1.0.feather \
      --out <artifact>
```

The three inputs are the public release under `gs://flyem-male-cns/v1.0/connectome-data/flat-connectome/`
(HTTPS mirror works). The artifact is deterministic apart from `manifest.json`'s `created_at_ms`.

## What is not committed, and why

`weights.bin` — 180,414,198 B, sha256 `53d87d18bf9515cbc9d58ecc64c5741917687e5a6c6dd95a917323a388b7b318`
— stays out of the tree: it is the bulk edge data a view does not need, and the importer above
reproduces it byte for byte from the released tables. The line is: bytes a build or a view needs are
in-tree; the bulk edges are referenced by digest.

## Licence

The Male CNS release is CC-BY-4.0 (<https://male-cns.janelia.org>); `attribution.json` carries the
citation. Provenance: the bytes here were written by `crates/connectome-cns`'s Rust importer from the
released tables; nothing in the console re-derives them.
