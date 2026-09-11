# Evidence captures

Ticket [#64](https://github.com/superposition/qualia/issues/64) makes
[`superposition/mage`](https://github.com/superposition/mage) the profiler and the evidence channel
for this repository. Every functional ticket that changes a kernel or a model step carries the
`needs:profile` label and lands a capture of a real run under this directory.

## Layout

```text
docs/evidence/
  README.md                  this file
  baseline-2026-09-11/       the reference capture later tickets must beat
  T<NN>/<slug>/              one directory per needs:profile ticket
```

A capture directory holds:

| File | Contents |
| --- | --- |
| `capture.json` | The exact profiler argv mage ran, the profiled argv, and the target's return code. |
| `kernels.json` | One object per CUDA launch, as mage's nsys backend read it out of the export. |
| `kernels.csv` | The same rows as CSV. |
| `capture.sqlite` | The Nsight Systems SQLite export for the run. |
| `README.md` | The shape, the iteration count and the numbers observed. |

The directory README names each kernel, the shape it ran with (grid, block, shared memory) and how
many launches the run produced, so a later ticket can be compared against it without opening the
SQLite. It also states what changed against the previous capture — a number, not a narrative. A
ticket is not `status:done` until its capture is committed here, and the capture is linked from the
ticket's own comment, so the comment stream says which directory holds the evidence.

Commit `capture.json`, `kernels.json`, `kernels.csv` and the README always. Commit `capture.sqlite`
while it fits the size the tree already carries: the largest committed file is `Cargo.lock` at about
196 KiB. When the full export is larger, keep only the two tables mage's nsys backend reads
(`StringIds` and `CUPTI_ACTIVITY_KIND_KERNEL`), drop the `StringIds` rows the kernel table does not
reference — the raw export carries the capturing host's `PATH`, `HOME` and distribution name — and
`VACUUM` the result. Say in the directory README that the committed file is a trimmed export, and
elide any absolute path the profiler argv embeds, so the committed evidence names no machine.

## Running a capture

mage profiles a native executable, so build the test binary first and hand `profile-exec` its path:

```bash
cargo test -p qualia-cuda --features cuda --no-run -j 4
BIN=$(find target/debug/deps -maxdepth 1 -type f -name 'gpu-*' -executable | head -1)
mage profile-exec --backend nsys --capture-range all \
  --output-dir docs/evidence/T<NN>/<slug> \
  -- "$BIN" --test-threads=1
```

`mage profile-exec` writes its artifacts into a fresh `mage-nsys-<random>/` directory under
`--output-dir`; copy the files you want to commit up out of it and drop the directory name, which
changes on every run.

Capture range: the ticket text in #64 writes `--capture-range cuda`. That range records only the
window a target opens with `cudaProfilerStart`/`cudaProfilerStop`, and no binary in this repository
calls either, so a `cuda` capture on today's runners records no kernels and mage fails with
`captured no CUDA kernel launches`. Until a runner grows those calls, captures use
`--capture-range all`.

## Host

mage resolves `triton>=3.0`, which publishes no `win_amd64` wheels, so `uv tool install
git+https://github.com/superposition/mage` fails on the Windows side of this workstation and the
capture path runs in the WSL2 Ubuntu-22.04 distribution instead. That distribution already carries
Nsight Systems, the CUDA toolkit and the Rust toolchain, and `nvidia-smi` there reports the same
`NVIDIA GeForce RTX 4090`. Build with a Rust toolchain of 1.88 or later: the cached WSL `stable`
is 1.85.0 and `cudarc 0.19.9` pulls `libloading 0.9`, which refuses to build on it. Point
`LD_LIBRARY_PATH` at `/usr/local/cuda/lib64` and `/usr/lib/wsl/lib` so `libnvrtc.so.12` and
`libcuda.so.1` resolve.

The 4090 is shared with other work on this workstation, so captures run one at a time and the
durations a capture records move with whatever else is running. Compare captures on shape and
launch count first and on duration second.

## Reading the SQLite history

The export next to a capture answers per-launch questions directly:

```bash
sqlite3 docs/evidence/baseline-2026-09-11/capture.sqlite \
  "SELECT s.value, (k.end - k.start) / 1000.0 AS duration_us,
          k.gridX, k.blockX, k.registersPerThread
     FROM CUPTI_ACTIVITY_KIND_KERNEL k
     JOIN StringIds s ON s.id = k.demangledName
    ORDER BY k.start;"
```

Unless a capture passes `--no-persist`, mage also appends the same run to its own history database,
`~/.mage/profiles.db`, as a session plus one `metrics` row per launch:

```bash
sqlite3 ~/.mage/profiles.db \
  "SELECT id, backend, command, start_time FROM sessions ORDER BY id DESC LIMIT 5;"
sqlite3 ~/.mage/profiles.db \
  "SELECT kernel_name, duration_us, grid_size, block_size
     FROM metrics WHERE session_id = <id> ORDER BY id;"
```

The history database is local to the machine that ran the capture; the committed export is the part
a later agent or reviewer can open.
