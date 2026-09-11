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

It is built against [`docs/frontend-lessons.md`](../../docs/frontend-lessons.md), which records what
five existing front ends taught; every design decision in the source cites the lesson it comes from.

## Running

```sh
cargo run -p qualia-console
```

Configuration is six environment variables and nothing else:

|Variable|Meaning|Unset behaviour|
|---|---|---|
|`QUALIA_AGENT_URL`|the agent base URL|`http://127.0.0.1:8080`|
|`QUALIA_SHM_NAME`|the shared region to attach|`/qualia_body`, the arena the manifest and every region-opening runner name|
|`QUALIA_STACK_MANIFEST`|a deployment's stack manifest; the Telemetry rows become its sensing runners, in its order|every sensing slot the console reads gets a row|
|`QUALIA_EVIDENCE_DIR`|the directory holding `*.mcap`|`artifacts/mcap`, a console-only convenience until a manifest or runner names an evidence root|
|`QUALIA_FLY_PRIOR_PATH`|the connectome prior directory the Brain view draws|the reference prior committed under `assets/brain/prior/`|
|`QUALIA_FLY_COUPLING_SCALE`|the coupling dial the Brain view marks on its time axis (T30)|read from the stack manifest's `env`, else absent|

The Telemetry view's rows are the ABI's sensing slots — `qualia-lidar`, `qualia-camera` and
`qualia-vslam` — one row each, carrying the newest frame its publisher wrote or an explicit
`no frame published`. That row set is the console's own and does not depend on a manifest naming a
sensing runner: the product's default stack (`config/stack-manifest.default.json`) declares none, so
deriving the table from it would leave the panel empty while the region the console is attached to
holds frames. A deployment manifest named by `QUALIA_STACK_MANIFEST` narrows the rows to the sensing
runners that stack declares, in the manifest's own order; a stack that declares none shows the honest
empty table.

There is no subnet autodiscovery and no host literal in the source. When no agent answers, the console
renders the committed fixture `tests/fixtures/braid-state.json` and names the reason in the Mission
window's banner rather than showing an empty window.

The binary's wgpu backends (`dx12`, `gles`, `metal`, `vulkan`) are declared on this crate's own
`wgpu` dependency, not only on the `egui_kittest` dev-dependency: `cargo test` unifies dev-dependency
features but `cargo run`/`cargo build` do not, so without them a plain build panicked before its
window opened.

## Tests

```sh
cargo test -p qualia-console
```

Seven named states — `mission_healthy`, `mission_degraded`, `belief_stale`, `evidence_empty`,
`world_fresh`, `brain_fresh` and `default_arrangement` — are asserted by accessible label and written
as image snapshots under `tests/snapshots/`. The first four are driven from that same fixture;
`world_fresh` opens a fresh shared region and pins that an attached but never-written World region
reads `pose: no fix`, `map: not published` and `voxels: not published` instead of zeroes;
`brain_fresh` publishes one fly-model state, the belief slots and one lidar scan into a fresh region
and pins the belief-lit graph, the matrices and the braid marker; `default_arrangement` pins the
opening picture, all six floating windows at their grid positions. To accept an intentional visual change, re-run with
`UPDATE_SNAPSHOTS=1`.

The snapshot harness drives `egui_kittest` with the wgpu backend (`.wgpu()`), so the test host needs
a GPU-capable adapter; development and verification run on the 4090. The snapshots assert no
wall-clock budget; the one timing measurement lives apart from them, in `tests/brain_frame.rs`, on a
synthetic 2048-type prior, so a loaded host cannot make a snapshot flake.

`tests/evidence.rs` covers the directory scan against a temporary directory, `tests/stack.rs` covers
the sensing runners a named stack contributes, and `tests/shm_views.rs` covers the region read paths
and the Telemetry rows — the ABI's slots, and the rows a named stack declares — against regions it
creates itself; none needs a running stack.
