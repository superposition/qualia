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

`<slug>` is free and names the capture rather than the ticket: the T16 capture is
`T16/three-kernels/`, while #64's `T16/belief-couple` example is another legal slug for the same
step. #64's test line, `ls docs/evidence/T*/README.md`, does not match that layout; the satisfiable
form is

```bash
ls docs/evidence/T*/*/README.md
```

## What a capture directory holds

Every capture directory holds `capture.json`, `kernels.json`, `kernels.csv` and its own `README.md`,
plus the export of the backend that ran — never a `capture.sqlite` from a target that has no `nsys`:

| File | Contents |
| --- | --- |
| `capture.json` | mage's manifest: the profiled argv (`argv`), mage's profiler argv with the absolute output path elided (`profiler_argv`), the target's return code (`returncode`), and `status`/`error` when the run produced no kernels. |
| `kernels.json` | One object per CUDA launch, as mage read it out of the run's export. |
| `kernels.csv` | The same rows as CSV. |
| `capture.sqlite` | An `nsys` capture's SQLite export. |
| `capture.ncu-rep` + `metrics.csv` | An `ncu` capture's report and its raw CSV. |
| `README.md` | The host the capture ran on, the shape, the iteration count and the numbers observed. |

A run that launches no CUDA kernel produces no `kernels.json` and no `kernels.csv`. mage raises
`captured no CUDA kernel launches`, and the only manifest it writes is `capture.json`, with
`status: "failed"` and that error string; the raw backend export and `process.log` stay on the
capturing machine. T35 (#51) is that case — the healing ladder's decision path is a CPU timeline with
no kernel in it — so its directory commits `capture.json` and `README.md` alone, and the README
carries the timeline numbers. Do not manufacture a kernel file for it.

Otherwise the directory README names each kernel, the shape it ran with (grid, block, shared memory)
and how many launches the run produced, so a later ticket can be compared against it without opening
the export. It also states what changed against the previous capture — a number, not a narrative. A
ticket is not `status:done` until its capture is committed here, and the capture is linked from the
ticket's own comment, so the comment stream says which directory holds the evidence.

Commit `capture.json`, `kernels.json`, `kernels.csv` and the README always, plus the export row for
the backend that ran. Commit the export while it fits the size the tree already carries: the largest
committed file is `Cargo.lock` at about 196 KB (191 KiB). When a full export is larger, trim it to
what mage reads and say in the directory README that it is a trimmed export. For `nsys` that is the
`StringIds` and `CUPTI_ACTIVITY_KIND_KERNEL` tables with the `StringIds` rows the kernel table does
not reference dropped — the raw export carries the capturing host's `PATH`, `HOME` and distribution
name — `VACUUM`ed. For `ncu`, `metrics.csv` is the raw CSV mage parses and `capture.ncu-rep` is the
report; a report too large to commit is left on the capturing machine and named in the README. Elide
every absolute path `capture.json` embeds — the output path in `profiler_argv` and the report
directory in `error` — so the committed evidence names no machine.

## Running a capture

mage profiles a native executable, so build the ticket's binary first and hand `profile-exec` its
path. The profiling target is **Pinkie**, the Waveshare-carried Jetson Orin NX at
`jetson@192.168.55.1` ([`decisions.md`](../decisions.md) D-010, D-012): it carries `ncu` at
`/usr/local/cuda/bin/ncu` and no `nsys`, so a capture there uses `--backend ncu`. Its binary is the
ticket's aarch64 build — the host's cross-build through the cross image (`Cross.toml`,
`docker/Dockerfile.cross-aarch64`) or a native build on the board (D-010) — copied to the board,
which has no DNS.

```bash
# on Pinkie: mage and the aarch64 binary are copied over first (the board has no DNS, D-010)
mage profile-exec --backend ncu --capture-range all \
  --output-dir ~/mage-capture-scratch/T<NN>/<slug> \
  -- ./gpu-<hash> --test-threads=1
```

`mage profile-exec` writes its artifacts into a fresh `mage-ncu-<random>/` directory under
`--output-dir`; copy the files you want to commit up into `docs/evidence/T<NN>/<slug>/` and drop the
directory name, which changes on every run. Keep `--output-dir` outside the tree, as the baseline
did, so the random directory, `process.log` and the untrimmed export never enter the repository. The
test binary's name carries a build hash that changes with the dependency graph; recover it with
`find target/aarch64-unknown-linux-gnu/debug/deps -maxdepth 1 -type f -name 'gpu-*' -executable |
head -1` after the cross-build, or `target/debug/deps` after a native build on the board.

Capture range: the ticket text in #64 writes `--capture-range cuda`. That range records only the
window a target opens with `cudaProfilerStart`/`cudaProfilerStop`, and no binary in this repository
calls either, so a `cuda` capture records no kernels. What mage reports then is backend-specific, and
on `nsys` it is not the empty-capture message: `nsys` is invoked with
`--capture-range=cudaProfilerApi --capture-range-end=stop`, writes no export for the window, and
fails before parsing with

```text
Error during profiling: Nsight Systems SQLite export is missing:
<output-dir>/mage-nsys-<random>/capture.sqlite
```

`captured no CUDA kernel launches` is the *kernel-less run* message instead: it comes from the
empty-capture check, which fires only when a backend's export exists and parses to zero launches, and
it names the backend — a kernel-less `nsys` run prints `nsys captured no CUDA kernel launches. Check
the target and capture range. Reports: …`, and a kernel-less `ncu` run prints the same text with
`ncu` in front. `ncu` runs the `cuda` range as `--profile-from-start off`, so a run with no
`cudaProfilerStart` captures nothing there either. Until a runner grows those calls, captures use
`--capture-range all`.

## Manual capture

Capture by hand when the target's own driver, not this repository, makes mage unavailable: Pinkie
carries no `nsys` and no mage (`triton>=3.0` publishes no aarch64 wheel), so the board's captures are
produced manually. State that reason and the exact command used in the directory README, and keep the
manifest's field names — `argv`, `returncode`, `status`, `error`. Commit the raw backend export
(`capture.ncu-rep` and `metrics.csv` for `ncu`) when it fits the size the tree carries; when it cannot
be committed, the directory README says `kernels.json` and `kernels.csv` were hand-normalised from
that export and records the export's hash and size. The kernel-less rule and the `nsys` SQLite rule
above are unchanged.

## Host

Profiling happens on the target, and per D-012 that target is **Pinkie**; the evidence ships with the
ticket from there. A capture on the dev host is not the convention. The host's WSL2 Ubuntu-22.04
distribution is where mage installs on this workstation (`triton>=3.0` publishes no `win_amd64`
wheel, so `uv tool install git+https://github.com/superposition/mage` cannot resolve on the Windows
side) and it carries Nsight Systems and the 4090; `baseline-2026-09-11/` was taken there with
`--backend nsys` before D-012. It stays a diagnostic, not the evidence a capture ticket lands.

So the backend a target actually carries decides the export, and the file table above follows it:

| Backend | Where it resolves | The export it writes |
| --- | --- | --- |
| `ncu` | Pinkie, `/usr/local/cuda/bin/ncu`; the host's WSL2 carries it too | `capture.ncu-rep` and `metrics.csv`; no SQLite |
| `nsys` | the dev host's WSL2 distribution; Pinkie has none | `capture.sqlite` |

Record the backend in `capture.json` and the export in the directory, and never demand a
`capture.sqlite` from a target that has no `nsys`.

Host GPU work stays serial and bounded (D-011, D-013, D-014): one GPU job at a time, `cargo -j 2` and
one build at a time while the host logs its corrected machine checks, and the resource guard of
[`agents.md`](../agents.md) §Host safety. A capture counts as GPU work, so announce one before it
runs.

## Reading a capture

`kernels.json` and `kernels.csv` carry the parsed launch rows. For an `nsys` capture, the committed
export answers per-launch questions directly:

```bash
sqlite3 docs/evidence/baseline-2026-09-11/capture.sqlite \
  "SELECT s.value, (k.end - k.start) / 1000.0 AS duration_us,
          k.gridX, k.blockX, k.registersPerThread
     FROM CUPTI_ACTIVITY_KIND_KERNEL k
     JOIN StringIds s ON s.id = k.demangledName
    ORDER BY k.start;"
```

For an `ncu` capture, `metrics.csv` is the raw `--csv --page raw` output mage parses, and
`capture.ncu-rep` is the report `ncu --export` wrote.

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
