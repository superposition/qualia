# qualia-rerun-bridge

A projection-only bridge from Qualia state into a Rerun recording.

The crate reads typed world-model and sync slices and writes them into an
optional recording; it never mutates canonical state and never contains control
authority. Callers choose the destination with `RerunSinkConfig`:

- `Buffered` — keep the recording in memory and let a viewer connect
- `Save(path)` — stream the recording into an `.rrd` file
- `Connect(url)` — stream into a running viewer or server over gRPC
  (`rerun+http://127.0.0.1:9876/proxy`), which is how the live wire works: the
  viewer is on the desktop, the logger is on the board
- `Disabled` — every call is a no-op, so instrumentation can stay in place

## Usage

1. Build a `QualiaRerunBridge` with `RerunBridgeConfig::default()` or an
   explicit `application_id`/`sink`.
2. Call `log_world_model_projection(tick, ...)` with proposals, coach
   decisions, accepted canonical state, and the operational envelope.
3. Call `log_sync_projection(tick, ...)` with the replica registry,
   materialized state, append log, and op log.
4. Call `send_default_thought_theater_blueprint()` to activate the default
   layout.
5. Call `export_session_replay(frames)` to write a multi-frame timeline into
   the same recording.

For the connectome, the same way round:

1. Call `log_connectome_cloud(cloud)` **once** with the placed nodes and the
   cell-type table: it writes the point cloud and its summary as static data
   (the cloud does not change from tick to tick).
2. Call `log_connectome_tick(tick, projection)` for each tick with that tick's
   firing node ids and wall clock: the firing set is highlighted, its count goes
   on the timeline as a scalar, and the tick's counts go under `stream/summary`.
3. Call `send_default_connectome_blueprint()` to open the 3D cloud beside the
   stream summary and the firing-count curve.

`collected_connectome_cloud_records` and `collect_connectome_tick_records`
expose the record streams without a recording.

`collect_world_model_projection_records` and `collect_sync_projection_records`
expose the record streams without a recording, which is what the tests assert
against.

## Entity paths

The taxonomy is stable and public:

- `qualia/world_model/summary`
- `qualia/world_model/proposals/{id}/{summary,body,geometry}`
- `qualia/world_model/proposals/graph/{nodes/{id},factors/{id}/{edge,summary},summary}`
- `qualia/world_model/canonical/{id}/{summary,body,geometry,lineage}`
- `qualia/world_model/canonical/graph/{nodes/{id},factors/{id}/{edge,summary},summary}`
- `qualia/world_model/canonical/spatial/{objects,regions,nodes,factors}/{id}`
- `qualia/world_model/operational/{pose,nav_goal,route,summary,planner,hazards/{id},consequences/{id}}`
- `qualia/world_model/coach/{summary,timeline/{kind},decisions/{id}}`
- `qualia/world_model/proposals/{id}/coach/{decision_id}`
- `qualia/world_model/canonical/{id}/coach/{decision_id}`
- `qualia/sync/{summary,replicas/{id},states/{namespace}/{key},append/{namespace}/{key}/{entry_id},ops/{op_id}}`
- `qualia/replay/summary`
- `qualia/connectome/brain/points` — every placed CSR node, coloured by cell type, static
- `qualia/connectome/brain/firing` — the firing set of the current tick, on the `qualia_tick` timeline
- `qualia/connectome/brain/summary` — the cloud's counts and palette rule
- `qualia/connectome/stream/summary` — the tick, its wall clock and firing counts
- `qualia/connectome/stream/firing_count` — firing count per tick

Proposal state stays under `proposals/...`; accepted state lives under
`canonical/...` and uses its own point/edge styling. The default blueprint
exposes `Accepted Graph` and `Accepted Spatial` views beside the proposal
graph, the operational slice, and a replay timeline pane.

## Tests

```powershell
cargo test -p qualia-rerun-bridge
```

The tests cover the record streams (counts, geometry, lineage, coach
timeline), the sync summaries, and the on-disk recording produced by
`RerunSinkConfig::Save`.
