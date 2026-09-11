# Figures — the agent in the loop

Two figures for the journal entry
[`the-agent-in-the-loop`](https://superposition.github.io/journal/the-agent-in-the-loop/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `mission-lifecycle.svg` (+ `.png`) | chart, two panels | Left: `open_missions` through the fold — the crate's test folding open m1, open m2, close m1, close m2 and a close for a mission it never saw (1, 2, 1, 0, 0), and the agent's write edge folding one open and one close (1, 0). Right: the belief pace gate — the window (1×), the stale window (8×), the hold region and the two stale publications `runners/map`'s tests assert (a 10 s-old belief tick under the 250 ms default; a 300 ms-old tick released at the 200 × 8 = 1,600 ms window). |
| `strand-flow.svg` (+ `.png`) | diagram | The four strands fanning into the one write edge (`POST /braid` carrying the frozen `BraidEvent`, 503 on a failed dispatch, `quarantined` refused with 400), then `observe` as the sole mutator, the folded `BraidState` and its six fields, the durable copies the braid does not keep (MCAP quarantine, the registry's rollback record), and the `GET /braid` read the console, TUI and operator page poll. |
| `loop-state.json` | data | The two event sequences the fold is replayed from, the pace gate's constants and its two asserted stale publications, and the console fixture's `BraidState` fields. |

## Data

`loop-state.json` records where every number comes from; `make_figures.py`
re-folds each event sequence the way `crates/braid/src/lib.rs`'s `observe` does
(opened adds one, closed subtracts one, saturating at zero) and refuses a
recording the fold does not hold, then re-reads the gate's constants from
`runners/map/src/main.rs` and checks the two test names still stand in
`runners/map/tests/runtime.rs`.

* **The fold** — `crates/braid/tests/braid.rs::mission_lifecycle_updates_open_count`
  asserts 2, 1, 0 and 0 for the two-mission sequence, including the stray close
  that must not wrap the count;
  `runners/agent/tests/braid.rs::braid_endpoint_reports_state` drives the same
  edge over HTTP and asserts 1 then 0, with the state before it
  (`generation` 0, `session_id` empty, `last_promotion_ns` 0,
  `last_quarantine_ns` null) and the promotion/rollback pair
  (`generation` 7 then 3, `last_promotion_ns` kept) around them.
* **The pace gate** — `runners/map/src/main.rs`'s `DEFAULT_BELIEF_PACE_MS = 250`
  and `BELIEF_PACE_STALE_WINDOWS = 8` (the same pair in
  `runners/pose/src/main.rs`), and the two assertions in
  `runners/map/tests/runtime.rs`: a belief tick ten seconds old publishes stale
  with the measured lag between 10,000 and 11,000 ms, and a tick 300 ms old
  under a 200 ms window waits at least 1.0 s before the 1,600 ms stale window
  releases it.
* **The state's fields** — `crates/braid/src/lib.rs` (`BraidState`) and
  `apps/qualia-console/tests/fixtures/braid-state.json` (generation 12, session
  `sess-2026-09-11-explore-frontier`, one open mission, with the drift block the
  console's panel reads).

## Regenerating

```console
$ py -3.13 docs/figures/the-agent-in-the-loop/make_figures.py
figures: crate fold [1, 2, 1, 0, 0], edge fold [1, 0], pace 250 ms × 8 (stale at 2000 ms), wrote mission-lifecycle, strand-flow
```

No Blender step: this entry is a state machine and a gate — a chart and a
schematic, not a numeric field with a shape to turn — so the render band of
`docs/figures/README.md` rule 2 is not added, and the entry's `.svg` figures are
its no-WebGL floor. The palette is the house one, defined once in
[`../_house.py`](../_house.py); output is deterministic for a given matplotlib
version and two consecutive runs are byte-identical. Every asset is under
400 KiB.

## What this does not establish

* **The board's readings are not drawn.** The accepted `POST /braid` on Pinkie
  (`open_missions` 0 → 1, PR #196) and `uncertainty_weight=0.625` on the
  exploration mission (PR #184) were taken on the board and are recorded in the
  ticket comments the entry cites; they are not in this tree as committed data,
  so the figure draws the host tests' non-board numbers and says so rather than
  restating a board value from prose.
* **The pose probe is not drawn either.** The live probe's held first pose (1.3 s)
  and stale log (1,604/1,605 ms at a 200 ms window) come from a bounded probe
  recorded in T24b's comment, not from a committed test, so the pace panel uses
  the two `runners/map` assertions instead. The probe's numbers sit just past the
  same 1,600 ms window these constants imply, which is what makes the shape the
  entry describes, but the figure does not plot them.
* **`open_missions` is a counter, not a mission.** Both panels are the fold of
  event sequences; no panel shows an exploration driven to completion, because
  none ran in this entry's period.
