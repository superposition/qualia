# `fly-brain` — the 3D firing model and the belief matrices

Figures for T51. Each is a static record of one of the ticket's milestones: the layout, the firing
model, and the belief matrices.

| File | Kind | What it shows |
| --- | --- | --- |
| `brain-layout.svg` (+ `.png`) | render | The committed prior in its committed spectral layout: 5 types, 9 edges, node size by degree. |
| `brain-firing.svg` (+ `.png`) | render | Node intensity from the belief slots' activity (per layer), and each edge's pulse as `weight × rate[source]` — the published fly rate vector, which the coupling term `crates/fly-circuit` integrates. |
| `brain-matrices.svg` (+ `.png`) | chart | One layer's decimated generative weight matrix and belief mean as heatmaps, over the short time axis with the braid's promotion marker and the coupling-scale marker on it. |
| `turntable.glb` | 3D asset | The same layout and firing, nodes and edges, with a 60-frame rotation baked as an animation (the T43 pattern). |
| `firing-sample.json` | data | The recording every figure above reads. |

The palette is the house palette (`docs/figures/_house.py`): background `#101217`, ink `#edf0f5`,
accent `#91dbba`, with the console's muted `#5c6673` as the idle end of the ramp.

## Where the numbers come from

`firing-sample.json` is not hand-written. `apps/qualia-console/examples/brain_evidence.rs` loads the
committed prior, steps `crates/fly-circuit`'s rate model over it **with a synthetic drive** (types 0
and 2; the runtime drives no type yet), publishes the model state and evolving belief slots into a
fresh shared region, and samples that region through `BrainView::sample` — the console's own read
path. The figures, the turntable and the panel therefore draw the same numbers.

The view reads two sources, and the split is deliberate:

* **Node intensity is the belief slots' activity**, live. The prior carries no type-to-layer map, so
  the crossing is stated: type `t` reads the `belief` slot of layer `t % 8`, and its intensity is
  that layer's belief mean at index `t / 8` (the same decimation the matrix panels read). In a
  running stack the belief runners write these slots, so the graph lights from live belief.
* **Edge pulses are `weight × rate[source]` from the published fly rate vector** (the `FlySimSlot`
  `crates/fly-circuit`'s model publishes), rendered faithfully. The fly drive is not wired yet, so a
  live stack publishes an all-zero vector and the pulses are **flat** — that is the honest picture of
  the model at rest, and wiring the drive belongs to the T30/T31 work. The pulses in these committed
  figures come from the example's synthetic drive above, not from a runner.

```console
$ cargo run -p qualia-console --example brain_evidence
brain evidence: 5 types, 9 edges, peak rate 0.7763, 64 lidar points, 512 history frames, 2 markers -> docs/figures/fly-brain/firing-sample.json
$ py -3.13 docs/figures/fly-brain/make_figures.py
figures: 5 nodes, 9 edges, belief node peak 0.4474, peak rate 0.7763, 8 layers, 512 history frames, 2 markers
$ "C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
      --python docs/figures/fly-brain/make_turntable.py
turntable: 5 nodes, 9 edges, 60 frames -> docs/figures/fly-brain/turntable.glb (90180 bytes)
```

Blender is invoked by path because it is not on `PATH`. `turntable.glb` is **not**
byte-reproducible: Blender's glTF exporter writes the same structure, counts and animation every run
(15 nodes, one `turntableAction` rotation channel over 60 frames) but not the same bytes in the
binary payload, so a rebuild shows up as a binary diff — and the byte count above is the committed
file's own, not a fixed size. Every input to it — `assets/brain/layout.json`, the prior and
`firing-sample.json` — is reproducible; only the exporter's float emission is not. The file is
committed so the figure has an asset to link either way.

The markers on the time axis are real: `PromotionAccepted` is the committed braid fixture's
`last_promotion_ns`, and the coupling-scale marker is the dial the example observed through the same
`QUALIA_FLY_COUPLING_SCALE` / stack-manifest read the panel uses.

## The turntable, and its fallback

T43's embed is one asset and no player:

```html
<script type="module"
        src="https://ajax.googleapis.com/ajax/libs/model-viewer/3.5.0/model-viewer.min.js"></script>
<model-viewer src="turntable.glb" camera-controls auto-rotate disable-zoom></model-viewer>
```

If a third-party script is rejected at review, the fallback is the same figure shipping the GLB as a
download link — `turntable.glb` is committed beside this README, so the link is the file — not a
redesign. The static `brain-layout.svg` and `brain-firing.svg` are the no-WebGL floor either way.

## What this does not establish

* The committed prior is the crate's small reference graph (see `assets/brain/README.md`), not the
  Male CNS dataset: the dataset is external and not vendored here. The same commands run at the
  fly's real scale by pointing `make_layout.py` at the deployment's `QUALIA_FLY_PRIOR_PATH`.
* A frame count on the time axis is presentation, not measurement: the axis is the console's own
  short history, and no journal number is read off it. The frame cost is measured separately (see
  `apps/qualia-console/tests/brain_frame.rs`).
