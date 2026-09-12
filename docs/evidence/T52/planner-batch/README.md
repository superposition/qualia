# T52 planner batch — the proposal step's per-step candidate scoring (ticket #225, step 3)

#172's row F1 measured `qualia-jepa-plan-eval propose` at **462 ms** for a 64 × 16 proposal
walk, against 24.8 ms of kernels: 1 024 `predict_step` calls, each paying its own launches,
uploads and three device-to-host readbacks. `crates/jepa-model/src/planner.rs` now gathers the
rows for one step index and scores them in a single `predict_steps` call, so a 64 × 16 walk makes
**16 calls instead of 1 024** and each step pays **three uploads and one readback instead of seven
uploads and three readbacks per row**. This directory is that refactor's before/after capture, the
value check that the emitted proposal did not move, and the falsification of the batched-matrix
form that would move it.

## Headline (milliseconds, 64 candidates × 16 steps = 1 024 predict calls)

| | before (`origin/main` per-candidate walk) | after (batched walk) | change |
| --- | --- | --- | --- |
| step, median of 3 plain runs | **453.6** (474.6 / 450.8 / 453.6) | **322.6** (315.7 / 322.6 / 341.6) | **−131.0 (−28.9 %)** |
| step per `predict_step` call | 0.443 | 0.315 | −0.128 |
| D2H readbacks, whole walk | **3 072** (3.00 per call) | **16** (0.0156 per call) | **−3 056 (−99.5 %)** |
| H2D uploads, whole walk | 7 218 (7.05 per call) | 4 194 (4.10 per call) | −3 024 |
| host launches, whole walk | 15 360 (15.0 per call) | 18 480 (18.05 per call) | +3 120 |
| allocations, whole walk | 20 530 (20.05 per call) | 17 570 (17.16 per call) | −2 960 |
| event records, whole walk | 34 866 (34.05 per call) | 41 266 (40.30 per call) | +6 400 |
| kernel time, whole walk | 24 649 µs | 28 071 µs | +3 422 µs |
| **compute kernel launches** | **12 288** | **12 288** | **0** |

The compute column is the point: the batched walk issues exactly the kernels the per-candidate
walk issued, in the same counts — `gemv2T` (512/128) 1 024, `gemvx` (32/16) 2 048, `gemv2T` (64/128)
1 024, `urelu_f32` 2 048, `bmaximum_f32` / `bminimum_f32` 1 024 each, `badd_f32` 4 096 (see
`*-kernels-aggregate.csv`). Only `copy2d_f32` moves, 3 072 → 6 192, and only because each row's
input views are materialised before the transition head reads them. The saving is in the copies
and the per-call host work, not in the arithmetic — which is what keeps the values still.

## The shape

64 candidates × 16 steps, the config's own maxima, from `propose-request.json` as #172's row F1
used it (sha256 `5d38e407…`, reused verbatim). The checkpoint is #172's own parity-fixture recipe,
regenerated with `harness-candidate.rs` (seed `0xc0ffee`); its weights sha256 is
`a390d36db7284ca0249386f50aaa3c4da09e73b6e4dd0bae42363d43cfceff58`, the same bytes T50's capture
loaded. It is a loadable checkpoint and nothing more: no number here is an accuracy claim.

## Values: bit-identical

`before-proposal.json` and `after-proposal.json` are the whole `RolloutProposal` the step returned,
serialised; they are **byte-identical** (both sha256
`32c4eaef74418b3d71741b025de0ef9c4bfdf1ae67b6191fde5ea996c8df7d24`), and so are the two runs taken
under `nsys`. On the device, `harness-steps.rs` scores 64 rows twice — once as one batched call,
once as 64 single calls — and compares bits:

```json
{"bit_identical":true,"mismatched_rows":0,"mean_mismatched_values":0,
 "log_variance_mismatched_values":0,"occupancy_logits_mismatched_values":0,"rows":64}
```

The ticket's own test is `the same emitted values across the batched planner step` in
`crates/jepa-model/src/planner.rs`: it scores a 64-candidate request whose horizons differ (16 down
to 13 steps, so candidates leave the batch at different indices) through the batched walk and
through the pre-batching per-candidate walk, and asserts every emitted field is bit-equal.

**It was falsified once.** Reversing the batch's row-to-candidate mapping
(`predictions.into_iter().zip(rows.iter().rev().copied())`) makes it fail:
`falsification-reversed-row.txt` holds the run. The first attempt with the module's existing
`FixturePredictor` **passed** under the same perturbation, because that fixture's log-variance is a
constant and its occupancy head depends only on the step's action, so it is blind to which belief a
row was handed; the test was rewritten around a `BeliefSensitivePredictor` whose mean,
log-variance and occupancy all read the belief. That is the whole value of the exercise: the weak
fixture would have shipped a test that cannot fail on the bug it exists to catch.

## Why the walk is batched by step, not by matrix

The 16-call form the ticket names can also be built by giving the transition head one `[64, …]`
batch — one matrix multiply per layer instead of 64 `matrix × vector` products. It is faster
(196.0 ms, −57 %) and it is **not shipped**, because it moves the values: the batched matmul
re-associates the accumulation, and `steps-designA-mbatch.json` records **64/64 rows** diverging
from the single-row walk on the device, while the crate's own
`a_batched_latent_step_matches_single_row_steps_bit_for_bit` fails the same comparison on the CPU.
Its proposal bytes for this one fixture happen to match, which is exactly the trap D-020 names: a
result that agrees on the fixture and is not the same computation is not a contract. The form
shipped here is the cheaper one that survives the bit comparison: it removes the copies and the
per-call readback, and leaves the launches and the arithmetic alone. Of the 257 ms the batched
matrix form would remove (453.6 → 196.0), it removes 131 ms.

## Files

| file | bytes | sha256 |
| --- | --- | --- |
| `before-capture.json` | 1 123 | `dbe764c2ac8daca86b3022f955af111a90fd859d03692a3aca2e3b7e3e7f4a96` |
| `after-capture.json` | 1 117 | `ab12dfd4e662dce7af997481da6103283f7c9494eaca26e427623d2d146ada31` |
| `before-capture.sqlite` | 1 093 632 | `bc78950bd6299d321c75256231e0ec49425cd344c14d25c8650cf6144333a043` |
| `after-capture.sqlite` | 1 306 624 | `b18c4cb9fb9efcc4752bbaaa80388d47f68231522ca4bb6039adb35d86f7973f` |
| `before-kernels-aggregate.csv` | 1 246 | `8813d4c52d963e6538abc77781fe51cb425fc5151f7bbf2577150880f6b313f7` |
| `after-kernels-aggregate.csv` | 1 245 | `2e06c8ac9a301d0d2fc8ac8ca7b8f42662764dfbb98fdf59eecf6be115f625f0` |
| `before-api.csv` | 1 769 | `8cc9ed88da739551abfba533985dd48a80b37b074f2e25e5b0031d3ed6145964` |
| `after-api.csv` | 1 763 | `bdb8ec025a0d3295dba9ea6f86a2e0ef806972ebf21a11ddac12bdeb3861c0c6` |
| `before-report.json` | 937 | `70c988c46b139b0eb8a7e91519d2bff2ece1db394b27a531b30d93153d186158` |
| `after-report.json` | 944 | `d4ac448459fd5d332a849719dad75c08499d0c3154505f759f0b1002ba396bc9` |
| `before-proposal.json` | 36 201 | `32c4eaef74418b3d71741b025de0ef9c4bfdf1ae67b6191fde5ea996c8df7d24` |
| `after-proposal.json` | 36 201 | `32c4eaef74418b3d71741b025de0ef9c4bfdf1ae67b6191fde5ea996c8df7d24` |
| `steps-designB.json` | 312 | `c54a4b2c39df74bd654ac33f544070ee7532401cae2502c56882ef4834126590` |
| `steps-designA-mbatch.json` | 205 | `a98129cc221e6a1a78c00fe9873f5e9bca4333d06cc98236cc12ce8d8b738165` |
| `falsification-reversed-row.txt` | 1 101 | `59fba5c2d6920f6a2ab86644bf57a5c55556f94b37ab7db8fda966f02e9beb1c` |
| `harness-probe.rs` | 3 384 | `303926a7453cfd47e8662ac49b18a18f09f700689f0c5c2b4610b1612590af73` |
| `harness-steps.rs` | 4 741 | `bfe1ac4938ede258cfc426a0a884865850edd76cdef780ce66d4c2ada6b2a7c2` |
| `harness-candidate.rs` | 2 632 | `92f368df4643db736a3a1fa641e15ecbaead73095db23e0da28cc79a3ed33573` |

`before-report.json` and `after-report.json` are the three plain runs each, as the probe printed
them; the median row in the headline comes from them. `*-capture.sqlite` are **trimmed** exports:
`StringIds` and `CUPTI_ACTIVITY_KIND_KERNEL`, with the `StringIds` rows the kernel table does not
reference dropped, `VACUUM`ed — the rule #172's other captures use. `*-api.csv` is a projection of
the untrimmed export's `CUPTI_ACTIVITY_KIND_RUNTIME` (joined to `StringIds` on `nameId`), one row
per API name. `steps-designA-mbatch.json` was produced before the harness grew per-head counters, so
it carries the row count and the worst bit delta but not the per-head split.

### Stays on the capturing machine

| artifact | bytes | why it is not committed |
| --- | --- | --- |
| both `capture.nsys-rep` | 5 922 327 / 5 307 327 | `nsys`'s own reports; the committed SQLite are their exports |
| both untrimmed `capture.sqlite` | 11 628 544 / 10 555 392 | carry the host's environment tables |
| before `kernels.csv` / `kernels.json` | 2 628 057 / 14 845 942 | 15 360 launches; `before-kernels-aggregate.csv` is their per-kernel projection |
| after `kernels.csv` / `kernels.json` | 2 912 831 / 17 614 236 | 18 480 launches; `after-kernels-aggregate.csv` is their per-kernel projection |
| both `process.log` | 7 401 / 7 176 | mage's transcript; the probe's report is committed as `*-report.json` |
| the fixture candidate's `weights.safetensors` + `manifest.json` | 15 432 280 / 1 471 | the test recipe's weights; regenerable with `harness-candidate.rs` |

## How it was taken

Host: Windows 11 with WSL2 `Ubuntu-22.04`, RTX 4090 through WSL GPU passthrough, `nsys`
2025.3.2.474, `mage profile-exec --backend nsys`, `cargo +1.98.1`, CUDA 12.8,
`LD_LIBRARY_PATH=/usr/local/cuda/lib64:/usr/lib/wsl/lib`. No display adapter, driver or `pnputil`
call was made (D-012); the captures ran one at a time under D-011/D-014. Profiling on the dev host
is the same carve-out T50's model capture used: these binaries need `candle-core/cuda`, which the
board's offline registry cache does not carry.

```sh
# both probes are scratch crates over this worktree and over origin/main respectively; the
# sources are committed here as harness-*.rs
cargo +1.98.1 build --release -j 2            # the after probe, target <target>/release
cargo +1.98.1 build --release -j 2            # the before probe, same target

./t52-candidate <scratch>/cand t52-candidate  # exit 0, weights sha a390d36d…

# the bit checks
./t52-steps        <scratch>/cand/t52-candidate --backend cuda --rows 64   # exit 0
./t52-steps-mbatch <scratch>/cand/t52-candidate --backend cuda --rows 64   # exit 0

# the plain triples
./t52-probe-before propose --checkpoint <cand> --request <req> --backend cuda --output <before-proposal.json>
./t52-probe-after  propose --checkpoint <cand> --request <req> --backend cuda --output <after-proposal.json>

# the captures (mage writes into a fresh mage-nsys-<random>/ under --output-dir)
mage profile-exec --backend nsys --capture-range all --no-persist --output-dir <scratch>/nsys/before -- \
  <target>/release/t52-probe-before propose --checkpoint <cand> --request <req> --backend cuda \
  --output <scratch>/runs/nsys-before-proposal.json       # exit 0, 15 360 launches
mage profile-exec --backend nsys --capture-range all --no-persist --output-dir <scratch>/nsys/after -- \
  <target>/release/t52-probe-after propose --checkpoint <cand> --request <req> --backend cuda \
  --output <scratch>/runs/nsys-after-proposal.json        # exit 0, 18 480 launches
```

`harness-probe.rs` calls the same public function `qualia-jepa-plan-eval propose` calls —
`evaluate_rollout_proposals` — on the same `CoherentJepaRuntime` the binary would load, because the
binary's provenance gate stops it before the step (T50's model-eval README says why).

## What changed against T50's F1 capture

T50 recorded 462 ms (461.2 / 461.9 / 532.2), 15 360 launches and 24 813 µs of kernels for the same
shape; this capture's before leg reproduces it (453.6 ms, 15 360 launches, 24 649 µs), and its after
leg is **322.6 ms** — **−139.4 ms, −30 %** against T50's number. Also a number: D2H readbacks per
`predict_step` call fall from 3.00 to 0.0156.
