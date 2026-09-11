# Figures — the ladder

Two figures for the journal entry
[`the-ladder`](https://superposition.github.io/journal/the-ladder/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `ladder-bands.svg` (+ `.png`) | chart | The healing ladder's escalation over squared Mahalanobis distance: four bands separated at the plan's 3, 5 and 8, each labelled with the step it selects, with the measured identity-prediction drift (`0.0`) marked on the axis. |
| `hero.webp` (+ `.png`) | render | The extruded psi mark beside the same four bands as four concentric extruded rings, the gaps at 3, 5 and 8. |

## Data

`ladder.json` records two things, kept apart: the escalation table the plan fixes
(epic [#11](https://github.com/superposition/qualia/issues/11), step 35) and the
anchors this repository measures — the identity-prediction drift
(`crates/braid/tests/drift.rs`), and the `DriftAbove` threshold `3.0` and
`LowerCoupling` factor `0.90` (`crates/braid/src/rules.rs`). Both figures read
that one recording, so the chart and the rings cannot disagree.

The 3, 5 and 8 thresholds are a **specification**, not a measurement: no code in
the tree compares a drift to them, `crates/braid/src/heal.rs` does not exist, and
no run has measured what a normal drift looks like on this stack. The chart says
so on its face; the rings' outermost band is open-ended and is drawn one band
deep because that is a drawing choice, not a value.

## Regenerating

```bash
py -3.13 docs/figures/the-ladder/make_figures.py
"C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background --factory-startup \
    --python assets/mark/build_mark.py -- --entry the-ladder
```

Blender is invoked by path because it is not on `PATH`. The chart is drawn by the
committed script from `ladder.json`; the hero is rendered by
`assets/mark/build_mark.py`, whose `--entry` mode reads the same recording through
`assets/mark/entries.json`. Output is deterministic — metadata dates are
suppressed, the SVG hash salt is fixed and Blender's stamp flags are off — so two
runs are byte-identical. Every asset is under 400 KiB. The palette is the house
one, defined once in [`../_house.py`](../_house.py).
