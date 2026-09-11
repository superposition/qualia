# T50 capture — the rank-1 kernel refactor on Pinkie (the Orin NX board)

This directory re-measures the two kernels the rank-1 refactor changed, on the same board and in the same
lane as `docs/evidence/T50/pinkie-kernels/`, so the committed numbers there stop describing the
unrefactored kernels. It is the board half of PR #221 (ticket #172); the dev-host `nsys` pair lives
separately at `docs/evidence/T50/kernels-serial-walks/` and is not mixed with these rows.

* **Head:** `5eb7c9fe60dd65539151824857b046eb84abbe17` (`ticket/T50-kernels`, PR #221).
* **Board and OS:** Pinkie, the Jetson Orin NX Engineering Reference Developer Kit, `aarch64`, kernel
  `5.15.148-tegra`, driver `NVRM 540.4.0`, CUDA 12.9.41, `ncu 2025.2.0.0 (build 35613519)`, cargo and
  rustc 1.94.0, compute capability **8.7**, `LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat` (NVRTC 12.9
  emits PTX ISA 8.8 the stock driver will not JIT). Same device-suite binary shape as the committed
  capture:
  `cargo test -p qualia-cuda --features cuda --test gpu --no-run --release --offline -j 2`, run
  `--test-threads=1 --nocapture`, one launch-stats/occupancy `ncu` pass.

## What changed, as numbers

| kernel | duration before (committed) | duration after (this capture) | speed-up | `l1tex__t_bytes.sum` before → after | sectors after (`…mem_global_op_ld.sum`) | regs before → after | static shared |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `belief_update` | 12.976 ms / 12.995 ms | **3.407 ms** | **3.81×** | 100 717 696 → **25 220 224** B (matrix 100 663 296 → 25 165 824, **4.000×**) | **525 281** | 44 → **42** | 8192 B both |
| `cognition_update` | 13.046 / 12.991 / 13.005 / 13.015 ms | **3.589 / 3.595 ms** | **3.62×** | 104 892 416 → **29 394 944** B (matrix 104 857 600 → 29 360 128, **3.571×**) | **656 192** | 48 → **40** | 8192 B both |

**Against the previous capture:**
`docs/evidence/T50/pinkie-kernels/` is the board's measurement of the **unrefactored** kernels —
12.976 / 12.995 ms for `belief_update` and 13.046 / 12.991 / 13.005 / 13.015 ms for `cognition_update`,
with 100 717 696 B and 104 892 416 B of L1 traffic and 44 / 48 registers. This capture measures
**3.407 ms** and **3.589 / 3.595 ms**, **25 220 224 B** and **29 394 944 B**, at 42 / 40 registers: a
**3.81×** and **3.62×** fall in kernel time with the launch shape unchanged (grid `(1,1,1)`, block
`(1024,1,1)`, static shared 8192 B, dynamic 0). Nothing else in that directory changed, and its rows
stay valid for the five kernels this PR does not touch.

* The "before" durations and byte counts are the committed `docs/evidence/T50/pinkie-kernels/kernels.csv`
  rows (both the `base` and `clock-control none` runs agree to ~0.2 %); the "after" rows are this capture's.
* The **shape is unchanged** as the ticket asks: `block (1024, 1, 1)`, `grid (1, 1, 1)`, static shared
  memory 8192 B per block, dynamic 0 — only the register count moved (44 → 42, 48 → 40), which is the
  refactor's own footprint.
* **The counter the refactor moves is available on this driver**:
  `l1tex__t_sectors_pipe_lsu_mem_global_op_ld.sum` (525 281 and 656 192 sectors per launch) — no fallback
  to `l1tex__t_bytes` was needed, and `l1tex__t_bytes.sum` is reported alongside it. One sector per
  `float4` access is exactly the 2× state above.
* **The fetch is 8× the useful bytes before and 2× after — not 1×.** A 4-byte scalar element at a
  4096-byte lane stride takes a whole 32-byte sector; a 16-byte `float4` takes *half* of one, so the two
  halves of a 32-byte chunk are two accesses inside one sector and the sector is still fetched for the
  first of them. The matrix traffic per launch is therefore `3 × 32 MiB → 3 × 8 MiB` for `belief_update`
  (its two row reads and one row write, **4.000×**) and `32 + 4 + 32 + 32 → 8 + 4 + 8 + 8 MiB` for
  `cognition_update` (its two strided rows, its coalesced transposed gradient and its row write,
  **3.571×**) — see the two matrix figures in the table, which are the measured totals less the
  non-matrix traffic that is identical on both sides (54 400 B and 34 816 B). The sector counter confirms
  the after side directly: 525 281 load sectors against the 524 288 `float4` lane-accesses the two
  `belief_update` row walks issue, i.e. one 32-byte sector per 16-byte access.
* `dram__bytes.sum` is still `n/a` on the Tegra driver, so the L1-side rate is derived from the byte
  counter and the launch duration. It is **flat across the change**: 100 717 696 B / 12.976 ms =
  **7.76 GB/s** before and 25 220 224 / 3.407 ms = **7.40 GB/s** after for `belief_update`; 104 892 416 /
  13.046 ms = **8.04 GB/s** and 29 394 944 / 3.589 ms = **8.19 GB/s** for `cognition_update`. The duration
  fell with the traffic at a constant rate, so these two kernels are bound by **L1/LSU throughput, not by
  DRAM** — and a DRAM figure would not be reachable anyway from a one-block launch (one SM draws a small
  fraction of 1008 GB/s). [INFERENCE] on the attribution; the flat rate and the traffic-proportional
  duration are measured. L2 traffic fell with it: 42.7 MB → 16.9 MB and 47.6 MB → 21.0 MB
  (`lts__t_bytes.sum`).

## Method

```sh
export LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat
cargo test -p qualia-cuda --features cuda --test gpu --no-run --release --offline -j 2   # 3m 39s
BIN=./target/release/deps/gpu-e5293633ad869b15
"$BIN" --test-threads=1 --nocapture                        # 8 passed; 0 failed in 2.27 s
echo jetson | sudo -S env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat /usr/local/cuda/bin/ncu \
  --section LaunchStats --section Occupancy --launch-count 24 \
  --kernel-name 'regex:add_one|costmap_stats|belief_update|cognition_update|cognition_patch' \
  --metrics <the committed 21-metric list plus l1tex__t_sectors_pipe_lsu_mem_global_op_ld.sum> --csv \
  "$BIN" --test-threads=1 --nocapture > kernels-refactored.csv
```

`sudo` is required on this board (as the account user `ncu` prints `Insufficient privileges to launch app
for profiling`). The kernel regex, the sections, `--launch-count 24` and the metric list are the committed
capture's, so the rows stay comparable; the single addition is the sector counter above.

One difference to note: this head's `crates/cuda/tests/gpu.rs` has **8 tests** (the committed capture was
taken when it had 5), so the capture holds 13 launches over 6 kernels including `add_one` ×8; the two
kernels this ticket changes appear once and twice, exactly as the metric spec requires.

## Files

`kernels-refactored.csv` is the `ncu --csv` output verbatim, and `kernels.csv`/`kernels.json` are its
hand-normalised projection (one object per launch under a `run` column, the raw metric identifiers kept,
thousands separators removed — `dram__bytes_sum` stays `n/a` as the export has it). `metrics.csv` +
`capture.ncu-rep` are the same configuration's details-page export and report, committed whole as the
convention has it for `ncu`.

Two committed files carry the board's own deploy path, and that is corrected here rather than elided:

* `kernels-refactored.csv` carries it once, in the profiled process's command line:
  `/home/jetson/qualia-deploy/T50c/target/release/deps/gpu-e5293633ad869b15`. The file is a verbatim
  export, so redacting it would break both that claim and the hash the board's comment records for it
  (`2718fea8…`), and the path is the board's own deploy directory, not a secret.
* `capture.ncu-rep` carries the same path family, as every `ncu` report does, because a report embeds the
  profiled command line and the convention commits it whole for that reason — the committed
  `docs/evidence/T50/pinkie-kernels/base-capture.ncu-rep` carries it the same way.
* `capture.json`, `kernels.csv`, `kernels.json` and `metrics.csv` carry no filesystem path.

| file | bytes | sha256 |
| --- | --- | --- |
| `capture.json` | 4,946 | `5493c2b5c75e8214ba68f05aa9d2835d324cbf93447430046b3166b5a7221538` |
| `kernels-refactored.csv` | 126,789 | `2718fea8b11f3790458dc79efff56b02909e9be1b1ec85039259c20e662a8226` |
| `kernels.csv` | 2,471 | `57361d9536365c85303598599c0411c711d52ef7c93a1342323c00486e7e0b32` |
| `kernels.json` | 13,707 | `acd39d0d660868d65fec1b1de88995ccd70ec732f6ad128f02960d8ef897523f` |
| `metrics.csv` | 126,023 | `f23c1b46e0b747c2dac70510934c8a2790d12b5811721be79631ce5b2fca3fa2` |
| `capture.ncu-rep` | 483,917 | `06ed66abefa46086616e89f81935d610bbc95078ca0d51225728a48918b015bc` |

Every hash above is the one the board's own comment on PR #221 records, checked on the staged bytes
before they were copied in. Three files the run produced stay on the capturing machine and are not
committed, as the convention has it for a capture's transcripts: `board-run.log` (77,469 B,
`ef7402698cf15e46569292ffeb5e0444a7148512a4ee489fa21e3f1705f90793`), `ncu-export-run.log` (1,578 B,
`a3079d2b6b69742b6f5f0cc375ed98961ebdb0b67dff3b78529d5625c9ef5ee3`) and `t50-suite.log` (560 B,
`201c2baa2528b609cbbd05f827f17ef4289cb25164fc2cff75b2cebf7ba0f95e`). The first two carry the board's
absolute scratch paths (`/home/jetson/qualia-deploy/T50c/…`) and the suite's result line is already
recorded in `capture.json`'s `suite` block; all three stay named with their sizes and hashes in
`capture.json`'s `files` map, which is committed byte-identical to what the board staged. Their bytes are
on the capturing machine, so a reader cannot re-derive those three hashes from this repository — that is
the convention's own trade for a run's transcripts, and the sizes and hashes are recorded so a copy
produced from the board can be checked against them.

`capture.json` keeps mage's manifest field names (`argv`, `profiler_argv`, `returncode`, `status`,
`kernel_count`) and adds the board, build, suite, changed-kernel and unavailable-metric facts. Its
`commit` field pins `5eb7c9f`, the head the board checked out, built and profiled; **the device code it
measured is the kernels commit `4d441d8`**, which introduced both kernel files, and the two commits after
it (`5eb7c9f`, `a32139c`) change evidence text only.

### Power state and clock control: not recorded, and what that costs

This capture did not record the board's NV power mode, and it carries no `--clock-control` setting and no
`--clock-control none` repeat of the kind the committed `pinkie-kernels/` capture has. Two things follow,
one supporting and one limiting.

* Supporting, and derivable from the committed rows: `sm__cycles_elapsed.avg` over the duration is
  1 038 752 / 3.407 ms = **304.9 MHz** here, against 3 973 383 / 12.976 ms = **306.2 MHz** in the
  committed `pinkie-kernels/` capture. Both captures ran at the same ~305 MHz state — the committed
  README identifies that state as the board's 10 W mode — so the before/after rows above compare like
  for like in clock, and the refactor does not win by being measured at a faster one.
* Limiting: the absolute **3.407 ms is one `ncu` sample at that state**, not a base-clock-controlled
  repeat, so it should be read as reproducible to about the spread the committed capture saw between its
  own two runs (~0.2 %) rather than as a pinned number. The counts this capture turns on — 525 281 and
  656 192 sectors, 25 220 224 and 29 394 944 L1 bytes, 42 and 40 registers, and the launch shape — are
  power-state-independent.
