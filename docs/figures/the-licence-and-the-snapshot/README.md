# Figures — the licence and the snapshot

Two figures for the journal entry
[`the-licence-and-the-snapshot`](https://superposition.github.io/journal/the-licence-and-the-snapshot/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `provenance-check.svg` (+ `.png`) | chart | For each commit, three bars: files tracked in this repository, files whose relative path also exists in the private reference checkout, and files that are byte-identical to it. The third bar is zero at both commits. |
| `licence-flow.svg` (+ `.png`) | diagram | The clean-room boundary (dashed, labelled), and below it the three attribution sources — the relicensed workspace root, Leash (MIT), the Male CNS dataset (CC-BY 4.0) — fanning into `NOTICE`, which `.github/scripts/notice-check.sh` guards. |

## Data

`provenance.json` records the two runs of this repository's own
`scripts/provenance_check.py` that the chart draws:

```bash
python scripts/provenance_check.py     # run at 144bb6d and at the head of main
```

At `144bb6d` (the commit that introduced the check, and the one that closed
EPIC-01's work) the tool printed `compared 104 file(s) ...; 0 identical` over 157
tracked files. At `a59b836` it printed `compared 110 file(s) ...; 0 identical`
over 170 tracked files. Both runs exited `0`. The reference checkout's path is
deliberately not recorded here.

The diagram's facts are the three committed files themselves (`LICENSE`,
`NOTICE`, `.github/scripts/notice-check.sh`) plus the workspace `license`
field in `Cargo.toml`.

## Regenerating

```bash
python make_figures.py
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
