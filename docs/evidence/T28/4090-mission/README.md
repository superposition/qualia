# Step 28 — the 4090 end-to-end mission

Run on the dev host (Windows 11, RTX 4090, `sm_89`) on 2026-09-11, from
`ticket/T28`, with the connectome prior committed at `assets/brain/prior/`.
The driver is [`scripts/mission-4090.ps1`](../../../scripts/mission-4090.ps1) and
the runbook is [`docs/runbooks/mission-4090.md`](../../../runbooks/mission-4090.md).

## Build

```text
cargo build --release -j 2 -p qualia-cli -p qualia-init -p qualia-agent -p qualia-explore
  -p qualia-arena-recorder -p qualia-session -p qualia-mcap -p qualia-health          EXIT=0 (830 s)
cargo build --release -j 2 -p qualia-cuda-service                                     EXIT=0 (147 s)
```

## Run

```text
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/mission-4090.ps1
  -MissionId explore-frontier-1 -SkipBuild -ScratchRoot <scratch>                     MISSION_EXIT=0 (21.8 s)
```

The stack is `qualia run --manifest config/stack-manifest.zero-motion.json` — the same
`qualia-init` every deployment uses. `qualia-cuda-service` opened the CUDA context and reported the
device, so `QUALIA_CUDA_SM=89` took effect:

```text
qualia-cuda-service: listening on 127.0.0.1:46329
qualia-cuda-service: cuda runtime status=ready device='NVIDIA GeForce RTX 4090' sm_89
```

## The four observables

`GET /braid`, before the mission, with it open, and after the cancel:

```json
{"schema_version":"qualia.braid-state.v1","generation":0,"session_id":"","open_missions":0,"last_promotion_ns":0,"last_quarantine_ns":null}
{"schema_version":"qualia.braid-state.v1","generation":0,"session_id":"explore-frontier-1","open_missions":1,"last_promotion_ns":0,"last_quarantine_ns":null}
{"schema_version":"qualia.braid-state.v1","generation":0,"session_id":"explore-frontier-1","open_missions":0,"last_promotion_ns":0,"last_quarantine_ns":null}
```

The mission record ends terminal (`status: cancelled`, `stage: terminal`,
`last_code: cancelled`) with the broker's own `stop_verified`; the mission never entered motion, so
the zero stop needs no transport.

Sealed MCAP (`QUALIA_MCAP_DURATION_SECONDS` bounded-run stop; the recorder's own log line):

```text
qualia-arena-recorder: recording session=explore-frontier-1 sources=unknown:/qualia_zero_motion_fixture(primary) root=<scratch>/arena
qualia-arena-recorder: completed <scratch>/arena/explore-frontier-1.mcap sha256=04e64e3c4c3047ae8870906a1d1558f008960451204e5ebc18294dfaf62e77c8 channels=12
```

`qualia-mcap-inspect <sealed>` replays it: `schema_version: qualia.mcap-inspection.v1`, 12 channels
(`/qualia/action/{applied,clamped,requested}`, `/qualia/belief`, `/qualia/camera`, `/qualia/health`,
`/qualia/jepa`, `/qualia/lidar`, `/qualia/planner`, `/qualia/pose`, `/qualia/prior`, `/qualia/vslam`),
0 messages each, 2395 bytes — a zero-motion history.

`qualia-session list` (one session, carrying the mission id):

```json
[{"id":1,"path":"<scratch>/arena/explore-frontier-1.mcap","filename":"explore-frontier-1.mcap",
  "media_kind":"mcap","analysis_kind":"raw_observation","status":"ready","duration_sec":0.0,
  "imported_at":"2026-09-11T20:31:42Z"}]
```

`qualia-session show --session-id 1` carries the recorder's sealed reference on the `arena_mcap`
stream: `{"schema_version":"qualia.mcap-reference.v1", "sha256":"04e64e3c…", "byte_length":2395,
"channels":[12]}`.

The console's Mission view and the watch Braid line both render this view (`session_id` and
`generation`); the driver's checks read the same `GET /braid` document, and the two front ends'
rendering of it is pinned by `apps/qualia-console/tests/snapshots.rs` (`mission_healthy` asserts the
session value) and `runners/watch/tests/braid.rs`.

## Checks

```text
braid_open_missions_1        : true
braid_open_missions_0        : true
braid_session_is_mission_id  : true
mission_terminal             : true
sealed_segment_exists        : true
sealed_segment_reads         : true
one_session                  : true
session_carries_mission_id   : true
session_has_sealed_evidence  : true

mission: OK
```

The raw driver output (including the full `evidence.json`) stays in the scratch directory named
above; the absolute host paths are elided here.

## Limits

- No plan and no motion: the evidence-grounded planner is not in this build, so the mission parks at
  `awaiting_fresh_evidence` and the operator's cancel ends it. Step 28 asserts the mission lifecycle
  and the evidence, not driving.
- The GUI and the TUI were not confirmed visually by this headless run; their rendering is pinned by
  their own tests and the runbook's manual step.
- The aarch64 build and the board run are Step 29 (T29), a native build on Pinkie (D-018).
