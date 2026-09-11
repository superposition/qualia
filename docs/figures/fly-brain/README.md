# `fly-brain` — the 3D firing model and the belief matrices

Figures for T51. Each is a static record of one of the ticket's milestones: the layout, the firing
model, and the belief matrices.

| File | Kind | What it shows |
| --- | --- | --- |
| `brain-layout.svg` (+ `.png`) | render | The committed prior in its committed spectral layout: 5 types, 9 edges, node size by degree. |
| `brain-firing.svg` (+ `.png`) | render | Node intensity from the fly rate model's rate vector, and each edge's pulse as `weight × rate[source]` — the coupling term `crates/fly-circuit` integrates. |
| `brain-matrices.svg` (+ `.png`) | chart | One layer's decimated generative weight matrix and belief mean as heatmaps, over the short time axis with the braid's promotion marker and the coupling-scale marker on it. |
| `turntable.glb` | 3D asset | The same layout and firing, nodes and edges, with a 60-frame rotation baked as an animation (the T43 pattern). |
| `firing-sample.json` | data | The recording every figure above reads. |

The palette is the house palette (`docs/figures/_house.py`): background `#101217`, ink `#edf0f5`,
accent `#91dbba`, with the console's muted `#5c6673` as the idle end of the ramp.

## Where the numbers come from

`firing-sample.json` is not hand-written. `apps/qualia-console/examples/brain_evidence.rs` loads the
committed prior, steps `crates/fly-circuit`'s rate model over it, publishes the model state and
evolving belief slots into a fresh shared region, and samples that region through
`BrainView::sample` — the console's own read path. The figures, the turntable and the panel therefore
draw the same numbers.

```console
$ cargo run -p qualia-console --example brain_evidence
brain evidence: 5 types, 9 edges, peak rate 0.7763, 64 lidar points, 512 history frames, 2 markers -> docs/figures/fly-brain/firing-sample.json
$ py -3.13 docs/figures/fly-brain/make_figures.py
figures: 5 nodes, 9 edges, peak rate 0.7763, 8 layers, 512 history frames, 2 markers
$ "C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
      --python docs/figures/fly-brain/make_turntable.py
turntable: 5 nodes, 9 edges, 60 frames -> docs/figures/fly-brain/turntable.glb (91088 bytes)
```

Blender is invoked by path because it is not on `PATH`. `turntable.glb` is **not**
byte-reproducible: Blender's glTF exporter writes the same structure, counts and animation every run
(15 nodes, one `turntableAction` rotation channel over 60 frames) but not the same bytes in the
binary payload, so a rebuild shows up as a binary diff. Every input to it —
`assets/brain/layout.json`, the prior and `firing-sample.json` — is reproducible; only the exporter's
float emission is not. The file is committed so the figure has an asset to link either way.

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
