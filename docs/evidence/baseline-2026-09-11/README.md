# Baseline capture — 2026-09-11

The first mage capture of this repository's merged CUDA kernels, taken to prove the tooling and to
give the `needs:profile` tickets a capture to beat. It is the previous capture T16–T18, T30–T31 and
T35 have to improve on.

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

13 launches in total, over 5 distinct kernels. All 5 tests passed (`ok. 5 passed; 0 failed`,
2.12 s of test time without the profiler attached).

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
  --export=sqlite --output=<dir>/capture ./target/debug/deps/gpu-ababab3f01c5ede7 --test-threads=1
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

## Files

`capture.json` and `kernels.json` are verbatim from the run. `kernels.csv` is mage's CSV rendering of
the same rows. `capture.sqlite` is the nsys export trimmed to the two tables mage's nsys backend
reads — `StringIds` and `CUPTI_ACTIVITY_KIND_KERNEL` — and `VACUUM`ed, which takes it from 344064 to
57344 bytes, under `Cargo.lock`'s size. It still parses through `mage.profiler.backends.nsys` and
yields the 13 launches above. The untrimmed export, the `capture.nsys-rep` and the run's
`process.log` were left on the capturing machine; regenerate them by rerunning the commands above
with a writable `--output-dir`.
