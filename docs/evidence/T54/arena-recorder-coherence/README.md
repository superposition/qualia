# The remaining unfenced reader: `coherent_belief`

Board evidence for [#235](https://github.com/superposition/qualia/issues/235) (T54, *the remaining
unfenced reader*), the third instance of the reader protocol class fixed in
[#234](https://github.com/superposition/qualia/pull/234). `runs/` holds the runs quoted below: dev-host
logs for the host half, and the board logs once the pending lease pass has run.

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
drives the writer from the reader's own progress: it publishes 32 times inside one copy, so the copy
straddles a rewrite of the buffer it is reading and the even number of flips returns the index to
where it started. On the pre-fix code, before the fix was applied:

```text
assertion `left == right` failed: a coherent read saw two publishes mixed together
  left: 1
 right: 0
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.08s
```

The same test passes with the fix on the host; the board run is the pending lease pass below.

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

## Board — pending the lease

The board pass is queued behind T58 and T55, recorded as `blocked_on: board` in this ticket's braid with
the command it will run (D-024). The two board logs from the earlier session were removed with this
push: the reviewer found they were produced from a tree whose `main.rs` still imported `BeliefSlot`
(the unused-import warning in the log), so they were not the artifact under review. The lease replaces
them with:

```bash
cargo build -p qualia-arena-recorder --offline -j 2
cargo test -p qualia-shm -p qualia-arena-recorder --offline -j 2
sh docs/evidence/T54/arena-recorder-coherence/run_repeats.sh t54_fixed 20
python3 docs/evidence/T54/arena-recorder-coherence/reverts.py prefix-guard .
sh docs/evidence/T54/arena-recorder-coherence/run_repeats.sh t54_prefix 5
```

in `/home/jetson/remediate/515a506` (a `git archive` of `515a506`, #227's merge; Jetson Orin NX,
aarch64, 6 cores, kernel `5.15.148-tegra`, rustc 1.94.0) with `RUSTFLAGS="-C target-feature=+fp16"
--offline -j 2`, restoring the four overlaid files to the archive's own bytes afterwards. The files and
their hashes at this head:

| path | md5 (board == worktree) |
| --- | --- |
| `crates/types/src/lib.rs` | `92d6eadd9fb7adc7d380411434c3f50e` |
| `crates/shm/src/lib.rs` | `ed6be29a75898fa9b59234f56e39dad0` |
| `crates/shm/tests/shm.rs` | `6bfad0dea1676c5364df9d52b0a3b966` |
| `runners/arena-recorder/src/main.rs` | `b42d1749cc7edacc16310a6024741ca9` |

Same caveat as #234's evidence: the archive predates #229 and the changed files are overlaid on it
byte-identical, so the runs carry for those files without the tree being literally the PR head.

## Falsification

Four scratch reverts, all reproducible with `reverts.py`; `runs/host-*.log` are the dev-host runs.

| mode | what it removes | host result |
| --- | --- | --- |
| `prefix-guard` | the parity writer **and** the fix's pair: the caller-side `coherent_belief` guard is back and the test calls it the way the recorder used to | **fails** — `a coherent read saw two publishes mixed together`, `left: 1`, `right: 0` |
| `writer-parity` | the counter (parity toggle restored); reader left fixed | passes — the fence keeps the closing load where it is, so a torn acceptance needs an even flip count *and* a copy that straddles a rewrite |
| `reader-fence` | the acquire fence; counter left fixed | passes — the counter alone rejects any copy a publish touched |
| `both` | both halves, but with the guard inside `snapshot` rather than at the caller | passes |

So the test catches the pre-fix *protocol* — the pair as it was, guard at the caller with no fence —
and each half alone is latent in the new shape on this CPU. The board half of this table (the same
`prefix-guard` revert failing on Pinkie, plus ≥20 repeats of the fixed test at this head) is the pending
lease pass.

```text
the test-first run, pre-fix code, dev host (runs/host-prefix-guard.log):
thread 'tests::a_coherent_read_of_the_recorded_belief_is_never_torn' panicked at ...:
assertion `left == right` failed: a coherent read saw two publishes mixed together
  left: 1
 right: 0
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.07s
```

## Host

```text
$ cargo test -p qualia-arena-recorder -p qualia-shm -j 2            exit 0, no warnings
test result: ok. 10 passed ... finished in 0.13s   (arena-recorder unit, incl. the new test)
test result: ok.  5 passed ... finished in 3.11s   (recorder.rs)
test result: ok. 14 passed ... finished in 0.24s   (shm.rs; Windows adds two platform cases)
$ cargo test -p qualia-l3-belief -j 2
test result: ok. 2 passed ... (the other reader of write_idx: it compares the value before and after a
                              refused start, which a counter preserves)
$ python scripts/provenance_check.py
provenance: OK
```
