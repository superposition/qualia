# T17 capture — three-kernel CPU/CUDA parity and the board's kernel time

The board capture for ticket #32: `crates/cuda/tests/parity.rs`'s single test re-established on the
Waveshare-carried NVIDIA Jetson Orin NX, plus the `ncu` measurement of the same dispatches.

* **Head:** `b097ecead4e2ed0216c65c0cce6bd5dfc18ba003` (`ticket/T17`, PR #208; ticket #32).
* **Board:** `aarch64`, driver `NVRM 540.4.0`, CUDA `12.9`, `ncu 2025.2.0.0 (build 35613519)`,
  compute capability **8.7**, `LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat`.
* **Kernel path:** this head carries no fatbin (T18's `build.rs` is a different branch), so the kernels
  compile under **NVRTC at process start** and the compat `libcuda` step is required — CUDA 12.9 PTX
  against the board's 540.4 driver.
* **Shape:** one release parity run, then one `ncu` pass over it —
  `--section LaunchStats --section Occupancy --launch-count 24
  --kernel-name 'regex:belief_couple|perception_voxel|action_score'` with the seven-metric list the
  ticket names, `--csv`. **10 kernel launches**: `belief_couple` 3, `perception_voxel` 3,
  `action_score` 4 — exactly the ticket's count, because the empty graph, empty slot list, empty sweep
  and no-candidate dispatch never launch.

## How it was taken

```sh
cargo test -p qualia-cuda --features cuda --test parity --no-run --release -j3 --offline
export LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat
BIN=$(find target/release/deps -maxdepth 1 -type f -name 'parity-*' -executable | head -1)
"$BIN" --test-threads=1 --nocapture                      # the parity claim, below
echo jetson | sudo -S env LD_LIBRARY_PATH=/usr/local/cuda-12.9/compat /usr/local/cuda/bin/ncu \
  --section LaunchStats --section Occupancy --launch-count 24 \
  --kernel-name 'regex:belief_couple|perception_voxel|action_score' \
  --metrics gpu__time_duration.sum,sm__cycles_elapsed.avg,launch__registers_per_thread,\
launch__shared_mem_per_block_static,launch__shared_mem_per_block_dynamic,launch__grid_size,\
launch__block_size --csv "$BIN" --test-threads=1 --nocapture > kernels-base.csv
```

`ncu` needs root on this board, hence `sudo -S` (the T50 route). `kernels-base.csv` is that command's
stdout, verbatim, with the harness's own lines interleaved where `--nocapture` wrote them.

## The parity claim on the board

```
running 1 test
test parity_within_tolerance ... belief_couple/fixed/belief: 4 values, worst relative error 0e0 at [0]
… (belief_couple and perception_voxel: 0e0 in every case) …
action_score/fixed/terminal-goal-distance: 2 values, worst relative error 8.5214936e-8 at [0]
action_score/maximum-size/terminal-goal-distance: 64 values, worst relative error 8.142438e-8 at [11]
action_score/deterministic-random/terminal-goal-distance: 5 values, worst relative error 7.983257e-8 at [4]
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.91s
```

The worst relative error anywhere is **8.52e-8** (the `action_score` terminal-goal-distance case);
every `belief_couple` and `perception_voxel` value is bit-identical (0e0). That is the ticket's device
evidence: `parity.rs` prints `skipping CUDA parity test` and *passes* on a host with no CUDA adapter, so
the claim rests on a device run — here the three kernels executed on the board's compute-capability 8.7
device with the crate's own CPU oracle (`qualia_cuda::cpu`) alongside, and every compared value sits
inside the ticket's `1e-4` bound. The harness transcript (`parity.log`, path-free) stays on the
capturing machine, its hash recorded below.

## The launches

| id | kernel | block | grid | duration | registers/thread | shared mem (dynamic) |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | `belief_couple` | (7, 1, 1) | (1, 1, 1) | 30336 ns | 26 | 12 B dyn |
| 1 | `belief_couple` | (1024, 1, 1) | (1, 1, 1) | 902528 ns | 26 | 8192 B dyn |
| 2 | `belief_couple` | (98, 1, 1) | (1, 1, 1) | 57152 ns | 26 | 148 B dyn |
| 3 | `perception_voxel` | (256, 1, 1) | (48, 1, 1) | 9728 ns | 21 | 0 B dyn |
| 4 | `perception_voxel` | (256, 1, 1) | (48, 1, 1) | 1046368 ns | 21 | 0 B dyn |
| 5 | `perception_voxel` | (256, 1, 1) | (48, 1, 1) | 280256 ns | 21 | 0 B dyn |
| 6 | `action_score` | (256, 1, 1) | (2, 1, 1) | 55392 ns | 30 | 0 B dyn |
| 7 | `action_score` | (256, 1, 1) | (1, 1, 1) | 6496 ns | 30 | 0 B dyn |
| 8 | `action_score` | (256, 1, 1) | (64, 1, 1) | 1222720 ns | 30 | 0 B dyn |
| 9 | `action_score` | (256, 1, 1) | (5, 1, 1) | 126080 ns | 30 | 0 B dyn |

Per kernel, against T16's 4090 capture (`belief_couple` 2.50 µs, `perception_voxel` 11.84 µs,
`action_score` 10.59 µs) — the parity test dispatches more shapes than T16's single pass, so the
comparison is per-launch shape, not per-kernel total:

| kernel | launches | board durations (µs, ncu replay) | T16 4090 (µs) |
| --- | --- | --- | --- |
| `belief_couple` | 3 | 30.336, 902.528, 57.152 | 2.50 |
| `perception_voxel` | 3 | 9.728, 1046.368, 280.256 | 11.84 |
| `action_score` | 4 | 55.392, 6.496, 1222.720, 126.080 | 10.59 |

The `1×1024` `belief_couple` launch (8192 B dynamic shared) and the `64×256` `action_score` launch are
the two long ones; every other launch is the same shape inventory the ticket lists.

## Files

`kernels-base.csv` is the profiled command's stdout; `metrics.csv` is the **details-page** `--csv`
export of the same configuration from a second `--export` pass (`capture.ncu-rep`), per D-017.
`kernels.json`/`kernels.csv` are a **hand-normalised projection** of `kernels-base.csv`: one object per
launch under a `run` column, the raw metric identifiers where the export carries them
(`gpu__time_duration.sum`, `sm__cycles_elapsed.avg`, `launch__*`) and snake-cased section names
otherwise, thousands separators removed. No value is invented and no data file carries a filesystem path.

### Committed

The four data files are byte-identical to the board's staged capture, whose hashes PR #208's board
comment records; the fifth row is the manifest the board staged, with two fields conformed below.

| file | bytes | sha256 |
| --- | --- | --- |
| `kernels.csv` | 5,114 | `d94226864b0a4020aa3a25e59d92ac2e89456ba5c9abc8cf54c395820beef667` |
| `kernels.json` | 28,905 | `896d128258f78653c9e60fd9c9ccb8ad9da50b6042d0d7a244237360aa2c6ca8` |
| `metrics.csv` | 65,371 | `0e55604945915e38b3b13af95781b0fd007a332270175f70cea058eda9b2ec78` |
| `capture.ncu-rep` | 358,828 | `dbef27fcbfb4568db7acf0bd8fcc033e7300c022af6c15df41415487b239c60e` |
| `capture.json` | 4,997 | `c9b696446ce2776a73eaff1782a367e4320489d6fec25d2348fade8da8522305` |

`metrics.csv` carries `launch__shared_mem_per_block_static` (this capture's `--metrics` list names it)
with unit `byte/block`: mage's own reader accepts only byte/kb/mb/gb for a `_bytes` field, so it rejects
that unit and cannot ingest this committed CSV as-is (D-017). `capture.ncu-rep` cannot be trimmed and is
committed whole (D-017); like the T18 and T50 reports it embeds the board's scratch paths.

### Two passes, one launch inventory

The ticket's own command (the `--metrics` list above, `--csv`) wrote `kernels-base.csv`; the export pair
is a second pass of the same configuration (`--export capture`, then
`ncu --import capture.ncu-rep --page details --csv`). The inventory is launch-for-launch identical — ten
IDs, the same kernel, block, grid, register count and shared-memory shape in both — and
`sm__cycles_elapsed.avg` agrees within 8 % throughout, but `gpu__time_duration.sum` is a wall clock the
replay's clock state moves: eight launches agree within 5 %, while the two `belief_couple` launches
differ (ID 1 `902.528` µs vs `449.09` µs, ID 2 `57.152` µs vs `108` µs) at `276,128`/`280,502` and
`35,646`/`33,005` cycles. The launch table above quotes the pass the ticket's own command produced, which
`capture.json`'s `launches` carries; opening the committed `metrics.csv` shows the export pass's clock on
those two rows.

### Stays on the capturing machine

Per `docs/evidence/README.md`, the profiler's process log and the raw output of the profiling command
are not committed; `capture.json`'s `files` map records each hash so the board's staged copy can be
checked.

| artifact | bytes | sha256 | why it is not committed |
| --- | --- | --- | --- |
| `kernels-base.csv` | 67,511 | `8918773f26a6c8f7dcc04d6ee6cf87f02e3e3ed0e89e6ab64e4a8f7df2515888` | the profiling command's stdout verbatim; its `==PROF== Connected to process …` line names the board's `/home/jetson/…` scratch path |
| `parity.log` | 1,966 | `40a8443719775389eadea1804bd790e855d6a121d6369ee6bb51d1dd340f7a74` | the parity harness's transcript; path-free, but not one of the convention's files — its numbers are the parity section above and `capture.json`'s `parity` block |
| `ncu-export-run.log` | 2,816 | `74055fd10d3af30f69bceeffb5be6030053ae9b1b0a1f56b461e1c73ac56df13` | the profiler's process log; embeds the board's `/home/jetson/…` paths |

`capture.json` (mage's field names: `argv`, `profiler_argv`, `returncode`, `status`, `kernel_count`)
adds the board, build, parity and per-kernel launch facts. It is the board's staged manifest with two
fields conformed to this directory's convention: `backend: "ncu"` added — mage's writer always emits it
and `docs/evidence/README.md` requires the backend recorded — and `status` set to mage's `complete`
(`ok` in the staged manifest, the value this tree's other three manifests carry). Its hash therefore
differs from the staged `52631e3d770473ac1cabe6bea96fee1ef6b2ebe4de5bff1ccb9870c43379d6d0`.
