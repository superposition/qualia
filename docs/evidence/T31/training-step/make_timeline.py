#!/usr/bin/env python3
"""Regenerate this directory's two derived tables from the window's raw export.

These two files are **projections of the untrimmed leg-3 export**, which stays on
the capturing machine (named by size and sha256 in the README's "Stays on the
capturing machine"); they are *not* derivable from the committed, trimmed
`capture.sqlite`, which drops the window's 398,063 `futex` rows by construction.

    python3 make_timeline.py <untrimmed capture.sqlite> <output directory>

Input: the SQLite `nsys export` of

    nsys profile --trace=osrt,cuda,nvtx --sample=none --cpuctxsw=none
      --stats=false --export=sqlite --delay=14 --duration=6
      --stop-on-exit=false --kill=none --wait=primary -- qualia-jepa-train ...

Rules, so the numbers in the README can be checked against these tables:

* the window's trace clock starts at the start of collection (14 s into the
  process); every timestamp is nanoseconds from it;
* the epoch's start is the `start` of the **last `write`** OSRT event in the
  window — the trainer's last `jepa materialized …` line, whose completion is
  the first instant of the epoch loop;
* a `write` event is phase `materialize progress`; any other event that starts
  before the epoch's start is `materialize I/O`; anything at or after it is
  `epoch`;
* `osrt-calls.csv` groups by (phase, api): `calls`, `total_ns`, `mean_ns`, the
  nearest-rank `p50_ns`/`p95_ns` of the per-call durations (index
  `min(n-1, int(f*n))` on the sorted durations), `max_ns`, the group's `span_s`
  (last start minus first start) and `calls_per_second` over that span;
* `timeline.csv` buckets every event by `(start - window_start) // 100 ms` and
  counts the calls per (bucket, api).
"""
import csv
import os
import sqlite3
import sys


def main() -> int:
    source, outdir = sys.argv[1], sys.argv[2]
    connection = sqlite3.connect(source)
    rows = list(connection.execute(
        "SELECT o.start, o.end, s.value, o.globalTid FROM OSRT_API o "
        "JOIN StringIds s ON s.id = o.nameId ORDER BY o.start"))
    if not rows:
        raise SystemExit(f"{source} has no OSRT_API rows")
    writes = [row for row in rows if row[2] == "write"]
    if not writes:
        raise SystemExit(f"{source} has no write event to mark the epoch's start")
    epoch_start = writes[-1][0]

    def phase(start: int, api: str) -> str:
        if api == "write":
            return "materialize progress"
        return "materialize I/O" if start < epoch_start else "epoch"

    groups: dict = {}
    for start, end, api, _ in rows:
        groups.setdefault((phase(start, api), api), []).append((start, end))

    def percentile(values, fraction):
        ordered = sorted(values)
        return ordered[min(len(ordered) - 1, int(fraction * len(ordered)))]

    with open(os.path.join(outdir, "osrt-calls.csv"), "w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["phase", "api", "calls", "total_ns", "mean_ns", "p50_ns",
                         "p95_ns", "max_ns", "span_s", "calls_per_second"])
        for (phase_name, api), events in sorted(groups.items()):
            durations = [end - start for start, end in events]
            starts = [start for start, _ in events]
            span = (max(starts) - min(starts)) / 1e9
            writer.writerow([
                phase_name, api, len(events), sum(durations),
                round(sum(durations) / len(events), 1), percentile(durations, 0.5),
                percentile(durations, 0.95), max(durations), round(span, 3),
                round(len(events) / span, 1) if span else "",
            ])

    buckets: dict = {}
    window_start = rows[0][0]
    for start, _, api, _ in rows:
        key = ((start - window_start) // 100_000_000, api)
        buckets[key] = buckets.get(key, 0) + 1
    with open(os.path.join(outdir, "timeline.csv"), "w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["bucket_100ms", "window_offset_ms", "api", "calls"])
        for (bucket, api), count in sorted(buckets.items()):
            writer.writerow([bucket, bucket * 100, api, count])

    print(f"{source}: {len(rows)} OSRT rows, epoch starts at {epoch_start} ns")
    print(f"wrote osrt-calls.csv ({len(groups)} rows) and timeline.csv "
          f"({len(buckets)} rows) into {outdir}")
    connection.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
