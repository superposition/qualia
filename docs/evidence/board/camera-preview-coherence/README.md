# Camera preview coherence: the seqlock publish/read pair on aarch64

Board evidence for [#100](https://github.com/superposition/qualia/issues/100) — *the preview
publish/read pair is not coherent on aarch64* — for the fix in
[PR #234](https://github.com/superposition/qualia/pull/234). It backs the Definition of done limb in
[`docs/agents.md`](../../../agents.md): the package builds here, its artifact is built for `aarch64`
and exercised on **Pinkie**, and the command and the output are quoted.

Two code paths are covered: `CameraPreview`'s publish/snapshot pair in `crates/types` (and the camera
runner that publishes through it), and `GET /perception/frame` in `runners/agent`, which read the same
slot with a hand-rolled, fence-less copy of the reader protocol.

## What was wrong

Two ordering gaps in the same seqlock, both invisible on the dev host's x86_64 TSO.

**Writer.** `publish_camera_preview` opened the seqlock with `seq.store(opening, Ordering::Release)`.
A release store orders the accesses sequenced before it, not the payload stores that follow, so on
aarch64 the byte copy can become visible under the previous even sequence.

**Reader.** `GET /perception/frame` and the camera test's reader both took the sequence with acquire,
copied the payload, then took the sequence again and compared. An acquire load orders accesses after
itself; it does not keep an earlier, independent load from being observed after it, so on aarch64 the
closing check can complete before the copy and accept a copy taken across a publish.

Measured on Pinkie: a *correct* reader against the test's original bare-`yield` driver caught only
**7..47 complete copies per five seconds** (ten of ten rounds under the floor), so the floor of 500
was reachable only by a reader that closed its window early. The same slot's `snapshot` — copy,
`fence(Acquire)`, re-read — accepted 2000 copies in 0.23 s at every window length measured.

## The fix

`CameraPreview` now carries the publish/read pair every sibling seqlocked slot in `qualia-types`
already has: `publish`/`clear` take the odd marker with `compare_exchange(current, writing, AcqRel,
Acquire)`, which orders the payload stores after it, and publish with a release store; `snapshot`
copies, fences acquire, then re-reads the sequence and accepts only an unchanged even value.
`publish_camera_preview` delegates, and `encoded_frame_get` reads through `snapshot` instead of its own
copy. **ABI: unchanged** — the `#[repr(C)]` layout, offsets and size of `CameraPreview` (`524_328`,
asserted by `crates/types/tests/abi.rs`) are untouched.

The camera test's driver also changed. A bare `yield_now()` between publishes gives the reader no
window on aarch64, so the writer now publishes and then waits for one complete copy before publishing
again: publishes still interrupt copies, and the floor is reachable. This directory's `reverts.py`
records the two reverts used to falsify the result.

## Files exercised on the board

Every run below happened in `/home/jetson/remediate/515a506`, a `git archive` of `515a506` (the then
`origin/main`, `d59e7ab`'s tree). The files there were replaced with the PR head's files and are
**byte-identical** to them (md5, board vs this worktree):

| path | md5 |
| --- | --- |
| `crates/types/src/lib.rs` | `f1dc8ccb3f0814036e790f425d090d07` |
| `runners/camera/src/lib.rs` | `5a3d42f8a76d7a10b3a95e8e7b7c6120` |
| `runners/camera/tests/preview.rs` | `6b8e0fa94663f5a0f0e9c6c47bfb5e15` |
| `runners/agent/src/perception.rs` | `1e7ece8ab7efb81953b29547cb2d486c` |
| `runners/agent/tests/surface.rs` | `2936327e839f2191c1796659ea88420a` |
| `runners/agent/tests/support/mod.rs` | `1f5b2c72e14b2b810053fd1cbb505ac0` |

The tree was restored to its pre-existing files afterwards (the same md5s for the three camera files
as before the session) and the scratch test files were deleted.

Board: Jetson Orin NX, `aarch64`, 6 cores, kernel `5.15.148-tegra`, rustc 1.94.0. Every command ran
with `RUSTFLAGS="-C target-feature=+fp16"` (D-016) `--offline -j 2`.

## Build

```text
$ cargo build -p qualia-camera --offline -j 2
   Compiling qualia-camera v0.1.0 (/home/jetson/remediate/515a506/runners/camera)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.04s           (exit 0)

$ cargo build -p qualia-agent --offline -j 2
   Compiling qualia-agent v0.1.0 (/home/jetson/remediate/515a506/runners/agent)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5m 07s          (exit 0)

$ cargo build -p qualia-init --offline -j 2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.78s           (exit 0)
```

## Fixed

`cargo test -p qualia-camera --offline -j 2` — exit 0:

```text
test result: ok. 0 passed; 0 failed; ... (lib, bin, doc-tests)
test result: ok. 6 passed; 0 failed; ... finished in 0.00s    (config)
test result: ok. 10 passed; 0 failed; ... finished in 2.34s   (degraded)
test result: ok. 7 passed; 0 failed; ... finished in 0.17s    (frame)
test result: ok. 5 passed; 0 failed; ... finished in 26.31s   (mjpeg)
test result: ok. 4 passed; 0 failed; ... finished in 0.83s    (preview)
```

20 repetitions of `readers_never_observe_a_torn_preview` (`runs/camera-fixed/`, one file per run):

```text
01 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.81s
02 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
03 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
04 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
05 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.81s
06 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.81s
07 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
08 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.81s
09 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
10 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
11 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.81s
12 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
13 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
14 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
15 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
16 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
17 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.81s
18 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
19 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.79s
20 test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.80s
```

`cargo test -p qualia-agent --offline -j 2` — exit 0 — and 20 repetitions of the endpoint's
`the_encoded_frame_endpoint_never_serves_a_torn_preview` (`runs/agent-fixed/`):

```text
test result: ok. 4 passed  (braid)      test result: ok. 5 passed  (env_contract)
test result: ok. 5 passed  (improvement) test result: ok. 16 passed (leash)
test result: ok. 8 passed  (mission_broker) test result: ok. 1 passed (startup)
test result: ok. 15 passed (surface)     test result: ok. 0 passed  (doc-tests)

01 EXIT=0 ... finished in 0.41s   02 EXIT=0 ... 0.37s   03 EXIT=0 ... 0.39s
04..20 EXIT=0 ... 0.37s..0.38s    (20/20 pass)
```

## Falsification

**The camera test catches the ordering removed.** `reverts.py camera-reordered` drops the reader's
acquire fence and takes its closing sequence load *before* the payload copy — the reordering aarch64
is permitted to make. 10 of 10 runs fail on a torn copy (`runs/camera-reordered/`); the assertion
counts mixed payloads out of 8000 accepted:

```text
01 left: 379  right: 0      06 left: 147  right: 0
02 left: 171  right: 0      07 left: 851  right: 0
03 left: 272  right: 0      08 left: 790  right: 0
04 left: 53   right: 0      09 left: 97   right: 0
05 left: 208  right: 0      10 left: 659  right: 0
```

**The pre-fix pair, literally reverted** (the parent revision's three camera files and its test), fails
10 of 10 on the board (`runs/camera-pre-fix/`): eight on the accepted floor and two on a torn copy,
which is the failure [#100](https://github.com/superposition/qualia/issues/100) reported
(`tests/preview.rs:163` torn, line 164 accepted):

```text
01 left: 10  right: 500    02 left: 0   right: 500    03 left: 21  right: 500
04 left: 1   right: 0      05 left: 79  right: 500    06 left: 30  right: 500
07 left: 2   right: 0      08 left: 3   right: 500    09 left: 55  right: 500
10 left: 6   right: 500
```

**Honest limits.** Two reverts do *not* fail on this board, and are recorded as such:

- `camera-fence-only` (the fence dropped, the load left in place) passes 5 of 5
  (`runs/camera-fence-only/`). This compiler and this CPU do not reorder the closing load past the
  memcpy-shaped copy, so the bare absence of the fence is latent here; the fence is required by the
  memory model and is what every sibling reader in `qualia-types` does.
- The agent endpoint's own reverts — `agent-pre-fix` (its hand-rolled reader back) 5 of 5 and
  `agent-reordered` 10 of 10 (`runs/agent-pre-fix/`, `runs/agent-reordered/`) — both pass. The
  endpoint's prologue (state clone, response machinery, the body copy) is long enough that its copy
  either finishes before the writer's ~3 µs publish or starts after it, so no copy straddles a
  payload write in that path. The reader the endpoint now shares is the one the camera falsification
  covers; the endpoint's own test is a contract-and-liveness test (whole frames, 2000 bodies, under a
  concurrent writer), and it is not claimed to catch the ordering by itself.

## The ticket's own smoke path

`qualia-init` holds the arena without spawning runners, the camera binary feeds it from a file
snapshot source, and the summary line comes back out of shared memory:

```text
$ QUALIA_INIT_OWNER_ONLY=1 QUALIA_SHM_NAME=/qualia_smoke ./target/debug/qualia-init
[init] Creating shared memory '/qualia_smoke'...
[init] Shared memory created: 64 MB
[init] Owner-only mode: holding shared memory without spawning runners.

$ QUALIA_SHM_NAME=/qualia_smoke QUALIA_CAMERA_SNAPSHOT_PATH=/tmp/fix100/snap128x96.png \
    QUALIA_CAMERA_POLL_MS=100 timeout 3 ./target/debug/qualia-camera
qualia-camera: polling snapshot path /tmp/fix100/snap128x96.png every 100ms
qualia-camera: frame_seq=2 src=128x96 thumb=64x48 luma_mean=0.502 luma_std=0.000 quality=low-contrast
```

`timeout` ends the polling loop by design (`CAMERA_EXIT=124`); the fixture is a 128×96 PNG.

## Host

The same tree on the dev host (`cargo test -p qualia-camera -j 2`, `cargo test -p qualia-agent -j 2`,
`python scripts/provenance_check.py`) is quoted in PR #234.

## Harness

- `run_repeats.sh <label> <runs> <package> <test-target> <test-name>` — runs one test repeatedly in
  the archive tree (`ROOT`), one log per run in `/tmp/fix100`, printing a `SUMMARY: pass=… fail=…`
  line. `CLEAN_SHM=1` unlinks `/dev/shm/qualia-agent-test-*` between runs: the agent's scratch region
  is 64 MiB and its libtest process exits without dropping it, so without that the board's 1.8 GiB
  `/dev/shm` fills and run 20 dies with SIGBUS (measured: the first 20-rep attempt, 19 pass + one
  SIGBUS).
- `reverts.py <mode> <tree>` — rewrites one file in place for a falsification run:
  `camera-fence-only`, `camera-reordered`, `agent-pre-fix`, `agent-reordered`. It requires an LF tree
  (the `git archive` extract) and fails loudly if its anchor is missing.

`runs/` holds the logs behind every number above, one file per run.
