# T60 — the live HUD

Ticket [#241](https://github.com/superposition/qualia/issues/241) (Step 60). This is a **live run**
capture, not a profile: the console reading real producers through binary stats frames, the agent
answering `GET /braid` over its own TLS certificate, and a headless render of the live state.

## The operator's commands

```console
# 1. build the console — this branch's client trusts the agent's certificate
cargo build -p qualia-console

# 2. the single agent on 8080 (from-main build), its certificate, and the mission-broker bearer file
QUALIA_WEB_PORT=8080 QUALIA_SHM_NAME=/qualia_body QUALIA_SHM_AUTOCREATE=1 \
  QUALIA_TLS_DIR=$HOME/.qualia_tls DEEPSEEK_API_KEY=… \
  QUALIA_MISSION_BROKER_TOKEN_FILE=C:/tmp/qualia-mission-broker.token \
  target/debug/qualia-agent.exe

# 3. the console: live braid over TLS, region panels live, a HUD panel per producer
QUALIA_AGENT_URL=https://127.0.0.1:8080 QUALIA_AGENT_TLS_DIR=$HOME/.qualia_tls \
  target/debug/qualia-console.exe
```

Step 1 matters: the operator's current window is a pre-CA build, so it shows the degraded Mission
banner against this TLS agent and must be replaced from this branch. Step 3 needs **no**
`QUALIA_SHM_NAME`: the stack below publishes into the default `/qualia_body`.

## What ran

Workstation, 2026-09-12 01:19–01:45 local, region **`/qualia_body`** (the default; `/qualia_t60_body`
was used earlier in the session and is history — the final run is on the default so the console needs
no env var).

| Process | Command / env | What it produced |
|---|---|---|
| supervisor | `target/debug/qualia-init.exe` with `QUALIA_LOG_DIR=C:/tmp/Impl241LiveHud/logs-default` | created the arena + the stats region; spawned the manifest runners it could find |
| health | spawned by the supervisor | 10 Hz `HealthReport` frames; telemetry frame `qualia-health` |
| vision | spawned by the supervisor (no `GEMINI_API_KEY`) | offline loop: world model, voxels, thoughts, **5 Hz** |
| camera | `target/debug/qualia-camera.exe` with `QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg` | **real robot frames** off the leash MJPEG stream |
| agent | from-main `target/debug/qualia-agent.exe`, `QUALIA_WEB_PORT=8080 QUALIA_SHM_NAME=/qualia_body QUALIA_SHM_AUTOCREATE=1 QUALIA_TLS_DIR=C:/Users/ericm/.qualia_tls DEEPSEEK_API_KEY=… QUALIA_MISSION_BROKER_TOKEN_FILE=C:/tmp/qualia-mission-broker.token` | `GET /braid` over TLS |

Evidence that the region is the default one and the producers are live there:

```text
qualia-vision: opening shm '/qualia_body'
qualia-vision: WARNING: GEMINI_API_KEY not set
qualia-vision: offline tick 180, 2 objects, brightness=0.00
```

The agent's own log lines (from-main build):

```text
qualia-agent: reusing TLS cert at C:/Users/ericm/.qualia_tls/cert.pem
qualia-agent: IGL dashboard at https://0.0.0.0:8080  (web dir: ./web/public)
qualia-agent: SHM stream at  wss://0.0.0.0:8080/ws
qualia-agent: WebRTC signal  wss://0.0.0.0:8080/signal
qualia-agent: TLS cert at    C:/Users/ericm/.qualia_tls/cert.pem
```

Live braid, over the deployment certificate:

```console
$ curl -sS --cacert C:/Users/ericm/.qualia_tls/cert.pem https://127.0.0.1:8080/braid
{"schema_version":"qualia.braid-state.v1","generation":0,"session_id":"","open_missions":0,"last_promotion_ns":0,"last_quarantine_ns":null}
$ curl -s -o /dev/null -w "%{http_code}" --cacert …/cert.pem https://127.0.0.1:8080/braid
200
```

Exactly one agent listener on 8080 (`netstat`, quoted):

```text
  TCP    0.0.0.0:8080           0.0.0.0:0              LISTENING       43984
  TCP    0.0.0.0:18081          0.0.0.0:0              LISTENING       12232
```

`18081` is another session's leftover process (pid 12232): my kill was refused (`Access is denied`)
and Main was killing it from an admin shell; it serves an arena page, not `/braid`. `8080` (pid
43984) is the one agent this run used.

Mission-broker wiring: the route is reachable, and a malformed body is refused by the deserializer
before the handler body runs:

```console
$ TOKEN=$(tr -d '\r\n' < C:/tmp/qualia-mission-broker.token)
$ curl -sS --cacert …/cert.pem -X POST https://127.0.0.1:8080/mission-control/envelopes \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -d '{"not":"an envelope"}'
Failed to deserialize the JSON body into the target type: not: unknown field `not`, expected one of
`schema_version`, `broker_id`, `producer_epoch`, `sequence`, `mission_id`, `idempotency_key`,
`command`, `issued_at_ms`, `deadline_ms`, `objective`, `constraints`, `evidence_refs`,
`fly_governed` at line 1 column 6
http=422
```

That `422` says nothing about auth. `envelope_post` takes `Json(envelope): Json<MissionEnvelopeV1>`
as an extractor argument (`runners/agent/src/mission_control.rs:466-477`), which axum runs **before**
the handler body, so a malformed body answers `422` with or without a bearer.

**Auth on this route is enforced, and the first reading of the `422` here was wrong.** Three probes
with a **well-formed** `MissionEnvelopeV1` — one that deserializes, so the extractor cannot answer
for the handler — sent against the live agent on 2026-09-12 by a client that read the token out of
the file instead of carrying it in the command line:

| probe | the agent's answer |
|---|---|
| no `Authorization` header | `401` `{"error":"authentication_required","schema_version":"qualia.auth-error.v1"}` |
| a wrong bearer | `401`, the same body |
| the bearer in `C:/tmp/qualia-mission-broker.token` | `400` `{"error":"mission t60-rev-probe-does-not-exist: cannot pause an unknown mission"}` |

The third answer is the handler's own: `command: "pause"` for a mission that does not exist is
refused by `MissionControl::ingest`, which can only run once `authorize` has passed. So
`401 → 401 → the handler's own error` **is** the auth decision, and the third probe changes no
mission state. Two code facts behind it:

- `runners/agent/src/auth.rs:80-82` does not extend the loopback bypass to this scope:
  `let loopback_bypass = remote_ip.is_loopback() && self.allow_loopback && !matches!(scope, AuthScope::Admin | AuthScope::MissionBroker);`
  and `envelope_post` authorizes `AuthScope::MissionBroker`.
- the extractor ordering above is what makes a malformed-body `422` look like a bypass.

Probe shape for the next reader: send a body that **deserializes**. A bare `400` with
`content-length: 0` and `connection: close` from a shell client is a broken probe, not something the
agent answered — re-run it with the token read from its file before concluding anything about auth.

## The capture

- **`live-hud-live.png` — the figure, rendered headlessly.** No window on any desktop:
  `cargo run -p qualia-console --example live_hud_evidence` samples the live region and the live
  agent through the console's own read paths and draws the real `render_view` through
  `egui_kittest`'s wgpu backend. Re-rendered for #247 (T64) at 1900x1060 with the three panels in a
  **two-column** right margin (a single column needed a 1500 px page before every row fit).

  The committed image is the re-render on the **rebased** tree (2026-09-12 02:28 local; branch
  rebased onto `main` `4f2a8ab`, D-026 below) — 90,569 px differ from the 02:11 render, bbox
  (87, 45)–(1898, 843), because the 02:11 figure was drawn before main's T56 brain view and theme
  landed and so showed a Brain view main had already replaced. It printed:

  ```text
  live_hud_evidence: region /qualia_body, 3 live runner panel(s)
    qualia-camera | 2.50 Hz | publishing | updated 102 ms ago | uptime 20 m 21 s | seq 6106.000, frame bytes 36036.000, failures 0.000
    qualia-health | 9.97 Hz | publishing | updated 58 ms ago | uptime 20 m 30 s | layers 8.000, l0 vfe 0.000, l0 comp 0.000, l0 cycle us 0.000
    qualia-vision | 4.99 Hz | publishing | updated 52 ms ago | uptime 20 m 30 s | objects 2.000, brightness 0.000, frames 6136.000, scene objs 2.000
  live_hud_evidence: no producer: qualia-l0-superposition, qualia-l1-belief, qualia-l2-belief, qualia-l3-belief, qualia-l4-behavior, qualia-l5-behavior, qualia-l6-semantic, qualia-agent, qualia-lidar, qualia-vslam
  Updated snapshot: tests/snapshots\live_hud_evidence.png
  ```

  The figure shows Mission live against `https://127.0.0.1:8080`, the region panels (Belief, World,
  Telemetry, Brain) reading `/qualia_body`, and the three HUD panels with those real rates, the
  `uptime` each runner's own `started_at_ns` gives, and every row — including all four labelled
  values — inside its panel.

- **`live-hud.png` — the pre-#247 capture of the windowed surface.** The windowed capture from
  earlier in the session (private `/qualia_t60_body` region, before the no-visible-window directive),
  kept as the record of the windowed surface: `qualia-camera` live 2.61 Hz / 121.49 KiB/s / 343 ticks
  / 0 errors, `qualia-vision` live 4.99 Hz / 715 ticks / 0 errors / objects 2 / frames 714, and the
  Telemetry row `qualia-camera | 640x480 luma 0.46±0.16 | 323 ms ago` — a real frame.

  It is the **before** half of #247's label fix: it was drawn by the build whose vision panel reads
  `scene object` (the wire field cut `scene objects` at 12 bytes) and whose health panel, clipped at
  300 px, never reached `l0 compressi`. Both are fixed and re-rendered in `live-hud-live.png`; D-023
  forbids opening a window, so this capture cannot be re-shot and is left as the dated record rather
  than passed off as current.

## #247 (T64) — the four honesty fixes, and how each was checked

Ticket [#247](https://github.com/superposition/qualia/issues/247), from the #245 correctness review.
Branch `ticket/T64-hud-defects`; the four fixes are commit `13c86cf`, and this figure and section ride
with the same branch. The `/qualia_body` producers were restarted
from that build so the live frames carry the fixes: the old PIDs were killed in one pass
(`taskkill /F /PID 18488 /PID 11028 /PID 27440 /PID 14484` — T60's supervisor and its health/vision
children, plus the hand-started camera), leaving one publisher per runner, and
`target/debug/qualia-init.exe` from the T64 worktree then created the arena and spawned health and
vision while `target/debug/qualia-camera.exe` took
`QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg`. The agent on 8080 was left
running: it is the console's `/braid`, not a producer.

| defect | before | after | where it is visible |
|---|---|---|---|
| `started_at_ns` never written | every frame carried 0; no panel could show a start | `StatsWriter::from_region` stamps its attach instant; the panel reads `uptime` | the live rows above: `uptime 2 m 06 s`, `2 m 15 s` |
| `PUBLISHING` never cleared | `hud.rs:328`'s `!row.publishing` branch was dead; a stopped runner kept its last state | `impl Drop for StatsWriter` clears the flag and stamps the frame, so the panel reads `stopped`; a runner that is killed cannot run `Drop` and still ages out to `stale` | the probe below: `0x0001` -> `0x0000` |
| 12-byte labels cut mid-word | `l0 compressi`, `scene object`, and lidar's `buffered poi` | the field stays 12 bytes — it is ABI under `RUNNER_STATS_VERSION` — and the three names move: `l0 comp`, `scene objs`, `buffered pts` | the live rows above |
| README claimed "the last two minutes" | `HISTORY_LEN` is 120 samples and `POLL_INTERVAL` is 250 ms (`poller.rs:19`), so the window is 30 s | the README says the last half-minute, the same figure the constant's own doc names | `apps/qualia-console/README.md` |

The flag clearing is the one claim a reader cannot check against a running stack, since it needs a
writer to be dropped; it was measured with a throwaway example that attached to its own region,
published once, and dropped the writer. The example was deleted after the run:

```text
$ cargo run -p qualia-console --example stop_probe
probe: publishing flags=0x0001 publishing=true label0="l0 comp"
probe: dropped   flags=0x0000 publishing=false label0="l0 comp" stamped_at_ns=1789193475679664200
```

The console's `StatsRow.publishing` is exactly that flag, so the second line is the panel's `stopped`
state; the `label0` round-trip is the third fix — `l0 comp` is what comes back out of the 12-byte
field.

**The seven committed snapshots were re-blessed.** `cargo test -p qualia-console -j 2` failed 7/7
before this ticket's first commit, by exactly 152 px in the same 23x11 box at (218, 16) — the `HUD`
menu button that T60 added to `theme::menu_strip` (`cc9eb6e`, 2026-09-12) while the snapshots were
last blessed by T51 (`a65f94f`, 2026-09-11). Every diff was measured to be that button and nothing
else, so `UPDATE_SNAPSHOTS=1 cargo test -p qualia-console -j 2` was run once (rc 0), and the suite
then passed without it (rc 0, 7/7 snapshots, no stray `*.new.png`).

**On the tree rebased onto `main` the seven images are `main`'s, and that is measured, not sided.**
Rebased onto `main` `4f2a8ab` the branch conflicted on the same seven PNGs (and on
`examples/live_hud_evidence.rs`, where `main`'s four-line `coach:` field in `Sample` was re-added to
the branch's rewrite). `main`'s bytes were taken as the starting point, then all seven were
**re-rendered on the rebased tree** — `UPDATE_SNAPSHOTS=1 cargo test -p qualia-console --test
snapshots -j 2` (rc 0, 7/7) — and the write was made provable rather than assumed by first moving the
seven PNGs out of `tests/snapshots/` and re-running: the render recreated all seven, byte for byte
identical to `main`'s blobs (`1c8bec68`, `807f2714`, `fa72d545`, `d10d546f`, `c4be5a6d`, `737a3f25`,
`02e9c706`).

That is the point D-026 names: siding with `539d188`'s binaries would have reverted main's render.
Measured against `main`'s committed images, the branch's seven differed by `belief_stale` 73,967 px
(bbox 441,44–1273,189), `brain_fresh` 132,009 px (857,44–1273,813), `default_arrangement` 109,475 px
(857,44–1273,813), `evidence_empty` 73,942 px (25,44–1273,437), `mission_degraded` 73,962 px
(25,44–1273,189), `mission_healthy` 73,962 px (25,44–1273,189) and `world_fresh` 23,657 px
(857,44–1273,189) — all of T56's brain re-render plus main's theme. And nothing the branch drew is
lost: `086017e` → `539d188` is still exactly 152 px in the one 23x11 box at (218, 16)–(240, 26), the
`HUD` menu button, which `main`'s images already carry (the `086017e` → `main` bbox starts at that
same box). The rebased tree changes no snapshot at all; the suite is green there without
`UPDATE_SNAPSHOTS` (rc 0, 35 tests: lib 16, `brain_frame` 1, `brain_sample` 1, `evidence` 2,
`shm_views` 5, `snapshots` 7, `stack` 3), and a second run left every blob unchanged.

Nothing else changed: the frame layout, `RUNNER_STATS_VERSION`, `RUNNER_STATS_SLOTS` and the region
name are untouched, and the three renamed labels are display names their publishers choose, not a
wire change.

## What is live, and what is a gap

Live: `qualia-camera` (real leash frames, 2.55 Hz), `qualia-vision` (4.99 Hz), `qualia-health`
(9.96 Hz), the agent's `GET /braid` over TLS, and the region-backed views.

Named gaps, by absence of a producer rather than a mocked panel:

- **Belief layers (L0–L6)** — need `cargo build --no-default-features --features cuda -p
  qualia-l0-superposition …`; the default `metal` build panics off macOS, so the supervisor logged a
  `WARNING` per layer. Their telemetry hook is already in `crates/cuda/src/cuda_impl.rs` (`run_layer`),
  so their panels appear the moment they run.
- **Connectome runner (#236)** — not in this tree; the HUD's menu names it `— no producer`.
- **lidar, pose, drive, explore, vslam, map, floor** — wired to publish frames, but no producer here:
  a serial device, an upstream slot or a dataset is missing (leash sensor ingest is #239's crate).
- **The 18081 process** — another session's; one agent instance per the directive means it should be
  killed from that session.

## Launching the producers

```console
# supervisor: arena + stats region + the runners it can find
QUALIA_LOG_DIR=artifacts/logs target/debug/qualia-init.exe
# real frames from the robot's leash camera
QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg target/debug/qualia-camera.exe
```

The console's README documents every panel, every env key and the frame layout.
