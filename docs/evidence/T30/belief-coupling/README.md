# T30 capture — the coupling dial at 1.0 and 2.5, and the board's belief kernel

The `needs:profile` evidence for ticket
[#46](https://github.com/superposition/qualia/issues/46): the bounded `QUALIA_FLY_COUPLING_SCALE`
dial proven on the Waveshare-carried NVIDIA Jetson Orin NX, plus a manual `ncu` capture of the belief
kernel the coupling runs with.

* **Head:** `58e5b6e728aebca577e710d49732c51c05e4272a` (`ticket/T30`, PR #210; ticket #46); the
  profiled tree came from a `git archive` of that sha (sha256
  `ab66dc54085c87493d55f75b6ac95a17a4eeca63f44be35f19f995fe022a4606`).
* **Board:** `aarch64`, driver `NVRM 540.4.0`, CUDA `12.9`, `ncu 2025.2.0.0 (build 35613519)`,
  compute capability **8.7**, `LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat`.
* **Capture:** **manual**, under `docs/evidence/README.md`'s `## Manual capture` clause — Pinkie
  carries no `nsys`, and at the time of writing had no DNS of its own and no mage installed (a
  network/install state, not architecture; D-022) — so the two dial runs and the `ncu`
  pass were driven by hand. `capture.json` keeps mage's manifest field names (`argv`,
  `profiler_argv`, `returncode`, `status`, `kernel_count`) with the board, build, dial and
  comparison facts alongside.
* **Shape:** one `ncu` pass at dial **2.5**, `--target-processes all --launch-count 12`, ncu's
  default clock and cache control. **12 launches, all `belief_update`** at block `1024×1` /
  grid `1×1` — the belief tick's kernel, one launch per tick.

## The dial, on the board

```
$ QUALIA_FLY_MODE=prior QUALIA_FLY_PRIOR_PATH=assets/brain/prior QUALIA_FLY_COUPLING_SCALE=1.0 \
    QUALIA_SHM_NAME=/qualia_t30_smoke timeout 20 qualia-l1-belief
fly prior: applied 2.7878788
$ QUALIA_FLY_MODE=prior QUALIA_FLY_PRIOR_PATH=assets/brain/prior QUALIA_FLY_COUPLING_SCALE=2.5 \
    QUALIA_SHM_NAME=/qualia_t30_smoke timeout 20 qualia-l1-belief     # the same graph, the dial at 2.5
fly prior: applied 6.969697
```

| dial | `fly prior: applied` | ratio |
| --- | --- | --- |
| 1.0 | 2.7878788 | 1.0 |
| 2.5 | 6.969697 | **2.5** |

`6.969697 / 2.7878788 = 2.5` exactly (`2.7878788 × 2.5 = 6.969697`). The same graph at 2.5 times the
dial couples at 2.5 times the weight, which is what the ticket asks the board to show. Both runs were
bounded at 20 s and ended by that timeout (`rc=124`); the runners print the line once per tick, so
the transcript repeats it. The two logs are `t30-scale-1.0.log` and `t30-scale-2.5.log`; every
`fly prior: applied` line in each carries the one value in the table above, and the two logs hold no
other reading.

## The launches

Every one of the twelve captured launches is the belief tick's kernel:

| id | kernel | block | grid | duration |
| --- | --- | --- | --- | --- |
| 0 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.52 ms |
| 1 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |
| 2 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |
| 3 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |
| 4 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 1.73 ms |
| 5 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |
| 6 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.52 ms |
| 7 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |
| 8 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.52 ms |
| 9 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |
| 10 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.52 ms |
| 11 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 3.53 ms |

One launch (id 4) is the short outlier at 1.73 ms; the other eleven sit at 3.52–3.53 ms. The
committed export says why: that launch's `SM Frequency` reading is 624.72 MHz where every other
launch reads 305.97–305.98 MHz, and its `sm__cycles_elapsed.avg` (1,082,309) is within 0.4 % of the
other eleven's 1,078,177–1,079,185 — the clock moved, not the work.

## The number that changed against the previous capture

The 4090 reference is `docs/evidence/T16/three-kernels` for ticket #31, with
`docs/evidence/baseline-2026-09-11` (PR #166) carrying the earlier `belief_update` row of the same
shape:

| kernel (shape) | board, this capture | T16 4090 | baseline 4090 | board ÷ T16 | board ÷ baseline |
| --- | --- | --- | --- | --- | --- |
| `belief_update`, `1024×1`, grid `1×1` | **3.53 ms** (median of 12; one short launch, id 4, at 1.73 ms) | **1573.013 µs** = 1.573 ms | **1547.834 µs** = 1.548 ms | **2.24×** | **2.28×** |

Same kernel, same block and grid; the board's per-launch time is 2.24× the T16 reference. The two
hosts run at different clocks and the board's is the one this capture measured (`SM Frequency`
305.97 MHz, in `metrics.csv`); T16 says of its own comparison to compare shape and launch count
first and duration second, because the shared 4090's durations move with the machine's other work.

## What this capture establishes

* **The dial is linear over the range the ticket names.** The applied prior weight goes from
  `2.7878788` at dial `1.0` to `6.969697` at dial `2.5` — a ratio of exactly `2.5`, read back from
  the board's own binary with the ticket's environment.
* **The kernel the coupling runs with is `belief_update`**, one launch per tick at block
  `1024×1` / grid `1×1`; the profiled window holds twelve of them, and the committed `metrics.csv`
  and `capture.ncu-rep` carry the per-launch inventory rather than a summary.
* **The board's numbers for that kernel are in the committed detail export**: `SM Frequency`
  305.97 MHz, `sm__cycles_elapsed.avg` 1,078,340 and `gpu__time_duration.sum` 3.52 ms for launch 0.

What it does not establish: nothing here is a cycle-for-cycle comparison against the 4090. T16 is a
different target (the `gpu.rs` device test) and this capture profiles the L1 belief runner, so the
2.24× is a cross-host wall-time ratio of the same kernel and shape, not a measurement of either
host's clock.

## How it was taken

```sh
cargo build -p qualia-l1-belief --no-default-features --features cuda,fly-prior --offline -j 2
cargo build -p qualia-init --offline -j 2
# the arena: only qualia-init may create it (runners/init/doc), owner-only = no children
QUALIA_INIT_OWNER_ONLY=1 QUALIA_SHM_NAME=/qualia_t30_smoke qualia-init &
# the two bounded dial readings, then the capture (sudo: ncu needs root on this board)
echo jetson | sudo -S env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat /usr/local/cuda/bin/ncu \
  --target-processes all --launch-count 12 --export capture --force-overwrite \
  /usr/bin/env QUALIA_FLY_MODE=prior QUALIA_FLY_PRIOR_PATH=assets/brain/prior \
  QUALIA_FLY_COUPLING_SCALE=2.5 QUALIA_SHM_NAME=/qualia_t30_smoke target/debug/qualia-l1-belief
ncu --import capture.ncu-rep --page details --csv > metrics.csv
```

`cargo test -p qualia-braid --offline -j 2` also ran on the board tree (exit 0) before the build. The
record of the run is `board-run.log`; it keeps the board's shell prompts, output and hashes.

For the next operator: a bare `qualia-l1-belief` refuses with
`qualia-l1-belief: failed to open shm: OS error 2` — the belief backend only *opens* the arena, and
neither the agent nor a bare run creates it. `qualia-init` with `QUALIA_INIT_OWNER_ONLY=1` is the
supported way to hold it while a single layer runs outside the supervisor. The profiled runner ticks
forever, so after the twelfth launch `ncu` was ended by killing the (root) app; `ncu-run.log` records
that exit (`==ERROR== The application returned an error code (9).`), while the report and the
details-page import are complete (`IMPORT_RC=0`; `metrics.csv` is 553 lines — the header plus 552
rows covering all twelve launches).

## Files

`metrics.csv` is the **details-page** `--csv` export per D-017; `capture.ncu-rep` is the report
`ncu --export` wrote; `kernels.json`/`kernels.csv` are a **hand-normalised projection** of the
details page: one object per launch under a `run` column, the raw metric identifiers where the export
carries them (`gpu__time_duration.sum`, `sm__cycles_elapsed.avg`) and snake-cased section names
otherwise, thousands separators removed, `nan` kept as `nan`. No value is invented, and
`capture.json`, `kernels.json`, `kernels.csv` and `metrics.csv` carry no filesystem path.

### Committed

The four data files are byte-identical to the board's staged capture, whose hashes PR #210's board
comment records; the fifth row is the manifest the board staged, with two fields conformed below.

| file | bytes | sha256 |
| --- | --- | --- |
| `kernels.csv` | 7,717 | `4f2b0ab967a0f3fde1cf467762c706573694fad0ddefadc81eba579d622d249e` |
| `kernels.json` | 46,864 | `6f16be5a34b7d881d92b7dc2065585abd6188200a1c68a20864b0d89f57bb82c` |
| `metrics.csv` | 113,527 | `fac3f1d866288c236d799c4c555a307475bca3a0a78d16b371c8a3146ae26dac` |
| `capture.ncu-rep` | 676,472 | `cc1548ba50bae87b9a6949ac254e5a04ce275bc03e8c17d6568b8a2f4961719d` |
| `capture.json` | 5,651 | `325775e41f20c2d4dd0863fdfe679086760c3a85399f4d5d77b9121ff2bd0be1` |

`capture.json` is the board's staged manifest with two fields conformed to this directory's
convention: `backend: "ncu"` added — mage's writer always emits it and `docs/evidence/README.md`
requires the backend recorded — and `status` set to mage's `complete` (the staged manifest read
`killed_after_launch_count`; the kill is `returncode: 9` and the verbatim `status_note`, and mage's
writer emits only `running`/`complete`/`failed`). Its hash therefore differs from the staged
`6641af194e87d24997d1f620a60b2f29413d0233a9c39da15952b0b1bb6019b9`.

`capture.ncu-rep` cannot be trimmed and is committed whole (D-017); like the T18 and T50 reports it
embeds the board's scratch paths. `metrics.csv` names `Dynamic Shared Memory Per Block` with unit
`byte/block` and `Static Shared Memory Per Block` with unit `Kbyte/block`: mage's own reader accepts
only byte/kb/mb/gb for a `_bytes` field, so it rejects those units and cannot ingest this committed
CSV as-is (D-017).

### Stays on the capturing machine

Per `docs/evidence/README.md` the profiler's process log, the raw output of the profiling command and
the dial transcripts are not committed; `capture.json`'s `files` map records each hash so the board's
staged copy can be checked.

| artifact | bytes | sha256 | why it is not committed |
| --- | --- | --- | --- |
| `ncu-run.log` | 649,299 | `66774aa69feb881249f29dd70ffd1de8fdf7a878a94628505951731c00baee42` | the profiler's process log; embeds the board's `/home/jetson/…` scratch paths (the app path and the report path) |
| `board-run.log` | 5,449 | `c4fa873568a0da6ea6533dfbea64910f46427ea75ab83ba009eeeb20488f78ca` | the board shell's transcript; embeds the board's `/home/jetson/…` paths, the earlier failed attempts and the run's own hash list |
| `t30-scale-1.0.log` | 56,346 | `e116033a0020ad6dfa0d8cb630207ab684b888a1a8fcff5928ba250dfce5f6e2` | the dial-1.0 runner transcript; path-free, but not one of the convention's files — its reading is the dial table and `capture.json`'s `dial` block |
| `t30-scale-2.5.log` | 55,305 | `4744bbac0d49fb463c477e55523866950bf69e96bb0dee7716977a81ddcd62fe` | the dial-2.5 runner transcript, same reason; also the dial 2.5 reading the profiled pass reprints per tick |
