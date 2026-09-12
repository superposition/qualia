# T56 — the floating brain

This directory backs #237's claim that the floating brain is visible inside qualia's own console: the
released male-CNS connectome as a point cloud, with each tick's firing set drawn over it. It is
**not** a profiler capture (the ticket changes no kernel and carries no `needs:profile` label), and it
carries no `capture.json` or `kernels.json`.

## What is here

| file | what it shows |
| --- | --- |
| `console-brain-panel.png` | the console's Brain panel, rendered offscreen, with the real artifact loaded |

The console window is never opened on this machine: evidence captures are headless. The figure is the
console's **own** snapshot surface (`apps/qualia-console/tests/snapshots/`), driven by
`egui_kittest`'s wgpu backend over the same `views::brain::render` the window calls.

## The figure, and how to reproduce it

```powershell
# The panel with the real artifact: the committed snapshot deliberately does not
# change, so this run fails its compare and writes the rendered frame as
# `brain_fresh.new.png`, which is copied here.
QUALIA_CONNECTOME_DIR=C:/tmp/Impl236ConnectomeRunner/artifact `
  cargo test -p qualia-console --test snapshots -j 2 -- brain_fresh
copy apps/qualia-console/tests/snapshots/brain_fresh.new.png docs/evidence/T56/floating-brain/console-brain-panel.png
```

The committed `brain_fresh.png` is blessed **without** `QUALIA_CONNECTOME_DIR`, so it stays
host-independent (the panel prints the artifact's directory, which is a host path); the environment
above is what makes the cloud appear.

The panel in the figure reads, in its own words:

- `cloud artifact` — the artifact directory the panel read;
- `cloud counts` — `166700 nodes, 139662 placed, 11752 types  (male CNS v1.0)`;
- `cloud source` — `body-annotations-male-cns-v1.0-minconf-0.5.feather`;
- `spike stream` — the unset arm, because the runner's stream is not on disk yet (see below);
- `cloud draw` — how many of the placed points and of the tick's firing set the layer drew.

The 3D canvas draws the cloud decimated to at most 10,000 points (the layer reports what it dropped,
like every other layer in this scene) and draws the tick's firing nodes on top, bright and larger.

## The artifact

Written by the importer in `crates/connectome-cns` (T55 / #236) at
`C:/tmp/Impl236ConnectomeRunner/artifact/`, read here by `qualia-connectome-stream` and checked by
`qualia-connectome-view verify`:

```text
schema=qualia.connectome-cns.v1 manifest=present nodes=166700 placed=139662 unplaced=27038 cell_types=11752
positions.bin: 3334012 bytes sha256=016a285b01756f16b44219fa84fe2a6d83f6276c22e01f24f5d73a17c41afa6e
types.txt: 92587 bytes sha256=545ec649b861b3cdeb41f597471b0061494007db9f25c592f2a43bbcf317024c
section id: sha256=fe2d710121425c82e242536b3b438c6ad62962cf96ec7d514a444639954a6e06
section x: sha256=7d73170c5e99792896308b7114e064196300e69a082c5289e519addac7ace7d4
section y: sha256=23422ba1d93c20c9e8c3453e28a8da013eae7f709f4d5fef69fd44aca47455de
section z: sha256=6f1d498e7ea3d1626df5faa2c527ee030db5500107dcf888d983fae19ac47d61
section cell_type: sha256=4af34959d1fdefc38191b8c22a06594dac3a6133a859ce5aa01d7dfb0e6d0a33
sections checked against the manifest: 5/5
source table: 211577 bodies, 139662 positioned, 11752 type labels
artifact: OK
```

The network the cloud belongs to, on the importer's corrected measurement: **166,700 neurons,
25,582,938 neuron-level edges, 124,177,617 synapses** (Σ weight), of which +94,542,746 excitatory,
−26,403,637 inhibitory, 3,232,234 unknown. The 151,856,684-row / 311,833,243-weight total is the
segment-resolution figure and is not the neuron-level number.

**Coverage, said plainly:** 139,662 of the 211,577 bodies in the annotation table carry a
`somaLocation`, so the cloud draws the somata we have, not every neuron; the other 27,038 CSR nodes
have no position and are left out rather than placed at a made-up coordinate.

## What is not here yet

- **The firing stream.** `spikes.bin` is T55's runner output and had not landed when this figure was
  taken, so the panel's `spike stream` line is the unset arm. Pointing
  `QUALIA_CONNECTOME_SPIKES` at the recorded file (or `tcp://host:port` for the runner over the USB
  link) is the only change needed; the highlight and the per-tick framing are already exercised by
  `the_spike_stream_round_trips_a_recorded_run`.
- **A Rerun figure.** The Rerun projection is the secondary artefact; its recording is written
  headlessly (`qualia-connectome-view view --positions <artifact> --spikes <run> --out brain.rrd`),
  but the Rerun *viewer* needs a window, so no Rerun screenshot is taken here.

## What this does not establish

This is a visualization of the network's activity, not evidence about the fly's behaviour. Nothing
here claims "the fly is in there" — the same caveat the connectomics community put on the Eon video.
