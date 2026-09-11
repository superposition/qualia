# Baseline capture — 2026-09-11

The first mage capture of this repository's merged CUDA kernels, taken to prove the tooling and to
give the `needs:profile` tickets a capture to beat. It is the previous capture T16–T18, T30–T31, T35
and T50 have to improve on.

It is an `nsys` capture from the dev host's WSL2 distribution, taken on 2026-09-11 before
[`decisions.md`](../../decisions.md) D-012 made **Pinkie** the profiling target. It is kept as the
reference these numbers are measured against; a new capture follows [`../README.md`](../README.md),
names the host it ran on, and is taken on Pinkie with `--backend ncu` (only the host has `nsys`).

## What ran

`qualia-cuda`'s device suite, `crates/cuda/tests/gpu.rs`, at commit
`76143f0536ff096cbe614404a2294c07e1c5f768` (`origin/main` when the capture was taken, merge of #146).
The suite ran once, single-threaded, so every launch below comes from a single pass of the tests:

| Test | Launches it contributes |
| --- | --- |
| `smoke_context_runs_a_kernel` | 8 × `add_one`, one per loop iteration |
| `costmap_stats_context_matches_the_host_reference` | 1 × `costmap_stats` |
| `belief_update_matches_the_host_reference` | 1 × `belief_update` |
| `cognition_dispatch_matches_the_host_reference` | 2 × `cognition_update`, one per tick |
| `cognition_weight_edits_are_observable` | 1 × `cognition_patch` |

13 launches in total, over 5 distinct kernels. All 5 tests passed (`ok. 5 passed; 0 failed`),
single-threaded and with no profiler attached. The suite NVRTC-compiles its kernels at run time
(`compile_for` in `crates/cuda/src/cuda_impl.rs`), so a bare run's reported test time depends on the
compiler warm-up and is not a fixed number; the pass count and the launch counts below are the
stable part.

Command sequence, WSL2 Ubuntu-22.04, `cargo 1.98.1`, `nsys 2025.3.2.474`:

```bash
cargo +1.98.1 test -p qualia-cuda --features cuda --no-run -j 4
export LD_LIBRARY_PATH=/usr/local/cuda/lib64:/usr/lib/wsl/lib:$LD_LIBRARY_PATH
mage profile-exec --backend nsys --capture-range all \
  --output-dir ~/mage-capture-scratch \
  -- ./target/debug/deps/gpu-ababab3f01c5ede7 --test-threads=1
```

The test-binary name carries a build hash and changes with the dependency graph; recover the path
with `find target/debug/deps -maxdepth 1 -type f -name 'gpu-*' -executable | head -1`.

The profiler argv mage built is recorded in `capture.json`:

```text
/usr/local/bin/nsys profile --trace=cuda,nvtx --sample=none --cpuctxsw=none --stats=false \
  --export=sqlite --output=<output-dir>/mage-nsys-<run>/capture ./target/debug/deps/gpu-ababab3f01c5ede7 --test-threads=1
```

## Shapes and numbers

`STATE_DIM` is 1024 and `WEIGHT_COUNT` is 1024 × 1024 = 1 048 576 (`crates/types/src/lib.rs`), which
is why the belief and cognition kernels take a 1 × 1024 block with 8 KiB of static shared memory.

| Kernel | Launches | Grid | Block | Regs | Shared | Duration (µs) | Total (µs) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `cognition_update` | 2 | 1×1×1 | 1024×1×1 | 48 | 8192 B | 1612.697, 1612.822 | 3225.52 |
| `belief_update` | 1 | 1×1×1 | 1024×1×1 | 44 | 8192 B | 1547.834 | 1547.83 |
| `add_one` | 8 | 1×1×1 | 4×1×1 | 16 | 0 B | 1.008 – 1.480 | 8.60 |
| `costmap_stats` | 1 | 16×1×1 | 256×1×1 | 16 | 0 B | 1.417 | 1.42 |
| `cognition_patch` | 1 | 1×1×1 | 1×1×1 | 16 | 0 B | 1.385 | 1.39 |

13 launches, 4784.75 µs of GPU kernel time. mage reported 5 unique kernels, mean 368.06 µs, min 1.01
µs, max 1612.82 µs, and persisted the session to `~/.mage/profiles.db` on the capturing machine. The
4090 is shared with other work on this workstation, so treat the durations as one contended sample
and the launch count and shapes as the stable part.

## What changed against the previous capture

None — this is the first capture, so it is the previous capture every later one is measured against.
The numbers to beat: 13 CUDA launches over 5 kernels and 4784.753 µs of kernel time, split 3225.519 µs
`cognition_update`, 1547.834 µs `belief_update`, 8.598 µs `add_one`, 1.417 µs `costmap_stats`,
1.385 µs `cognition_patch`; mean 368.058 µs, min 1.008 µs, max 1612.822 µs.

## Files

`kernels.json` is mage's own summary of the run, and `kernels.csv` is its CSV rendering of the same 13
rows. `capture.json` is mage's manifest with the profiler's `--output` path elided to
`<output-dir>/mage-nsys-<run>/capture`, so the committed evidence names no machine; the unedited
manifest is reproducible from the commands above. `capture.sqlite` is the nsys export reduced to the
two tables mage's nsys backend reads — `StringIds` and `CUPTI_ACTIVITY_KIND_KERNEL` — with the 128
`StringIds` rows the kernel table does not reference removed. Those rows are nsys internals and the
run's captured environment (`PATH`, `HOME`, the WSL distribution name, unrelated tool config), which
is why they are dropped rather than committed; the five that remain are the five kernel names. The
file goes 344064 → 57344 → 12288 bytes, under `Cargo.lock`'s size, has `pragma integrity_check` `ok`,
and still parses through `mage.profiler.backends.nsys` into the same 13 metrics and 4784.753 µs. The
untrimmed export, the `capture.nsys-rep` and the run's `process.log` were left on the capturing
machine; regenerate them by rerunning the commands above with a writable `--output-dir`.
