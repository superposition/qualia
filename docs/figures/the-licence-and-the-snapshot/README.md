# Figures — the licence and the snapshot

Three figures for the journal entry
[`the-licence-and-the-snapshot`](https://superposition.github.io/journal/the-licence-and-the-snapshot/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `provenance-check.svg` (+ `.png`) | chart | For each commit, three bars: files tracked in this repository, files whose relative path also exists in the private reference checkout, and files that are byte-identical to it. The third bar is zero at both commits. |
| `licence-flow.svg` (+ `.png`) | diagram | The clean-room boundary (dashed, labelled), and below it the three attribution sources — the relicensed workspace root, Leash (MIT), the Male CNS dataset (CC-BY 4.0) — fanning into `NOTICE`, which `.github/scripts/notice-check.sh` guards. |
| `turntable.glb` | 3D asset | The extruded mark (the committed `assets/mark/psi.glb`) plus three pillars per run in `provenance.json` — tracked files, files also in the reference, files byte-identical — the provenance chart's six numbers, each height proportional to its count, with a 60-frame rotation baked as an animation. |

## Data

`provenance.json` records the two runs of this repository's own
`scripts/provenance_check.py` that the chart draws:

```bash
cargo run --quiet -p qualia-gates -- provenance     # run at 144bb6d and at the head of main
```

At `144bb6d` (the commit that introduced the check, and the one that closed
EPIC-01's work) the tool printed `compared 104 file(s) ...; 0 identical` over 157
tracked files. At `a59b836` it printed `compared 110 file(s) ...; 0 identical`
over 170 tracked files. Both runs exited `0`. The reference checkout's path is
deliberately not recorded here.

The diagram's facts are the three committed files themselves (`LICENSE`,
`NOTICE`, `.github/scripts/notice-check.sh`) plus the workspace `license`
field in `Cargo.toml`.

## The turntable, and its fallback

Step 43's embed is the same figure inside the entry's existing
`<figure class="measurement">` markup — one asset, no player, and it works on a
phone:

```html
<figure class="measurement">
  <script type="module"
          src="https://ajax.googleapis.com/ajax/libs/model-viewer/3.5.0/model-viewer.min.js"></script>
  <model-viewer src="https://superposition.github.io/journal/the-licence-and-the-snapshot/turntable.glb"
                camera-controls auto-rotate disable-zoom></model-viewer>
  <figcaption>…</figcaption>
</figure>
```

If a third-party script is rejected at review, the fallback is the same figure
shipping the GLB as a download link — `turntable.glb` is committed beside this
README, so the link is the file — not a redesign. The static
`provenance-check.svg` is the no-WebGL floor either way.

`turntable.glb` is **not** relied on to be byte-reproducible: Blender's glTF
exporter writes the same structure, counts and animation every run (six nodes —
the mark mesh, one pillar per non-zero datum, and the `turntable` empty; one
`turntableAction` rotation channel over 60 frames) but the convention does not
treat its binary payload as stable, so a rebuild may show up as a binary diff
(`docs/figures/README.md` rule 6). Three consecutive runs on this workstation
were in fact byte-identical; the guarantee this figure makes is the structure,
not the bytes. `turntable.glb` is 73,196 bytes — the file's own count, not a
fixed size. `identical` is 0 at both commits and draws no pillar, so the
element has four. The file is committed so the figure has an asset to link
either way.

## Regenerating

```bash
python make_figures.py
"C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
    --python make_turntable.py
```

The script needs matplotlib, which the workstation's Python 3.8 does not have;
it was run under Python 3.13 with matplotlib 3.11.1. **This is the upstream
path, not the hand-authored fallback**: the charts are drawn by the committed
script, not pasted, so the numbers in them can only come from `provenance.json`.

Output is deterministic for a given matplotlib version — metadata dates are
suppressed and the SVG hash salt is fixed, and two consecutive runs were
verified byte-identical (`cmp` over each `.svg` and `.png`). Every file is under
120 KB, well inside the 400 KB budget. The palette is the house one, defined once
in [`../_house.py`](../_house.py).
