# Figures — the fly in the belief matrices

Three figures for the journal entry
[`the-fly-in-the-belief-matrices`](https://superposition.github.io/journal/the-fly-in-the-belief-matrices/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `coupling-normalisation.svg` (+ `.png`) | chart, two panels | The factor `CouplingPrior::couple` applies to a mapped belief slot, per type: left, the tests' three-type fixture (in-strength 4, 8, 2 → 0.5, 1.0, 0.25, applied 1.75); right, the committed five-type artifact (in-strength 19, 3, 33, 12, 25, peak 33, applied 2.7879, mean 0.5576). The blue bar is the peak type, which couples at unit weight. |
| `new-kernels.svg` (+ `.png`) | chart | The T16 capture's per-kernel time on the RTX 4090 (log axis): 16 launches over 8 kernels, 4892.893 µs, of which the three kernels this epic added (`belief_couple` 2.496, `action_score` 10.592, `perception_voxel` 11.84 µs) are 24.928 µs. The baseline it replaced: 13 launches over 5 kernels, 4784.753 µs. |
| `prior-path.svg` (+ `.png`) | diagram | The connectome artifact's path into the belief slots: dataset → committed `graph.bin`/`manifest.json` → digest-checked load → peak-normalised in-strength → `couple(belief, slots)` → the layer belief the panel reads, with the off-by-default `fly-prior` flag, the refusal on an unmappable mapping, and the coupling's host-side (no-kernel) nature called out. |
| `coupling-strength.json` | data | The two graphs the coupling numbers fold from (the fixtures' CSR and the artifact's path), the capture's shape, and where each is committed. |

## Data

`coupling-strength.json` records two graphs, and `make_figures.py` re-decodes both
before it draws: a factor that is not in the data is a refusal, not a figure.

* **The tests' fixture** — `crates/jepa/tests/prior.rs`'s `graph()` (three types,
  four edges; `rowptr [0,1,3,4]`, `cols [1,1,2,0]`, `weights [3,5,2,4]`) and
  `crates/cuda/tests/coupling.rs`'s `prior()`, the same graph. Its in-strengths
  are 4, 8 and 2 against a peak of 8, so the factors are 0.5, 1.0 and 0.25 and
  the applied total is **1.75** — the number
  `coupling_applies_the_prior_by_type_strength` asserts, and the number the entry
  quotes from the 4090.
* **The committed artifact** — `assets/brain/prior/manifest.json` and
  `assets/brain/prior/graph.bin` (5 types, 9 edges, three SHA-256 section
  digests). `make_figures.py` verifies those digests the way
  `CouplingPrior::load` does, then folds the CSR: in-strength 19, 3, 33, 12, 25,
  peak 33, applied 2.787878…, mean 0.557576… per type. That mean is the field
  `runners/explore/src/main.rs`'s `PlannerBeliefRiskContext::from_prior` sends as
  `uncertainty_weight`.
* **The T16 capture** — `docs/evidence/T16/three-kernels/kernels.csv` (16 rows,
  one per CUDA launch) and `docs/evidence/baseline-2026-09-11/kernels.csv` (13
  rows). The chart's per-kernel totals and both suite totals are summed from
  those CSV rows, and the recording's `launches`, `kernels` and `total_us` are
  checked against them.

The coupling arithmetic itself is `crates/jepa/src/prior.rs`'s `couple`: an
edge's weight is summed into its column type, each type's sum is divided by the
largest, and the mapped slot is multiplied by that factor. Nothing in these
figures is a new measurement; the only derived quantity is the mean, which is the
sum divided by the type count, the way the exploration runner computes it.

## Regenerating

```console
$ py -3.13 docs/figures/the-fly-in-the-belief-matrices/make_figures.py
figures: fixture applied 1.75 (types 3), artifact 5 types / 9 edges, in-strength [19, 3, 33, 12, 25], peak 33, applied 2.78788, mean 0.557576; T16 4892.893 µs over 8 kernels; wrote coupling-normalisation, new-kernels, prior-path
```

No Blender step: this entry's subject is a path and a per-type normalisation, not
a numeric field with a shape to turn, so the render band of
`docs/figures/README.md` rule 2 is not added here — T51's `fly-brain` set already
carries the fly's own turntable. The palette is the house one, defined once in
[`../_house.py`](../_house.py); output is deterministic for a given matplotlib
version (metadata suppressed, SVG hash salt fixed) and two consecutive runs are
byte-identical. Every asset is under 400 KiB.

## What this does not establish

* **The two graphs are not the same graph at different scales.** The fixture's
  1.75 is a test's number and the artifact's 2.7879 is the committed prior's;
  the entry's board reading of `uncertainty_weight=0.625` is neither of them,
  and no figure here draws it or claims the committed artifact reproduces it.
* **The chart labels round.** `new-kernels.svg` prints each kernel's total to
  three significant figures; the exact CSVs' values are the ones in the table
  above and in the generator's own summary line.
* **`couple`'s effect on the published belief is not shown.** The functions are
  the CPU reference's, and the artifact's factors are arithmetic on committed
  data; no run that moved a belief through the coupling is drawn.
* **The capture's durations are one contended session on a shared 4090.** The
  launch count and shape are the stable part, which is why the chart states them
  beside the µs.
