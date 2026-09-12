# The remaining unfenced reader: `coherent_belief`

Board evidence for [#235](https://github.com/superposition/qualia/issues/235) (T54, *the remaining
unfenced reader*), the third instance of the reader protocol class fixed in
[#234](https://github.com/superposition/qualia/pull/234). `runs/` holds the board logs quoted below.

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

The same test passes with the fix on the host and on the board.

## Audit of the remaining workspace

`grep` for a pair of sequence/index loads around a payload copy with no `fence` on the closing read:
the shape existed in exactly three places — `runners/camera` and `runners/agent/src/perception.rs`
(both fixed in #234; this branch's base predates it) and `runners/arena-recorder` (this ticket). Every
other such pair carries the fence: `crates/types` (six readers: lidar scan, lidar occupancy grid,
camera frame, jepa slots, applied-action history, camera preview) and
`runners/jepa-runtime/src/lib.rs::coherent_pose`. Nothing else was found; the nearest non-pair is
`runners/vision/src/main.rs:703`, a whole-array read of the world model with no guard at all, which is
a different shape and not this ticket's scope.

## Board

Pinkie, Jetson Orin NX, `aarch64`, 6 cores, kernel `5.15.148-tegra`, rustc 1.94.0, in
`/home/jetson/remediate/515a506` (a `git archive` of `515a506`, #227's merge), `RUSTFLAGS="-C
target-feature=+fp16" --offline -j 2`. For this run four files were overlaid from the PR head; all four
are byte-identical between the board and this worktree:

| path | md5 (board == worktree) |
| --- | --- |
| `crates/types/src/lib.rs` | `92d6eadd9fb7adc7d380411434c3f50e` |
| `crates/shm/src/lib.rs` | `4789c886c0bf9b271511dd5b20fc02cb` |
| `crates/shm/tests/shm.rs` | `6bfad0dea1676c5364df9d52b0a3b966` |
| `runners/arena-recorder/src/main.rs` | `b42d1749cc7edacc16310a6024741ca9` |

Every path either session touched was restored to the archive's own bytes afterwards, md5-verified
(`crates/types/src/lib.rs` `282f59d0`, `crates/shm/src/lib.rs` `e227ece4`, `crates/shm/tests/shm.rs`
`5b590253`, `runners/arena-recorder/src/main.rs` `a4abcf9c`, plus the camera and agent files
`4195327f`/`883c7638`/`eb43c1fe`/`2cc8c2f8`/`b7730a80`).

```text
$ cargo build -p qualia-arena-recorder --offline -j 2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 27.48s      (exit 0, no warnings)

$ cargo test -p qualia-shm -p qualia-arena-recorder --offline -j 2
     Running unittests src/main.rs (target/debug/deps/qualia_arena_recorder-...)
running 10 tests
test tests::a_coherent_read_of_the_recorded_belief_is_never_torn ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s
     Running tests/recorder.rs (target/debug/deps/recorder-...)
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.46s
     Running unittests src/lib.rs (target/debug/deps/qualia_shm-...)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/shm.rs (target/debug/deps/shm-...)
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.23s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
                                                                              (exit 0)
```

## Host

```text
$ cargo test -p qualia-arena-recorder -p qualia-shm -j 2
test result: ok. 10 passed ... (arena-recorder unit)   test result: ok. 5 passed ... (recorder)
test result: ok. 14 passed ... (shm integration, Windows includes two platform cases)
                                                                              (exit 0)
$ cargo test -p qualia-l3-belief -j 2
test result: ok. 2 passed ... (the other reader of write_idx: it compares the value before/after a
                              refused start, which a counter preserves)
                                                                              (exit 0)
```
