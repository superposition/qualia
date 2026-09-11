# Figures — the mark

Three figures for the journal entry
[`the-mark`](https://superposition.github.io/journal/the-mark/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `mark-geometry.svg` (+ `.png`) | chart, two panels | Left: each part of the monogram's centrelines — stem, left arm, right arm, descender — as the length of its centre line in viewBox units. Right: the same part as its x-extent and y-extent in viewBox units. Both panels are computed from `assets/mark/psi.json`, the geometry source |
| `mark-anatomy.svg` (+ `.png`) | diagram | The four parts stroked at the geometry source's proportions inside the inset-8, viewBox-100 frame `psi.svg` draws: stem and arms in ink `#edf0f5`, the descender in accent `#91dbba`, each part in a box naming its centre-line length |
| `turntable.glb` | 3D asset | The extruded mark (the committed `assets/mark/psi.glb`, itself built from `psi.svg`) plus one pillar per part of `psi.json`, its height the part's centre-line length — the chart's number — with a 60-frame rotation baked as an animation |

## Data

The one source is `assets/mark/psi.json`: four parts — `stem`, `left-arm`,
`right-arm`, `descender` — each a point list in a `0..1` unit square, each with
a `colour` key of `ink` or `accent`, plus the `viewbox` (100), `inset` (8) and
`stroke` (0.085) the flat mark is drawn with. Every number in the chart is
computed from that file, and every solid in the turntable is scaled from it.

The three downstream assets are committed beside it and are not inputs here:

```console
$ python assets/mark/render_mark.py             # psi.json -> psi.svg
$ "C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
      --python assets/mark/build_mark.py        # psi.svg -> psi.glb, psi-hero.*, psi-*.png
```

`mark-anatomy.svg` draws the same centre lines `psi.svg` draws, in the same
colours, at the source's inset; it is a reader's picture of the geometry, not a
substitute for the committed `psi.svg`.

## The turntable, and its fallback

Step 43's embed is the same figure inside the entry's existing
`<figure class="measurement">` markup — one asset, no player, and it works on a
phone:

```html
<figure class="measurement">
  <script type="module"
          src="https://ajax.googleapis.com/ajax/libs/model-viewer/3.5.0/model-viewer.min.js"></script>
  <model-viewer src="https://superposition.github.io/journal/the-mark/turntable.glb"
                camera-controls auto-rotate disable-zoom></model-viewer>
  <figcaption>…</figcaption>
</figure>
```

If a third-party script is rejected at review, the fallback is the same figure
shipping the GLB as a download link — `turntable.glb` is committed beside this
README, so the link is the file — not a redesign. The static
`mark-anatomy.svg` is the no-WebGL floor either way.

## Regenerating

```console
$ py -3.13 make_figures.py
$ "C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
      --python make_turntable.py
```

`make_figures.py` needs matplotlib, which the workstation's Python 3.8 does not
have; it was run under Python 3.13 with matplotlib 3.10.1. Output is
deterministic for a given matplotlib version (metadata dates suppressed, SVG
hash salt fixed) and two consecutive runs were verified byte-identical
(`cmp` over each `.svg` and `.png`). Blender is invoked by path because it is
not on `PATH`.

`turntable.glb` is **not** relied on to be byte-reproducible: Blender's glTF
exporter writes the same structure, counts and animation every run (six nodes —
the mark mesh, one pillar per non-zero part, and the `turntable` empty; one
`turntableAction` rotation channel over 60 frames) but the convention does not
treat its binary payload as stable, so a rebuild may show up as a binary diff
(`docs/figures/README.md` rule 6). Three consecutive runs on this workstation
were in fact byte-identical; the guarantee this figure makes is the structure,
not the bytes. Every input to it — `psi.json`, `psi.glb`, `psi.svg` — is
reproducible. The file is committed so the figure has an asset to link either
way.

## What this does not establish

The chart and the diagram describe the geometry source, not the mark as
rendered: the 3D mesh's face counts and the raster sizes are the Blender
scripts' business (`assets/mark/build_mark.py`), and no number here is read off
a raster. The turntable is a figure — the element is one pillar per part, not a
measurement of the mesh. Whether the mark reads well at 32 px is an eye's
judgement, not this check's.
