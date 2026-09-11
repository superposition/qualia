# Figures — the operating model

Two figures for the journal entry
[`the-operating-model`](https://superposition.github.io/journal/the-operating-model/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `ledger-status.svg` (+ `.png`) | chart, two panels | Left: the 95 ticket issues by status label. Right: how many of those ticket ids the committed dependency diagram `docs/architecture/epics.mmd` draws, and how many it omits. |
| `operating-loop.svg` (+ `.png`) | diagram | The claim → work → PR → breadcrumb → merge loop, the `request-changes` edge back to the work, the blocked branch, the agent-dies branch (label `resume`, read the last `braid` block), and the one resume command a cold shell runs. |

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

## Regenerating

```bash
python make_figures.py
```

The script needs matplotlib, which the workstation's Python 3.8 does not have; it
was run under Python 3.13 with matplotlib 3.11.1. **This is the upstream path,
not the hand-authored fallback.** Output is deterministic for a given matplotlib
version (metadata dates suppressed, SVG hash salt fixed) and two consecutive runs
were verified byte-identical. Every file is under 120 KB, inside the 400 KB
budget. The palette is the house one, defined once in
[`../_house.py`](../_house.py).
