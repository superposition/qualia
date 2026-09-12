#!/usr/bin/env python3
"""Diff two safetensors checkpoints per tensor (max abs difference)."""
import json
import struct
import sys


def load(path):
    with open(path, "rb") as handle:
        raw = handle.read()
    header_len = struct.unpack("<Q", raw[:8])[0]
    header = json.loads(raw[8 : 8 + header_len])
    base = 8 + header_len
    out = {}
    for name, info in header.items():
        if name == "__metadata__":
            continue
        start, end = info["data_offsets"]
        count = (end - start) // 4
        out[name] = struct.unpack(f"<{count}f", raw[base + start : base + end])
    return out


left = load(sys.argv[1])
right = load(sys.argv[2])
changed = []
for name in sorted(set(left) | set(right)):
    a, b = left[name], right[name]
    diff = max(abs(x - y) for x, y in zip(a, b))
    norm = max(abs(x) for x in a)
    changed.append((diff, norm, name))
changed.sort(reverse=True)
print(f"{'max_abs_diff':>14s} {'scale':>12s}  tensor")
for diff, norm, name in changed:
    print(f"{diff:14.6e} {norm:12.6e}  {name}")
print(f"... {len(changed)} tensors total")
