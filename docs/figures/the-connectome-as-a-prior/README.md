# Figures — the connectome as a prior

Two figures for the journal entry
[`the-connectome-as-a-prior`](https://superposition.github.io/journal/the-connectome-as-a-prior/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `csr-matrix.svg` (+ `.png`) | chart | The committed fixture's 12 segment rows summed into the 9 nonzero `(pre_type, post_type)` pairs of the type-level CSR, as a weight matrix; empty cells are the 16 absent pairs. |
| `hero.webp` (+ `.png`) | render | The extruded psi mark beside the same 9 nonzero pairs as raised blocks on the 5×5 type lattice, each block's height its summed weight. |

## Data

`crates/connectome-prior/tests/fixtures/mini-type-edges.csv` — the committed
hand-authored fixture, 12 rows over the five types `DNp01`, `EPG`, `KCg-m`,
`OA-VPM3`, `PEN_a`. Both the chart and the hero element aggregate it the same
way `crates/connectome-prior`'s own oracle does: sum `weight` per
`(pre_type, post_type)` pair, leaving 9 pairs. The fixture README states the
same invariants (12 rows, 5 types, 9 pairs, every weight a positive integer);
nothing here is derived from the Male CNS dataset.

## Regenerating

```bash
py -3.13 docs/figures/the-connectome-as-a-prior/make_figures.py
"C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background --factory-startup \
    --python assets/mark/build_mark.py -- --entry the-connectome-as-a-prior
```

Blender is invoked by path because it is not on `PATH`. The chart is drawn by
the committed script from the CSV directly; the hero is rendered by
`assets/mark/build_mark.py`, whose `--entry` mode reads the same CSV through
`assets/mark/entries.json`. Output is deterministic — metadata dates are
suppressed, the SVG hash salt is fixed and Blender's stamp flags are off — so two
runs are byte-identical. Every asset is under 400 KiB. The palette is the house
one, defined once in [`../_house.py`](../_house.py).
