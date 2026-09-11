# T50 model step — the batched head readback (ticket #172, rank 2)

The model step's per-step overhead: `CoherentJepaRuntime::infer` published its five heads with
five separate device-to-host copies, and closed its latency window with a second `synchronize`.
This capture is that path's before and after, taken with the tooling and the shape #172's row C
used (`mage profile-exec --backend nsys`, `--backend cuda --iterations 30 --warmup 3` → **33
steps**), so the two rows are comparable.

* **Before:** `origin/main` `0e038ac`, binary `qualia-jepa-runtime-probe-before`
  `sha256 20b06c4a5e60ddd8e86538e3985a0ebf0aab5eb904f63475b8fa5225126ac57f`.
* **After:** this branch (`ticket/T50-step`), binary `qualia-jepa-runtime-probe-after`
  `sha256 4592f3dd8bb29eb372ef968e2aa8756deff541a1d01e4d5d97ad2c93aff06879`.
* **Host:** the development workstation — Windows 11 with WSL2 Ubuntu-22.04, an RTX 4090 through
  WSL GPU passthrough. `nsys` 2025.3.2.474 at `/usr/local/bin/nsys`, `mage`
  `profile-exec --backend nsys` installed with `uv`, `cargo +1.98.1`, `nvcc` 12.8 (CUDA 12.8.93).
  No display adapter, driver or `pnputil` call was made (D-012); the two captures ran one at a
  time, each bounded by `--iterations 30`.
* **Why not Pinkie:** the model step needs `candle-core/cuda`, and the board's offline registry
  cache carries no `candle-core` with no DNS to fetch it — #172's own board report records that
  prerequisite. Row C is a dev-host 4090 capture, so the before/after pair is too.

## The numbers (milliseconds)

| quantity | before | after | source |
| --- | --- | --- | --- |
| step p50, plain run, median of 7 | **1.107** | **0.982** | `probe-repeat.csv` |
| step p95, plain run, median of 7 | **2.022** | **1.468** | `probe-repeat.csv` |
| step max, plain run, median of 7 | **2.172** | **1.594** | `probe-repeat.csv` |
| step p50, under `nsys` (this capture) | 2.307 | 2.138 | `after-capture.json`, `before-capture.json` |
| step p95, under `nsys` | 4.060 | 2.734 | same |
| whole call, 1200-iteration delta | **1.238** | **1.199** | `step-cost.txt` |
| launches per step | 68.3 | 73.3 | `*-capture.sqlite` |
| kernel time per step (µs) | 136.6 | 141.8 | `*-capture.sqlite` |

The p50 median falls 11 %, the p95 27 % and the max 27 %; the plain run's own spread collapses
(p50 range 0.934–2.061 ms before, 0.918–1.024 ms after). The whole-call delta, which is the one
estimator here that cancels process start-up, moves −39 µs (−3.2 %). The profiled pair moves less
in one direction than the plain pair, because `nsys` charges waiting time to whichever call is in
flight — read the plain row, not the profiled one, for the step's cost.

## The shape

`--iterations 30 --warmup 3`, one transition per call, fixture seed `12648430`
(`CoherentJepaRuntime::frozen_fixture`). 33 steps, 2255 launches before and 2420 after; the
`nsys` captures (kernel_count in each `*-capture.json`) agree.

| per step | before | after |
| --- | --- | --- |
| kernel launches | 68.3 | 73.3 |
| device-to-host copies | **5.00** | **1.00** |
| bytes copied to the host | 24,576 | 23,552 |
| host time charged to those copies (µs) | 177.2 | 144.8 |
| device synchronizes | 2 | 2 |
| host time charged to those synchronizes (µs) | 509.0 | 323.1 |
| host-to-device copies | 25.91 | 25.91 |
| `cuMemAllocAsync` | 83.24 | 84.24 |
| `cuEventRecord` | 99.67 | 115.67 |

`device-copies.csv` and `api-calls.csv` carry these per-call counts and medians for both runs; both
are derived from the `nsys` export, whose `CUPTI_ACTIVITY_KIND_MEMCPY` table the committed
`*-capture.sqlite` carries.

## What changed

`crates/jepa-model/src/runtime.rs` only.

1. `head_values` concatenates the five published heads on the device and reads them back with
   **one** `to_device(Cpu)` instead of five. `split_values` splits the single readback at the
   contract widths (`HEAD_WIDTHS`). `observe_transition`'s scored NLL rides in the same copy as a
   trailing one-element column, so that call drops from six host copies to one. The concatenation
   is not free — it is 5 launches, one allocation and 16 event records per step — and it still pays
   because a host copy costs 29 µs against a launch's 6.4 µs.
2. `predict_latent_step` loses two `device.synchronize()` calls: both only drained a queue nothing
   measured, and the `tensor_values` readbacks that follow drain it anyway.
3. The input tensors are **not** made resident; see the last section.

**Values are unchanged, bit for bit.** A throwaway probe (not committed) dumped all thirteen
published vectors — `infer`'s five heads, `observe_transition`'s five, the three rollout vectors —
plus both NLL scalars as raw `f32` bit patterns, at `origin/main` and at this branch, on CPU and on
CUDA. Both dumps hash equal: CPU `6921e9b610b521fbe95fde104d8eee4e8e4a74f9c321a2849b1627cc336561f9`,
CUDA `9df6d5b4d57033a8c8b94e3791a8ccac19d4927a3dfa7ab4c4a42573411b9d17`.

`cargo +1.98.1 test -p qualia-jepa-model -j 2` → exit 0: 19 lib tests, 6 + 2 integration tests, 0
failed.

## One falsification, quoted

The claim is "the step published its heads with five host copies". Revert the batching — the
`before` binary *is* the batching reverted — and the count rises from 1.00 to 5.00 copies per step:

```text
$ python3 - <<'PY'   # device-copies.csv, DtoH rows
run,kind,bytes,calls,calls_per_step
before,DtoH,1024,66,2.00
before,DtoH,2048,33,1.00
before,DtoH,4096,33,1.00
before,DtoH,16384,33,1.00
after,DtoH,23552,33,1.00
PY
```

The bytes follow: 24,576 before against 23,552 after, because five `to_device(Cpu)` calls copied the
device buffer behind each head — one of those buffers is 2,048 B for a head whose contract width is
256 values — where the single concatenated copy carries exactly the contract widths.

## The synchronize pair was kept, and that is measured

The report's row C proposes "one synchronize instead of two". That was built and measured as a
separate build (`after-window-on-readback` in `step-cost.txt`): the window closes on the readback
instead of on the second synchronize. There is no second `device.synchronize()` left in `infer`,
but the call costs the same:

| build | per-call wall |
| --- | --- |
| `before` (`origin/main`) | 1238.1 µs |
| `after-window-on-readback` (one synchronize) | **1192.1 µs** |
| `after` (this branch, both kept) | **1199.2 µs** |

The pair is not redundant: the second synchronize is the wait for the 137 µs of queued device work,
and removing it does not remove that wait — it moves it into the readback
(`cuMemcpyDtoHAsync_v2` median 29,313 ns before, 97,401 ns in the variant, because the copy then
waits for the whole queue itself). Keeping the pair also keeps `synchronized_latency_us`'s
definition — the window still brackets device work only — so the before and after captures stay
directly comparable.

## Where the step's time actually goes

Per step, from the before capture, with the host medians in `api-calls.csv`:

| item | calls/step | host µs/step |
| --- | --- | --- |
| kernel launches (`cuLaunchKernel` + `cudaLaunchKernel`) | 68.3 | 1426 |
| event operations (record, create, wait, destroy) | 599.1 | 608 |
| allocations (`cuMemAllocAsync` + `cuMemFreeAsync`) | 166.5 | 443 |
| host-to-device copies | 25.91 | 227 |
| device-to-host copies | 5.00 | 177 |
| device synchronizes | 2.00 | 509 |
| device kernel time | 68.3 | 137 |

Those API times overlap, so they do not sum to the 1.238 ms call. What the shape says is that the
step is bound by **per-operation dispatch** across 68.3 candle operations — 599 event calls and
166.5 allocations per step are candle's per-op bookkeeping, not the runtime's. The readback this PR
batches is five copies, and the report's own framing ("~2.5 ms/step is launch + sync + host
readback") is only right about the launch half: on this host the plain step is 1.238 ms, not
2.5 ms, and of it the readback is 177 µs and the synchronizes 509 µs at most. The unfused encoder
path (`crates/jepa-model/src/lib.rs:117-140`, six convolutions and three projections per step) is
where operation count falls, and that is the next refactor, not this one.

## The input tensors are not resident, and the ceiling is measured

Row C's third item is "inputs resident instead of rebuilt per call". It is **not** in this PR:

* the three input tensors (`observation` 18,080 B, `action` 16 B, `delta` 4 B) are **3 of the 25.91
  host-to-device copies per step**; their device time is 1.61 + 0.41 + ~0.4 µs/step and their host
  time ~17 µs/step, i.e. 1.4 % of the 1.238 ms call (`device-copies.csv`, `api-calls.csv`);
* `candle-core` 0.9.1 exposes no way to refill a device tensor from host memory — `Tensor::from_slice`
  is the only host-to-device path and `copy_strided_src`/`slice_set` are device-to-device — so
  residency is expressible only as a cache keyed on the input's value, which pays only while the
  input does not change. That would flatter this fixture (which infers the same input 33 times) and
  not a runner that reads a new observation each tick.

## Files

| file | bytes | sha256 |
| --- | --- | --- |
| `before-capture.json` | 745 | `801747c182a6b42a30bf76001e2cca3c6ce489e245470c2cea40c76d0a1843c0` |
| `after-capture.json` | 742 | `f6f928953bf96d3f686a068a19a6c5aba416bf56077276154364a456f7f2f17a` |
| `before-kernels.csv` | 374,769 | `2ecbc530acb00a9d2e3df7cc68a99008966fec649574ff84d3d0df6d6cc555e2` |
| `after-kernels.csv` | 389,984 | `8c5f873ed2ecd270fdb975ba74f6bdc38b586ba4d567dc377c3254f2ca0571e4` |
| `before-kernels.json` | 740,800 | `fe62fa1c3a4bab8eca203b33d64c99f6f0cd95edcfe4464fd7fe1fe15305bc15` |
| `after-kernels.json` | 782,910 | `82584f6fc37908416bee7328b1347307ea6b5a11f2f78f04fffe9d2d9ca79ebc` |
| `before-capture.sqlite` | 262,144 | `c150f089a73d3083d231d3daf4c4c676a69982a61ba59b903d58d79bee9a637e` |
| `after-capture.sqlite` | 266,240 | `c9c0900f8a6be9e7dfef820d0f2f92fa08d039dd87e7fa5b5050b6c1be43e79f` |
| `device-copies.csv` | 2,550 | `6b434bcdc07a4f2ba99335207fb33ca5bf8cc6a3a5f87c50479e300c1395a416` |
| `api-calls.csv` | 2,965 | `8c495604d689e2677caaab7e6a2226d39138d22e069ffbc63e451d76f96a86e3` |
| `probe-repeat.csv` | 483 | `5d461387d4e3944df25fe9ddd4dbf929c2d68bc7b2e60b3cf6facaa5bcfdb457` |
| `step-cost.txt` | 2,002 | `18675ea4220fc1605a7c40780420ace1d9752ef349b3ce7603e15a5cdb94217e` |

This directory carries two runs, so the files `docs/evidence/README.md` names are prefixed
`before-` and `after-` — `before-capture.json` and so on — rather than one bare `capture.json`, the
way `T50/pinkie-kernels/` spells its two runs `base-` and `none-`.

Two trims, both stated rather than silent. `kernels.json` keeps mage's field names and values but
drops the fields that are `null` in every row of the run — `nsys` reports no `ncu` counter, so 28
fields reduce to 9 and one launch is one line (before: 2,168,236 B whole; after: 2,314,723 B whole).
`capture.sqlite` keeps `StringIds`, `CUPTI_ACTIVITY_KIND_KERNEL` and `CUPTI_ACTIVITY_KIND_MEMCPY`;
`CUPTI_ACTIVITY_KIND_RUNTIME` is 31,565 rows and is committed as the `api-calls.csv` projection
instead. `*-capture.json` is mage's manifest with the absolute `--output=` directory elided, and the
`argv` paths are the worktree-relative binaries that ran.

### Stays on the capturing machine

| artifact | bytes | why it is not committed |
| --- | --- | --- |
| `capture.nsys-rep` (before) | 875,117 | `nsys`'s report; the committed SQLite is its export |
| `capture.nsys-rep` (after) | 900,654 | same |
| `capture.sqlite` (before, untrimmed) | 2,088,960 | carries the 31,565-row runtime table projected into `api-calls.csv` |
| `capture.sqlite` (after, untrimmed) | 2,142,208 | same |
| `process.log` (both) | — | mage's process log; carries the probe's own JSON report (the p50/p95/max rows above) and embeds the scratch directory |

## How it was taken

```sh
# build the probe (WSL2, one build at a time, -j 2 per D-014)
cd <worktree> && PATH=/usr/local/cuda/bin:$PATH cargo +1.98.1 build -p qualia-jepa-model \
  --features cuda --release --bin qualia-jepa-runtime-probe -j 2          # exit 0

# capture, once per build, with the same argv #172's row C used
mage profile-exec --backend nsys --capture-range all --no-persist \
  --output-dir <scratch>/nsys-final/before -- \
  ./target/release/qualia-jepa-runtime-probe-before --backend cuda --iterations 30 --warmup 3  # exit 0
mage profile-exec --backend nsys --capture-range all --no-persist \
  --output-dir <scratch>/nsys-final/after -- \
  ./target/release/qualia-jepa-runtime-probe-after  --backend cuda --iterations 30 --warmup 3  # exit 0

# the plain-run repeats and the three-way wall comparison
for i in $(seq 7); do ./target/release/qualia-jepa-runtime-probe-<build> \
  --backend cuda --iterations 30 --warmup 3; done                          # probe-repeat.csv
# step-cost.txt: interleaved runs at 30 and 1230 iterations, seven times each,
# per_call_us = (wall(1230) - wall(30)) / 1200

# the crate's own suite
cargo +1.98.1 test -p qualia-jepa-model -j 2                               # exit 0
```

`nsys` is 2025.3.2.474; mage drove it as `nsys profile --trace=cuda,nvtx --sample=none
--cpuctxsw=none --stats=false --export=sqlite --output <dir>/capture <binary> …`, exactly the
`profiler_argv` each `*-capture.json` carries. The `-before` and `-after` suffixes exist so both
binaries could sit in the same target directory; the artifact is `qualia-jepa-runtime-probe`.
