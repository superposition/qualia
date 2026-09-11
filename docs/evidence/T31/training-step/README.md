# T31 capture — the training step, `qualia-jepa-train` through the braid's manifest

The `needs:profile` evidence for ticket
[#47](https://github.com/superposition/qualia/issues/47) (step 31 — the braid closes the
training/promotion loop) and for ticket
[#64](https://github.com/superposition/qualia/issues/64) (step 47 — the mandatory `T30`–`T31` captures).

Step 31 wires two existing binaries into the improvement loop: on `EvidenceSealed` for a closed mission
the agent builds a `DatasetManifest` with `qualia-jepa-dataset`, submits `qualia-jepa-train` through the
single-flight queue under a **120 s** deadline, and hands the trainer's report to
`qualia-jepa-registry`. This capture is those two binaries, driven directly with the argv the loop
itself builds:

```rust
// crates/braid/src/improvement.rs
pub const TRAINING_JOB_DEADLINE_MS: u64 = 120_000;
pub const CHECKPOINT_ID_PREFIX: &str = "cnn-";
pub const CHECKPOINT_ID_DIGEST_CHARS: usize = 12;
// TrainingJob::dataset_argv  -> qualia-jepa-dataset --catalog <catalog> --output-dir <dataset_dir>
// TrainingJob::training_argv -> qualia-jepa-train --manifest <printed path> --checkpoint-id cnn-<hex>
//                               --output-dir <checkpoint_dir> --backend <cpu|metal|cuda>
```

**Which path, and why not the agent's own.** The loop's entry point is `EvidenceSealed` on a *closed
mission*; no mission exists in this repository or in the fixture, so the step cannot be driven through
the braid itself. What is captured instead is the argv the loop would run — the dataset binary followed
by the trainer, one job at a time, under the argv builder above — against a catalog of sealed sessions
that is a **synthesized fixture** at the smallest scale the dataset promotion gate admits. The trainer's
own interval count is bounded to one epoch (`--epochs 1`); the loop's argv carries no `--epochs`, so its
default schedule is 20 (see §"The bound the step runs under").

## Host and tools

WSL2 `Ubuntu-22.04` on the development workstation (32 logical CPUs), `nsys`
2025.3.2.474-253236389321v0, `mage` 0.1.0 installed with `uv tool install` from
`superposition/mage` at `7a798f6` (the clone T35 recorded), `rustc` 1.98.1, `candle-core`
0.9.1 with `default-features = false`. The capture is **CPU**: the trainer ran `--backend cpu`, no CUDA
device was opened, and the retained export carries no `CUPTI_ACTIVITY_KIND_KERNEL` table at all. The
host's 4090 was not touched (D-011 holds one GPU job at a time, and another agent held it).

The binaries:

| binary | sha256 |
| --- | --- |
| `qualia-jepa-train` | `aa4d8becbae0fdd70cc481b1176fa894ef9a1bb5a7fe5fbafccbfcc05cdaf569` |
| `qualia-jepa-dataset` | `0c744e6c936d9003f328c4c211d544c8dfee6ed2dd9b5f334738c3c11c1d8c75` |

**Why not Pinkie.** The step is a CPU computation with no kernel in it — the kernel-less case
`docs/evidence/README.md` names, and `T35/healing-ladder` is its precedent — so the board's `ncu`
backend has no launch to record; the board's offline registry cache also carries no `candle-core` and
it has no DNS to fetch one (T50's board report records that prerequisite). The capture therefore ran on
the dev host, exactly as T35's kernel-less capture did, and no board work was attempted.

## Shape

The catalog is 12 sealed MCAP sessions (`session-0` … `session-11`, environments `room-00` …
`room-11`, conditions `day`/`dim`/`motion` cycling), 4168 frames each, written by a throwaway fixture
writer that uses the recipe the crate's own `sealed_session` unit test uses
(`crates/jepa-dataset/src/tests.rs`). It is the gate minimum: **50 004 valid transitions**, 12 sessions,
12 environments, 3 conditions, and the environment split puts **41 670 train / 4 167 validation /
4 167 test**. Both counts are the dataset gate's floor (50 000 transitions, 12 sessions, 3 conditions,
3 environments, ≥ 4096 per split), so nothing smaller can train.

The dataset binary wrote the manifest and cleared its own gate:

```text
$ qualia-jepa-dataset --catalog ~/t47t31-capture/fixture/data/catalog.json \
    --output-dir ~/t47t31-capture/datasets
~/t47t31-capture/datasets/jepa-dataset-b9992b243b197933719ad77beb8e7e459bb38f1e68d7a90c43a996bc7769ff52.json
valid=50004 candidates=50004 sessions=12 environments=12 conditions=3          # exit 0
```

Manifest digest `b9992b243b197933719ad77beb8e7e459bb38f1e68d7a90c43a996bc7769ff52`, 101 555 193 bytes;
the checkpoint id `cnn-b9992b243b19` is the loop's own construction — the `cnn-` prefix plus the
digest's first 12 hex characters.

**Iteration count: one epoch = 1303 batches of 32.** Every leg's report agrees on
`epochs: 1, batch_size: 32, cnn_steps: 1303, flat_steps: 1303, skipped_singletons: 0, seed: 42,
backend: "cpu"`. `cnn_steps` and `flat_steps` are the two trainers the step runs per batch — the CNN
candidate and the flat-MLP baseline whose held-out numbers the gates compare against.

## The numbers

Three legs, same argv, same manifest: the convention's mage leg, one untraced leg, and the bounded
timeline window.

| quantity | plain (untraced) | under `nsys` (mage leg) | T50's measured baseline |
| --- | --- | --- | --- |
| one epoch | **367.197 s** | **375.136 s** | 386.764 s |
| per batch (epoch ÷ 1303) | **281.81 ms** | **287.90 ms** | 296.8 ms |
| training samples/s | **113.48** | **111.08** | 107.74 |
| whole process | **465.983 s** | 484 s (mage command) | 506.2 s |

All three legs end the same way: the report is written, the checkpoint is published, the gates fail on
the synthetic fixture (`all_gates_passed: false`) and the binary exits 1 with
`candidate failed held-out predictive, grounding, or calibration gates`. That is the expected outcome
for a fixture — the dataset gate is cleared, the *training* gates are not, and the step's consequence
("the fly can never promote itself") is that the registry sees a candidate that did not pass. The three
report digests are `d750e0c7…` (plain), `fa5722ef…` (mage leg) and `13916c67…` (window leg); they differ
because `created_at_ms` and the published paths differ, while every measured field agrees.

**The bound the step runs under.** The loop submits each binary with a 120 s deadline
(`TRAINING_JOB_DEADLINE_MS`) and kills it when the deadline passes
(`runners/agent/src/improvement.rs`, `run_bounded`). Measured here, the dataset binary clears its
deadline easily (4.7 s) and the trainer's **one epoch is 367.197 s — 3.06× the 120 s bound**, so on this
fixture and backend the job is killed inside its epoch and no report reaches the registry. The loop's
argv passes no `--epochs`, so the schedule the loop actually asks for is the default 20 epochs:
`20 × 367.197 s = 7 343.9 s ≈ 2.04 h`, i.e. 61.2× the bound. That is arithmetic on the measured epoch,
not a measurement of a 20-epoch run — the bound makes the 20-epoch run unobservable through the loop.

## What changed against the previous capture

The training step's cost against T50's measured baseline — the `--epochs 1` run of this same trainer on
a gate-minimum fixture of the same shape (`T50 JEPA profiling: measure the naive paths…`, report row D,
on #172: 506.2 s wall, 386.764 s epoch, 296.8 ms/batch, 107.74 samples/s):

| quantity | T50 baseline | here, plain | here, under `nsys` | Δ plain | Δ profiled |
| --- | --- | --- | --- | --- | --- |
| epoch | 386.764 s | **367.197 s** | **375.136 s** | **−5.1 %** | **−3.0 %** |
| per batch | 296.8 ms | **281.81 ms** | **287.90 ms** | **−5.1 %** | **−3.0 %** |
| samples/s | 107.74 | **113.48** | **111.08** | **+5.3 %** | **+3.1 %** |

**No code on this path changed between the two heads.** `git log --oneline -- crates/jepa-model/src/train.rs`
reports one commit (`00f865f`) for the whole crate, and the trainer's epoch loop, batch size and
binaries are the same; T50's own proposed training-loop refactor is a separate, unlanded branch. The
−5.1 % is therefore host-to-host and run-to-run movement on the same work, not an optimisation, and a
later capture of this step must beat **367.197 s plain / 375.136 s under `nsys`** (and keep the
1303-batch count) to claim otherwise. The neighbouring set, `T30/belief-coupling`, profiles a different
path on the board — the coupling update's kernel — and shares no number with this one.

**Which baseline cell to compare.** T50's row-D evidence names `fixture/data/train1` and `bench` and no
`nsys/train` directory, so the baseline reads as an **untraced** dev-host run; on that reading the
like-for-like cell is **Δ plain (−5.1 %)** and the −3.0 % carries `nsys` overhead on one side only.
The baseline's fixture is a separate instance of the same recipe (12 sessions × 4168 frames), not this
capture's catalog, and its wall time is 506.2 s in its own summary table against 506.4 s in its command
list — T50's discrepancy, not this capture's to fix. Per-batch cells are `epoch ÷ 1303` from the epochs
above: 367.197 / 1303 = 281.810 ms and 375.136 / 1303 = 287.901 ms, rounded to 281.81 / 287.90.

## The CPU timeline (bounded window)

The training loop makes no CUDA call, so the timeline is an `osrt` trace. Collection is a **window of
6 s, 14 s after the launch** (`--delay=14 --duration=6`); 7 of the trainer's 24 materialization lines
fall inside it as `write` events (the other 17 are before the window), and the **last of them completes
at 3 107 538 875 ns** on the trace clock — the epoch's start; that write begins at 3 107 537 733 ns,
the highest `start` in the committed export (14 s + 3.108 s = 17.108 s into the process; the untraced
leg materialized everything by 16.242 s).

| window (100 ms, trace clock) | events |
| --- | --- |
| 0.0–3.0 (buckets 0–29) | the materialization tail: **5 `write`** events and **14** file-I/O calls (`close` 3, `open64` 4, `read` 4, `statx` 3) |
| **3.1** (bucket 31) | the boundary's last **2 `write`** events and then the epoch's own first 100 ms: **32 `pthread_create`**, 32 `mmap64`, 32 `mprotect`, **1 286 `open64`**, 191 `statx`, 8 `read`, 3 `close`, 6 554 `futex` |
| 3.2–6.0 (buckets 32–59) | **`futex` only**, 8 399–20 112 calls per 100 ms |

The rows are the bands, not the phases: the phase totals are in `osrt-calls.csv`, where the 7 `write`
events span 0.376–3.108 s, so the 2 in bucket 31 are counted there too. The whole window holds 399 669
OSRT rows. In the epoch's part (2.867 s):

| api | calls | calls/s | p50 | p95 | max | summed |
| --- | --- | --- | --- | --- | --- | --- |
| `futex` | 398 063 | 138 828.6 | 32 554 ns | 418 140 ns | 562 735 888 ns | 85 823 833 958 ns |
| `open64` | 1 286 | 224 801.8 (a 6 ms burst) | 1 407 ns | 1 907 ns | 39 165 ns | 2 015 889 ns |
| `statx` | 191 | 35 110.1 (the same burst) | 1 191 ns | 2 176 ns | 7 122 ns | 260 888 ns |
| `pthread_create` | 32 | 2 555.1 | 195 980 ns | 993 394 ns | 1 211 943 ns | 8 913 464 ns |
| `mmap64` / `mprotect` | 32 / 32 | — | 39 414 ns / 28 482 ns | — | 1 063 090 ns / 565 554 ns | 3 820 626 ns / 1 684 061 ns |
| `read` / `close` | 8 / 3 | — | — | — | — | 162 249 ns / 13 298 ns |

33 thread ids appear in the epoch; the 32 created inside the window are its worker threads. The 1 286
`open64` calls come from the **main** thread inside the epoch's first 18 ms, and a 120 s `strace`
probe of the same binary names what they are: `/sys/devices/system/cpu/cpuN/cache/index*/…` (the scan
opens an index directory 256 times, and 128 times each `ways_of_associativity`, `type`, `uevent`,
`shared_cpu_list`, `physical_line_partition`, `number_of_sets` and `id`, 96 times each `size`, `level`,
`coherency_line_size` and `shared_cpu_map`, plus the `cache` directories 64 + 32 times), with
`/sys/devices/system/cpu/online`, `/sys/fs/cgroup/cpu/cpu.cfs_period_us`, `cpu.cfs_quota_us` and
`/proc/self/cgroup` — a CPU/cache-topology probe, after which 32 worker threads are created, one per
logical CPU of this host. [INFERENCE] That is a CPU-backend thread pool being initialised lazily by the
first training batch; the trace itself shows only the file reads, the thread creations and the futexes,
so the attribution is the inference, and the counts are the measurement. After that burst the epoch's
only OS activity is `futex`: no further file, allocation or process call appears in the window.

Two caveats, both measured. The window covers the epoch's first 2.867 s of 367.197 s, so it describes
that slice, not the whole loop. And it is *under the profiler*: `nsys` charges per-call overhead to the
instrumented process, so read the plain leg for the step's cost and this table for its shape — the same
split `T50/model-step` draws between its plain and profiled rows.

## Why an `osrt` trace of the epoch is bounded, and was not taken whole

The first attempt traced the whole epoch in one collection. It is **unbounded**: the retained window
measures **399 669 rows in 6 s (~67 000/s)**, so a 367 s epoch is of the order of 25 million rows, and
the intermediate stream `nsys` writes under `/tmp` grows with it. That attempt was killed during a
host-safety stop when the guest's vdisk had grown by ~93 GB; killing `nsys` deletes the intermediate
stream, so nothing of that attempt landed in this directory — the lesson is the reason collection is a
bounded window here, and the reason the whole-epoch trace is not a thing a later agent should retry
"just to have the whole thing": the timeline is bounded by construction, the shape is in the window and
the cost is in the trainer's own printed line.

## Files

| file | bytes | sha256 |
| --- | --- | --- |
| `capture.json` | 1 408 | `dd444ec82453b20b3804d6d268eb7e6be242c078dc296bf1ce6b388d8d5dc00f` |
| `capture.sqlite` | 77 824 | `bca6b0818f167a56865e0d12a1aac5d3cca713cd3c5a92650e182f4947957f43` |
| `osrt-calls.csv` | 989 | `8ca4370c6037d315105c6ddc285f12906ce79ca214bef5ec3279a9bbb3967cbb` |
| `timeline.csv` | 979 | `911f750dff7754875f7736c38e4451b290f957c329a8177e43d8e46632e9e713` |
| `train-progress.txt` | 2 388 | `1ed747723fdeebebd37f9a7e584e276024b94b4eab326c9b1d791f8416674be6` |
| `make_timeline.py` | 4 438 | `83d112bda58e85a381164dc01abe3caeb168320255999008b6ac454219115d93` |

`capture.json` is mage's manifest for the mage leg, with every absolute path elided to home-relative
(the profiler's `--output=` directory in `profiler_argv`, the report directory in `error`) and the nsys
progress text mage folded into `error` dropped after its own sentence. It reads `status: "failed"`,
`returncode: 1`, `error: "RuntimeError: nsys exited with code 1. Reports: ~/t47t31-capture/mage/mage-nsys-ohluu939"`.
That is mage's own text for *this* run and not the convention's `captured no CUDA kernel launches`
message: the trainer exits 1 on this fixture, `nsys` relays that code, and mage reports the relayed code
before it counts kernels. The kernel-less fact itself is in the retained export — it has no
`CUPTI_ACTIVITY_KIND_KERNEL` table — and no `kernels.json`/`kernels.csv` exists for a run that launches
no kernel. `--capture-range all` and not `cuda`, per the convention: no binary here calls
`cudaProfilerStart`/`cudaProfilerStop`, so a `cuda` range records nothing.

`capture.sqlite` is the **window** leg's exported SQLite, **trimmed** to the rule: keep `OSRT_API` rows
with `start <` the epoch's start + 20 ms; keep `ThreadNames`, `TARGET_INFO_SESSION_START_TIME`, and the
`StringIds` rows referenced by *either* kept table (T35's precedent); `VACUUM`. That is **1 525 of the
window's 399 669 rows** — all of the window's `open64` (1 290), `statx` (194), `read` (12), `write` (7),
`close` (6) and the one `[Unknown]`, **11 of the 32 `pthread_create`**, **4 of the 32 `mmap64`** and
**none of the 32 `mprotect`** (the 20 ms cut falls through the pool creation — the `pthread_create`
rows run from +17.9 ms to +30.4 ms) — with 54 `StringIds` rows, so `ThreadNames` joins **161 of 161**
rows to its names. Those 54 are the rows the two kept tables reference, and that rule keeps the host's
own service-table names among them (`systemd-*`, `ollama`, `zsh`, `Relay(5752)`) — names only, no
path, user or hostname. The export therefore answers the phase boundary directly. It is `VACUUM`ed from
the untrimmed export named in the table below. The 398 063 sustained `futex` rows that follow are **not** in
the committed export; they
are projected into `osrt-calls.csv`, the way `T50/model-step` ships its 31 565-row runtime table as
`api-calls.csv` instead. The trace clock's zero is the start of collection, 14 s into the process;
event timestamps are nanoseconds from it, and the epoch's start is the last `write` row. Read it with:

```bash
python3 - <<'PY'
import sqlite3
con = sqlite3.connect("docs/evidence/T31/training-step/capture.sqlite")
for row in con.execute(
    "SELECT s.value, o.start, o.end, o.end - o.start, o.globalTid FROM OSRT_API o "
    "JOIN StringIds s ON s.id = o.nameId ORDER BY o.start"):
    print(row)
PY
```

`osrt-calls.csv` and `timeline.csv` are **projections of the untrimmed window export**, not of the
committed trim: the first is the window's full event set aggregated by (phase, api) — calls, total,
mean, nearest-rank p50/p95, max, the group's span and rate — including the 398 063 `futex` rows the
committed export does not carry; the second counts the window per 100 ms bucket per API, which is the
band table above. **They cannot be regenerated from the committed `capture.sqlite`** — it drops those
rows by construction — so they are only re-derivable with the untrimmed export, which stays on the
capturing machine (its size and sha256 are in the table below). The generator is committed beside them
and reproduces both files byte for byte from that input:

```bash
python3 docs/evidence/T31/training-step/make_timeline.py <untrimmed capture.sqlite> \
  docs/evidence/T31/training-step
# 399669 OSRT rows, epoch starts at 3107537733 ns
# wrote osrt-calls.csv (14 rows) and timeline.csv (48 rows)
```

`train-progress.txt` is the untraced leg's own output, every line stamped with seconds since the
child's start: the 24 materialization lines, the epoch line, the calibration line and the publication
line, with the home paths elided to `~`. It is the "iteration count and the number observed" the ticket
asks a capture to carry, in the trainer's emitted wording (D-008/D-009).

### Stays on the capturing machine

| artifact | bytes | sha256 | why it is not committed |
| --- | --- | --- | --- |
| the window leg's untrimmed `capture.sqlite` | 30 658 560 | `0c84a56c3c29d75e28fb27cd1d005e1fa18a9d714dc424a8819404e3bff9fe2e` | carries the 398 063 `futex` rows projected into `osrt-calls.csv`; the committed trim is from it |
| the window leg's `capture.nsys-rep` | 6 191 759 | `c13132443733a2c82537824f204a06a6f1a9d12d7110a2120b2a1225746695d1` | `nsys`'s report; the committed SQLite is its export |
| the mage leg's `capture.sqlite` | 1 003 520 | `9f1136b4d46ccb3ef1c0a9d2e51e8a06b2be63450f0138366b13fb1079ec3087` | the same run's `cuda,nvtx` export: no kernel, no `CUPTI` table, nothing to project |
| the mage leg's `capture.nsys-rep` | 163 773 | `5a675aa00857856451c436bc9bd224f321ea4cd8e704b3e8c68f95a68e140c5b` | `nsys`'s report for the mage leg |
| the checkpoint weights (all three legs) | ~15 MB each | `b98cbc092a1c48dbeec8e1eefe543f305feb94b4d4357efce45ecc493c5bd573` (one hash for all three) | the trainer's published checkpoint; deterministic here, so the three legs wrote the same bytes |
| the training reports | ~3.3 KB each | `d750e0c7…` (plain), `fa5722ef…` (mage), `13916c67…` (window) | the trainer's published report, digest-named; its fields and its own digest are in the filename |
| `process.log`, `nsys.log`, the fixture writer and the catalog | — | — | the profilers' logs and the throwaway fixture, all of which embed this machine's scratch paths; the fixture writer is quoted in §"How it was taken" |

## How it was taken

```sh
# the fixture (throwaway; the crate's own sealed_session recipe), 12 sessions x 4168 frames
$HOME/t47t31-target/release/t47t31-fixture --root ~/t47t31-capture/fixture/data \
  --sessions 12 --frames 4168                       # exit 0, 3.7 s

# the agent's dataset argv
$HOME/t47t31-target/release/qualia-jepa-dataset \
  --catalog ~/t47t31-capture/fixture/data/catalog.json \
  --output-dir ~/t47t31-capture/datasets            # exit 0, manifest b9992b24…

# leg 1 — the convention's mage leg, capture.json
mage profile-exec --backend nsys --capture-range all --no-persist \
  --output-dir ~/t47t31-capture/mage \
  -- $HOME/t47t31-target/release/qualia-jepa-train \
     --manifest ~/t47t31-capture/datasets/jepa-dataset-b9992b24….json \
     --checkpoint-id cnn-b9992b243b19 --output-dir ~/t47t31-capture/mage/train \
     --backend cpu --epochs 1                       # exit 1, 484 s

# leg 2 — the untraced run, every emitted line timestamped (train-progress.txt)
python3 -u -c '
import subprocess, sys, time
p = subprocess.Popen(sys.argv[1:], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                     text=True, bufsize=1)
t0 = time.time()
for line in p.stdout:
    print(f"+{time.time()-t0:9.3f} {line}", end="")
rc = p.wait()
print(f"[exit {rc}] total {time.time()-t0:.3f} s")
' $HOME/t47t31-target/release/qualia-jepa-train \
     --manifest ~/t47t31-capture/datasets/jepa-dataset-b9992b24….json \
     --checkpoint-id cnn-b9992b243b19 --output-dir ~/t47t31-capture/plain/train \
     --backend cpu --epochs 1                       # exit 1, 465.983 s

# leg 3 — the bounded timeline window (manual: mage's nsys argv is fixed at --trace=cuda,nvtx,
# so the osrt trace is not reachable through `mage profile-exec`)
nsys profile --trace=osrt,cuda,nvtx --sample=none --cpuctxsw=none --stats=false \
  --show-output=true --export=sqlite --output=~/t47t31-capture/nsys-window/capture \
  --force-overwrite=true --delay=14 --duration=6 --stop-on-exit=false --kill=none --wait=primary \
  -- $HOME/t47t31-target/release/qualia-jepa-train \
     --manifest ~/t47t31-capture/datasets/jepa-dataset-b9992b24….json \
     --checkpoint-id cnn-b9992b243b19 --output-dir ~/t47t31-capture/nsys-window/train \
     --backend cpu --epochs 1
# nsys warns "setting --wait with --stop-on-exit false is contradictory. Ignoring --wait." and exits
# after the export (~26 s); the app it detached keeps running and publishes report 13916c67… on its own

# the committed artifacts from the three legs' outputs
python3 make_timeline.py ~/t47t31-capture/nsys-window/capture.sqlite .   # the two CSVs
# capture.sqlite: the trim rule in §Files, applied to the same export and VACUUMed
sed "s#$HOME#~#g" ~/t47t31-capture/plain/plain.log > train-progress.txt
# capture.json: the mage manifest with the two absolute paths elided to ~ and the nsys progress
# text dropped from `error`
```

The three commands ran one at a time and one build at a time (`cargo +1.98.1 build --release -j 2 -p
qualia-jepa-dataset -p qualia-jepa-model --bins`, exit 0 in 25.6 s on a copied target directory);
nothing ran on the board, and no display adapter, driver or `pnputil` call was made (D-012).

## The test line

`ls docs/evidence/T*/*/README.md` — the ticket's own test, satisfiable in the form above and quoted here
after this capture is committed:

```text
docs/evidence/T16/three-kernels/README.md
docs/evidence/T17/three-kernel-parity/README.md
docs/evidence/T18/fatbin-sm-87/README.md
docs/evidence/T28/4090-mission/README.md
docs/evidence/T30/belief-coupling/README.md
docs/evidence/T31/training-step/README.md      <- this capture
docs/evidence/T35/healing-ladder/README.md
docs/evidence/T50/pinkie-kernels/README.md
```

Every ticket carrying `needs:profile` now has a committed capture: #31/T16, #32/T17, #33/T18,
#46/T30, #47/T31 and #51/T35, plus #172/T50 (`T50/pinkie-kernels`, with `T50/model-step` on its own
branch). The remaining gap is none of the three mandatory sets — `T16`–`T18`, `T30`–`T31` and `T35`
are all committed — and #172 keeps only its own open state, not a missing directory.

## What this does not establish

* It is not a mission. The catalog is a synthesized fixture at the gate minimum; no real mission
  evidence was trained on, and the training gates fail on it (`all_gates_passed: false`), so nothing
  here is a promotion or a claim about candidate quality.
* It is not the loop. The two binaries and their argv are the loop's, but the `EvidenceSealed` trigger,
  the single-flight queue and the registry's promotion decision are not exercised; those run under
  `cargo test -p qualia-braid -p qualia-jepa-registry`.
* The epoch numbers are host and run dependent. The three legs' spread (465.983 s / 484 s walls) is the
  spread of the same work on a shared machine, and the profiled numbers are profiled.
