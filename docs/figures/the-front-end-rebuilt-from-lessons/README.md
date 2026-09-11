# Figures — the front end, rebuilt from lessons

Two figures for the journal entry
[`the-front-end-rebuilt-from-lessons`](https://superposition.github.io/journal/the-front-end-rebuilt-from-lessons/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `lesson-sources.svg` (+ `.png`) | chart | How many of the console's decisions each of the five front ends that `docs/frontend-lessons.md` read informs, counted from the document's closing "Comes from" column; the most-named source is picked out. |
| `hero.webp` (+ `.png`) | render | The extruded psi mark beside the console's five views as five staggered extruded tiles, in `VIEWS` order — the five floating windows the console opens. |

## Data

`docs/frontend-lessons.md` is read directly by both the chart and the hero.
The chart counts, per source, the decisions whose "Comes from" cell names it
(nine decisions, eighteen references; a decision may name several sources). The
hero's tiles are the console's own `VIEWS` table in
`apps/qualia-console/src/lib.rs`, in the order the console declares it:
mission, belief, world, evidence, telemetry. The lessons document is the entry's
substance — it is the piece a reader should follow from the entry.

## Regenerating

```bash
py -3.13 docs/figures/the-front-end-rebuilt-from-lessons/make_figures.py
"C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background --factory-startup \
    --python assets/mark/build_mark.py -- --entry the-front-end-rebuilt-from-lessons
```

Blender is invoked by path because it is not on `PATH`. The chart is drawn by the
committed script from the committed document; the hero is rendered by
`assets/mark/build_mark.py`, whose `--entry` mode reads the same `VIEWS` table
through `assets/mark/entries.json`. Output is deterministic — metadata dates are
suppressed, the SVG hash salt is fixed and Blender's stamp flags are off — so two
runs are byte-identical. Every asset is under 400 KiB. The palette is the house
one, defined once in [`../_house.py`](../_house.py).
