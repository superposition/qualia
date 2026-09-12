# qualia-console

The braid's operator console: one native `egui` + `eframe` binary showing six views over one
state — **Mission** (the braid the agent reports on `GET /braid`), **Belief** (the belief layers in
the shared region), **World** (pose, map and voxels), **Evidence** (sealed MCAP segments, quarantined
partials and the belief ledger), **Telemetry** (the newest frame each sensing runner published) and
**Brain** (the fly brain: the connectome prior in its committed 3D layout, its nodes lit by the
belief layers' activity and its edges pulsing by the published fly rate vector, the lidar cloud and
the world voxels, and the per-layer belief matrices).

The six views are floating windows on one page — movable, resizable, closable and arranged in a 3×2
grid so the whole picture is readable at once instead of one view at a time — and the slim strip
at the top carries the `Console` menu that re-opens a closed window and requests an immediate poll.
Over them sits the operator's **HUD**: one floating panel per runner, fed by the binary telemetry
frame that runner publishes beside the shared region (see [The live HUD](#the-live-hud)).

It is built against [`docs/frontend-lessons.md`](../../docs/frontend-lessons.md), which records what
five existing front ends taught; every design decision in the source cites the lesson it comes from.

## Running

```sh
cargo run -p qualia-console
```

Configuration is ten environment variables and nothing else:

|Variable|Meaning|Unset behaviour|
|---|---|---|
|`QUALIA_AGENT_URL`|the agent base URL; `https://` is accepted|`http://127.0.0.1:8080`|
|`QUALIA_AGENT_TLS_DIR`|the directory holding the agent's own `cert.pem`, added as a root when the URL is `https://`|`$HOME/.qualia_tls`, where the agent generates it|
|`QUALIA_SHM_NAME`|the shared region to attach|`/qualia_body`, the arena the manifest and every region-opening runner name|
|`QUALIA_STATS_SHM_NAME`|the runner-telemetry region the HUD reads|`QUALIA_SHM_NAME` plus `_stats`|
|`QUALIA_STACK_MANIFEST`|a deployment's stack manifest; the Telemetry rows become its sensing runners, in its order, and the HUD's gap list is its runner list|every sensing slot the console reads gets a row, and the compiled-in default stack names the gaps|
|`QUALIA_EVIDENCE_DIR`|the directory holding `*.mcap`|`artifacts/mcap`, a console-only convenience until a manifest or runner names an evidence root|
|`QUALIA_FLY_PRIOR_PATH`|the connectome prior directory the Brain view draws|the reference prior committed under `assets/brain/prior/`|
|`QUALIA_FLY_COUPLING_SCALE`|the coupling dial the Brain view marks on its time axis (T30)|read from the stack manifest's `env`, else absent|
|`QUALIA_CONSOLE_LAYOUT`|where the HUD's panel positions and open/closed state are remembered|the per-user state directory (`%LOCALAPPDATA%\qualia\console-layout.bin` on Windows, `$XDG_STATE_HOME/qualia/console-layout.bin` or `$HOME/.local/state/qualia/console-layout.bin` elsewhere)|
|`QUALIA_COACH_URL`|the mission broker's status surface, read by the Coach panel (T63)|`http://127.0.0.1:8091`, the broker's own `QUALIA_COACH_STATUS_PORT` default|

The Telemetry view's rows are the ABI's sensing slots — `qualia-lidar`, `qualia-camera` and
`qualia-vslam` — one row each, carrying the newest frame its publisher wrote or an explicit
`no frame published`. That row set is the console's own, not the compiled-in stack's: with no
manifest named every slot gets a row, because a stack manifest that declares no sensing runner must
not narrow the rows to nothing while the region the console is attached to holds frames. (The
shipped default, `config/stack-manifest.default.json`, now names `qualia-camera` and
`qualia-leash-sensors`, but it is not the console's row set.) A deployment manifest named by
`QUALIA_STACK_MANIFEST` narrows the rows to the sensing runners that stack declares, in the
manifest's own order; a stack that declares none shows the honest empty table.

There is no subnet autodiscovery and no host literal in the source. When no agent answers, the console
renders the committed fixture `tests/fixtures/braid-state.json` and names the reason in the Mission
window's banner rather than showing an empty window. The region-backed panels (Belief, World,
Telemetry, Brain and the HUD) read the shared region whether or not the agent answers, so a dead
braid no longer blanks a live stack.

## The Coach panel is a second source

The **Coach** panel (`views/coach.rs`, T63) shows the mission broker's model state — configured,
no key, timeout, error, last latency, last error — and the newest decisions with their provenance
(model id, response id, token usage, `llm_priors_ablated`), read from the broker's read-only
loopback surface `GET /coach` (`qualia.coach-state.v1`, default `http://127.0.0.1:8091`,
`QUALIA_COACH_URL` to move it). A broker that is not running degrades the panel with one named line
("coach broker not running at …") — the console never draws a decision it did not receive.

This is deliberately the console's **only** second source beside the agent, and it is temporary. The
agent owns missions; the durable home of a coach decision should be the agent's world-model surface
(`/world-model/decisions`, a `503` stub in this build), and `runners/mission-broker/README.md`
records the same direction from the broker's side. When the agent grows that stream, the Coach panel
should read it and `GET /coach` should be retired — do not grow a third way to see the same thing.

The binary's wgpu backends (`dx12`, `gles`, `metal`, `vulkan`) are declared on this crate's own
`wgpu` dependency, not only on the `egui_kittest` dev-dependency: `cargo test` unifies dev-dependency
features but `cargo run`/`cargo build` do not, so without them a plain build panicked before its
window opened.

## The live HUD

Every runner that does work publishes a fixed-width binary stats frame
(`qualia_types::RunnerStats`) into a small stats region beside the data arena — the arena's own name
plus `_stats` (`/qualia_body_stats`), or `QUALIA_STATS_SHM_NAME` when a deployment names one. The
framing is the house one: a magic and a version, then fixed-width little-endian fields, no JSON, and
a seqlock so a reader never sees a half-written frame. A frame carries what an operator needs at a
glance — ticks/s or frames/s (`rate_milli_hz`), bytes/s, backlog, the last values the runner emitted
and their labels, the error count, and the time of the last update. `qualia-init` creates the region
with the arena; a producer started by hand creates it itself; a console that finds none says so
rather than drawing zeroes.

The HUD is one floating window per runner that has published a frame: movable, resizable,
collapsible, and remembered between runs. Each panel shows the producer's own measured rate (its
last one-second window, so a 4 Hz console still shows a 30 Hz camera as 30 Hz), a rate bar and a
sparkline of the last two minutes, then backlog, ticks, errors, the age of the last update, and the
runner's last values by label. `Ctrl+H` shows or hides the whole HUD from anywhere in the window;
the `HUD` menu does that, toggles one panel at a time, and lists the runners the stack declares that
have published no frame as `— no producer`, so a missing producer is named rather than drawn as an
empty panel.

Producers wired today: `qualia-camera`, `qualia-lidar`, `qualia-pose`, `qualia-drive`,
`qualia-explore`, `qualia-arena-recorder`, `qualia-health`, `qualia-vision` and the belief layers
(`crates/cuda`'s `run_layer`, the backend the board runs; the macOS `qualia-metal` backend does not
publish yet). `qualia-init` creates the region. A runner not in that list has no frame: the
connectome runner (#236) is the richest one still missing, and the listing above is the honest gap
list the menu shows.

### Launching against a live stack

The HUD reads a producer, not a fixture, so something has to be writing. On this workstation the
minimal real stack — no robot, no board — is the supervisor plus the two hardware-free producers:

```sh
cargo run -p qualia-init                                   # arena, control endpoint, stats region
cargo run -p qualia-vision                                 # world model + voxels + thoughts, 5 Hz
cargo run -p qualia-health                                 # every layer slot, 10 Hz
cargo run -p qualia-console                                # the console, HUD panels open
```

`qualia-vision` with no `GEMINI_API_KEY` runs its offline loop, which is a real production path: it
reads the senses layer and writes the world model, the voxel grid and thought lines. `qualia-health`
is the stack's health tap; its panel is the read rate of the arena. For real frames from the robot's
own camera over the USB link, point the camera runner at the leash's MJPEG stream (the address below
is the one measured on this host: 31 complete JPEGs in 6 s):

```sh
QUALIA_CAMERA_STREAM_URL=http://192.168.55.1:8000/camera/stream.mjpg cargo run -p qualia-camera
```

`cargo run -p qualia-init` with the default manifest starts the whole declared stack instead —
including the belief layers, which publish a frame per layer at their `default_params` cadence
(1000 Hz for L0 down to 0.05 Hz for L6). Those layers need a compute backend compiled in, so build
them for the target: on the board `cargo build --no-default-features --features cuda -p
qualia-l0-superposition …`, on macOS the default `metal` feature.

The Mission view is the one panel that needs the agent: it polls `GET /braid`, and the agent serves
TLS only. The console validates certificates (rustls, no insecure bypass — source 2's lesson), so an
`https://` agent URL is trusted only through the certificate the deployment names:
`QUALIA_AGENT_TLS_DIR/cert.pem` (default `$HOME/.qualia_tls`, where the agent writes its generated
certificate) is added as a root. With no such certificate the braid reads degraded and Mission names
the reason; every region-backed panel stays live either way.

## Tests

```sh
cargo test -p qualia-console
```

Seven named states — `mission_healthy`, `mission_degraded`, `belief_stale`, `evidence_empty`,
`world_fresh`, `brain_fresh` and `default_arrangement` — are asserted by accessible label and written
as image snapshots under `tests/snapshots/`. The first four are driven from that same fixture;
`world_fresh` opens a fresh shared region and pins that an attached but never-written World region
reads `pose: no fix`, `map: not published` and `voxels: not published` instead of zeroes;
`brain_fresh` publishes one fly-model state, the belief slots, a decimated weight pattern and one
lidar scan into a fresh region and pins the belief-lit graph, the matrices and the braid marker;
`default_arrangement` pins the opening picture, all six floating windows at their grid positions. To
accept an intentional visual change, re-run with `UPDATE_SNAPSHOTS=1`.

The image compare allows a bounded number of differing pixels on the Linux board, because its wgpu
backend rasterizes a handful of anti-aliased edges differently (6 observed) while the committed PNGs
are rendered here; on this host it allows none. It is a floor for rasterizer noise, not a licence to
drift: reverting the node-intensity source moves hundreds of pixels and still fails.

The snapshot harness drives `egui_kittest` with the wgpu backend (`.wgpu()`), so the test host needs
a GPU-capable adapter; development and verification run on the 4090. The snapshots assert no
wall-clock budget; the one timing measurement lives apart from them, in `tests/brain_frame.rs`, on a
synthetic 2048-type prior, so a loaded host cannot make a snapshot flake.

`tests/evidence.rs` covers the directory scan against a temporary directory, `tests/stack.rs` covers
the sensing runners a named stack contributes, and `tests/shm_views.rs` covers the region read paths
and the Telemetry rows — the ABI's slots, and the rows a named stack declares — against regions it
creates itself; none needs a running stack.
