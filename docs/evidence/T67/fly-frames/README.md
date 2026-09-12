# T67 — the fly loop's per-tick wheel stream

The missing producer shim is implemented as `qualia-connectome-cns loop --frames-out -`. Both the
CPU and CUDA paths decode, record and emit each tick before acquiring the next image. The CSV and
JSON come from the same decision and throttle; `T` is the original tick and hold emits explicit
zeroes. `--max-wheel-speed` defaults to `0.04`. The mapping is a bounded differential pivot, described
in the [runner README](../../../../crates/connectome-cns/README.md#wheel-frames-for-the-leash-transport).
The transport source and its safety logic are unchanged.

## What was exercised

These are **local checks, not robot actuation evidence**. On 2026-09-12, the Windows host ran the
released Male CNS artifact from `C:/tmp/Impl236ConnectomeRunner/artifact` against a loopback HTTP
server serving the previously captured `snap_test.jpg` in that directory's parent. No request went
to the robot. The server's deliberate gap and error were local fault injection. The trace's automatic
`input live JPEG camera at http://127.0.0.1:53812/...` label describes the runner's HTTP input mode;
the image was a recording, not a live camera view.

Input SHA-256 hashes:

```text
snap_test.jpg: 01c66c745b0eae7d7e307322db3a56dd1076f3ea4927349a8ef9ba6324c22128
artifact/manifest.json: 3c52a54404e4f80b29697c7010bf55862b5f9ccccd0206add77be928c6dc4fbc
```

The temporary driver is `C:/tmp/qualia-driving/check_stream.py`; no test file is a deliverable. It
held the second JPEG response until it had consumed the first JSON frame. A producer that waited
until the loop finished would fail this check. It then delayed the response by 800 ms and verified
that the stream contained no extra or repeated frame during the gap. JSON was parsed independently,
and each command and throttle was recomputed from the QLSP spike IDs and the artifact's descending
and motor populations. All recorded ticks, CSV readouts and emitted wheel values agreed.

Each normal run used this binary command, with `NAME`, `DEVICE` and `N` set to the cases below:

```text
C:/Users/ericm/qualia/target/debug/qualia-connectome-cns.exe loop
  --artifact C:/tmp/Impl236ConnectomeRunner/artifact
  --camera http://127.0.0.1:53812/camera/snapshot --ticks N --device DEVICE
  --spikes C:/tmp/qualia-driving/NAME.bin
  --trace docs/evidence/T67/fly-frames/NAME.csv
  --session recorded-jpeg-local-check --frames-out -
```

| Name | Device / ticks | Evidence |
| --- | --- | --- |
| `gpu-stream` | RTX 4090 / 30 | JSON, CSV and stderr; left 17, hold 4, right 9; 0.969 s injected arrival gap |
| `cpu-stream` | CPU / 12 | JSON, CSV and stderr; left 5, hold 4, right 3 |
| `camera-failure` | RTX 4090 / 3 requested | Second fetch returns HTTP 503; one JSON frame, then exit 1 and EOF |
| `broken-pipe` | RTX 4090 / 4 requested | Consumer closes after the first frame; exit 1 with `wheel frame: The pipe is being closed. (os error 232)` |

[`checks.log`](checks.log) is the driver's captured summary. The `.jsonl` files contain stdout only;
the `.stderr.log` files contain the runner's status and errors. The broken-pipe CSV can contain a row
whose frame could not be delivered: recording the decision precedes attempting to write it. Neither
a trace row nor a successful pipe write asserts that leash applied a command.

Four invalid limits (`NaN`, `inf`, `-0.1`, `1.1`) were refused before loading the artifact or emitting
anything. Spikes used for the independent comparison remain in the named scratch directory; they
are recordings, not source, and are not committed.

The correctness review found an existing frame destination could be truncated before refusal if it
also named the trace or spike file. The fix checks for an existing destination before opening any
output and retains `create_new` at the actual frame-file open. The rebuilt binary passed all three
existing-destination checks (trace alias, spike alias and independent path), preserving every prior
byte and refusing before artifact loading. A new frame file at wheel limit zero also recorded six
ticks with zero wheel values and no stdout. See [`file-handling.log`](file-handling.log); the temporary
driver is `C:/tmp/qualia-driving/check_file_handling.py`. The fix rebuild exited 0 in 3.95 s.

## Build and repository checks

All Cargo commands ran from `C:/Users/ericm/qualia-driving`, sequentially, with
`CARGO_TARGET_DIR=C:/Users/ericm/qualia/target`, `-j 2` and `--offline`:

```text
cargo build -j 2 -p qualia-connectome-cns --features cuda --offline
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 30s

cargo test -j 2 --offline -p qualia-connectome-cns --features cuda --test cns a_mutated_sign_fails_the_digest_check -- --exact --nocapture --test-threads=1
test a_mutated_sign_fails_the_digest_check ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.02s

cargo run -j 2 --quiet --offline -p qualia-gates -- provenance
provenance: compared 152 authored file(s) (1 generated skipped); 0 identical, 27 EOL-identical, 7 code runs over 20, 0 prose runs over 2
provenance: OK

cargo run -j 2 --quiet --offline -p qualia-gates -- figures
figures: OK (11 entries, 53 figures, 400 KiB budget)

cargo run -j 2 --quiet --offline -p qualia-gates -- journal --repo superposition/qualia --pr 261
journal-gate: roles OK - accuracy=approve(1), teaching=approve(0), style=approve(0)
journal-gate: entry source: docs/figures/the-fly-brain-on-the-robot/README.md at the PR head b61b3fd708c4 (the entry as it exists at the head, not the diff's added lines)
journal-gate: OK

cargo run -j 2 --quiet --offline -p qualia-gates -- mission --self-test
mission-check: self-test OK (40 cases)
```

Those commands exited 0. Bare `journal` was also tried: it exited 1 because no entry PR was supplied;
the successful run names the existing fly-driving article's PR. The mission check is its offline
self-test, not a live mission submission. The full released-table import test was not rerun. No
WHEA-Logger events were returned for the queried 15-minute window containing these builds and checks;
the persisted fault stamp was still `2026-09-11T06:27:43.3860000Z`.

## Acceptance still blocked

The operator has not yet said to retry the robot's drive acknowledgement. **No new zero-speed probe,
authorize, drive, stop, estop or reset request was made.** No bearer was fetched or read, so this run
has no new bearer-grep proof to claim. No board lease or board job was taken and no window was opened.

The last measured refusal remains the historical line in
[`../leash-drive/transport-http-live.log`](../leash-drive/transport-http-live.log):

```text
POST http://192.168.55.1:8000/motors/drive refused HTTP 400: {"error":"runtime v2 Waveshare acknowledgement timed out","ok":false}
```

That is a quote from the previous run, not a fresh measurement. #262 stays open. On the operator's
word, probe zero first; if accepted, demonstrate the bounded low-speed run and the deadman with
leash's own applied-action evidence and telemetry. The estop latch/refusal/resume demonstration also
requires the operator to choose how the latch will be cleared. Local frame evidence does not satisfy
any of those three robot demonstrations or the board deployment requirement.
