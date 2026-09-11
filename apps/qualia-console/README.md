# qualia-console

The braid's operator console: one native `egui` + `eframe` binary showing five views over one
state — **Mission** (the braid the agent reports on `GET /braid`), **Belief** (the belief layers in
the shared region), **World** (pose, map and voxels), **Evidence** (sealed MCAP segments, quarantined
partials and the belief ledger) and **Telemetry** (the newest frame each sensing runner published).

It is built against [`docs/frontend-lessons.md`](../../docs/frontend-lessons.md), which records what
five existing front ends taught; every design decision in the source cites the lesson it comes from.

## Running

```sh
cargo run -p qualia-console
```

Configuration is four environment variables and nothing else:

|Variable|Meaning|Unset behaviour|
|---|---|---|
|`QUALIA_AGENT_URL`|the agent base URL|`http://127.0.0.1:8080`|
|`QUALIA_SHM_NAME`|the shared region to attach|`/qualia_body`, the arena the manifest and every region-opening runner name|
|`QUALIA_STACK_MANIFEST`|the stack manifest that declares the runners|`config/stack-manifest.default.json`, compiled into the binary|
|`QUALIA_EVIDENCE_DIR`|the directory holding `*.mcap`|`artifacts/mcap`, a console-only convenience until a manifest or runner names an evidence root|

The Telemetry view's rows are exactly the sensing runners the stack manifest declares
(`qualia-lidar`, `qualia-camera`, `qualia-vslam` when it names them); the console keeps no runner
list of its own.

There is no subnet autodiscovery and no host literal in the source. When no agent answers, the console
renders the committed fixture `tests/fixtures/braid-state.json` and names the reason in the status
line rather than showing an empty window.

## Tests

```sh
cargo test -p qualia-console
```

Five named states — `mission_healthy`, `mission_degraded`, `belief_stale`, `evidence_empty`,
`world_fresh` — are asserted by accessible label and written as image snapshots under
`tests/snapshots/`. The first four are driven from that same fixture; `world_fresh` opens a fresh
shared region and pins that an attached but never-written World region reads `pose: no fix`,
`map: not published` and `voxels: not published` instead of zeroes. To accept an intentional visual
change, re-run with `UPDATE_SNAPSHOTS=1`.

The snapshot harness drives `egui_kittest` with the wgpu backend (`.wgpu()`), so the test host needs
a GPU-capable adapter; development and verification run on the 4090. Nothing in the suite asserts a
wall-clock frame budget, so a loaded host does not make the snapshots flake.

`tests/evidence.rs` covers the directory scan against a temporary directory, `tests/stack.rs` covers
the manifest-derived runner set, and `tests/shm_views.rs` covers the region read paths against regions
it creates itself; none needs a running stack.
