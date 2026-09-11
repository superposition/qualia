# T18 capture — the real `sm_87` fatbins on Pinkie (the Orin NX board)

This is the board capture the ticket's handover asks for: the eight CUDA kernels built by `nvcc` into
**real `sm_87` cubins** (the fix for the board's `CUDA_ERROR_UNSUPPORTED_PTX_VERSION`, the C36 caveat),
profiled with `ncu` on the Waveshare-carried NVIDIA Jetson Orin NX Engineering Reference Developer Kit.

* **Head:** `456fdfb4d00db17c5675aca56a7c6472f32bb598` (`ticket/T18`, PR #207; ticket #33).
* **Board:** `aarch64`, kernel `5.15.148-tegra`, driver `NVRM 540.4.0`, CUDA `12.9` (`V12.9.41`),
  `ncu 2025.2.0.0 (build 35613519)`, compute capability **8.7**, `LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat`.
* **Shape:** one `ncu` pass over the release GPU suite (`gpu-e5293633ad869b15 --test-threads=1`),
  `--target-processes all --launch-count 8`, no `--clock-control` override — the profiler's default clock and
  cache control, as the T50 capture used. **8 launches over 7 kernels**, `8 passed; 0 failed` in 20.17 s;
  the harness keeps running after the eighth launch, so `smoke`'s kernel is not in the capture.

## How it was taken (manual capture; mage is not on the board)

```sh
CUDAARCHS=87-real NVCC=/usr/local/cuda/bin/nvcc \
  cargo test -p qualia-cuda --features cuda --release --offline --no-run
cuobjdump --list-elf target/release/build/qualia-cuda-*/out/belief_update.fatbin   # sm_87 only
BIN=target/release/deps/gpu-e5293633ad869b15
# the board's password goes to `sudo -S` on stdin; PR #207's board comment quotes that full line
sudo -S env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat /usr/local/cuda/bin/ncu \
  --target-processes all --launch-count 8 --export ~/t18-capture/capture --force-overwrite \
  "$BIN" --test-threads=1
ncu --import ~/t18-capture/capture.ncu-rep --page details --csv > ~/t18-capture/metrics.csv
```

`ncu` needs root on this board (`sudo -n` has no NOPASSWD, `perf_event_paranoid=2`), which is why the
command runs under `sudo -S` with the board's password on stdin and the profiler argv carries `sudo`;
the T50 capture used the same route. The eight fatbins embed **only**
`sm_87` cubins — `action_score` 14 472 B, `belief_couple` 7 792 B, `belief_update` 43 888 B,
`cognition_update` 45 744 B, `costmap_stats` 3 832 B, `evidence_layout` 3 064 B, `perception_voxel`
5 624 B, `smoke` 3 064 B — and the device loaded them: the run reached every kernel with no
`CUDA_ERROR_UNSUPPORTED_PTX_VERSION`.

## The launches

| id | kernel | block | grid | duration |
| --- | --- | --- | --- | --- |
| 0 | `action_score` | (256, 1, 1) | (2, 1, 1) | 97.12 us |
| 1 | `belief_couple` | (4, 1, 1) | (1, 1, 1) | 23.33 us |
| 2 | `belief_update` | (1024, 1, 1) | (1, 1, 1) | 13.00 ms |
| 3 | `cognition_update` | (1024, 1, 1) | (1, 1, 1) | 12.96 ms |
| 4 | `cognition_update` | (1024, 1, 1) | (1, 1, 1) | 12.94 ms |
| 5 | `cognition_patch` | (1, 1, 1) | (1, 1, 1) | 16.26 us |
| 6 | `costmap_stats` | (256, 1, 1) | (16, 1, 1) | 20.83 us |
| 7 | `perception_voxel` | (256, 1, 1) | (48, 1, 1) | 261.63 us |

Two shapes dominate: `belief_update` at 13.00 ms and `cognition_update` at 12.96 + 12.94 ms are
98.9 % of the 39.32 ms of kernel time across the eight launches,
both a single 1024-thread block (the T50 finding, unchanged by this ticket, which is about the *fatbin*,
not the kernel shape).

Against the previous board capture of the same suite (`T50/pinkie-kernels`, same 1024-thread blocks,
1 ms resolution): `belief_update` 12.98 → 13.00 ms, `cognition_update` 13.05 + 12.99 → 12.96 + 12.94 ms —
inside the run-to-run spread, as expected: this ticket changes how the kernel is loaded, not what it
computes.

## Files

`metrics.csv` is the **details-page** `--csv` export per D-017 (mage's parser accepts the long form it
writes); `capture.ncu-rep` is the report `ncu --export` wrote. The profiler's own stdout,
`ncu-run.log` (1,411 B, sha256 `c4de6ba17fb9113052052d6ebf748b62589c3f76ff8a7265edbdef213bbf22f7`),
holds the `8 passed; 0 failed` line but embeds the board's absolute scratch paths, so it stays on the
capturing machine and is not committed — `capture.json`'s `files` map records its hash and its
`harness` block records the test line. `kernels.json`/`kernels.csv` are a **hand-normalised projection**
of the details page: one object per launch under a `run` column, the launch shape plus every counter the
board's driver returned, with the sections' human names snake-cased and thousands separators removed. No
value is invented — `Memory Throughput` is `nan` in the export itself — and neither file contains a
filesystem path.

| file | bytes | sha256 |
| --- | --- | --- |
| `kernels.csv` | 2,461 | `97d66620bbac6cf1c26748960770a313c612df1ec9fb09a0ffdd5b6a445c11cb` |
| `kernels.json` | 12,709 | `07baa0e7bf6ab51cff91eb647074781703926973c019cd39e6eb1463bb26f8c4` |
| `metrics.csv` | 76,589 | `10d64d2adf9ea3a5a390b875b6cc269933369b49e8bd53f0c816610fe0f8c063` |
| `capture.ncu-rep` | 598,551 | `459226bde65caad1f29893ebb6578f95ebf3f4008acc490778a0ece9bfd592bd` |
| `capture.json` | 3,495 | `53bf79414865bb997983691b90cec8a242731277cc3719d5ff853772d9bd6daf` |

`capture.json` keeps mage's manifest field names (`argv`, `profiler_argv`, `returncode`, `status`,
`kernel_count`) with the board, build and environment facts alongside, as `docs/evidence/README.md`'s
manual-capture clause allows.

The four data files above are byte-identical to the board's staged capture, whose hashes PR #207's board
comment records. `capture.json` is that staged manifest with two fields conformed to this directory's
convention: `backend: "ncu"` added — mage's writer always emits it (`src/mage/profiler/backends/native.py`)
and `docs/evidence/README.md` requires the backend recorded in `capture.json` — and `status` set to mage's
`complete` (`ok` in the staged manifest), the value this tree's other three manifests carry. Its hash
therefore differs from the staged `153ef6104bb2b804bb649297a3f6e832e54a064e9fecb5cd99758cfe6dca6dfa`.
