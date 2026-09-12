# T16 board capture — the eight device tests' kernels on Pinkie

The board half of ticket [#31](https://github.com/superposition/qualia/issues/31)'s definition of done:
one `ncu` pass over `crates/cuda/tests/gpu.rs` on **Pinkie**, the Waveshare-carried Jetson Orin NX
(D-010), with the real `sm_87` fatbins built on the board. It lands the capture the remediation
batch's board job (`BoardRemediateRuns`, its run comment on #31) left in board scratch
(`/home/jetson/remediate/515a506/remediate/t16-ncu/`) and whose run the ticket's own comment stream
quotes — the handoff below is that comment's command lines, verbatim, so the capture and the stream
agree. The capture itself predates this directory's landing; it is board
evidence for [#31](https://github.com/superposition/qualia/issues/31) and for
[#230](https://github.com/superposition/qualia/issues/230)'s retroactive audit.

`docs/evidence/T16/three-kernels/` is the dev-host reference this sits beside: the same suite captured
with mage/`nsys` on the shared RTX 4090 before D-012 made the board the profiling target.

## Host

- **Run on** Pinkie: `aarch64`, kernel `5.15.148-tegra`, driver `NVRM 540.4.0`, toolkit CUDA
  `12.9.41` at `/usr/local/cuda`, `ncu 2025.2.0.0 (build 35613519)`, compute capability `8.7`,
  board clock `2026-09-02T14:30Z` (≈9 days behind the dev host, D-010). `cargo`/`rustc` `1.94.0`.
- **Head:** `515a5063cc6655f87c821f7f3fe931cc77ca8f55` (`origin/main` at the time), shipped as a
  `git archive` and unpacked at `/home/jetson/remediate/515a506`.
- **GPU not exclusively held:** `llama-server` (`--n-gpu-layers 99`, pid 1304) has been resident
  since board clock `2026-09-01T16:56`, as it was for every earlier board capture. These are the
  board's own smoke numbers on a shared GPU under `ncu`'s default clock control, not a controlled
  comparison.

## Invocation

The build carries the `sm_87` cubins on the board (`CUDAARCHS=87-real`), and `LD_LIBRARY_PATH`
points at the forward-compat `libcuda` because the board's toolkit is newer than its driver
(`CUDA_ERROR_UNSUPPORTED_PTX_VERSION` without it — D-016, `T50/pinkie-kernels`).

```sh
CUDAARCHS=87-real NVCC=/usr/local/cuda/bin/nvcc \
  cargo test -p qualia-cuda --features cuda --release --test gpu --offline -j 2 --no-run
echo jetson | sudo -S env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat \
  /usr/local/cuda/bin/ncu --target-processes all --launch-count 8 \
  --export remediate/t16-ncu/capture --force-overwrite "$BIN" --test-threads=1
/usr/local/cuda/bin/ncu --import remediate/t16-ncu/capture.ncu-rep --page details --csv > remediate/t16-ncu/metrics.csv
```

`BIN` is `target/release/deps/gpu-e5293633ad869b15`; the build log records `qualia-cuda: embedded
fatbins for sm_87`. The suite's `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured;
0 filtered out; finished in 22.26s` is quoted in #31's comment with the harness output.

## Shape

**8 launches over 7 kernels** — the cap is `--launch-count 8`, not the suite's full launch total: one
pass of the eight device tests launches one kernel each plus `smoke`'s eight `add_one` launches (16 in
all, as the 4090's `nsys` capture of the same suite records), and the profiler keeps running past the
eighth launch, so `smoke`'s kernel is not in this capture (the same cut `T18/fatbin-sm-87` records).

`belief_update` and `cognition_update` are 96.2 % of the kernel time.

| id | kernel | block | grid | regs | static smem | duration |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | `action_score` | (256, 1, 1) | (2, 1, 1) | 30 | 1.02 Kbyte | 96.61 us |
| 1 | `belief_couple` | (4, 1, 1) | (1, 1, 1) | 26 | 0 | 21.06 us |
| 2 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 42 | 8.19 Kbyte | 3.41 ms |
| 3 | `cognition_update` | (1024, 1, 1) | (1, 1, 1) | 40 | 8.19 Kbyte | 3.60 ms |
| 4 | `cognition_update` | (1024, 1, 1) | (1, 1, 1) | 40 | 8.19 Kbyte | 3.60 ms |
| 5 | `cognition_patch` | (1, 1, 1) | (1, 1, 1) | 16 | 0 | 16.93 us |
| 6 | `costmap_stats` | (256, 1, 1) | (16, 1, 1) | 16 | 0 | 19.42 us |
| 7 | `perception_voxel` | (256, 1, 1) | (48, 1, 1) | 21 | 0 | 259.90 us |

The eight launches total **11 023.92 us** of kernel time. `SM Frequency` is a per-launch reading, not
one number: 305.98 MHz on `belief_update`, 305.34 on `action_score`, 619.63 on `belief_couple` — the
`kernels.csv` column spans 302.28–619.63 MHz under `ncu`'s default clock control, the same ~306 MHz
band `T50/pinkie-kernels` records. These durations are well below
`T18/fatbin-sm-87`'s (`belief_update` 3.41 ms here vs 13.00 ms there; `cognition_update` 3.60 ms vs
12.96 + 12.94 ms; totals 11.02 ms vs 39.32 ms over the same eight launches) — the same board, the
same eight launches, different by 3.57×. Reported as measured, with no claim attached to the
difference; the T18 capture is the same shape and the same clock setting, so the variable is not the
kernel.

## What changed against `three-kernels`

The dev-host capture is a superset: 16 launches over 8 kernels on the RTX 4090, including `smoke`'s
eight `add_one` launches, **4892.89 us** of kernel time. Comparing the eight launches both captures
share:

| kernel | 4090 (`three-kernels`) | board (this capture) | ratio |
| --- | --- | --- | --- |
| `perception_voxel` | 11.84 us | 259.90 us | 21.95× |
| `costmap_stats` | 1.44 us | 19.42 us | 13.49× |
| `cognition_patch` | 1.41 us | 16.93 us | 12.01× |
| `action_score` | 10.59 us | 96.61 us | 9.12× |
| `belief_couple` | 2.50 us | 21.06 us | 8.42× |
| `cognition_update` | 1641.7 us × 2 | 3600 us × 2 | 2.19× |
| `belief_update` | 1573.0 us | 3410 us | 2.17× |
| **total** | **4884.18 us** | **11 023.92 us** | **2.26×** |

So the board runs this suite **2.26× slower** than the 4090 server card, and the gap is widest on the
small parallel kernels (8–22×) and narrowest on the two 1024-thread blocks the T50 capture identified
as serial-row loops (2.17–2.19×).

## Files

| File | Contents |
| --- | --- |
| `capture.json` | the run's manifest in the field names `docs/evidence/README.md` fixes (`backend`, `argv`, `profiler_argv`, `returncode`, `status`, `kernel_count`), plus the board, profiler, source, target and environment blocks. Absolute paths are elided: the profiled argv is checkout-relative and the export path is tree-relative. |
| `capture.ncu-rep` | the report `ncu --export` wrote, 491 230 B, sha256 `f348d985b31923dd7a3a33d08d77be5402904ca98b6c5b7b1abfc2145f4b3e16`. |
| `metrics.csv` | the details-page export `ncu --import … --page details --csv` wrote (D-017's schema), 366 rows, 76 584 B, sha256 `b85da5167f38cc568b39ce06ce463d59b2173a70f1d295843a71bbebb2b46366`. |
| `kernels.json` | the eight launches as a hand-normalised projection of `metrics.csv`, with the projection's mapping, the export's hashes and the unit notes in its `capture` block. |
| `kernels.csv` | the same eight rows as CSV. |

The projection maps the details page's `Metric Name` rows onto the row's columns —
`Duration` → `duration_us` (the export prints ms or us; the ms rows are multiplied by 1000:
`3.41 ms` → `3410`), `SM Frequency` → `sm_frequency_mhz`, `Elapsed Cycles` → `elapsed_cycles`,
`Total L2 Elapsed Cycles` → `l2_elapsed_cycles`, `Registers Per Thread` → `registers_per_thread`,
`Static Shared Memory Per Block` → `static_shared_mem_kb` (kept in ncu's decimal Kbyte, so `8.19`
where the kernel declares 8192 B), `Dynamic Shared Memory Per Block` → `dynamic_shared_mem_kb` (its
unit is `byte/block`), `Driver Shared Memory Per Block` → `driver_shared_mem_per_block_kb`,
`Shared Memory Configuration Size` → `shared_memory_config_kb`, `Stack Size` → `stack_bytes`,
`Threads` → `threads`, `Waves Per SM` → `waves_per_sm`, the four `Block Limit *` rows,
`Theoretical Active Warps per SM`, `Theoretical Occupancy` → `theoretical_occupancy_pct` and
`Achieved Occupancy` → `achieved_occupancy_pct`. No value is changed, and the profiler's `nan`
(present on this pass for `Achieved Occupancy` and the SM/L1 throughput counters) becomes `null`,
not a substituted zero. `RUSTFLAGS`/`CUDAARCHS` are not in the export; they are in `capture.json`.

## What this capture does not establish

- **No timeline.** This is `ncu`, one launch at a time; the board now also carries `nsys` and `mage`
  (see [`docs/evidence/board/readiness/`](../board/readiness/README.md)), which a later ticket can use
  for a step-level timeline.
- **No per-launch counters for `smoke`.** The `add_one` loop is beyond `--launch-count 8`.
- **No controlled comparison.** The GPU was shared with `llama-server` and `ncu`'s clock control was
  left at its default, as in `T50/pinkie-kernels` and `T18/fatbin-sm-87`.
- **No claim about the 3.57× spread against `T18/fatbin-sm-87`.** Both captures are on this board with
  the same eight launches and the same shape; the numbers are reported as observed.
