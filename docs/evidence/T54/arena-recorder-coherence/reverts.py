#!/usr/bin/env python3
"""Regenerate the scratch reverts behind this directory's falsification runs.

Each mode rewrites `crates/shm/src/lib.rs` in place, removing the ordering the
fix added, so the same board command can be run against it:

  writer-parity   put the parity toggle back in `LayerWriter::publish`: the
                  counter is gone, so the guard's `before == after` is
                  satisfied by two publishes inside one copy again
  reader-fence    drop the acquire fence from `LayerReader::snapshot`, leaving
                  the closing load free to be observed before the copy
  both            the pair as it was before the fix: parity writer and
                  fence-less closing load
  prefix-guard    the pre-fix pair in its own shape: parity writer, and the
                  caller-side `coherent_belief` guard back in arena-recorder
                  (the test calls it the way the recorder used to)

The tree must have LF endings (a `git archive` extract); the anchors are LF.
Usage: python3 reverts.py <mode> <tree>
"""

import sys

SHM = "crates/shm/src/lib.rs"
ARENA = "runners/arena-recorder/src/main.rs"

ARENA_IMPORT = """use qualia_types::{
    AppliedActionSnapshot, JepaEvidencePayload, JepaTelemetryPayload,
    LidarOccupancyGridSnapshot, LidarScanSnapshot, NUM_LAYERS,
};"""
ARENA_IMPORT_PREFIX = """use qualia_types::{
    AppliedActionSnapshot, BeliefSlot, JepaEvidencePayload, JepaTelemetryPayload,
    LidarOccupancyGridSnapshot, LidarScanSnapshot, NUM_LAYERS,
};"""
ARENA_NOW_NS = """/// Wall-clock time in nanoseconds since the Unix epoch, or zero if the clock is
/// set before it.
fn now_ns() -> u64 {"""
ARENA_COHERENT_BELIEF = """/// Reads one layer without accepting a half-written belief.
fn coherent_belief(region: &ShmRegion, layer: usize) -> Option<BeliefSlot> {
    let slot = region.layer_slot(layer);
    for _ in 0..SNAPSHOT_ATTEMPTS {
        let before = slot.write_idx.load(Ordering::Acquire);
        let belief = *LayerReader::new(slot).read();
        let after = slot.write_idx.load(Ordering::Acquire);
        if before == after {
            return Some(belief);
        }
    }
    None
}

/// Wall-clock time in nanoseconds since the Unix epoch, or zero if the clock is
/// set before it.
fn now_ns() -> u64 {"""
ARENA_TEST_CALL = "let belief = LayerReader::new(slot).snapshot(SNAPSHOT_ATTEMPTS);"
ARENA_TEST_CALL_PREFIX = "let belief = coherent_belief(&region, layer);"

PUBLISH = """    pub fn publish(&self) {
        self.slot.write_idx.fetch_add(1, Ordering::AcqRel);
    }"""
PUBLISH_PARITY = """    pub fn publish(&self) {
        let front = self.slot.write_idx.load(Ordering::Acquire) & 1;
        self.slot.write_idx.store(1 - front, Ordering::Release);
    }"""

FENCE = """            let belief = *self.read();
            fence(Ordering::Acquire);
            let after = self.slot.write_idx.load(Ordering::Acquire);"""
FENCE_REVERTED = """            let belief = *self.read();
            let after = self.slot.write_idx.load(Ordering::Acquire);"""


def read(path: str) -> str:
    with open(path, "r", encoding="utf-8", newline="") as handle:
        return handle.read()


def write(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)


def rewrite(path: str, old: str, new: str) -> None:
    text = read(path)
    if text.count(old) != 1:
        raise SystemExit(f"{path}: anchor appears {text.count(old)} times")
    write(path, text.replace(old, new))


def main() -> None:
    mode, tree = sys.argv[1], sys.argv[2]
    path = f"{tree}/{SHM}"
    if mode == "writer-parity":
        rewrite(path, PUBLISH, PUBLISH_PARITY)
    elif mode == "reader-fence":
        rewrite(path, FENCE, FENCE_REVERTED)
    elif mode == "both":
        rewrite(path, PUBLISH, PUBLISH_PARITY)
        rewrite(path, FENCE, FENCE_REVERTED)
    elif mode == "prefix-guard":
        rewrite(path, PUBLISH, PUBLISH_PARITY)
        arena = f"{tree}/{ARENA}"
        rewrite(arena, ARENA_IMPORT, ARENA_IMPORT_PREFIX)
        rewrite(arena, ARENA_NOW_NS, ARENA_COHERENT_BELIEF)
        rewrite(arena, ARENA_TEST_CALL, ARENA_TEST_CALL_PREFIX)
    else:
        raise SystemExit(f"unknown mode {mode}")


if __name__ == "__main__":
    main()
