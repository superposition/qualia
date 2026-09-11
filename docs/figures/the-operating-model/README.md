# Figures — the operating model

Two figures for the journal entry
[`the-operating-model`](https://superposition.github.io/journal/the-operating-model/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `ledger-status.svg` (+ `.png`) | chart, two panels | Left: the 95 ticket issues by status label. Right: how many of those ticket ids the committed dependency diagram `docs/architecture/epics.mmd` draws, and how many it omits. |
| `operating-loop.svg` (+ `.png`) | diagram | The claim → work → PR → breadcrumb → merge loop, the `request-changes` edge back to the work, the blocked branch, the agent-dies branch (label `resume`, read the last `braid` block), and the one resume command a cold shell runs. |
| `turntable.glb` | 3D asset | The extruded mark (the committed `assets/mark/psi.glb`) plus one pillar per `status:*` label in `ledger-snapshot.json` — the ledger chart's five counts — its height proportional to the count, with a 60-frame rotation baked as an animation. |

## Data

`ledger-snapshot.json` is one timestamped reading of a live tracker:

```bash
gh issue list --repo superposition/qualia --label ticket --state all --limit 300 \
  --json number,labels
```

At `2026-09-11T05:43:11Z`, at `e2d27ca`: 95 ticket issues, 16 epic issues, 19
labels, and by status label 56 `ready`, 19 `claimed`, 12 `done`, 8 `review`, 0
`blocked` — which sums to 95. Twelve tickets were already closed while their
status label still read `review` or `claimed`.

The right panel's three missing ids (`C43`, `C44`, `C45`) come from the check the
plan records:

```bash
comm -23 \
  <(gh issue list --repo superposition/qualia --label ticket --state all --limit 300 \
      --json title --jq '.[].title' | grep -oE '^[TC][0-9]+b?' | sort -u) \
  <(grep -oE '[TC][0-9]+b?' docs/architecture/epics.mmd | sort -u)
```

`epics.mmd` was last written at `01df4e2`; those three tickets were created after
it, and its in-file note (`docs/architecture/README.md`) says the `.mmd` files
are the source and the README blocks are copies.

The loop diagram's facts are `docs/agents.md` (the breadcrumb block, the claim
command, the handoff and blockage rules, the resume command),
`docs/architecture/agents.mmd`, `docs/architecture/states.mmd` and
`docs/waves.md`.

## The turntable, and its fallback

Step 43's embed is the same figure inside the entry's existing
`<figure class="measurement">` markup — one asset, no player, and it works on a
phone:

```html
<figure class="measurement">
  <script type="module"
          src="https://ajax.googleapis.com/ajax/libs/model-viewer/3.5.0/model-viewer.min.js"></script>
  <model-viewer src="https://superposition.github.io/journal/the-operating-model/turntable.glb"
                camera-controls auto-rotate disable-zoom></model-viewer>
  <figcaption>…</figcaption>
</figure>
```

If a third-party script is rejected at review, the fallback is the same figure
shipping the GLB as a download link — `turntable.glb` is committed beside this
README, so the link is the file — not a redesign. The static
`operating-loop.svg` is the no-WebGL floor either way.

`turntable.glb` is **not** relied on to be byte-reproducible: Blender's glTF
exporter writes the same structure, counts and animation every run (six nodes —
the mark mesh, one pillar per non-zero status, and the `turntable` empty; one
`turntableAction` rotation channel over 60 frames) but the convention does not
treat its binary payload as stable, so a rebuild may show up as a binary diff
(`docs/figures/README.md` rule 6). Three consecutive runs on this workstation
were in fact byte-identical; the guarantee this figure makes is the structure,
not the bytes. `status:blocked` is 0 in the snapshot and draws no pillar, so the
element has four. The file is committed so the figure has an asset to link
either way.

## Regenerating

```bash
python make_figures.py
"C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
    --python make_turntable.py
```

The script needs matplotlib, which the workstation's Python 3.8 does not have; it
was run under Python 3.13 with matplotlib 3.11.1. **This is the upstream path,
not the hand-authored fallback.** Output is deterministic for a given matplotlib
version (metadata dates suppressed, SVG hash salt fixed) and two consecutive runs
were verified byte-identical. Every file is under 120 KB, inside the 400 KB
budget. The palette is the house one, defined once in
[`../_house.py`](../_house.py).
