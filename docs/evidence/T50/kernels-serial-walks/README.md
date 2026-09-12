# The belief and cognition kernels' serial row walks

Ticket [#172](https://github.com/superposition/qualia/issues/172) rank 1, branch `ticket/T50-kernels`.
`kernels/belief_update.cu` and `kernels/cognition_update.cu` are replaced walk for walk; no launcher
changes, no new kernel names, no new arguments.

This pair is the before and after of one run of the same workload: `crates/cuda/tests/gpu.rs`, the
device suite, built from the same tree, single-threaded, profiled with mage.

## Host and backend, and the substitution this capture names

`nsys` runs on the **dev workstation's WSL2 Ubuntu-22.04 distribution** against the **RTX 4090**
(`GPU-fb7c6e5e-95d2-8da6-98ae-b08353b6fe01`), Nsight Systems **2025.3.2.474**, driven by mage's
`profile-exec --backend nsys`. [D-012](../../../decisions.md) makes Pinkie the profiling target, but
the board carried **no `nsys` and no mage** when this capture was taken (T50's board report records
both as absent then; since 2026-09-12 it carries `nsys` 2024.5.4 and `mage` 0.1.0, both installs —
`../../board/readiness/README.md`), so the pair was taken on the dev host; the board's own `ncu`
counters for these two kernels are in [`../pinkie-kernels/`](../pinkie-kernels/) and are the cross-check for
the shape and the register counts below. This directory is therefore the **dev-host substitution**
that [`../../baseline-2026-09-11/`](../../baseline-2026-09-11/README.md) also uses, and the board re-measure of the refactor is still owed by
this ticket.

## What ran

Two builds of the same test binary, differing only in the two kernel sources:

| | binary | md5 | registers | static shared memory |
| --- | --- | --- | --- | --- |
| before | `gpu-5e740812f0917bab` at `1256a33` | `d897d2587a68d5977c60d98f58f8d8b3` | `belief_update` 44, `cognition_update` 48 | 8192 B, 8192 B |
| after | the same target after the refactor | `6bc93cf8daf204094556d1004ae06093` | `belief_update` 42, `cognition_update` 40 | 8192 B, 8192 B |

```bash
cargo +1.98.1 test -p qualia-cuda --features cuda --no-run -j 2
export LD_LIBRARY_PATH=/usr/local/cuda/lib64:/usr/lib/wsl/lib:$LD_LIBRARY_PATH
mage profile-exec --backend nsys --capture-range all --output-dir <scratch> \
  -- ./target/debug/deps/gpu-5e740812f0917bab --test-threads=1
```

The command and the binary name are the ones [`../../baseline-2026-09-11/README.md`](../../baseline-2026-09-11/README.md) records; the
device test binary's name carries a cargo metadata hash that does not change with the crate's
source, so the after binary keeps it. `--test-threads=1` gives one pass of the device suite's eight tests:
**16 launches over 8 kernels**, of which the two kernels this PR owns are **1 `belief_update`** and
**2 `cognition_update`**. Every launch of both kernels runs at grid `1×1×1`, block `1024×1×1`,
8 KiB of static shared memory — the shape is unchanged, which is the ABI requirement.

The `LD_LIBRARY_PATH` line is load-bearing: without it `Adapter::new(0)` returns
`CUDA_ERROR_NO_DEVICE`, and every device test reports and returns as a **pass**, so a run without it
measures nothing and says `ok` while doing so.

## The shapes that were replaced, and why the walk was serial

| file:line | shape |
| --- | --- |
| `kernels/belief_update.cu:70-73` (before) | one thread per belief dimension; each thread walks its own 1024-float row of the 4 MiB matrix, one scalar `row[column] * prior_mean[column]` per column, ascending, into one accumulator. That accumulator is a 1024-deep chain of dependent fused multiply-adds and one dependent load per link. |
| `kernels/belief_update.cu:122-125` (before) | a **second** serial walk of the same row for the Hebbian step, a read-modify-write per element. |
| `kernels/cognition_update.cu:31`, `:38-39`, `:48-51` (before) | three serial walks: the prediction row walk, the transposed gradient (`weights[row * 1024 + tid]`, ascending rows — coalesced across lanes, but one dependent load and one accumulator chain per thread), and the row weight step. |

The dominant cost is the *sector* the L1 fetches per element, not the arithmetic. The matrix is
row-major and a warp's lanes are thread `tid` = row, so at column `c` the 32 lanes of a warp touch
`weights[tid * 1024 + c]` for 32 different rows: 32 addresses 4096 B apart, each a 4-byte read inside
its own 32-byte sector. Every element therefore costs a **fully fetched 32-byte sector to use 4
bytes of it — 8× the sectors the data needs**. For one pass over the 4 MiB matrix:

```text
scalar walk: 4 194 304 B of data / 4 B per access  = 1 048 576 accesses × 32 B fetched = 33 554 432 B (32 MiB)
float4 walk: 4 194 304 B of data / 16 B per access =   262 144 accesses × 32 B fetched =  8 388 608 B ( 8 MiB)
```

So the vector walk takes the fetch from **8× the useful bytes to 2×** — not to 1×. A `float4` is 16 B
and the lanes are still 4096 B apart, so each 16-byte access takes a 32-byte sector half used: the two
halves of a 32-byte chunk are two accesses inside one sector, and the sector is fetched once for both.
The board capture measures exactly this (see [`../pinkie-kernels-refactored/`](../pinkie-kernels-refactored/README.md)):
the tick's three passes over the matrix move 100 663 296 B → 25 165 824 B, a flat **4.000×**, with the
non-matrix traffic identical on both sides. Reaching 1× would need one whole 32-byte sector per lane per
access, which these walks do not do and which is the experiment the capture leaves open (see below).

The access pattern is one the compiler cannot coalesce on its own: `weights + tid * 1024` is 16-byte
aligned but nvcc cannot vectorize a `float*` it only knows to be 4-byte aligned, so the before kernel
issues 1024 scalar loads per thread where 256 `float4` loads carry the same bytes.

## before / after

Committed pair: `before-*` is the first run of the repeat set below, `after-*` is the first run
of the final build; the repeats are tabulated under the table.

| Kernel | Launches | Before (µs) | After (µs) | Ratio | Useful bytes per tick | Before GB/s used | After GB/s used | Peak % before → after |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `belief_update` | 1 | **1582.071** | **417.336** | **3.79×** | 12 582 912 (read 2×, write 1×) | 7.95 | 30.15 | 0.79 % → 2.99 % |
| `cognition_update` | 2 | **1651.925 + 1648.709** | **465.606 + 464.836** | **3.55×** | 16 777 216 per launch (read 3×, write 1×) | 10.17 | 36.06 | 1.01 % → 3.58 % |
| the two kernels together | 3 | **4882.705** | **1347.778** | **3.62×** | | | | |
| suite of 8 kernels | 16 | 4919.562 | 1384.342 | 3.55× | | | | |

`belief_update` runs both walks in this fixture: `default_params(2)` gives threshold 0.05 and the
fixture's VFE is 1165.7, so the gate is open and the tick is the two-walk case, not the cheap one.

The rest of the suite is untouched and moves only as noise: `action_score` 10.646 → 10.616,
`perception_voxel` 11.256 → 11.226, `belief_couple` 3.120 → 3.015, `costmap_stats` 1.479 → 1.476,
`cognition_patch` 1.544 → 1.539, `add_one` 1.029–1.479 → 1.026–1.508 µs.

### Repeats

The same binaries, the same command, run back to back on the same machine. Across the five `after`
runs `belief_update` spans 414.70–423.64 µs (2.2 %) and `cognition_update` 461.3–472.9 µs (2.5 %),
and the win does not depend on which run is quoted.

| Run | `belief_update` (µs) | `cognition_update` (µs, both launches) | Suite (µs) |
| --- | --- | --- | --- |
| before, committed | 1582.071 | 1651.925 + 1648.709 | 4919.562 |
| before, repeat | 1580.370 | 1653.060 + 1648.850 | 4919.210 |
| after, committed | 417.336 | 465.606 + 464.836 | 1384.342 |
| after, same build, repeat | 423.640 | 472.880 + 472.850 | 1406.570 |
| after, pre-comment-fix build | 414.954 | 462.505 + 462.473 | 1376.514 |
| after, pre-comment-fix build, repeat 1 | 414.700 | 462.240 + 461.320 | 1375.160 |
| after, pre-comment-fix build, repeat 2 | 417.520 | 466.460 + 466.140 | 1387.120 |

The `after` rows span two builds of the same arithmetic. `belief_update.cu`'s alignment comment was
reworded after the first three, which changes the source text the binary embeds through
`include_str!` and so the binary's md5, and changes no device code; `after-capture.*` here is the
first run of the later build, and the bit-identity hashes below were re-measured against it. Averaged
over those five `after` runs: 417.63 µs (`belief_update`) and 465.73 µs per launch
(`cognition_update`).

The **first** capture of each binary in the session is not used: it was the process's first GPU work
after an idle period and reads 1801.94 µs (`belief_update`) / 2304.39 + 1636.76 µs
(`cognition_update`) before, and 416.77 / 464.13 + 466.12 after. Both warm up to the rows above.

## Roofline, and the gap that is left

[`../../baseline-2026-09-11/`](../../baseline-2026-09-11/README.md) and T50's board report both quote the 4090's **1008 GB/s**
DRAM peak, and the report's memory floor for one pass over the 4 MiB matrix is
`4194304 / 1008e9` = **4.16 µs**.

| | Achieved | Floor for the same bytes | Gap |
| --- | --- | --- | --- |
| `belief_update`, one tick with the gate open | 417.336 µs | 12.48 µs | **33.4×** |
| `cognition_update`, one tick | 465.221 µs | 16.64 µs | **28.0×** |

The refactor cuts the fetched sectors **4×** and the time with them: on the board the L1-side rate is
**flat across the change** (7.76 → 7.40 GB/s for `belief_update`, 8.04 → 8.19 GB/s for
`cognition_update`) while the duration falls 3.81× and 3.62× and the L1 byte counter falls exactly
4.000× and 3.571×. That is the signature of a kernel bound by **L1/LSU throughput rather than by
DRAM**: the bytes it must fetch are what buy the time, at a rate one SM's L1 sustains. A DRAM roofline
is a whole-device number — 1008 GB/s is what 128 SMs draw together — and the launch shape is
`grid_dim: (1, 1, 1)`, one block on one SM, so the 12.48 µs DRAM floor for a tick is not reachable
from this shape whatever its access pattern. [INFERENCE] on that attribution; the flat L1 rate and the
traffic-proportional time are measured, on the board, in the capture named above.

**Widening the unroll from 4 to 8 changed nothing** (`belief_update` 416.69 µs with
`#pragma unroll 8` — inside the 414.70–423.64 µs spread with `#pragma unroll 4` — and cognition's first
launch 723.22 µs against 461.3–472.9 µs, so worse), so the residual is not load-issue slack.

Two things are left, and neither belongs to this PR. The first is a further **2× in fetched sectors**:
one whole 32-byte sector per lane per access, i.e. two `float4`s per iteration, would take the fetch
from 2× to 1×. Whether that buys time depends on whether the binding resource is fetched sectors or LSU
wavefronts, and the capture above cannot separate the two — the four-fold fall in both is consistent
with either — so it is named here as the honest next experiment, not as headroom this PR claims. The
second is the launch shape: splitting the reduction across blocks is **not available without breaking
the ABI** — the VFE reduction is a fixed 1024-term binary tree in one block's shared memory, the scalar
fields are written by thread 0, and `dispatch_belief_update` launches one kernel.

## What changed, and what was rejected

**Kept bit for bit.** Both walks keep the reference's accumulation order — one multiply-add per
column, columns ascending — and load four columns per instruction through `float4` (`belief_update`
prediction and Hebbian walks; `cognition_update` prediction and row walks). `cognition_update`'s
transposed gradient already reads coalesced across lanes; only its shared-memory error reads are
vectorized, four rows at a time, in the same ascending order. The launcher signature is unchanged:
same kernel names, same argument lists, same `grid`/`block`/`shared_mem_bytes` (`1×1×1` / `1024` /
0), and the static shared memory is still 8 KiB, which the captures measure
(`static_shared_mem_bytes 8192` in `after-kernels.csv`; `launch__shared_mem_per_block_static 8.19
Kbyte/block` in `pinkie-kernels-refactored/metrics.csv`) and the kernels declare
(`kernels/belief_update.cu:78-79`, `kernels/cognition_update.cu:37-38`, 2 × 1024 `f32`), with
`shared_mem_bytes: 0` dynamic in both launchers (`crates/cuda/src/cuda_impl.rs:283`, `:1135`). No test
asserts it: `crates/cuda/tests/budget.rs` is the costmap memory plan and
`crates/cuda/tests/kernel_abi.rs` pins device struct offsets.

**Rejected: the tiled, shared-memory reduction over column tiles.** The report's recommendation is a
block-per-row-tile reduction, and a reduction changes the order of the 1024 adds. The contract is
`crates/cuda/src/cpu.rs`, whose `belief_update` accumulates each row in ascending column order and
whose own doc comment calls the ordering part of the twin: *"the same clamps, the same reduction
order for the VFE tree, and the same ordering of the belief, weight and precision updates. Any
change to one side must be mirrored in the other."* Reordering a row is a value change, and D-008/D-009
make emitted values interface: that is the whole ground of the refusal, and it does not rest on what is
left on the table. The order-preserving walks above keep the oracle's values bit for bit while cutting
the fetched sectors 4×; the tiled reduction is refused, not deferred.

**The two-walk fold is not available here, and the reason is a gate, not an oversight.** For
`belief_update` the weight step is gated by `vfe > threshold`, a **block-wide** predicate that
depends on every row's prediction, so the element a thread loaded for its prediction cannot be
updated until the whole matrix has been walked. For `cognition_update` the coefficient (`row_step`)
is row-local, but it exists only once that row's dot product has completed — the same walk the
thread has just finished. Holding a row on chip to fold the two is out of reach: 1024 floats per
thread against a 48 KiB static budget. What the refactor does instead is make the second walk cost
the same half-sector access as the first — a 16-byte `float4` chunk, two accesses per 32-byte sector,
read-modify-write — which is where the 4× in fetched sectors above comes from.

## Values: bit-identical, and how that was checked

The two kernels must emit the values they emitted before. That was checked by dispatching both
kernels on the `gpu.rs` fixtures from a scratch integration test (`crates/cuda/tests/zz_bitdump.rs`,
deleted before commit) and hashing the exact f32 bit patterns of every output field with FNV-1a:
`mean`, `precision`, `prediction`, `residual`, `vfe`, `challenge_vfe`, `confirm_streak`,
`compression`, `layer`, all 1 048 576 weights and all 1024 biases for `belief_update`; the first and
second tick's state, and layer 0's weights and biases, for `cognition_update`.

| | before (`d897d258…`) | after (`6bc93cf8…`) |
| --- | --- | --- |
| `belief_update` | `0xf05fae5a8376b3fe` | `0xf05fae5a8376b3fe` |
| `cognition_update` | `0x8d674c680601cbbc` | `0x8d674c680601cbbc` |

Equal, field for field, bit for bit. Every intermediate field is conserved too (`vfe`
`0x4491a2dc`, `prediction[0]` `0x3f818c79`, `weights[0]` `0xbbf8a25f`).

**The one thing that had to be pinned to get there, and why it matters.** The first build of the
refactor had `belief_update` at `0x5d4e7dfe275ca59e` — not equal. A per-field bisect put the whole
difference in the weight matrix: 30 470 of 1 048 576 elements (2.9 %) one ULP apart, with
`mean`, `precision`, `prediction`, `residual`, `vfe` and the scalar fields all exactly equal. The
cause is fused multiply-add contraction. `value * decay + step * mean` has two roundings to choose
from, and nvcc's choice is per instruction, not per source line: the unrolled vector loop fused the
second product (`fma(step, mean, value * decay)`, matching all 1 048 576 elements) where the scalar
loop fuses the first (`fma(value, decay, step * mean)`, matching all 1 048 576 elements in the
`before` binary). Neither is wrong and the parity tests cannot see the difference — a 1-ULP move at
a weight of ~0.01 is ~1e-9, while `assert_close` allows `1e-5 + 1e-4 · scale`. The walk therefore
pins the contraction the scalar loop chose, with `fmaf(value, decay, step * mean)` (and
`fmaf(step, state, value)` in `cognition_update`, which is what that kernel's scalar form already
emitted), so the vector walk reproduces the scalar values rather than whatever the loop's new shape
selects.

## Tests

```text
$ cargo +1.98.1 test -p qualia-cuda --features cuda -j 2      # exit 0
  unittests src/lib.rs            2 passed   (embedded_kernels_compile_under_nvrtc, evidence_layout_fixture_compiles)
  tests/budget.rs                 1 passed   (fits_orin_nano)
  tests/coupling.rs               1 passed   (coupling_is_noop_without_feature)
  tests/cpu_fallback.rs          13 passed   (the oracle suite)
  tests/gpu.rs                    8 passed   (belief_update_matches_the_host_reference,
                                              cognition_dispatch_matches_the_host_reference, …)
  tests/kernel_abi.rs             4 passed
  tests/parity.rs                 1 passed
  TOTAL 30 passed; 0 failed

$ cargo +1.98.1 test -p qualia-cuda --features cuda,fly-prior -j 2 --test coupling   # exit 0
  2 passed  (coupling_applies_the_prior_by_type_strength, coupling_refuses_an_unmappable_slot)
```

**The falsification.** One column index slipped in the vector walk — `prediction += weight.w * mean.w`
became `prediction += weight.w * mean.x` — the kind of slip a four-at-a-time rewrite invites:

```text
$ cargo +1.98.1 test -p qualia-cuda --features cuda --test gpu -j 2 belief_update
test belief_update_matches_the_host_reference ... FAILED
belief.prediction: device 1.0065767 vs host 1.0120996 (difference 0.0055229664, tolerance 0.000111209956)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 7 filtered out
```

Restored, rebuilt, hash re-checked — the pinned values above are from the restored tree, and the
`after` binary this directory profiles has md5 `6bc93cf8daf204094556d1004ae06093`, which is byte for
byte the binary the suite above ran against and the binary the bit-identity probe was re-run on.

`python scripts/provenance_check.py` → `provenance: OK` (152 authored files compared).

## What changed against the previous capture

[`../../baseline-2026-09-11/`](../../baseline-2026-09-11/README.md) is the capture these two kernels are measured against, and its numbers are
the 13-launch suite from before T16's three kernels landed. Its rows for these kernels against the
committed `after`:

| Kernel | baseline-2026-09-11 (µs) | after (µs) | Ratio |
| --- | --- | --- | --- |
| `belief_update` | 1547.834 | 417.336 | 3.71× |
| `cognition_update` | 1612.697 + 1612.822 | 465.606 + 464.836 | 3.47× |

The suite totals are **not** like for like — 13 launches then, 16 now, because `action_score`,
`perception_voxel` and `belief_couple` joined the suite — so the comparison that means anything is
the per-kernel rows above, and the `before` column of this directory's own pair for the
launch-set-matched one. `../pinkie-kernels/` remains the board's measurement of the unrefactored
kernels (12 976 128 ns `belief_update`, 12 991 136 / 13 046 368 ns `cognition_update`); the board had
no `nsys` when this pair was taken (it has since 2026-09-12, so the refactor can now be re-measured
there — `../../board/readiness/README.md`) and the refactor has not been measured there. That
re-measure is owed by this ticket.

## Files

`before-capture.json` / `after-capture.json` are mage's manifests with the absolute output path
elided to `<output-dir>/mage-nsys-<run>` and the target to `<binary>`, so nothing here names a
machine. `before-kernels.json`, `before-kernels.csv` and the `after-` pair are mage's own 16 rows.
`before-capture.sqlite` and `after-capture.sqlite` are the `nsys` exports reduced to the two tables
mage's `nsys` backend reads — `StringIds` (8 rows, the kernel names the kernel table references) and
`CUPTI_ACTIVITY_KIND_KERNEL` (16 rows) — with the rest dropped and `VACUUM`ed: 356 352 → 12 288
bytes each, `pragma integrity_check` `ok`, and both still parse through
`mage.profiler.backends.nsys` to the same 16 metrics and the same totals (4919.562 µs before,
1384.342 µs after) that the tables above quote. The untrimmed exports, the `capture.nsys-rep`
reports and mage's `process.log` stay on the capturing machine.
