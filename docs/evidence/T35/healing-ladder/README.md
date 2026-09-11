# T35 capture — the healing ladder's decision path

Capture of the healing ladder's decision path for ticket
[#51](https://github.com/superposition/qualia/issues/51), the one `needs:profile` step that is not a
kernel: [T35 is the kernel-less case](../README.md#what-a-capture-directory-holds) the convention
names. The path is `next_step` and `Ladder::step` in `crates/braid/src/heal.rs` — a comparison against
`3.0`, `5.0` and `8.0` plus the observe-only hold — exercised by the eight tests in
`crates/braid/tests/heal.rs`.

## Host and tools

WSL2 `Ubuntu-22.04` on the dev host, RTX 4090, driver 591.74, `nsys` 2025.3.2.474 (the host carries
no `ncu` in the guest), `mage` 0.1.0 built from `superposition/mage` at `7a798f6`, rustc 1.98.1 (the
guest's stable 1.85 cannot build the braid dependency tree). The trace ran on the dev host, not on
Pinkie: Pinkie has no `nsys`, and the decision path touches no device, so there is no board capture
to take (D-012's backend table).

## Shape

The braid test binary, one test thread at a time, under an `nsys` CPUTrace:

```text
$ ~/t47-target/debug/deps/heal-f96c58e469ce4c8b --test-threads=1
running 8 tests
test a_reading_that_leaves_observe_only_restarts_the_hold ... ok
test exhausted_attempts_hold_observe_only ... ok
test ladder_escalates_with_drift ... ok
test no_opinion_leaves_a_live_hold_where_it_was ... ok
test no_opinion_selects_nothing ... ok
test safe_stop_follows_a_ten_second_hold_above_eight ... ok
test the_band_edges_belong_to_the_lower_rung ... ok
test the_hold_needs_a_drift_still_above_eight ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

8 tests, 8 traced thread windows, **31 ladder decisions** (that is the iteration count this capture
carries: 17 `next_step` calls and 14 `Ladder::step` calls, one per ladder-call assertion in the suite
— its 32nd assertion checks the ragged sample's `sample_count`, line 70, and is not a ladder call).
The decisions are pure comparisons — no syscall, no clock, no device call — so the trace resolves the
path to one window per test and cannot separate the decisions inside a window. The window bounds are
the test thread's own first and last `osrt` event; `timeline.csv` gives every decision its window's
bounds and the parent's create/join end for the same test.

## Timeline

Timestamps are nanoseconds on the trace clock, whose session start is
`2026-09-11T21:23:28.511357438Z` (the `utcEpochNs` field of the export's
`TARGET_INFO_SESSION_START_TIME`; its `utcTime` string is that second, second-precision); the
absolute column is that epoch plus the window start. Windows are in run order — libtest sorts the
test names and, with `--test-threads=1`, runs them one at a time in that order, and the trace's
`pthread_create`/`pthread_join` cycles follow it one for one (so window *i* is test *i* of the order
above; the names are not interned in the trace, the order is the binding).

| # | Window (test) | Thread | Start (ns) | Start (UTC) | Duration | Decisions |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `a_reading_that_leaves_observe_only_restarts_the_hold` | 281629914309670 | 36 142 808 | 21:23:28.547500 | 1212.711 µs | 5 |
| 2 | `exhausted_attempts_hold_observe_only` | 281629914309671 | 37 686 365 | 21:23:28.549043 | 914.072 µs | 5 |
| 3 | `ladder_escalates_with_drift` | 281629914309672 | 38 808 144 | 21:23:28.550165 | 342.867 µs | 4 |
| 4 | `no_opinion_leaves_a_live_hold_where_it_was` | 281629914309673 | 39 570 705 | 21:23:28.550928 | 1145.255 µs | 3 |
| 5 | `no_opinion_selects_nothing` | 281629914309674 | 41 000 225 | 21:23:28.552357 | 490.026 µs | 2 |
| 6 | `safe_stop_follows_a_ten_second_hold_above_eight` | 281629914309675 | 41 704 274 | 21:23:28.553061 | 595.398 µs | 4 |
| 7 | `the_band_edges_belong_to_the_lower_rung` | 281629914309676 | 42 570 095 | 21:23:28.553927 | 2624.480 µs | 6 |
| 8 | `the_hold_needs_a_drift_still_above_eight` | 281629914309677 | 45 433 706 | 21:23:28.556791 | 508.273 µs | 2 |

7 833.082 µs of traced window, thread start and exit dominated — not decision time. The main test
thread (281629914309651, `heal-f96c58e469`) shows the spine: eight `pthread_create`/`pthread_join`
cycles, each with a `futex` wait between them, and eleven `write` events (the harness's own console
lines). Inside a window the ladder's rungs ran in the ticket's order:

| Rung | Decisions | Drift inputs |
| --- | --- | --- |
| nothing (healthy) | 6 | `2.0` at attempts 0 and 3, `3.0` at 0, the ragged sample at 0 (twice: the plain call and the hold test's `now_ns=5 s` reading) and at 3 |
| `Recalibrate` | 5 | `4.0` at 0 (twice: the plain call and the hold test's `now_ns=9 s` reading) and at 2, `5.0`, `3.000001` |
| `RollBack` | 4 | `7.0` at 0 and 2, `8.0`, `5.000001` |
| `ObserveOnly` | 12 | `12.0` ×7 (three of them inside a live hold), `8.000001`, `4.0` at attempts 3, `7.0` at attempts 3 ×3 (one held 60 s) |
| `SafeStop` | 4 | `12.0` with the observe-only hold aged to 10 s, 11 s, 12 s and 20 s |

17 distinct readings fired those 31 decisions: ten squared Mahalanobis values (`2.0`, `3.0`,
`3.000001`, `4.0`, `5.0`, `5.000001`, `7.0`, `8.0`, `8.000001`, `12.0`) at the attempt counts the
tests set, plus the ragged sample `measure(&[1.0, 2.0], &[1.0], &[0.0, 0.0])` — `sample_count == 0`,
which selects nothing at any budget. The band edges are exercised as the table writes them: `3.0`
selects nothing, `5.0` recalibrates, `8.0` rolls back, and each threshold's neighbour a hair above
escalates one rung.

## The request-vs-assert fact

Four decisions selected `SafeStop` (decisions 5, 17, 22, 23 in `timeline.csv`) and every one of them
is a value the test asserted: the trace has no `CUPTI_ACTIVITY_KIND_KERNEL` table — zero CUDA
launches — no `fork`, `execve` or `posix_spawn` event, and the only threads it created are the
harness's eight test threads. Nothing left the process. The ladder *requested* a safe stop; no motor,
leash or device was called, which is the crate's authority statement measured rather than read.

## Invocation

mage has no `Cargo.toml` (only the `examples/cutile` and `examples/oxide` samples do), so the
ticket's `cargo build --release -j 2` does not apply to it; the deliberate fallback is the project's
own build — `uv build` (hatchling) then `uv tool install`:

```bash
git clone https://github.com/superposition/mage.git /mnt/c/tmp/Impl47MageT35/mage
cd /mnt/c/tmp/Impl47MageT35/mage && uv build                 # wheel + sdist, exit 0
uv tool install --force /mnt/c/tmp/Impl47MageT35/mage        # exit 0; mage 0.1.0 from the clone
```

The target is the repo's braid test binary, built `-j 2` with the toolchain the tree needs:

```bash
cargo +1.98.1 test -p qualia-braid --test heal --no-run -j 2   # tests/heal.rs, exit 0
```

The kernel-less mage run this directory's `capture.json` is from:

```bash
mage profile-exec --backend nsys --capture-range all \
  --output-dir ~/mage-capture-scratch/T47/heal-mage2 \
  -- ~/t47-target/debug/deps/heal-f96c58e469ce4c8b --test-threads=1
# Error during profiling: nsys captured no CUDA kernel launches. Check the target and capture
# range. Reports: ~/mage-capture-scratch/T47/heal-mage2/mage-nsys-yteulr8z        # exit 1
```

`--capture-range all` and not `cuda`: a `cuda` range records nothing here either, and on `nsys` it
fails earlier — measured on this host with the GPU binary in the tooling check below,
`mage profile-exec --backend nsys --capture-range cuda …` printed `Error during profiling: Nsight
Systems SQLite export is missing: ~/mage-capture-scratch/…/capture.sqlite` and exited 1, because
`nsys` writes no export for a window no `cudaProfilerStart` opens.

The CPU timeline is a **manual** capture: mage's nsys argv is fixed at `--trace=cuda,nvtx`, so the
`osrt` trace the timeline needs is not reachable through `mage profile-exec`
(`docs/evidence/README.md` §Manual capture: state the reason and the exact command). The delta from
mage's argv is exactly the trace set:

```bash
nsys profile --trace=osrt,cuda,nvtx --sample=none --cpuctxsw=none --stats=false \
  --export=sqlite --output=~/mage-capture-scratch/T47/cpu-trace/capture \
  ~/t47-target/debug/deps/heal-f96c58e469ce4c8b --test-threads=1     # exit 0
```

## Tooling check on this host

mage drove a real GPU capture too (the same guest, one bounded run): `cargo build --release -p
qualia-cuda-bench -j 2` then

```bash
QUALIA_CUDA_BENCH_SHAPE=1024 QUALIA_CUDA_BENCH_ITERS=2 mage profile-exec --backend nsys \
  --capture-range all --output-dir ~/mage-capture-scratch/T47/all-range \
  -- ~/t47-target/release/qualia-cuda-bench                              # exit 0
```

10 launches over 3 kernels, 332.36 µs of kernel time, `capture_range: all`, session 40 in
`~/.mage/profiles.db`: `cutlass::Kernel2<cutlass_80_simt_sgemm_128x64_8x5_nn_align1>` 7 × 44.6–46.3 µs
(128×128), `at::native::…normal_and_transform` 2 × 4.99–5.18 µs (768×256) and
`at::native::reduce_kernel` 5.37 µs (1×128). The `--capture-range cuda` failure above is from the
same binary.

## Files

`capture.json` is mage's manifest for the kernel-less run, with every absolute path elided to
home-relative (the profile directory in `profiler_argv` and `error`, the target binary in `argv` and
`command`); `status: "failed"`, `returncode: 0` and the `captured no CUDA kernel launches` error are
mage's own text. No `kernels.json`/`kernels.csv` exists for it and none is manufactured.

`timeline.csv` binds the suite's call sequence to the trace's windows: one row per ladder decision,
31 rows, each carrying its window's bounds, absolute start, the parent thread's create/join ends, the
API, the reading it was handed and the rung it selected. The window bounds, thread ids and UTC start
are the trace's; the API, reading and rung are the suite's call sequence
(`crates/braid/tests/heal.rs`), which the trace does not carry — inside a window the decisions are
not separable, so they take that window's bounds.

`capture.sqlite` is a **trimmed** export of the manual trace — the tables the timeline is read from
(`OSRT_API`, `ThreadNames`, `StringIds` only as far as those two reference it, and
`TARGET_INFO_SESSION_START_TIME`), `VACUUM`ed from 208 KiB to 20 KiB. It answers the timeline
directly and carries no host path — the raw export's `StringIds` interns the launch's `PATH`, `HOME`,
hostname and working directory, and its `META_DATA_CAPTURE` repeats them, so both were dropped:

```bash
sqlite3 docs/evidence/T35/healing-ladder/capture.sqlite \
  "SELECT o.globalTid, s.value, o.start, o.end, o.end - o.start AS duration_ns
     FROM OSRT_API o JOIN StringIds s ON s.id = o.nameId
    ORDER BY o.globalTid, o.start;"
```

The raw `capture.nsys-rep` (**57 554 bytes**, sha256
`8ebee20ce01fc5e399a831a5194e27850431b295a69907f0ec81ac22064d8c38`) is **not committed**: it is the
whole report and it embeds this machine's `PATH`, `HOME` and hostname, which the convention's export
rule keeps out of the tree. It stays on the capturing machine and the command above regenerates it.
This is the deliberate substitution for the assignment's "commit the whole `.nsys-rep`" — the
convention's committed export is the trimmed SQLite, as it is for every `nsys` capture in this tree.

## What changed against the previous capture

First capture for this path: before it, T35's only evidence was the closed ticket. The naive
expectation the ticket's own `--capture-range cuda` invocation sets is that the path produces a
kernel manifest; measured, it produces **0 kernels** — no `CUPTI_ACTIVITY_KIND_KERNEL` table, mage's
`captured no CUDA kernel launches`, `status: "failed"`. Against that, this capture's numbers are
8 traced windows / **31 ladder decisions** / 7 833.082 µs of traced window / 0 CUDA launches. A later
capture of this path must keep the 31 decisions and their rungs and, if it adds decisions, say by
how many; the window durations are host-scheduling noise, not a budget.
