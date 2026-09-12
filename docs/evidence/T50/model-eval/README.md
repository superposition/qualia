# T50 model-eval — `qualia-jepa-parity` and `qualia-jepa-plan-eval` on the 4090

Rows **E** and **F** of ticket [#172](https://github.com/superposition/qualia/issues/172)'s step-2
report, the last two JEPA paths with no measured step time. That report could only record `exit 1`
for both, because no manifest this pipeline had published was readable:

```text
$ qualia-jepa-parity --checkpoint <published artifact> --target cuda ...
qualia-jepa-parity: invalid type: null, expected f64 at line 26 column 27          # exit 1
$ qualia-jepa-plan-eval propose|compare ...
Error: Error("invalid type: null, expected f64", line: 26, column: 27)             # exit 1
```

This is the **first capture of these two paths** (there is no earlier one to compare against), taken
after PR #219 guarded both writers. It measures the steps; it also records what still blocks the
promoted-model run, because the blockers turned out to be two, not one.

## Host and tools

WSL2 `Ubuntu-22.04` on the development workstation, RTX 4090 through WSL GPU passthrough
(`nvidia-smi -L` → `GPU 0: NVIDIA GeForce RTX 4090`), `nsys` 2025.3.2.474, `mage`
`profile-exec --backend nsys`, `cargo +1.98.1`, `nvcc` 12.8 (CUDA 12.8.93), `LD_LIBRARY_PATH`
`/usr/local/cuda/lib64:/usr/lib/wsl/lib`. Captures ran one at a time under D-011/D-014; no display
adapter, driver or `pnputil` call was made (D-012).

Both binaries are the ticket's own, built from this branch with the accelerator target the report
said was missing:

```bash
cargo +1.98.1 build --release -j 2 -p qualia-jepa-model --features cuda \
  --bin qualia-jepa-parity --bin qualia-jepa-plan-eval      # MODEL_RC=0
cargo +1.98.1 build --release -j 2 --target aarch64-unknown-linux-gnu -p qualia-jepa-model \
  --bin qualia-jepa-parity --bin qualia-jepa-plan-eval      # rc 101: error[E0463], no `core`
```

The aarch64 build the DoD's limb asks for **was attempted and cannot run here**: the WSL toolchain has
no `aarch64-unknown-linux-gnu` target (`rustup target list --installed` → wasm32-unknown-unknown,
x86_64-pc-windows-gnu, x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu), no `aarch64-linux-gnu-gcc`
and no `cross`, so the build dies at rc 101 with `error[E0463]: can't find crate for 'core'`; D-018
records the same fault. The record would be D-018's lane (a) — a native aarch64 build on Pinkie from a
`git archive` of the head — except that these two binaries need `candle-core`, which the board's
offline cache does not carry and has no DNS to fetch, so no aarch64 record for the model binaries
exists on either side today. No cross-build is claimed.

*(Both clauses here are provisional limits, not host or board properties: the cross-build pieces are
an installed `aarch64-unknown-linux-gnu` target and a linker route (PR #194 cross-built a crate on
this host and ran it on Pinkie twice; D-018 is amended), and the board's `candle-core`/DNS clause is
superseded by D-022, which measures the board building and running this package natively. The build
lines above stand as the record of what this attempt did.)*

`ldd` on them resolves `libcuda.so.1`, `libcublas.so.12` and `libcurand.so.10`.

## The checkpoint, and why it is not a promoted model

**A promoted model does not exist on this host, and this capture does not pretend otherwise.** Two
facts, both measured here:

1. **The gate-minimum fixture cannot be published at all.** `qualia-jepa-train --backend cuda` on
   the 12 × 4168 fixture (50 004 transitions, the shape #64 describes) refuses to publish, at one
   epoch and at twenty, because the *learned* models' free-running held-out rollout diverges over the
   4 167-step chain the fixture forces (`rollout_error` is `NaN`). A second fixture shaped to keep the
   chains short — 156 sessions × 330 frames, 51 324 transitions, also gate-minimum — refuses with
   `inf` at depth 329. The exact lines are in `trainer-refusals.txt`; the cause is the predictor's
   recursion (`crates/jepa-model/src/lib.rs:182-198`: an unbounded two-layer map applied to its own
   mean for every example of a split, `crates/jepa-model/src/train.rs:455-479`). That is row D's
   territory (#47, T30–T31); for rows E and F the residual is carried by **T52
   ([#225](https://github.com/superposition/qualia/issues/225))**, not by this ticket.
2. **Both binaries refuse anything that is not promotion-ready.** `CoherentJepaRuntime::from_checkpoint`
   rejects a manifest whose baseline gate does not pass (`crates/jepa-model/src/runtime.rs:110-113`),
   and `qualia-jepa-plan-eval` additionally requires a training report that clears
   `TrainingReport::validate_for_promotion` (`crates/jepa-model/src/lib.rs:736-800`, called from
   `crates/jepa-model/src/bin/qualia-jepa-plan-eval.rs:183` and `:260`).

So the step is measured against **the repository's own parity fixture candidate** — the recipe in
`crates/jepa-model/src/parity.rs:433-487` (a deterministic `JepaCandidateModel` written through the
public `write_candidate_checkpoint`, the same synthetic gate block the crate's own tests use), built
into a brand-new directory for this capture by `harness-candidate.rs`. It is a loadable checkpoint
and nothing more: no number here is an accuracy claim, and the weights are the test recipe's, not a
trained model's. What the step costs does not depend on which weights are loaded; the *promoted-model*
run is blocked, and §"What still blocks the promoted-model run" says by what.

The candidate's identity: `checkpoint_id t50close-candidate`, `weights_sha256
a390d36db7284ca0249386f50aaa3c4da09e73b6e4dd0bae42363d43cfceff58`, weights 15 432 280 B,
gate `constant 2.0/2.0`, `flat_mlp 1.5/1.5`, `tiny_cnn 1.0/1.0`. The loader accepts it, which is the
only property this capture needs from it.

## The numbers (milliseconds)

Plain runs are three repeats, wall time between `date +%s%N` stamps around the process; the profiled
run is the committed capture. **Row E is the ticket's own binary; rows F1 and F2 name the probe binary
that stands in for it** — `t50close-probe` (`harness-probe.rs`), which calls the same two public
functions `qualia-jepa-plan-eval` calls, on the same runtime, because the binary's provenance gate
stops it before them (§"The checkpoint"). `step` is the step's own work inside the process (`infer` + eight
`predict_latent_step` calls for E; 64 × 16 = 1 024 `predict_step` calls for F1; the 120-decision
comparison for F2), and the plain-wall column is the **whole process** around it — for F2 that is
39 ms of probe start-up against a 52 µs step. The plain runs are measured, not artifacted: only the
`nsys` captures and the two probe reports (`propose-report.json`, `compare-report.json`) are
committed, so the three plain-wall triples (E 879/934/1046, F1 905/1061/1496, F2 39/39/40) are this
README's record of runs whose command lines are in §"How it was taken".

| # | path | plain wall (3 runs) | step | under `nsys` | device work | dominant cost |
| --- | --- | --- | --- | --- | --- | --- |
| E | `qualia-jepa-parity --target cuda` | **934 ms** (879 / 934 / 1046) | 2 × 9 synchronized steps ≈ **3 ms** warm (2.8 CPU + 3.0 CUDA) + the 11.3/66.0 ms cold first call | capture committed | 192 launches, **351 µs** of kernels | process start-up + CUDA module/library load (**38.6 + 12.0 ms**), one cold first step (66.0 ms CUDA / 11.3 ms CPU) |
| F1 | `plan-eval propose` — `t50close-probe` → `evaluate_rollout_proposals` (64 candidates × 16 steps) | 1 061 ms (905 / 1 061 / 1 496) | **462 ms** (461.2 / 461.9 / 532.2) | capture committed | 15 360 launches, **24 813 µs** of kernels | per-call host↔device round trips: **0.450 ms per `predict_step`** (15 launches — 11 driver-API + 4 runtime-API — 3 D2H, 7 H2D, 20 allocations, 34 event records each) |
| F2 | `plan-eval compare` — `t50close-probe` → `compare_offline_planners` (120 decisions) | **39 ms** process (39 / 39 / 40) | **0.052 ms** (`step_ms` in `compare-report.json`) | kernel-less capture | no launch | the binary's dataset parse + integrity validation (**0.91 s**) in front of the 52 µs comparison |

Row E's report is committed as `parity-report.json`: `passes: true`, eight-step `predicted_mean`
rmse 4.58e-6 against thresholds `rmse_max 2e-4` / `max_abs_max 2e-3` / `cosine_min 0.99999`, and
per-step synchronized latencies p50 **326 µs CPU / 372 µs CUDA** with a single 11 268 µs (CPU) and
65 954 µs (CUDA) outlier on the first step — the cold context plus module load, which is why the
step's *traced* work is milliseconds while the process is ~1 s.

### The naive shape, with file and line

- **F1** — `crates/jepa-model/src/planner.rs:427-448`: `score_candidate` walks one candidate's steps
  in order, one runtime call per step, and feeds the prediction back as the next input
  (`belief = prediction.mean` at `:448`), which forces a device-to-host copy per step. Sixty-four
  candidates that share the same start latent and the same step geometry pay that round trip 1 024
  times, in 64 independent walks. **Refactor:** batch by step index — sixteen batched `predict_step`
  calls instead of 1 024 — which pays the per-call allocation/event/copy/sync 16 times, not 1 024.
  The measured gap is the whole of it: 24.8 ms of kernel time against a 461 ms step.
- **E** — `crates/jepa-model/src/parity.rs:141-152` and `:341-360`: the step builds two complete
  runtimes and records two traces one synchronized step at a time. Its cost is start-up, not
  arithmetic: `cuModuleLoadData` is 38.6 ms of the run and the first launch 12.7 ms
  (`parity-api.csv`). There is no cheap refactor inside a CLI whose whole job is one cold comparison
  — the lever, if one is wanted, is a warm runtime reused across checks, outside this ticket.
- **F2** — `crates/jepa-model/src/bin/qualia-jepa-plan-eval.rs:250` is the comparison's verification call site; the work before
  it is `verify_comparison_evidence`, which parses the dataset manifest and re-hashes the MCAPs it
  cites. That prologue is the cost (**0.91 s** measured), and the comparison itself is **52 µs**
  (`compare-report.json` `step_ms`).

## What still blocks the promoted-model run

The two blockers, restated as the ticket's closure question:

1. **A promotable checkpoint.** Neither a fresh manifest nor a finite one is enough: the loader and
   the planner both require a model that clears the promotion gates, and no artifact this host can
   produce does. Whether a synthetic fixture can ever clear them (calibration slope 0.9–1.1,
   `clamp_fraction ≤ 0.01`, a held-out rollout that beats both frozen baselines) is **T52's question
   ([#225](https://github.com/superposition/qualia/issues/225))**, which carries this run, the rollout
   divergence and F1's batching refactor and is `blocked_on:` a promotion-passing checkpoint; row D's
   own record stays #47's capture at `docs/evidence/T31/training-step/`. The slope figure (3.375,
   against the gate's 0.9–1.1) lives in #172's 21:33Z profiling comment rather than in
   `trainer-refusals.txt`, which records the refusals only.

2. **Pinkie.** *(Superseded 2026-09-11 by measurement: the board's registry cache is complete for the
   head, it fetches through the host's gadget proxy, and it builds and runs the package natively —
   `7m 19s`, and `5m 30s` with `--features cuda`, probe `outputs_finite: true` on both; the board
   job's comments on #225/#228, D-022.)* As recorded at the time of writing, the board's offline
   registry cache carried no `candle-core` and the board had no DNS,
   so the model steps could not be built or run there; the step-5 bar ("including on Pinkie") was
   satisfied only by the kernel captures (`docs/evidence/T50/pinkie-kernels*/`). This capture is a
   dev-host 4090 capture for that reason, and the reason is the DoD's stated-reason clause.

## What changed against the previous capture

**Nothing — there is no previous capture for these paths.** The step-2 report has no measured time
for rows E or F, only `exit 1`; this is their first. The regeneration the report asked for is in
`trainer-refusals.txt`: at this head the two writers **refuse** the run that would have published a
`null` manifest (`invalid type: null, expected f64` at line 26 column 27 for the already-published
one, still), so the historical artifacts stay red and no new unreadable one can be written.

## Files

| file | bytes | sha256 |
| --- | --- | --- |
| `parity-capture.json` | 958 | `102a5d215910694bf4d9f6c1136b1715fc9bab24b822a4a7acd731df124133cd` |
| `parity-kernels.csv` | 32 335 | `f88af89ac1605b445847463130b642ff3495c12ec5e9bf43cc549a450a9a5530` |
| `parity-kernels.json` | 184 783 | `3fac3e1490df0bae28fe366c73bc71b4e69818c3fc08953470a5fda6f446417e` |
| `parity-capture.sqlite` | 20 480 | `baf52bb813e72c61b080411c4f2aba1de9fd967a79b9003138d6ce0ca9c73957` |
| `parity-kernels-aggregate.csv` | 2 990 | `a283c782835de01e4b6d02ac531d7aeed0d821eb699daf7a71844ff9ee5b3e92` |
| `parity-api.csv` | 2 002 | `65dac508176a2376f79133ed4c8066a947f9a41efd06afa34b66b75c60a4145e` |
| `parity-report.json` | 2 649 | `324ddc3ca1d7e34079827512dd362eca6546b8a4bfa435936153ad7a0999f40d` |
| `propose-capture.json` | 971 | `ad3b19d8890a37893f78f0b3b64e5cc3d907410f4daf53b7a2048c3c7592233b` |
| `propose-capture.sqlite` | 573 440 | `88caa095985911fbf15fa86ba31a8004ee583070f9cc7a00e32efd845e6e873b` |
| `propose-kernels-aggregate.csv` | 1 239 | `298b957f792a962b043b2ba2f17271a43c45fa14b04265cf9b94eed208b0e8c5` |
| `propose-api.csv` | 1 742 | `59c1f7385ea3e5b8e0faf954f1954a238ae40edbdacf4e37e76c1350581fc554` |
| `propose-report.json` | 232 | `e6df8ddc682b3527f57844cff77cf83868743014e8498c7c35bcedf412607669` |
| `compare-capture.json` | 1 015 | `e091f755d11f226c345529256a8d2f3125393498f8ab411a1a81d92cc9504b4e` |
| `compare-report.json` | 1 030 | `3252f8b12254ddf6ddcafc5043802d762fe478c7db722550889f0fa62db25302` |
| `propose-request.json` | 113 877 | `5d38e407e0d139af6ff834f9c39c705a5165d9000152b0e4eaabb09e2b849aca` |
| `comparison-input.json` | 54 436 | `5ccfb5d10cf65df3922887914caee9692c9374255fd9ad3557ae04813801d070` |
| `trainer-refusals.txt` | 1 458 | `120b52a797c73aab941eb29b562bf5a34a208671d4802dfe14f425030885b861` |
| `harness-candidate.rs` | 2 862 | `d03071b664c5a17940cb2cea94cea9c1e70bfbfe867a446b9b7ed569079254da` |
| `harness-probe.rs` | 4 845 | `a711190cb45c0bf41e9bf6f2a068c86fb3e95b575149d409451256c156c6b4df` |

`*-capture.sqlite` are **trimmed** exports: `StringIds` and `CUPTI_ACTIVITY_KIND_KERNEL`, with the
`StringIds` rows the kernel table does not reference dropped, `VACUUM`ed — the same rule #172's other
captures use. `*-api.csv` and `*-kernels-aggregate.csv` are projections of the untrimmed export (per
API and per kernel name), not of the trim; the launch-level `kernels.csv`/`kernels.json` that `mage`
wrote for the 15 360-launch proposal run are 2.6 MB and 14.8 MB and stay on the capturing machine,
because the aggregate answers the same question at 1 KB. `compare` launched no kernel, so it commits
`capture.json` and this README only — the kernel-less case the convention names.

### Stays on the capturing machine

| artifact | bytes | why it is not committed |
| --- | --- | --- |
| `propose` `kernels.csv` / `kernels.json` (per launch, verbatim) | 2 626 785 / 14 844 670 | 15 360 rows; `propose-kernels-aggregate.csv` is their per-kernel projection |
| both untrimmed `capture.sqlite` exports | 692 224 (parity) / 12 648 448 (propose) | carry the host's environment tables; the committed files are the trims |
| both `capture.nsys-rep` | 160 802 / 6 076 443 | `nsys`'s own reports; the committed SQLite are their exports |
| the fixture candidate's `weights.safetensors` + `manifest.json` | 15 432 280 / 1 471 | the test recipe's weights; regenerable with `harness-candidate.rs` |
| the two fixtures, the dataset manifests, the run logs | ~450 MB | scratch inputs; the counts and digests are here |

## How it was taken

```sh
# the harness (scratch crate, sources committed here)
cargo +1.98.1 build --release -j 2              # EVAL_BUILD_RC=0
./target/release/t50close-candidate <scratch>/cand t50close-candidate      # exit 0

# E — the ticket's binary, end to end, three plain runs then the capture
qualia-jepa-parity --checkpoint <scratch>/cand/t50close-candidate --target cuda \
  --output <scratch>/runs/parity-plain-<stamp>.json                        # exit 0, 879/934/1046 ms
mage profile-exec --backend nsys --capture-range all --no-persist \
  --output-dir <scratch>/nsys/mage-parity-<stamp> -- \
  qualia-jepa-parity --checkpoint <scratch>/cand/t50close-candidate --target cuda \
  --output <scratch>/runs/parity-nsys-<stamp>.json                         # exit 0, 192 launches

# F1 — the proposal step's own function, the shapes the request file carries
t50close-probe propose --checkpoint <scratch>/cand/t50close-candidate --backend cuda \
  --request <scratch>/cand-requests/propose-request.json                   # exit 0, 1 024 predict calls
mage profile-exec --backend nsys --capture-range all --no-persist -- \
  t50close-probe propose ...                                               # exit 0, 15 360 launches

# F2 — the comparison step (CPU only; nsys records no launch)
t50close-probe compare --request <scratch>/cand-requests/comparison-input.json # exit 0, 39 ms process; 0.052 ms step
mage profile-exec --backend nsys --capture-range all --no-persist -- \
  t50close-probe compare ...                                               # exit 1, "captured no CUDA kernel launches"

# the binary's own `propose`/`compare`, for the boundary this capture reports
qualia-jepa-plan-eval propose --checkpoint <scratch>/cand/t50close-candidate ...  # exit 1, ENOENT: no training report
qualia-jepa-plan-eval compare --dataset <dataset> --checkpoint <scratch>/cand/... # exit 1, after 0.91 s of dataset work
```

`propose-request.json` is the request the proposal step ran with: 64 candidates × 16 steps, the
config's own maxima (`max_candidates 64`, `max_steps 16`), each step inside the candidate's measured
action support. `comparison-input.json` is 120 decisions citing one sealed session.

`harness-probe.rs` calls the same two public functions the binary calls —
`evaluate_rollout_proposals` (`crates/jepa-model/src/planner.rs:229`) and `compare_offline_planners`
(`:715`) — on the same `CoherentJepaRuntime` the binary would load, because the binary's provenance
gate stops it first (§"The checkpoint"). Everything the binary would do between its argv and those
calls is argv parsing, which the probe does not pay for.
