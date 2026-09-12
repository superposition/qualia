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

Mission-broker wiring, checked with the bearer the broker reads:

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

The same request **without** any `Authorization` header answers 422 with the identical validation
error, so on loopback the route admits the caller before any bearer check — recorded as a finding,
not as proof the token file is required. The positive control is valid either way: the route is
reachable, auth (whatever admits it) passed, and validation ran.

## The capture

- **`live-hud-live.png` — the figure, rendered headlessly.** No window on any desktop:
  `cargo run -p qualia-console --example live_hud_evidence` samples the live region and the live
  agent through the console's own read paths and draws the real `render_view` through
  `egui_kittest`'s wgpu backend. It printed:

  ```text
  live_hud_evidence: region /qualia_body, 3 live runner panel(s): qualia-camera 2.55 Hz, qualia-health 9.96 Hz, qualia-vision 4.99 Hz
  live_hud_evidence: no producer: qualia-l0-superposition, … qualia-l6-semantic, qualia-agent, qualia-lidar, qualia-vslam
  Updated snapshot: live_hud_evidence.png
  ```

  The figure shows Mission live against `https://127.0.0.1:8080`, the region panels (Belief, World,
  Telemetry, Brain) reading `/qualia_body`, and the three HUD panels with those real rates.

- **`live-hud.png` — historical.** The windowed capture from earlier in the session (private
  `/qualia_t60_body` region, before the no-visible-window directive). Kept only as the record of the
  windowed surface: `qualia-camera` live 2.61 Hz / 121.49 KiB/s / 343 ticks / 0 errors,
  `qualia-vision` live 4.99 Hz / 715 ticks / 0 errors / objects 2 / frames 714, and the Telemetry row
  `qualia-camera | 640x480 luma 0.46±0.16 | 323 ms ago` — a real frame.

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
