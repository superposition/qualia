#!/usr/bin/env python3
"""Diff two safetensors checkpoints tensor by tensor.

Usage: diff_weights.py <before.safetensors> <after.safetensors>

Prints the maximum absolute difference and the before-scale of every tensor,
plus how many tensors in each parameter scope moved at all. T52's capture left
every `encoder.*` and `target_encoder.*` tensor at exactly zero difference
between a one-epoch and a ten-epoch run of the same fixture; this script is how
this capture shows that they move once the encoder is in the objective.
"""
import hashlib
import json
import struct
import sys

SCOPES = ("encoder.", "target_encoder.", "predictor.", "occupancy_decoder.", "adapter.")


def load(path):
    with open(path, "rb") as handle:
        raw = handle.read()
    header_len = struct.unpack("<Q", raw[:8])[0]
    header = json.loads(raw[8 : 8 + header_len])
    base = 8 + header_len
    tensors = {}
    for name, info in header.items():
        if name == "__metadata__":
            continue
        start, end = info["data_offsets"]
        count = (end - start) // 4
        tensors[name] = struct.unpack(f"<{count}f", raw[base + start : base + end])
    return tensors


def digest(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


before = load(sys.argv[1])
after = load(sys.argv[2])
print(f"before {sys.argv[1]} sha256={digest(sys.argv[1])}")
print(f"after  {sys.argv[2]} sha256={digest(sys.argv[2])}")

rows = []
for name in sorted(set(before) | set(after)):
    left, right = before[name], after[name]
    rows.append((max(abs(a - b) for a, b in zip(left, right)), max(abs(a) for a in left), name))
rows.sort(reverse=True)

print(f"{'max_abs_diff':>14s} {'scale':>12s}  tensor")
for difference, scale, name in rows:
    print(f"{difference:14.6e} {scale:12.6e}  {name}")

moved = [name for difference, _, name in rows if difference != 0.0]
print(f"tensors={len(rows)} moved={len(moved)} unchanged={len(rows) - len(moved)}")
for scope in SCOPES:
    in_scope = [name for _, _, name in rows if name.startswith(scope)]
    moved_in_scope = [name for name in in_scope if name in moved]
    print(f"  {scope:20s} {len(moved_in_scope)}/{len(in_scope)} tensors moved")
