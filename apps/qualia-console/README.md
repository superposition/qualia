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

Configuration is three environment variables and nothing else:

|Variable|Meaning|Unset behaviour|
|---|---|---|
|`QUALIA_AGENT_URL`|the agent base URL|`http://127.0.0.1:8080`|
|`QUALIA_SHM_NAME`|the shared region to attach|the belief, world and telemetry views report "region unattached"|
|`QUALIA_EVIDENCE_DIR`|the directory holding `*.mcap`|the evidence view reports the unset variable|

There is no subnet autodiscovery and no host literal in the source. When no agent answers, the console
renders the committed fixture `tests/fixtures/braid-state.json` and names the reason in the status
line rather than showing an empty window.

## Tests

```sh
cargo test -p qualia-console
```

Four named states — `mission_healthy`, `mission_degraded`, `belief_stale`, `evidence_empty` — are
driven from that same fixture, asserted by accessible label, and written as image snapshots under
`tests/snapshots/`. To accept an intentional visual change, re-run with `UPDATE_SNAPSHOTS=1`.

`tests/evidence.rs` covers the directory scan against a temporary directory, and
`tests/shm_views.rs` covers the telemetry read path against a region it creates itself; neither needs
a running stack.
