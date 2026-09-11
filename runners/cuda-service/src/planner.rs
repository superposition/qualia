//! Grid planning for the `plan_path` request.
//!
//! Both advertised algorithms are the same bounded best-first search over
//! 4-connected cells: `grid_astar` estimates the remaining distance with the
//! Manhattan metric, `uniform_cost` passes a zero estimate and so expands by
//! accumulated cost alone. The estimate only orders the frontier; relaxation,
//! footprint checks and the reconstructed path are shared, which is why a
//! given grid yields a shortest path under either name.

use crate::{
    ComputeError, PathDebug, PathPlanRequest, PathPlanResponse, PathPoint, PathSummary,
    PlannerGrid, ResponseMeta, PLANNER_MAX_PATH_POINTS,
};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// The names `capabilities` advertises, in the order it lists them.
pub(crate) const ALGORITHMS: [&str; 2] = ["grid_astar", "uniform_cost"];

/// The neighbour sweep every expansion uses, in visit order.
const STEPS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// One grid cell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) x: i32,
    pub(crate) z: i32,
}

/// A frontier entry. Ordering pops the smallest estimate first and breaks ties
/// toward the cheapest node reached so far, so a search is reproducible.
#[derive(Clone, Copy, Eq, PartialEq)]
struct Node {
    idx: usize,
    cell: Cell,
    estimate: u32,
    cost: u32,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .cmp(&self.estimate)
            .then_with(|| other.cost.cmp(&self.cost))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// How the search estimates the distance that is left.
#[derive(Clone, Copy)]
enum Estimate {
    Manhattan,
    Zero,
}

impl Estimate {
    fn between(self, from: Cell, goal: Cell) -> u32 {
        match self {
            Estimate::Manhattan => from.x.abs_diff(goal.x) + from.z.abs_diff(goal.z),
            Estimate::Zero => 0,
        }
    }
}

pub(crate) fn supported_algorithms() -> Vec<String> {
    ALGORITHMS.iter().map(|name| (*name).to_string()).collect()
}

/// The one grid shape rule both `plan_path` and `costmap_stats` enforce: the
/// dimensions are non-zero and the arrays cover exactly width times depth.
pub(crate) fn check_grid(grid: &PlannerGrid) -> Result<(), ComputeError> {
    if grid.width == 0 || grid.depth == 0 {
        return Err(ComputeError::plain(
            "invalid_request",
            "grid dimensions must be non-zero",
        ));
    }
    let cells = grid.width as usize * grid.depth as usize;
    if grid.occupied.len() != cells || grid.cost.len() != cells {
        return Err(ComputeError::plain(
            "invalid_request",
            "grid arrays do not match width * depth",
        ));
    }
    Ok(())
}

pub(crate) fn plan_path(
    request: &PathPlanRequest,
    service_instance: &str,
) -> Result<PathPlanResponse, ComputeError> {
    if request.schema_version != crate::SCHEMA_VERSION {
        return Err(ComputeError::plain(
            "unsupported_version",
            format!("unsupported schema_version: {}", request.schema_version),
        ));
    }
    check_grid(&request.grid)?;
    check_pose(&request.start, request.grid.width, request.grid.depth, "start")?;
    check_pose(&request.goal, request.grid.width, request.grid.depth, "goal")?;

    let start = Cell {
        x: request.start.cell_x,
        z: request.start.cell_z,
    };
    let goal = Cell {
        x: request.goal.cell_x,
        z: request.goal.cell_z,
    };
    let radius = request.constraints.robot_radius_cells;

    if !request.grid.is_clear(start, radius) {
        return Err(ComputeError::plain(
            "start_blocked",
            "start footprint intersects occupied or out-of-bounds cells",
        ));
    }
    if !request.grid.is_clear(goal, radius) {
        return Err(ComputeError::plain(
            "goal_blocked",
            "goal footprint intersects occupied or out-of-bounds cells",
        ));
    }

    let limit = request.constraints.max_iterations as usize;
    let (cells, algorithm) = match request.algorithm.as_str() {
        "grid_astar" => (
            best_path(&request.grid, start, goal, radius, limit, Estimate::Manhattan)?,
            "grid_astar",
        ),
        "uniform_cost" => (
            best_path(&request.grid, start, goal, radius, limit, Estimate::Zero)?,
            "uniform_cost",
        ),
        other => {
            return Err(ComputeError::plain(
                "invalid_request",
                format!("unsupported planner algorithm: {other}"),
            ))
        }
    };

    let mut path = cells
        .iter()
        .take(PLANNER_MAX_PATH_POINTS)
        .map(|cell| PathPoint {
            cell_x: cell.x,
            cell_z: cell.z,
            x_m: request.grid.world_x(cell.x),
            z_m: request.grid.world_z(cell.z),
        })
        .collect::<Vec<_>>();
    // The caller's own poses win over the cell centres, so a consumer sees back
    // exactly the frame it asked from.
    if let Some(first) = path.first_mut() {
        first.x_m = request.start.x_m;
        first.z_m = request.start.z_m;
    }
    if let Some(last) = path.last_mut() {
        last.x_m = request.goal.x_m;
        last.z_m = request.goal.z_m;
    }

    let cell_cost = path
        .iter()
        .map(|point| {
            let cell = Cell {
                x: point.cell_x,
                z: point.cell_z,
            };
            f32::from(request.grid.cost[request.grid.index(cell)])
        })
        .sum::<f32>();
    let path_cost = cell_cost + path.len() as f32 + risk_penalty(request);

    Ok(PathPlanResponse {
        meta: ResponseMeta {
            schema_version: request.schema_version.clone(),
            service_instance: service_instance.to_string(),
            request_id: request.request_id.clone(),
            result_type: "path_result".to_string(),
            status: "ok".to_string(),
        },
        planning_ms: Some(0.0),
        path,
        summary: Some(PathSummary {
            path_cost,
            expanded_nodes: cells.len() as u32,
            reachable: true,
        }),
        debug: Some(PathDebug {
            costmap_seq: request.world_context.voxel_seq,
            algorithm: algorithm.to_string(),
            replan_of: request
                .replan
                .as_ref()
                .map(|replan| replan.previous_request_id.clone()),
            replan_reason: request.replan.as_ref().and_then(|replan| replan.reason.clone()),
            uncertainty_weight: request.belief_risk.as_ref().map(|risk| risk.uncertainty_weight),
            semantic_novelty: request.belief_risk.as_ref().map(|risk| risk.semantic_novelty),
        }),
        error: None,
    })
}

/// The belief layer's caution about a cell map, folded into the reported cost
/// so a consumer sees the same number the policy was scored with.
fn risk_penalty(request: &PathPlanRequest) -> f32 {
    match &request.belief_risk {
        Some(risk) => {
            let weight = risk.uncertainty_weight.clamp(0.0, 1.0);
            let novelty = risk.semantic_novelty.clamp(0.0, 1.0);
            (weight * 12.0 + novelty * 4.0) * request.grid.resolution_m.max(0.1)
        }
        None => 0.0,
    }
}

fn check_pose(
    pose: &crate::PlannerPose,
    width: u32,
    depth: u32,
    label: &str,
) -> Result<(), ComputeError> {
    if pose.cell_x < 0
        || pose.cell_z < 0
        || pose.cell_x >= width as i32
        || pose.cell_z >= depth as i32
    {
        return Err(ComputeError::plain(
            "invalid_request",
            format!("{label} cell out of range"),
        ));
    }
    Ok(())
}

/// Best-first search with an admissible estimate; `Estimate::Zero` makes it
/// uniform-cost. Returns the visited cells from start to goal.
fn best_path(
    grid: &PlannerGrid,
    start: Cell,
    goal: Cell,
    radius: u32,
    limit: usize,
    estimate: Estimate,
) -> Result<Vec<Cell>, ComputeError> {
    let cells = grid.width as usize * grid.depth as usize;
    let first = grid.index(start);
    let last = grid.index(goal);
    let mut came_from = vec![usize::MAX; cells];
    let mut best_cost = vec![u32::MAX; cells];
    let mut open = BinaryHeap::new();

    best_cost[first] = 0;
    open.push(Node {
        idx: first,
        cell: start,
        estimate: estimate.between(start, goal),
        cost: 0,
    });

    let mut expanded = 0usize;
    while let Some(current) = open.pop() {
        expanded += 1;
        if expanded > limit.max(1) {
            return Err(ComputeError::retryable(
                "timeout",
                "planner exceeded max_iterations",
            ));
        }
        if current.idx == last {
            return Ok(trace(&came_from, current.idx, first, grid.width));
        }

        for (dx, dz) in STEPS {
            let next = Cell {
                x: current.cell.x + dx,
                z: current.cell.z + dz,
            };
            if !grid.is_clear(next, radius) {
                continue;
            }
            let idx = grid.index(next);
            let tentative = current.cost.saturating_add(1 + u32::from(grid.cost[idx]));
            if tentative < best_cost[idx] {
                came_from[idx] = current.idx;
                best_cost[idx] = tentative;
                open.push(Node {
                    idx,
                    cell: next,
                    cost: tentative,
                    estimate: tentative.saturating_add(estimate.between(next, goal)),
                });
            }
        }
    }

    Err(ComputeError::plain("no_path", "no path found"))
}

fn trace(came_from: &[usize], current: usize, start: usize, width: u32) -> Vec<Cell> {
    let mut reversed = Vec::new();
    let mut at = current;
    loop {
        reversed.push(Cell {
            x: (at % width as usize) as i32,
            z: (at / width as usize) as i32,
        });
        if at == start {
            break;
        }
        at = came_from[at];
    }
    reversed.reverse();
    reversed
}
