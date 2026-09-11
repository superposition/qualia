# T16 capture — the three perception/action kernels

Capture of `crates/cuda/tests/gpu.rs` on the shared RTX 4090, for ticket
[#31](https://github.com/superposition/qualia/issues/31). It is the reference the
later `needs:profile` tickets in this phase (`T17` #32, `T18` #33) have to beat:
compare shape and launch count first, duration second, because the 4090 is shared
with other work and the durations move with it.

## Shape

One pass of the eight device tests in `crates/cuda/tests/gpu.rs`, one test thread.
Every device context is constructed once per test, so each kernel runs the number
of launches below; the smoke test's own loop accounts for the eight `add_one`
launches. The three kernels this ticket adds each run once:

| Kernel | Launches | Grid × Block | Shared (static + dynamic) | Duration |
| --- | --- | --- | --- | --- |
| `perception_voxel` | 1 | 48 × 256 | 0 B | 11.84 µs |
| `action_score` | 1 | 2 × 256 | 1024 B + 0 B | 10.59 µs |
| `belief_couple` | 1 | 1 × 4 | 0 B + 12 B | 2.50 µs |

The pre-existing kernels in the same run, for context: `cognition_update` 2 × 1641.7 µs
(1 × 1024), `belief_update` 1 × 1573.0 µs (1 × 1024), `costmap_stats` 1 × 1.44 µs
(16 × 256), `cognition_patch` 1 × 1.41 µs (1 × 1), `add_one` 8 × 1.02–1.47 µs (1 × 4).

16 launches over 8 kernels, 4892.89 µs of kernel time. The earlier baseline capture
([#166](https://github.com/superposition/qualia/pull/166), `docs/evidence/baseline-2026-09-11/`)
profiled the same file with five device tests: 13 launches over 5 kernels, 4784.75 µs.
This capture adds the three kernels and their device tests; the pre-existing rows are
within run-to-run noise of that baseline.

## Invocation

mage resolves `triton>=3.0`, which has no `win_amd64` wheel, so the capture ran in the
WSL2 `Ubuntu-22.04` distribution, which passes the 4090 through and carries Nsight
Systems. The WSL `stable` toolchain is older than `cudarc 0.19.9`'s `libloading 0.9`
needs, so the build pins 1.98.1. The test binary is a Linux ELF built in WSL, not the
Windows checkout's `target/`.

```bash
cd /mnt/c/Users/ericm/wt/T16
CARGO_TARGET_DIR=/home/superposition/t16-target cargo +1.98.1 test -p qualia-cuda --features cuda -j 4 --no-run
export LD_LIBRARY_PATH=/usr/local/cuda/lib64:/usr/lib/wsl/lib:$LD_LIBRARY_PATH
mage profile-exec --backend nsys --capture-range all \
  --output-dir /home/superposition/mage-t16 \
  -- /home/superposition/t16-target/debug/deps/gpu-ababab3f01c5ede7 --test-threads=1
```

`--capture-range all` and not `cuda`: no runner here calls `cudaProfilerStart`, so a
`cuda` range records nothing. mage's recorded profiler argv is in `capture.json`; the
run returned 0 and persisted as session 38 in `~/.mage/profiles.db`.

Host and tools: RTX 4090, driver 591.74, 24564 MiB; WSL2 `Ubuntu-22.04`; cargo/rustc
1.98.1; `mage` 0.1.0; `nsys` 2025.3.2.474.

## Files

`capture.json`, `kernels.json` and `kernels.csv` are mage's artifacts unchanged.
`capture.sqlite` is a **trimmed** export: the full Nsight Systems SQLite is 320 KiB,
over the ~196 KiB the tree already carries, so it holds only the two tables mage's
nsys backend reads (`StringIds`, `CUPTI_ACTIVITY_KIND_KERNEL`) and was `VACUUM`ed to
36 KiB. It still parses through `mage.profiler.backends.nsys` into the same 16
launches. To regenerate the full export, rerun the invocation above; it lands under a
fresh `mage-nsys-<random>/` directory, and the untrimmed `capture.sqlite` is next to it.

## What a later ticket must beat

The three new kernels have to keep their launch shape (one thread per slot, one thread
per voxel, one block per candidate) and their launch count of one each per test pass.
The numbers to beat on this host are the durations in the table above; the whole-file
total to beat is 4892.89 µs.
