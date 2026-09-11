# T50 capture — the JEPA kernels on Pinkie (the Orin NX board)

The profiling target for this ticket is the board, not the development host (D-012). This is the first
board capture in the tree — the only other capture at this head is `docs/evidence/T16/three-kernels/`
on `main`, and the 4090 baseline is `docs/evidence/baseline-2026-09-11/` in PR #166 — and it is taken
on the Waveshare-carried Jetson Orin NX. One pass of the five device tests in
`crates/cuda/tests/gpu.rs` at `a28736f` put **13 CUDA launches over 5 kernels** on the Orin's GPU and
cost **39.18 ms** of kernel time, against **4.78 ms** for the same 13 launches, shapes and launch
counts on the RTX 4090 (`baseline-2026-09-11/`, which PR #166 carries): **8.19× slower**. Two kernels
are 99.6 % of it — `belief_update` 12.976 ms and `cognition_update` 12.991 + 13.046 ms — and both are
written as one block of 1024 threads where each thread walks a full 1024-wide row of the weight matrix
in a serial loop. The counters say the problem is not occupancy: the single 1024-thread block is
resident (65.59 % of the SM's peak sustained-active warps) and the SM issues **1.52 %** of its peak
instruction rate, with an active warp present in only **25.0 %** of the elapsed cycles. The same board
reaches **16.58 %** SM throughput on `costmap_stats`, the one kernel written as a parallel map over
its 4096 elements. That gap is the measured difference between the naive shape and a parallel one, on
the hardware the naive shape actually ships on.

## Layout

`docs/evidence/README.md` — the layout this directory follows — lands with #166. A capture directory
holds `capture.json`, `kernels.json`, `kernels.csv`, the backend's export and this README; this
directory holds six files — those five plus the second run's export.

This capture is **manual**, under #166's `## Manual capture` clause: mage cannot run on Pinkie (no
`nsys`, and `triton>=3.0` publishes no aarch64 wheel to install mage with), so the two `ncu` runs were
driven by hand. `capture.json` therefore keeps mage's manifest field names for the primary (`base`)
run — `argv` (the profiled argv), `profiler_argv`, `returncode`, `status`, `kernel_count` — with the
board, build and environment data added alongside, and the `--clock-control none` run under `runs[]`.
`kernels.json` and `kernels.csv` carry the same rows: **one object per CUDA launch**, 13 for each of
the two runs, with the launch shape, the launch-statistics section and every counter the board's
driver returned. They are a **hand-normalised projection of the raw export**, not mage's reader
output: the columns are renamed to the names in §"The raw export", the two runs are merged under an
added `run` column, and numeric formatting is normalised. No value is changed — `dram__bytes.sum` is
`n/a` in the export itself, not injected — and the projection was checked column-for-column against
the exports. There is no `capture.ncu-rep`: neither run was passed `--export`, so no report was
written for them and the board holds none (measured); §"The raw export" carries the exports' sizes
and hashes. The runs are labelled `base` and `none` in the `run` column.

## Shape

One pass of the five device tests, one test thread. Every device context is constructed once per
test, so each kernel runs the number of launches below. The clock column is Nsight Compute's own
`--clock-control` setting: `base` is its default and holds the GPC clock near 306 MHz, `none` leaves
the clock alone. Both runs are on the same board, back to back. Their **totals** agree to 0.001 %
(39 182 176 vs 39 181 760 ns) and `belief_update` to 0.14 %; per launch the spread runs to 3.42 %
(`cognition_patch`), 2.69 % (`add_one`, one of its eight launches), 0.98 % (`costmap_stats`), 0.32 %
and 0.18 % (`cognition_update`) — the totals agree, the individual launches do not all.

| Kernel | Launches | Grid × Block | Regs | Static smem | Duration (`base`) | SM cycles | Duration (`none`) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `belief_update` | 1 | 1 × 1024 | 44 | 8192 B | 12 976 128 ns | 3 973 383 | 12 994 688 ns |
| `cognition_update` | 2 | 1 × 1024 | 48 | 8192 B | 13 046 368 / 12 991 136 ns | 3 976 442 / 3 977 719 | 13 004 768 / 13 014 528 ns |
| `cognition_patch` | 1 | 1 × 1 | 16 | 0 B | 16 864 ns | 3 163 | 16 288 ns |
| `costmap_stats` | 1 | 16 × 256 | 16 | 0 B | 19 520 ns | 4 246 | 19 328 ns |
| `add_one` | 8 | 1 × 4 | 16 | 0 B | 16 064–16 992 ns | 3 077–3 297 | 16 224–17 088 ns |

13 launches, **39 182 176 ns (39.18 ms)** of kernel time with the clock held, 39 181 760 ns without.

The 1 × 1024 kernels launch **one block in total** — 1024 threads, 32 warps — and it runs on one SM;
`waves_per_sm` 0.25 is that single block averaged over the board's 4 SMs (`# SMs` 4, `# TPCs` 2 in
every row of `kernels.csv`). Occupancy is capped by registers at **one block per SM**
(`occupancy_limit_registers` = 1, `..._shared_mem` = 1, `..._warps` = 1, `..._blocks` = 16), which is
what holds theoretical occupancy at 66.67 %: 1024 resident threads of the 1536 a Jetson SM can hold
(32 of the 48 warps that SM can hold).
Those limits and the 44/48 registers per thread are in every row of `kernels.csv`; the loop shape, not
residency, is what the counters below indict.

## What the counters say

`belief_update`, one launch on one SM:

| Metric | Value |
| --- | --- |
| `gpu__time_duration.sum` | 12 976 128 ns |
| `sm__cycles_elapsed.avg` | 3 973 383 |
| `sm__cycles_active.avg` | 992 091 (25.0 % of elapsed) |
| `sm__warps_active.avg.pct_of_peak_sustained_active` | 65.59 % |
| `sm__throughput.avg.pct_of_peak_sustained_elapsed` | 1.52 % |
| `smsp__inst_executed.sum` | 316 048 |
| `l1tex__t_bytes.sum` | 100 717 696 B |
| `lts__t_bytes.sum` | 42 711 904 B |
| `dram__bytes.sum` | n/a (not exposed by the Tegra driver) |

`cognition_update` is the same picture with one more pass over the matrix: 3 976 442 cycles,
997 457 active cycles, 64.76 % warps, 1.41 % SM throughput, 403 680 instructions, 104 892 416 B L1
and 47 616 320 B L2.

The contrast that matters is across kernels on the *same* board and the same run:

| Kernel | Threads | Waves/SM | Theoretical occ. | Warps active | SM throughput | Instructions |
| --- | --- | --- | --- | --- | --- | --- |
| `belief_update` | 1024 (1 block) | 0.25 | 66.67 % | 65.59 % | **1.52 %** | 316 048 |
| `cognition_update` | 1024 (1 block) | 0.25 | 66.67 % | 64.76 % | **1.41 %** | 403 680 |
| `costmap_stats` | 4096 (16 blocks) | 0.67 | 100 % | 64.58 % | **16.58 %** | 4 352 |
| `add_one` | 4 (1 block) | 0.02 | 33.33 % | 2.08 % | 0.14 % | 13 |

`belief_update` and `costmap_stats` have almost the same *residency* (65.59 % vs 64.58 % warps) and
differ by **11×** in issued work per cycle. Residency is not the constraint; the loop is.
`belief_update` issues 316 048 warp-instructions in 3 973 383 cycles — one warp-instruction per 12.6
cycles of the SM clock — while an active warp exists in only a quarter of the cycles, i.e. the
resident warps spend most of their time waiting on a dependent load inside a serial 1024-iteration
loop.

Where the loop lives, at the measured commit:

- `kernels/belief_update.cu:66`–`:69` — one thread per dimension; each reads its own 1024-float row
  and accumulates a 1024-term dot product against the shared `prior_mean`.
- `kernels/belief_update.cu:118` — a second serial 1024-iteration walk of the same row for the
  Hebbian weight step, a read-modify-write per element.
- `crates/cuda/src/cuda_impl.rs:85` — `cognition_update`'s prediction loop, same shape.
- `crates/cuda/src/cuda_impl.rs:92`–`:93` — `transposed_grad`, the same matrix read **column-major**
  (`weights[row * 1024 + tid]`, stride 4096 B), the worst access pattern in either kernel.
- `crates/cuda/src/cuda_impl.rs:102`–`:105` — the third serial pass, the row weight step.
- `crates/cuda/src/cuda_impl.rs:67` is the kernel entry; both kernels stage the row they multiply
  against in `__shared__` scratch of 1024 floats (`prior_mean`, `prior_state`), and both compile to
  44 and 48 registers per thread respectively.

**On `dram__bytes.sum` being `n/a`:** the Tegra driver exposes no DRAM counter to Nsight Compute, so a
"memory-bound" verdict cannot be earned on this board. The L1 and L2 byte counts are real and are in
the CSV; the DRAM number is not, and is not claimed.

## Does the board reproduce the 4090's ranking?

Yes for the ranking, no for the durations. On the 4090 the same file cost 4 784.75 µs with the same 13
launches and the same shapes; here it costs 39 182.18 µs at 306 MHz, and the board's `belief_update`
took 3 973 383 SM cycles. [INFERENCE] The 4090's 1547.8 µs at its ~2.5 GHz clock is about 3.9 M
cycles, so the two machines appear to spend the *same* number of cycles on the same loop and to
differ mainly in clock — which is the shape D-011 predicts for a naive, latency-bound kernel: a
slower clock multiplies the wall time, and the board is where the naive shape is worst. This capture
did not measure the 4090's clock, so the cycle count on that side is inferred, not measured.

## Invocation

Two obstacles stood between the board and a capture, and the commands below are the ones that worked.
Neither touches a display adapter, and no host profiler counter was attempted (D-012).

**1. Getting the target onto the board.** Docker is not available on the development workstation, so
`Cross.toml` and `docker/Dockerfile.cross-aarch64` cannot be used; the board has cargo 1.94.0, rustc
1.94.0 and gcc 11.4.0, so it builds natively from a source archive. The board has no DNS and its
registry cache is stale in two ways, so:

```bash
# on the development host, from the checkout
git archive HEAD | gzip > qualia.tar.gz
scp -i ~/.ssh/qualia_jetson_ed25519 qualia.tar.gz jetson@192.168.55.1:~/waveshare-profile/
ssh -i ~/.ssh/qualia_jetson_ed25519 jetson@192.168.55.1
mkdir -p ~/waveshare-profile/qualia
tar xzf ~/waveshare-profile/qualia.tar.gz -C ~/waveshare-profile/qualia
```

`cargo` resolves every workspace member even for a single `-p`, and the board's offline cache has no
`tempfile`, so the root manifest's `members` list is trimmed to `crates/cuda`, `crates/types` and
`crates/shm` with `[workspace.dependencies]` reduced to the two local paths those use. The checked-in
`Cargo.lock` pins `serde_derive 1.0.229`, which the board's cached index does not have (it has
1.0.228), so the lock is moved aside and cargo re-locks offline from the cache. Both edits are
recorded in `capture.json`. Then, on the board:

```bash
export PATH=$HOME/.cargo/bin:$PATH
cargo test -p qualia-cuda --features cuda --test gpu --no-run --release -j3 --offline
# Finished `release` profile [optimized] target(s) in 39.38s
# Executable tests/gpu.rs (target/release/deps/gpu-dd04a78e6201dd63)
```

**2. The board's toolkit is newer than its driver.** `/usr/local/cuda` is 12.9.41, cudarc compiles
the embedded kernels with NVRTC at process start, the driver is 540.4 (CUDA 12.6) and NVRTC has no
PTX-version switch, so without help every device test skips itself:

```text
test belief_update_matches_the_host_reference ... skipping CUDA belief test: PTX load failed: DriverError(CUDA_ERROR_UNSUPPORTED_PTX_VERSION, "the provided PTX was compiled with an unsupported toolchain.")
```

A direct probe of the driver API on the board found the ceiling and the fix:

```text
cuInit            result=0
driver version    12060 (CUDA 12.6)
device            Orin (cc 8.7)
cuCtxCreate       result=0
load ptx-8.5    result=0   CUDA_SUCCESS                     no error
load ptx-8.6    result=222 CUDA_ERROR_UNSUPPORTED_PTX_VERSION the provided PTX was compiled with an unsupported toolchain.
load ptx-8.8    result=222 CUDA_ERROR_UNSUPPORTED_PTX_VERSION the provided PTX was compiled with an unsupported toolchain.
```

CUDA 12.9 emits PTX ISA 8.8; this driver JITs 8.5 and below. The toolkit ships a
forward-compatibility `libcuda`, and pointing the loader at it is enough — with
`LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat` the same PTX 8.8 image loads
(`driver=12090 ... result=0`) and all five tests pass:

```bash
export LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat
./target/release/deps/gpu-dd04a78e6201dd63 --test-threads=1 --nocapture
# test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.09s
```

One further route was checked and not needed: a cubin from the board's own `nvcc 12.9.41`
(`-arch=sm_87 -cubin`) loads on the stock driver (result 0), so a precompiled-SASS build is a
fallback if the compat loader is ever unavailable. The loader path above was used because it changes
neither the kernels nor the driver.

**3. The capture.** From the checkout root on the board:

```bash
METRICS=gpu__time_duration.sum,sm__cycles_elapsed.avg,sm__cycles_active.avg,\
sm__warps_active.avg.pct_of_peak_sustained_active,sm__throughput.avg.pct_of_peak_sustained_elapsed,\
smsp__inst_executed.sum,l1tex__t_bytes.sum,lts__t_bytes.sum,dram__bytes.sum,\
launch__registers_per_thread,launch__shared_mem_per_block_static,launch__shared_mem_per_block_dynamic,\
launch__grid_size,launch__block_size,launch__waves_per_multiprocessor,\
launch__occupancy_limit_registers,launch__occupancy_limit_shared_mem,launch__occupancy_limit_warps,\
launch__occupancy_limit_blocks,sm__maximum_warps_per_active_cycle_pct
KERNELS='regex:add_one|costmap_stats|belief_update|cognition_update|cognition_patch'
BIN=./target/release/deps/gpu-dd04a78e6201dd63

sudo env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat /usr/local/cuda/bin/ncu \
  --section LaunchStats --section Occupancy \
  --launch-count 24 --kernel-name "$KERNELS" --metrics "$METRICS" --csv \
  "$BIN" --test-threads=1 --nocapture > kernels-base.csv

sudo env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat /usr/local/cuda/bin/ncu \
  --clock-control none --section LaunchStats --section Occupancy \
  --launch-count 24 --kernel-name "$KERNELS" --metrics "$METRICS" --csv \
  "$BIN" --test-threads=1 --nocapture > kernels-none.csv
```

Observed, both runs: `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`,
`ncu` exit 0, 13 launches, harness wall time 49.69 s (base) and 48.36 s (unlocked). `sudo` is
required — as the account user `ncu` prints `Insufficient privileges to launch app for profiling` and
captures nothing. `--launch-count 24` bounds each run, each run started only after the previous one
finished, and the host's 4090 was not used by this work at any point (D-011). The board itself was
not exclusively mine: other agents were building and running on it in the same window (one reported
an aarch64 build plus a single four-thread kernel run), which is why the two committed runs are
reported as agreeing rather than as an undisturbed measurement. Across the session `belief_update`
was captured five times between 12 976 128 ns and 13 003 168 ns — a 0.21 % spread — so whatever else
was running moved the second decimal, not the number. No `--export` was passed: with `--csv` alone
the CSV goes to stdout, so there is no report to re-import, and `--page details` (ncu's `--csv`
default) is the page these rows came from.

Two notes for the next person. `--set basic` reports `nan` for every throughput and occupancy metric,
and combining `--set basic` with `--metrics` silently leaves every named counter except the duration
at `nan`; naming the metrics explicitly and asking for the two sections by name
(`--section LaunchStats --section Occupancy`) keeps both the named counters and the section values
that `kernels.csv` carries.

## The raw export

The two commands above send ncu's stdout and the profiled test binary's stdout to the same file, so
each raw export starts with the harness's `running 5 tests …` lines and ncu's CSV header follows. The
committed exports are that CSV from ncu's `"ID",` header down — the harness lines before it are
dropped, which also keeps the board's `/home/jetson/...` binary path out — and they are otherwise
verbatim: one row per metric per launch, from the `--page details` page ncu's `--csv` writes.

| Committed | Source on the capturing machine | Size | SHA-256 |
| --- | --- | --- | --- |
| `metrics.csv` | `~/waveshare-profile/final-base.csv` (base clock) | 122 810 B | `3538a433fe0afa1581b3cde6e38b49edaed6fe8f5c44ac6105025c33ca8a8cf3` |
| `metrics-none.csv` | `~/waveshare-profile/final-none.csv` (`--clock-control none`) | 122 810 B | `3e95299882ae44e6398c72be7a93f8e99353c2edb79fec62db5e00b0b8814fc1` |

The source log files are 123 352 B and 123 474 B; the difference is the dropped harness lines. There
is no `capture.ncu-rep` for these runs: neither was passed `--export`, and neither the capturing
machine nor the board holds a `.ncu-rep` for either committed run (both checked after the session).
The only report that exists is an exploratory base-clock pass's, `capture-basic.ncu-rep` — 832 246 B,
SHA-256 `cbb08639fa4ad36f4d19eb44e18d5e3cb89df77a3389421910c69d5808a81c66`, left on the capturing
machine because it is over the ~196 KB the tree carries — and it is not either committed run's report:
its `--section` metric set is not this capture's `--metrics` list and it profiled a different
`belief_update` replay (3 973 027 cycles against the committed 3 973 383). The `--page raw` page the
convention names is unavailable for the committed runs for the same reason — it is an `ncu --import`
page and there is nothing to import. Re-run with `--export <dir>/capture` and `--csv --page raw` to
get a report and a raw page for the committed configuration.

`kernels.csv` and `kernels.json` are a renamed projection of the export: each committed column keeps
the export's `Metric Value` for its identifier, formatted (thousands separators and trailing zeros
removed; `n/a` kept). The mapping from the committed column name to ncu's metric identifier:

| Committed column | `Metric Name` in the export |
| --- | --- |
| `duration_ns` | `gpu__time_duration.sum` |
| `cycles_elapsed` / `cycles_active_sm` | `sm__cycles_elapsed.avg` / `sm__cycles_active.avg` |
| `warps_active_pct_of_peak` | `sm__warps_active.avg.pct_of_peak_sustained_active` |
| `sm_throughput_pct_of_peak` | `sm__throughput.avg.pct_of_peak_sustained_elapsed` |
| `instructions_per_smsp_sum` | `smsp__inst_executed.sum` |
| `l1tex_bytes` / `l2_bytes` / `dram_bytes` | `l1tex__t_bytes.sum` / `lts__t_bytes.sum` / `dram__bytes.sum` |
| `registers_per_thread` | `launch__registers_per_thread` |
| `static_shared_bytes` / `dynamic_shared_bytes` | `launch__shared_mem_per_block_static` / `launch__shared_mem_per_block_dynamic` |
| `grid` / `block` / `waves_per_sm` | `launch__grid_size` / `launch__block_size` / `launch__waves_per_multiprocessor` |
| `occupancy_limit_registers` / `..._shared_mem` / `..._warps` / `..._blocks` | `launch__occupancy_limit_registers` / `launch__occupancy_limit_shared_mem` / `launch__occupancy_limit_warps` / `launch__occupancy_limit_blocks` |
| `theoretical_occupancy_pct` and `section_theoretical_occupancy_pct` | `Theoretical Occupancy` (Occupancy section) |
| `sm_count` / `tpc_count` | `# SMs` / `# TPCs` |
| `shared_mem_config_bytes` / `driver_shared_mem_per_block_bytes` / `stack_bytes` / `threads` | `Shared Memory Configuration Size` / `Driver Shared Memory Per Block` / `Stack Size` / `Threads` |
| `block_limit_sm` / `block_limit_shared_mem` / `block_limit_warps` | `Block Limit SM` / `Block Limit Shared Mem` / `Block Limit Warps` |
| `theoretical_active_warps_per_sm` | `Theoretical Active Warps per SM` |

`kernels.csv` equals `kernels.json` row for row — 26 rows, 13 `base` + 13 `none` — and every number
in this README is recomputed from them.

## What this does not establish

- **The JEPA model steps are not measured, and there is no model to measure yet.** At the captured
  commit — and still at this branch's base and at `origin/main` `afd9a75` — `crates/jepa-model/src/lib.rs`
  is a one-line doc-comment stub, `runners/jepa-runtime/src/main.rs` is `fn main() {}`, no `.rs` file
  in the tree mentions candle (it is a manifest dependency of a crate with no code, and no file
  constructs a candle device), and the three executables #172 step 1 names — `qualia-jepa-train`,
  `qualia-jepa-parity`, `qualia-jepa-plan-eval` — exist nowhere in the tree. Building candle's CUDA
  backend on the board would profile candle's own kernels with no qualia code in the process, so it
  was not attempted. The board's offline registry cache also lacks the CUDA-path crate set and the
  matching index entries, so a candle build there needs a vendor upload as well — a separate step from
  this capture.
- **No timeline capture.** There is no `nsys` binary on the board and no network to install one, and
  `mage` is not installed there either, so #64's `mage profile-exec --backend nsys` path cannot run on
  Pinkie. This capture is `ncu` only, and kernel-level rather than step-level. `mage` also resolves
  `triton>=3.0`, which has no aarch64 wheel published for it, so installing mage on the board is not a
  matter of copying a binary.
- **No DRAM verdict.** See the `n/a` above.
- **Duration is a profiler duration.** Both runs report the duration of an isolated, replayed launch,
  not a throughput measurement of the pipeline. Compare launch count and shape first, then cycles,
  then duration.
- **Not verifiable from the committed files.** The exports are ncu's own rows, so the counters in them
  are as recorded; these claims rest on the capturing session instead: that both invocations ran under
  `sudo` and exited 0, the `5 passed` line and the 49.69 s / 48.36 s harness times (self-reported in
  `capture.json` — nothing in the committed bytes checks its `returncode`), the board facts in
  §"Board" (driver 540.4, `NV Power Mode: 10W`, toolkit 12.9.41, cargo/rustc/gcc versions, 161 GiB
  free, "GPU idle at the start"), the five-capture 0.21 % spread for `belief_update` (only two runs
  are committed), and the 4090's clock in the `[INFERENCE]` paragraph. The 8.19× also divides an ncu
  isolated-replay duration (ncu's default clock *and* cache control) by an nsys wall-clock duration
  from a different profiler, host and commit; the shapes and launch counts are the comparable part.
- **The 306 MHz is the board's 10 W power mode**, not a profiler artefact: the unlocked repeat, which
  runs with `--clock-control none`, reports the same total kernel time to 0.001 %.

## Board

`ssh -i ~/.ssh/qualia_jetson_ed25519 jetson@192.168.55.1` — L4T 5.15.148-tegra, aarch64, 6 cores,
3.6 GiB RAM, `NV Power Mode: 10W`, driver 540.4.0, CUDA toolkit 12.9.41, `ncu` 2025.2.0.0 at
`/usr/local/cuda/bin/ncu` (not on `PATH`), `nvcc` 12.9.41 present, no `nsys`, no `mage`, 161 GiB free
on `/`. The GPU was idle at the start: `nvidia-smi` reported no running processes. The host's 4090 was
not used by this work at any point.
