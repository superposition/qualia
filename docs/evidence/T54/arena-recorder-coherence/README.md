# The remaining unfenced reader: `coherent_belief`

Board evidence for [#235](https://github.com/superposition/qualia/issues/235) (T54, *the remaining
unfenced reader*), the third instance of the reader protocol class fixed in
[#234](https://github.com/superposition/qualia/pull/234). `runs/` holds the runs quoted below: dev-host
logs for the host half, and the board logs from the lease pass below — the quiet set, with
`board-identity.txt`, `board-timeline.txt` and each `run_repeats.sh` summary beside them.

## What was wrong

`runners/arena-recorder/src/main.rs::coherent_belief` read one layer like this:

```rust
let before = slot.write_idx.load(Ordering::Acquire);
let belief = *LayerReader::new(slot).read();
let after = slot.write_idx.load(Ordering::Acquire);
if before == after { return Some(belief); }
```

`LayerSlot` is a double buffer: a writer fills the buffer the reader is not looking at and then
publishes, and `write_idx` was a parity bit (`store(1 - front, Release)`). Two holes:

1. **Ordering.** No fence sits between the payload copy and the closing `write_idx` load. On aarch64 a
   load may be observed before an earlier, independent load, so the closing check can complete before
   the copy does and a copy taken across a publish is accepted — the same hole #234 fixed in
   `CameraPreview`. x86_64 TSO never reorders load-with-load, which is why the package's suite passed
   there.
2. **Parity.** With a two-valued index, two publishes inside one copy return it to where it started,
   so `before == after` holds and the torn copy is accepted. This one is *architecture-independent*:
   the new test fails on the pre-fix code on the dev host too, which is where it was first observed.

## The fix

- `LayerWriter::publish` advances a monotonic counter (`fetch_add(1, Ordering::Release)`) instead of
  toggling a parity bit. Its low bit is still the front buffer, so every reader keeps selecting the
  way it always did.
- `LayerReader::snapshot(max_attempts)` copies the front buffer, executes `fence(Ordering::Acquire)`,
  re-reads the counter and accepts only an unchanged value, retrying otherwise. With a counter, equal
  means "no publish landed in that copy" — something a parity bit cannot express.
- `coherent_belief` is gone; `capture_beliefs` calls the slot's own pair directly, and the test does
  the same.

**ABI.** The layout is unchanged: `write_idx` is the same `AtomicUsize` at the same offset, no field
moves, and no wire or JSON field changes. What changes is that field's *value convention*: `0/1`
becomes a publication counter whose low bit is the front index. Every in-repo consumer selects with
`& 1` (`LayerReader::read`, `LayerWriter::back_buffer`), the CUDA/Metal paths reach the buffers through
those two types rather than the index, and the one test that pinned the raw `0/1` values
(`crates/shm/tests/shm.rs`) now pins the counter and the new snapshot.

## Test written first

`a_coherent_read_of_the_recorded_belief_is_never_torn` (in `runners/arena-recorder`'s test module)
drives the writer from the reader's own progress: it publishes 500 times inside one copy, each publish
writing the same marker set spread through the belief, so the copy reads markers from different publishes
while an even number of flips returns the index to where it started. On the pre-fix shape, committed as
`runs/host-prefix-guard.log`:

```text
assertion `left == right` failed: a coherent read saw two publishes mixed together
  left: 1447
 right: 0
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.07s
```

The same test passes with the fix on the host, and on Pinkie in the quiet set in §Board below. (The
driver's first form rewrote the whole buffer per publish and showed a single torn copy once; that form is
superseded and its number is not quoted.)

## Audit of the remaining workspace

`grep` for a pair of sequence/index loads around a payload copy with no `fence` on the closing read:
the shape existed in exactly three places — `runners/camera` and `runners/agent/src/perception.rs`
(both fixed in #234; this branch's base predates it) and `runners/arena-recorder` (this ticket). Every
other such pair carries the fence: `crates/types` (six readers: lidar scan, lidar occupancy grid,
camera frame, jepa slots, applied-action history, camera preview) and
`runners/jepa-runtime/src/lib.rs::coherent_pose`. Nothing else of that shape was found.

The adjacent *no-guard* family is wider, and is reported rather than changed:
`runners/vision/src/main.rs:703` (a whole-array read of the world model),
`runners/agent/src/realtime.rs:255-257` (the whole 4 MiB `LayerSlot::weights` behind a stray fence and
no version check), and `apps/qualia-console/src/views/brain/matrices.rs:49-51` (the same array, no
guard). Their writer, `ShmRegion::write_weight_tile`, copies tiles with no publication protocol at all —
the weights sit outside the layer's double buffer, so this slot's counter does not cover them either.
That is a different shape, out of this ticket's scope.

## Board (lease pass)

Pinkie, Jetson Orin NX, `aarch64`, 6 cores, kernel `5.15.148-tegra`, rustc 1.94.0, in
`/home/jetson/remediate/515a506` (a `git archive` of `515a506`, #227's merge), `RUSTFLAGS="-C
target-feature=+fp16" --offline -j 2`. **The numbers below are the quiet set**, run 02:19:52..02:21:49 board-local (the lease itself
02:11..02:22:04; per-leg windows and the log mtimes are in `board-timeline.txt`) after T59's
`cargo build --release -p qualia-lidar -p qualia-drive` (started 02:17 inside this lease) was killed:
no `cargo`/`rustc` in the 02:19:41 look (taken before this pass's own commands; the build it started then
is the pass itself), `uptime` load 1.26 falling from T59's build. The lease was taken at 02:11
with the board idle (load 0.25, no `cargo`/`rustc`, `/dev/shm` 4%); the sets I ran after 02:15 were
under that build (load ~2.4; the log mtimes put the 20 repeats at 02:16:26..02:16:48 and the revert runs
at 02:17:28..02:17:33) and were discarded — their `20/20` and `1849..1912 torn` are superseded by the
quiet set quoted here. The four changed files were overlaid from this head and every path was
restored to the archive's own bytes afterwards md5-verified
(`crates/types/src/lib.rs` `282f59d0`, `crates/shm/src/lib.rs` `e227ece4`, `crates/shm/tests/shm.rs`
`5b590253`, `runners/arena-recorder/src/main.rs` `a4abcf9c`).

| path (overlaid from this head) | md5 (board == worktree) |
| --- | --- |
| `crates/types/src/lib.rs` | `92d6eadd9fb7adc7d380411434c3f50e` |
| `crates/shm/src/lib.rs` | `ed6be29a75898fa9b59234f56e39dad0` |
| `crates/shm/tests/shm.rs` | `6bfad0dea1676c5364df9d52b0a3b966` |
| `runners/arena-recorder/src/main.rs` | `77719c4d90d02d14ce2b5fd7e91929bd` |

```text
$ cargo build -p qualia-arena-recorder --offline -j 2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 27.94s      (exit 0, no warnings)

$ cargo test -p qualia-shm -p qualia-arena-recorder --offline -j 2
running 10 tests
test tests::a_coherent_read_of_the_recorded_belief_is_never_torn ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s  (arena-recorder unit, incl. the new test)
test result: ok.  5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.47s  (recorder.rs)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s  (shm.rs)
                                                                              (exit 0)

$ sh run_repeats.sh t54c_fixed 20
t54c_fixed summary: pass=20 fail=0    (0.16..0.30s each; runs/board-fixed/01..20.log + summary.txt)
```

## Falsification

Four scratch reverts, all reproducible with `reverts.py`; `runs/board-*/` and `runs/host-*.log` are the
runs.

| mode | what it removes | host | board |
| --- | --- | --- | --- |
| `prefix-guard` | the parity writer **and** the fix's pair: the caller-side `coherent_belief` guard is back and the test calls it the way the recorder used to | **fails**, 1447 torn of 2000 (`host-prefix-guard.log`) | **fails 5/5**, 1906..1930 torn of 2000 (`board-prefix/01..05.log`) |
| `writer-parity` | the counter (parity toggle restored); reader fixed | passes (`host-writer-parity.log`), supportive-only, not run on the board | — |
| `reader-fence` | the acquire fence; counter fixed | passes (`host-reader-fence.log`), supportive-only, not run on the board | — |
| `both` | both halves, but with the guard inside `snapshot` rather than at the caller | passes (`host-both.log`), supportive-only | — |

What the board runs establish is that the test catches **the pre-fix pair**: the parity writer *with*
the guard at the caller and no fence. Five of five board runs fail there, 1906..1930 mixed copies out of
2000 accepted (`board-prefix/`), and the dev host fails the same revert (1447 of 2000).

They do **not** isolate the parity hole on their own. `writer-parity` — the parity index with the guard
moved inside `snapshot` — passes on the host and was not run on the board, so the ~95 % tear rate above
is the guard-placement/ordering half of the pair: the closing check itself was being observed before the
copy on the caller-side shape. The parity half is argued from the code rather than from a live failure —
a two-valued index cannot express "no publish happened", so an even number of flips satisfies the guard —
and this directory claims it only that far. With the fix the counter rejects every copy a publish touched
(20/20 on the board, 20 repeats).

Two honest notes. First, getting there needed the writer's *cycle* to be much faster than the reader's
copy: with each publish rewriting 16 KiB the copy finished before the writer reached the copied buffer,
so the earlier driver showed nothing even on the pre-fix shape. Second, the two halves alone are latent
on both machines (`writer-parity`, `reader-fence`, `both`): in the new shape a torn acceptance needs an
even parity subset *and* a copy that straddles a rewrite, and the guard inside `snapshot` keeps the
closing load where the fence put it on these compilers.

```text
the reverted form on the board, runs/board-prefix/01.log (quiet set):
thread 'tests::a_coherent_read_of_the_recorded_belief_is_never_torn' panicked at ...:
assertion `left == right` failed: a coherent read saw two publishes mixed together
  left: 1930
 right: 0
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.09s
```

## Host

```text
$ cargo test -p qualia-arena-recorder -p qualia-shm -j 2            exit 0, no warnings
test result: ok. 10 passed ... (arena-recorder unit, incl. the new test)
test result: ok.  5 passed ... (recorder.rs)
test result: ok. 14 passed ... (shm.rs; Windows adds two platform cases)
$ cargo test -p qualia-l3-belief -j 2
test result: ok. 2 passed ... (the other reader of write_idx: it compares the value before and after a
                              refused start, which a counter preserves)
$ python scripts/provenance_check.py
provenance: OK
```

## Residues folded after the merge (Main, `affb5c6` → this commit)

Three things the post-merge verification could not reconcile, kept here rather than left for a reader to trip on:

- **The committed logs predate the harness.** `run_repeats.sh` in the tree before this commit wrote per-log
  `# board-local start … load1 …` headers and `start=/end=` summaries; none of the committed
  `runs/board-fixed/*.log` or its `summary.txt` carries either, so that version could not have produced this
  evidence. The script here is now the revision that did (`b63f571`'s form, restored). If the header harness is
  wanted, it should be re-cut against a fresh lease pass, not presented as the source of these runs.
- **No committed board log carries a timestamp.** `runs/board-timeline.txt` derives its windows from `stat` on
  the board and says so; the window/quiet-set claim is therefore re-derivable from durations and order, not from
  bytes inside the logs.
- **`runs/board-identity.txt` was captured after the lease exited** (02:33:55 against a 02:22:04 handover) and
  records no load at the start of a run. It identifies the machine; it does not date the runs. The PR body also
  says `t54_fixed` where these logs and this README say `t54c_fixed`.
