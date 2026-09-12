# T56 — the floating brain

This directory backs #237's claim that the floating brain is visible inside qualia's own console: the
released male-CNS connectome as a point cloud, with each tick's firing set drawn over it. It is
**not** a profiler capture (the ticket changes no kernel and carries no `needs:profile` label).

**The acceptance artefact is the console panel**, rendered headlessly through the console's own
snapshot surface. D-023 forbids putting windows on the operator's desktop, so the Rerun viewer is not
an acceptance artefact: the Rerun projection exists as the secondary path and writes its recording
with `qualia-connectome-view view --positions <artifact> --spikes <run> --out brain.rrd`, no window
involved.

## What is here

| file | what it shows |
| --- | --- |
| `console-brain-panel.png` | the Brain panel with the **real runner stream** loaded: the cloud and the tick's firing set |
| `console-brain-panel-no-stream.png` | the same panel with no stream configured — the committed `brain_fresh.png`, byte-identical, kept because it is what CI renders |

Both are rendered by `apps/qualia-console/tests/snapshots/brain_fresh`, so neither can drift from the
panel: the test fails if it does.

```powershell
# with the runner's stream (writes the mismatching frame as brain_fresh.new.png):
QUALIA_CONNECTOME_SPIKES=<artifact>/spikes.bin `
  cargo test -p qualia-console --test snapshots -j 2 -- brain_fresh
copy apps/qualia-console/tests/snapshots/brain_fresh.new.png docs/evidence/T56/floating-brain/console-brain-panel.png

# with no env at all — the committed snapshot, and the in-tree artifact:
cargo test -p qualia-console --test snapshots -j 2 -- brain_fresh
copy apps/qualia-console/tests/snapshots/brain_fresh.png docs/evidence/T56/floating-brain/console-brain-panel-no-stream.png
```

The panel in the firing figure reads: the artifact `committed assets/brain/connectome-cns`,
`166700 nodes, 139662 placed, 11752 types`, source `body-annotations.feather`,
the stream's tick and its firing count, and `cloud draw` — what the layer drew. On the frames this
figure was taken from the layer drew **9,976 of the 139,662 placed points and 3,325–3,730 of the tick's
19,223–64,135 firing nodes** (the cloud is decimated to 10,000 and the firing set strided to 4,096, and
the panel reports both, like every other layer in that scene).

## The stream this figure shows

`spikes.bin` — 40,199,848 B, sha256
`a0a27acbe61e0a9a5f013f77a9741ce466221b872bba711f7a07409f68593b28` (T55 / #236), read by
`qualia-connectome-stream`: magic `QLSP`, u16 version, u16 reserved (8 bytes), then frames
`tick u64 | t_ns u64 | count u32 | ids u32[]`.

| | |
| --- | --- |
| frames | 200 (ticks 0..199), 10,048,960 firing ids in total |
| ids | CSR node indices, `0..166,699` — the same row order as `positions.bin`, so an id indexes a row directly |
| first non-empty tick | tick 3 fires 19,223; tick 4 58,109; tick 5 44,431; tick 6 36,480 (ticks 0–2 fire nothing: the network starts at rest) |
| driven by | the robot's own camera, through the leash's JPEG snapshot endpoint (`http://192.168.55.1:8000/camera/snapshot`), one snapshot per tick, host GPU, 112.7 ticks/s |
| read back by this repo | `qualia-connectome-view view --positions assets/brain/connectome-cns --spikes spikes.bin --max-ticks 5` → 5 ticks, 77,332 firing points, 2,533,686 points/s logged, positions sha256 `016a285b…afa6e` |

**Caveat the producer gave, recorded here:** the camera scene was nearly uniform (luminance
0.4613–0.4679), so the drive is weak and the network's recurrent dynamics dominate; a run on the
Orin's own rate may replace the file at the same path, and if it differs we will say so.

## The artifact

Committed at `assets/brain/connectome-cns/` — `positions.bin`, `types.txt`, `nodes.txt`, the
importer's `manifest.json` and `attribution.json`, with the digests in the README beside them.
`weights.bin` (180,414,198 B) stays off-tree by digest. `qualia-connectome-view verify --positions
assets/brain/connectome-cns` re-reads it and checks every section digest:

```text
schema=qualia.connectome-cns.v1 manifest=present nodes=166700 placed=139662 unplaced=27038 cell_types=11752
positions.bin: 3334012 bytes sha256=016a285b01756f16b44219fa84fe2a6d83f6276c22e01f24f5d73a17c41afa6e
types.txt: 92587 bytes sha256=545ec649b861b3cdeb41f597471b0061494007db9f25c592f2a43bbcf317024c
sections checked against the manifest: 5/5
source table: 211577 bodies, 139662 positioned, 11752 type labels
artifact: OK
```

The network the cloud belongs to, on the importer's corrected measurement: **166,700 neurons,
25,582,938 neuron-level edges, 124,177,617 synapses** (Σ weight), +94,542,746 excitatory,
−26,403,637 inhibitory, 3,232,234 unknown. The 151,856,684-row / 311,833,243-weight total is the
segment-resolution figure, not the neuron-level number.

**Coverage, said plainly:** 139,662 of the 211,577 bodies in the annotation table carry a
`somaLocation`, so the cloud draws the somata we have, not every neuron; the other 27,038 CSR nodes
are left out rather than placed at a made-up coordinate.

## What is not established here

- **The live `tcp://host:port` path is unexercised.** The recorded file is the path that matters; the
  socket framing carries the same bytes but has only been checked over a local socket, never over the
  USB link to the board.
- The camera drive was weak (near-uniform scene, above), so this is the connectome's own dynamics
  under a weak visual drive, not a claim about what the fly does with a real visual scene.
- This is a visualization of the network's activity, not evidence about the fly's behaviour. Nothing
  here claims "the fly is in there" — the same caveat the connectomics community put on the Eon video.
