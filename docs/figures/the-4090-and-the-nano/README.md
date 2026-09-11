# Figures — the 4090 and the Nano

Three figures for the journal entry
[`the-4090-and-the-nano`](https://superposition.github.io/journal/the-4090-and-the-nano/).

| File | Kind | What it encodes |
| --- | --- | --- |
| `kernel-durations.svg` (+ `.png`) | chart, two panels | Left: each of the five kernels' mean time per launch on the RTX 4090 and on the Orin NX, log axis, with its launch count — the same 13 launches summing to 4,784.753 µs and 39,182.176 µs (8.19×). Right: the board's residency against its issued work per cycle — `belief_update` and `costmap_stats` at 65.59% and 64.58% of peak sustained-active warps but 1.52% and 16.58% of peak SM throughput. |
| `deploy-path.svg` (+ `.png`) | diagram | The stage-and-ship path: `git archive` on the host → `scp` to `jetson@192.168.55.1` → unpack with the workspace trimmed → native `--release --offline` build → the PTX ceiling (driver 540.4 JITs ≤ 8.5, NVRTC 12.9 emits 8.8) and the compat-loader fix → 5 passed and the two `ncu` runs, with each obstacle as a note beside the stage it stopped. |
| `turntable.glb` | 3D asset | The extruded mark (the committed `assets/mark/psi.glb`) plus one pillar per kernel in the board's capture, height proportional to its mean time on the Orin NX — two pillars and three stubs, which is the 99.6% the entry states — with a 60-frame rotation baked as an animation. |

## Data

Both figures read the two committed captures directly, one row per CUDA launch,
and re-sum them before drawing: a total the CSV does not hold is a refusal.

* **RTX 4090** — `docs/evidence/baseline-2026-09-11/kernels.csv` (mage's `nsys`
  rendering, 13 rows; `kernels.json`, `capture.json`, `capture.sqlite` and the
  README sit beside it). Summed here to 4,784.753 µs over 13 launches across 5
  kernels, at commit `76143f0`.
* **Orin NX** — `docs/evidence/T50/pinkie-kernels/kernels.csv` (13 rows for each
  of two `ncu` runs, merged under a `run` column). Only the `base` rows are read
  — ncu's default clock control; the README records that the `--clock-control
  none` run agrees with it to 0.001% in total, which is a second run, not a
  second figure. Summed here to 39,182,176 ns over the same 13 launches and 5
  kernels, at `a28736f`. The right panel's warps and throughput columns come
  from these same rows.
* **The deploy path's commands and obstacles** — the T50 README's §Invocation
  (the `git archive`/`scp`/`tar` sequence, the trimmed `members` list, the
  offline relock, the `PTX 8.5`/`8.6`/`8.8` probe, the compat loader, the two
  `ncu` invocations and `--launch-count 24`) and `docs/decisions.md`
  (D-010, D-012, D-016, D-018). The turntable's `nvcc -arch=sm_87 -cubin`
  fallback is the README's own named fallback, checked and not used.

The captures are committed evidence, not figures, so they live under
`docs/evidence/` and this directory does not copy them: `make_figures.py` and
`make_turntable.py` both name their paths and read them in place.

## The turntable, and its fallback

Step 43's embed is one asset and no player:

```html
<script type="module"
        src="https://ajax.googleapis.com/ajax/libs/model-viewer/3.5.0/model-viewer.min.js"></script>
<model-viewer src="https://superposition.github.io/journal/the-4090-and-the-nano/turntable.glb"
              camera-controls auto-rotate disable-zoom></model-viewer>
```

This is the entry whose subject is a numeric field — per-kernel time on two
devices, with five values on the board — so the render band of
`docs/figures/README.md` rule 2 is added here and not to the two entries beside
it in this batch. If a third-party script is rejected at review, the fallback is
the same figure shipping the GLB as a download link — `turntable.glb` is
committed beside this README, so the link is the file — not a redesign. The
static `kernel-durations.svg` is the no-WebGL floor either way.

`turntable.glb` is **not** relied on to be byte-reproducible: Blender's glTF
exporter writes the same structure, counts and animation every run (the mark
mesh, five pillars, the `turntable` empty; one `turntableAction` rotation channel
over 60 frames) but the convention does not treat its binary payload as stable
(`docs/figures/README.md` rule 6). Five pillars, because every kernel's mean time
is above zero — the three small ones are the minimum-height stubs, which is the
point: `cognition_update` and `belief_update` dominate the ring. The file is
75,212 bytes, the committed file's own count, not a fixed size.

## Regenerating

```console
$ py -3.13 docs/figures/the-4090-and-the-nano/make_figures.py
figures: 4090 5 kernels / 13 launches / 4784.753 µs; board 5 kernels / 13 launches / 39182.176 µs; ratio 8.19×; wrote kernel-durations, deploy-path
$ "C:/Program Files/Blender Foundation/Blender 4.3/blender.exe" --background \
      --python docs/figures/the-4090-and-the-nano/make_turntable.py
turntable: mark mesh + 5 pillars, 60 frames -> docs/figures/the-4090-and-the-nano/turntable.glb (75212 bytes); cognition_update 13,019 µs, belief_update 12,976 µs, costmap_stats 20 µs, cognition_patch 17 µs, add_one 17 µs
```

Blender is invoked by path because it is not on `PATH`. The palette is the house
one, defined once in [`../_house.py`](../_house.py); the two `.svg` figures are
deterministic for a given matplotlib version and two consecutive runs are
byte-identical. Every asset is under 400 KiB.

## What this does not establish

* **The 8.19× divides two different profilers.** The board number is an `ncu`
  isolated replay with its own clock and cache control; the 4090 number is an
  `nsys` wall-clock capture from a different host and commit. The figure's left
  panel states launch counts and shapes beside the µs for that reason; the ratio
  is indicative, not a like-for-like measurement.
* **The 4090's cycle count is an inference.** The entry's claim that both
  machines spend about the same cycles and differ in clock is the T50 README's
  `[INFERENCE]`, not a drawn number: this figure draws no cycle count for the
  4090, because the capture did not measure its clock.
* **No mission is drawn.** The deploy path reaches the kernel suite; the epic's
  end-to-end mission had not run in this entry's period, and the panel says
  nothing about one.
* **One board capture, on a shared board.** Other agents built and ran on Pinkie
  in the same window; the capture's own README records `belief_update` between
  12,976,128 ns and 13,003,168 ns across five captures (0.21% spread), so the
  figure's board values are one run's, not a distribution.
