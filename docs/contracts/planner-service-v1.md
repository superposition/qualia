# planner-service-v1

This document freezes the `compute.v1` planner contract between `qualia-agent` and
`qualia-cuda-service`.

## Transport

- Local socket.
- Windows default: `127.0.0.1:46321`.
- Non-Windows default: `/tmp/qualia-compute.sock`.
- Encoding: newline-delimited JSON, one request and one response per line.

## Request

`request_type: "plan_path"`.

Top-level fields:

- `schema_version: "compute.v1"`
- `request_id: string`
- `request_type: "plan_path"`
- `timestamp_ns: u64`
- `algorithm: string`
- `goal: PlannerPose`
- `start: PlannerPose`
- `grid: PlannerGrid`
- `constraints: PlannerConstraints`
- `world_context: PlannerWorldContext`
- `replan: PlannerReplanContext | null`
- `belief_risk: PlannerBeliefRiskContext | null`

`PlannerPose`: `cell_x: i32`, `cell_z: i32`, `x_m: f32`, `y_m: f32`, `z_m: f32`, `yaw_rad: f32`.

`PlannerGrid`: `width: u32`, `depth: u32`, `resolution_m: f32`, `occupied: Vec<u8>`, `cost: Vec<u8>`.

`PlannerConstraints`: `robot_radius_cells: u32`, `allow_unknown: bool`, `max_iterations: u32`.

`PlannerWorldContext`: `nav_seq: u64`, `voxel_seq: u64`, `footprint_seq: u64`.

`PlannerReplanContext`: `previous_request_id: string`, `reason: string | null`.

`PlannerBeliefRiskContext`:

- `uncertainty_weight: f32`
- `semantic_novelty: f32`

Both fields are **clamped to `0.0..=1.0` by the sender**. `runners/cuda-service/src/planner.rs` scales
`uncertainty_weight` by 12 and `semantic_novelty` by 4, then multiplies by the grid resolution, so an
out-of-range value would silently distort the cost map rather than being rejected.

## Success response

Top-level fields:

- `schema_version: "compute.v1"`
- `service_instance: string`
- `request_id: string`
- `result_type: "path_result"`
- `status: "ok"`
- `planning_ms: f32 | null`
- `path: Vec<PathPoint>`
- `summary: PathSummary | null`
- `debug: PathDebug | null`
- `error: null`

`PathPoint`: `cell_x: i32`, `cell_z: i32`, `x_m: f32`, `z_m: f32`.

`PathSummary`: `path_cost: f32`, `expanded_nodes: u32`, `reachable: bool`.

`PathDebug`: `costmap_seq: u64`, `algorithm: string`, `replan_of: string | null`,
`replan_reason: string | null`, `uncertainty_weight: f32 | null`, `semantic_novelty: f32 | null`.

## Error response

Top-level fields:

- `schema_version: "compute.v1"`
- `service_instance: string`
- `request_id: string`
- `result_type: "error"`
- `status: "error"`
- `error: ComputeError`

`ComputeError`: `code: string`, `message: string`, `retryable: bool`.

Known `code` values: `invalid_request`, `unsupported_version`, `start_blocked`, `goal_blocked`,
`no_path`, `timeout`.

## Authority

The planner proposes. It never acts. A returned path is a proposal that `qualia-agent` may forward to
Leash over `QUALIA_LEASH_BASE_URL`; Leash is the motion authority and decides. Nothing in this
contract, and nothing in the planner service, writes to a motor path or to the ROS graph.

## Stability

- `compute.v1` is the compatibility boundary for the planner path.
- `semantic_goal` is an agent-side convenience input, resolved into a concrete `goal` before this
  contract is sent.
- `costmap_stats` and `cuda_smoke` are separate request types and are not part of this contract.
- New planner algorithms may be added without changing the envelope shape.
