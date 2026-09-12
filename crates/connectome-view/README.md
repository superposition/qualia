# `qualia-connectome-view`

The desktop entry point for the floating brain: the released male-CNS connectome as a Rerun point
cloud, with each tick's firing set highlighted, on the tick timeline so scrubbing replays the
activity.

It is a projection, not a controller. It reads a positions artifact
(`qualia-connectome-stream`) and a stream of firing sets, logs them through `qualia-rerun-bridge`
into a Rerun recording, and has no path back into the network.

## Two ways in, one projection

| mode | source | how |
|---|---|---|
| **replay** | a recorded `QLSP` run on disk | `--spikes <file>` |
| **live** | the runner on the Jetson over the USB link | `--live <host:port>` |

Both hand the same reader the same bytes, and both log the same projection, so the viewer code does
not know which one it is running.

### Replay

```powershell
# record the projection into a file, then open it (scrub the tick timeline):
cargo run -p qualia-connectome-view -- view --positions <artifact-dir> --spikes <run.bin> `
    --out brain.rrd --speed 0
rerun brain.rrd

# or watch it at the stream's own pace:
rerun --serve-grpc
cargo run -p qualia-connectome-view -- view --positions <artifact-dir> --spikes <run.bin> `
    --attach rerun+http://127.0.0.1:9876/proxy --speed 1
```

### Live

```powershell
# on the desktop:
rerun --serve-grpc
# while the runner is serving its stream on the board (USB link, 192.168.55.x):
cargo run -p qualia-connectome-view -- view --positions <artifact-dir> --live 192.168.55.1:7777 `
    --attach rerun+http://127.0.0.1:9876/proxy --speed 1
```

`--speed 1` paces the replay against the stream's own `t_ns`, `--speed 0` logs as fast as the reader
goes (what the throughput figure is measured with). `--max-ticks` bounds a run.

### Verify

```powershell
cargo run -p qualia-connectome-view -- verify --positions <artifact-dir>
```

Recomputes every section digest, the file digests and the counts, and compares them with
`manifest.json`; exits non-zero on any disagreement.

## Entity paths

Stable and public, matching `crates/rerun-bridge/src/connectome.rs`:

- `qualia/connectome/brain/points` — every placed node, one point, coloured by cell type (logged
  once, static);
- `qualia/connectome/brain/firing` — the firing set of the current tick, larger and hotter than the
  cloud, logged on the `qualia_tick` timeline;
- `qualia/connectome/brain/summary` — the cloud's counts and its palette rule;
- `qualia/connectome/stream/summary` — the tick, its wall clock and the firing counts;
- `qualia/connectome/stream/firing_count` — firing count per tick, the curve the time-series pane
  plots.

The default Brain blueprint opens the 3D cloud beside the two documents and the curve. Colours come
from a golden-ratio walk over the cell-type index, so a type keeps one colour without the recording
carrying an eleven-thousand-entry palette; the firing colour is brighter than any of them.

## What the cloud shows

One point per CSR node the annotations place: 139,662 somata of the 211,577 bodies in the annotation
table, for a network of 166,700 neurons and ~1.07M neuron-level edges (see
`crates/connectome-stream/README.md` for the counting basis). Unplaced nodes are not drawn.

## What this does not establish

This is a visualization of the network's activity, not evidence about the fly's behaviour. Nothing
here claims "the fly is in there"; the same caveat the connectomics community put on the Eon video
applies.
