# T60 — the live HUD

Ticket [#241](https://github.com/superposition/qualia/issues/241) (Step 60). This is a **live run**
capture, not a profile: the console reading real producers through the binary stats frames, and the
agent answering `GET /braid` over its own TLS certificate.

## What ran

All processes on this workstation, 2026-09-12 01:19–01:3x local, region `/qualia_t60_body` (a
private arena, so the run could not disturb any other worktree's `/qualia_body`), stats region
`/qualia_t60_body_stats`.

| Process | Command / env | What it produced |
|---|---|---|
| supervisor | `target/debug/qualia-init.exe` with `QUALIA_SHM_NAME=/qualia_t60_body QUALIA_LOG_DIR=C:/tmp/Impl241LiveHud/logs` | created the arena and the stats region; spawned the manifest's runners it could find |
| health | spawned by the supervisor (`qualia-health`) | 10 Hz `HealthReport` frames over every layer slot; telemetry frame `qualia-health` |
| vision | spawned by the supervisor (`qualia-vision`, no `GEMINI_API_KEY`) | offline loop: world model, voxels, thought lines, **5 Hz** |
| camera | `target/debug/qualia-camera.exe` with `QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg` | **real robot frames** off the leash MJPEG stream (~5.2 fps at the source; the runner publishes 2–3 Hz after decode) |
| agent | `qualia-agent.exe` with `QUALIA_WEB_PORT=8080 QUALIA_SHM_NAME=/qualia_t60_body QUALIA_SHM_AUTOCREATE=1 QUALIA_TLS_DIR=C:/Users/ericm/.qualia_tls DEEPSEEK_API_KEY=…` | `GET /braid` over TLS |

The supervisor's own log, quoted:

```text
[init] Creating shared memory '/qualia_t60_body'...
[init] Shared memory created: 64 MB
[init] Stats region created: '/qualia_t60_body_stats'
[init] Spawning qualia-l0-superposition...
[init] WARNING: qualia-l0-superposition: The system cannot find the file specified. (os error 2)
... (l1..l6, qualia-agent: no binary on this host)
[init] Spawning qualia-health...
[init]   pid 45524
[init] Spawning qualia-vision...
[init]   pid 43672
[init] 2 runners launched.
```

The camera's log (real frames, real luminance):

```text
qualia-camera: consuming live MJPEG stream http://192.168.55.1:8000/camera/stream.mjpg; publishing at most every 100ms
qualia-camera: frame_seq=2 src=640x480 thumb=64x48 luma_mean=0.460 luma_std=0.163 quality=usable
```

Vision's child log:

```text
qualia-vision: opening shm '/qualia_t60_body'
qualia-vision: WARNING: GEMINI_API_KEY not set
qualia-vision: Running in offline mode — synthetic world model only
qualia-vision: offline tick 60, 2 objects, brightness=0.00
```

The agent started on the right port with the operator's certificate, and `GET /braid` answers a live
braid (HTTP 200, verified with the CA):

```console
$ curl -sS --cacert C:/Users/ericm/.qualia_tls/cert.pem https://127.0.0.1:8080/braid
{"schema_version":"qualia.braid-state.v1","generation":0,"session_id":"","open_missions":0,"last_promotion_ns":0,"last_quarantine_ns":null}
$ curl -s -o /dev/null -w "%{http_code}" --cacert C:/Users/ericm/.qualia_tls/cert.pem https://127.0.0.1:8080/braid
200
```

```text
qualia-agent: reusing TLS cert at C:/Users/ericm/.qualia_tls/cert.pem
qualia-agent: IGL dashboard at https://0.0.0.0:8080  (web dir: ./web/public)
qualia-agent: TLS cert at    C:/Users/ericm/.qualia_tls/cert.pem
```

## The capture

`live-hud.png` — the console with the six views plus the three HUD panels, on the live region. The
panel readings visible in it (also read back from the panel crops):

| Panel | State | Rate | Bytes/s | Ticks | Errors | Updated | Last values |
|---|---|---|---|---|---|---|---|
| `qualia-camera` | live | 2.61 Hz | 121.49 KiB/s | 343 | 0 | 323 ms ago | — |
| `qualia-health` | live | ≈10 Hz | ≈2.5 KiB/s | — | 0 | fresh | l0 belief cycle |
| `qualia-vision` | live | 4.99 Hz | 0 B/s | 715 | 0 | 23 ms ago | objects 2, brightness 0.000, frames 714, scene objects 2 |

The Telemetry panel showed the camera row with a real frame — `qualia-camera | 640x480 luma
0.46±0.16 | 323 ms ago` — while the lidar and vslam rows honestly said `no frame published`.

**Disclosure:** `live-hud.png` was captured before the operator-directive that no qualia window may
be opened on the operator's desktop (`Write-Host`-free headless runs from here on). It is a real
capture of the live stack, taken while the panels were updating; it is kept as the record of the
windowed surface. The figures and the readings above are reproducible headlessly with
`cargo test -p qualia-console` (`UPDATE_SNAPSHOTS=1` renders the same `render_view` path to PNG
without a desktop window), and the operator's own console is the live one from now on.

## What is live, and what is a gap

Live in this capture: `qualia-camera` (real leash frames), `qualia-vision` (5 Hz), `qualia-health`
(10 Hz), the agent's `GET /braid` over TLS, and the region-backed views (World voxels, Telemetry
camera row, Belief matrices reading `—` because no layer has published).

Named gaps, all by absence of a producer rather than by a mocked panel:

- **Belief layers (L0–L6)** — the binaries need a compute backend compiled in
  (`cargo build --no-default-features --features cuda -p qualia-l0-superposition …`). The default
  `metal` build panics off macOS, so the supervisor logged `WARNING` for each and the stack ran
  without them. Their telemetry hook exists (`crates/cuda/src/cuda_impl.rs`, `run_layer`), so their
  panels appear the moment they run.
- **Connectome runner (#236)** — not landed in this worktree; the HUD names it `— no producer` in
  the menu's gap list once the runner publishes a frame (`StatsWriter::attach(…, "qualia-connectome")`
  at its pump loop).
- **lidar, pose, drive, explore, vslam, map, floor** — wired to publish frames, but no producer here:
  they need a serial device, an upstream slot, or a dataset. `pose`/`map` need lidar scans; the leash
  sensor path is #239's `runners/leash-sensors`.
- **The stale agent on `0.0.0.0:18081`** — Main asked for exactly one agent instance; the process
  (pid 12232) is owned by another session and this shell's kill was refused (`Access is denied`), so
  it is still listening. The 8080 instance above is the one serving the console. Main should
  terminate 18081 from the session that owns it.

## Launching it again

```console
# 1. supervisor: arena + stats region + the runners it can find
QUALIA_SHM_NAME=/qualia_t60_body QUALIA_LOG_DIR=artifacts/logs target/debug/qualia-init.exe
# 2. real frames from the robot's leash camera
QUALIA_SHM_NAME=/qualia_t60_body \
  QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg target/debug/qualia-camera.exe
# 3. one agent, on 8080, over the operator's certificate
QUALIA_WEB_PORT=8080 QUALIA_SHM_NAME=/qualia_t60_body QUALIA_SHM_AUTOCREATE=1 \
  QUALIA_TLS_DIR=~/.qualia_tls DEEPSEEK_API_KEY=… target/debug/qualia-agent.exe
# 4. the console: live braid over TLS, region panels live, HUD panels for every producer
QUALIA_AGENT_URL=https://127.0.0.1:8080 QUALIA_AGENT_TLS_DIR=~/.qualia_tls \
  QUALIA_SHM_NAME=/qualia_t60_body target/debug/qualia-console.exe
```

`QUALIA_AGENT_TLS_DIR/cert.pem` is added as a root certificate (no verification bypass); the
console's README documents every panel and every env key.
