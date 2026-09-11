//! `qualia-explore` — pick a reachable frontier goal from the shared binary map
//! and let the compute service plan a path to it.
//!
//! The runner reads the pose and map slots, ranks frontier clusters, asks the
//! planner over the compute socket for a path, and publishes the first accepted
//! goal into the nav-goal slot. It never actuates anything: the goal it writes
//! is a proposal that `runners/drive` may chase and leash may refuse.
//!
//! Its mission is a frontier sweep, opened at stack start: it reports
//! `MissionOpened { mission_id: "explore-frontier" }` to the braid, and every
//! plan request carries the fly prior's type-level in-strength in the
//! `compute.v1` request's `belief_risk` field.

use qualia_braid::{observe, BraidError, BraidEvent, BraidState};
use qualia_jepa::prior::{clamp_coupling_scale, CouplingPrior, COUPLING_SCALE_DEFAULT};
use qualia_shm::ShmRegion;
use qualia_types::{BinaryMapGrid, NavGoal};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::Ordering as AtomicOrdering;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::TcpStream as PlannerStream;
#[cfg(not(windows))]
use tokio::net::UnixStream as PlannerStream;
use tokio::time::{sleep, timeout, Duration};

/// Re-evaluation period when nothing is planned.
const DEFAULT_INTERVAL_MS: u64 = 1_000;
/// Deadline for each connect/write/read against the compute socket.
const DEFAULT_TIMEOUT_MS: u64 = 300;
#[cfg(windows)]
const DEFAULT_COMPUTE_ADDR: &str = "127.0.0.1:46321";
#[cfg(not(windows))]
const DEFAULT_COMPUTE_ADDR: &str = "/tmp/qualia-compute.sock";

/// The planner sees a coarse copy of the fine map.
const PLAN_W: usize = 32;
const PLAN_H: usize = 32;
/// A frontier cluster smaller than this is not worth a plan.
const FRONTIER_MIN_CLUSTER: usize = 8;
/// Goals closer than this to the robot are not worth planning.
const MIN_GOAL_DISTANCE_CELLS: f32 = 5.0;
/// An accepted goal is left in place for this long before replanning.
const GOAL_HOLD_NS: u64 = 7_000_000_000;
/// Coarse-cell radius searched around a frontier for a stand-off goal.
const GOAL_SEARCH_RADIUS_CELLS: i32 = 4;
/// A plan shorter than this is treated as a refusal.
const MIN_PATH_LEN: usize = 4;
/// The mission opened at stack start: a sweep of the reachable frontier.
const DEFAULT_MISSION_ID: &str = "explore-frontier";

/// Cell values shared with `runners/map`.
const UNKNOWN: u8 = 0;
const FREE: u8 = 1;
const OCCUPIED: u8 = 2;

/// Tunables read once at start-up; tests construct them directly.
#[derive(Clone, Copy)]
struct ExploreTuning {
    frontier_min_cluster: usize,
    min_goal_distance_cells: f32,
    goal_hold_ns: u64,
    goal_search_radius_cells: i32,
    min_path_len: usize,
}

impl ExploreTuning {
    fn from_env() -> Self {
        Self {
            frontier_min_cluster: env_or(
                "QUALIA_EXPLORE_FRONTIER_MIN_CLUSTER",
                FRONTIER_MIN_CLUSTER,
            ),
            min_goal_distance_cells: env_or(
                "QUALIA_EXPLORE_MIN_GOAL_DISTANCE_CELLS",
                MIN_GOAL_DISTANCE_CELLS,
            ),
            goal_hold_ns: env_or("QUALIA_EXPLORE_CURRENT_GOAL_HOLD_NS", GOAL_HOLD_NS),
            goal_search_radius_cells: env_or(
                "QUALIA_EXPLORE_GOAL_SEARCH_RADIUS_CELLS",
                GOAL_SEARCH_RADIUS_CELLS,
            ),
            min_path_len: env_or("QUALIA_EXPLORE_MIN_PATH_LEN", MIN_PATH_LEN),
        }
    }
}

/// Process wiring, separated from the loop so it can be read in one place.
struct RunnerConfig {
    shm_name: String,
    interval_ms: u64,
    compute_addr: String,
    plan_timeout_ms: u64,
}

impl RunnerConfig {
    fn from_env() -> Self {
        Self {
            shm_name: std::env::var("QUALIA_SHM_NAME")
                .unwrap_or_else(|_| "/qualia_body".to_string()),
            interval_ms: env_or("QUALIA_EXPLORE_INTERVAL_MS", DEFAULT_INTERVAL_MS),
            compute_addr: std::env::var("QUALIA_COMPUTE_SOCKET")
                .unwrap_or_else(|_| DEFAULT_COMPUTE_ADDR.to_string()),
            plan_timeout_ms: env_or("QUALIA_COMPUTE_TIMEOUT_MS", DEFAULT_TIMEOUT_MS),
        }
    }
}

/// The fly prior, loaded once at start-up from the environment the supervisor
/// hands down.
///
/// `QUALIA_FLY_MODE` is `off` by default and `QUALIA_FLY_PRIOR_PATH` names the
/// artifact directory; only `prior` loads it. Any other mode, an unset path, or
/// an artifact [`CouplingPrior::load`] rejects disables the prior with a log
/// line and an `off` runner — the runner must never fail to start because of
/// the prior. `QUALIA_FLY_COUPLING_SCALE` is the agent's dial on how hard the
/// prior is applied: it is read at the coupling's bound, so a hand-edited
/// manifest cannot drive the coupling to zero or to infinity.
struct FlyPrior {
    prior: CouplingPrior,
    /// The bounded dial reading the coupling is applied at.
    scale: f32,
}

impl FlyPrior {
    fn from_env() -> Option<Self> {
        Self::from_settings(
            std::env::var("QUALIA_FLY_MODE").ok(),
            std::env::var("QUALIA_FLY_PRIOR_PATH").ok(),
            std::env::var("QUALIA_FLY_COUPLING_SCALE").ok(),
        )
    }

    /// The prior the two settings select; `None` disables it.
    ///
    /// Split from the environment so every disabled path is a pure function of
    /// its inputs and can be tested without touching the process environment.
    fn from_settings(
        mode: Option<String>,
        path: Option<String>,
        scale: Option<String>,
    ) -> Option<Self> {
        let mode = mode.unwrap_or_else(|| "off".to_string());
        if mode != "prior" {
            if mode != "off" {
                println!("fly prior: disabled (mode {mode})");
            }
            return None;
        }
        let path = path.unwrap_or_default();
        if path.is_empty() {
            println!("fly prior: disabled (QUALIA_FLY_PRIOR_PATH is unset)");
            return None;
        }
        let scale = scale
            .and_then(|raw| raw.trim().parse::<f32>().ok())
            .map_or(COUPLING_SCALE_DEFAULT, clamp_coupling_scale);
        match CouplingPrior::load(Path::new(&path)) {
            Ok(prior) => Some(Self { prior, scale }),
            Err(error) => {
                println!("fly prior: disabled ({error})");
                None
            }
        }
    }
}

/// The braid event that opens the runner's default mission.
fn default_mission_event() -> BraidEvent {
    BraidEvent::MissionOpened {
        mission_id: DEFAULT_MISSION_ID.to_string(),
    }
}

/// Report the runner's default mission to the braid. The first mission at
/// stack start is a frontier sweep; the event is the report, and
/// [`observe`] folds it into the view the strand holds.
fn open_default_mission(braid: &mut BraidState) -> Result<(), BraidError> {
    observe(braid, &default_mission_event())
}

#[tokio::main]
async fn main() {
    let config = RunnerConfig::from_env();
    let tuning = ExploreTuning::from_env();

    let shm = match ShmRegion::open(&config.shm_name) {
        Ok(shm) => shm,
        Err(err) => {
            eprintln!(
                "qualia-explore: failed to open shm '{}': {err}",
                config.shm_name
            );
            std::process::exit(1);
        }
    };

    println!(
        "qualia-explore: starting frontier selection shm={}",
        config.shm_name
    );

    // The strand's view of the braid, opened with the runner's own mission.
    let mut braid = BraidState::default();
    match open_default_mission(&mut braid) {
        Ok(()) => println!("qualia-explore: mission opened id={DEFAULT_MISSION_ID}"),
        Err(error) => eprintln!("qualia-explore: braid mission open failed: {error}"),
    }

    let prior = FlyPrior::from_env();
    let belief_risk = PlannerBeliefRiskContext::from_prior(
        prior.as_ref().map(|fly| (&fly.prior, fly.scale)),
    );
    if let Some(risk) = belief_risk {
        println!(
            "fly prior: risk uncertainty_weight={} semantic_novelty={}",
            risk.uncertainty_weight, risk.semantic_novelty
        );
    }

    loop {
        match explore_once(
            &shm,
            &config.compute_addr,
            config.plan_timeout_ms,
            tuning,
            belief_risk,
        )
        .await
        {
            Some(outcome) => println!(
                "qualia-explore: selected frontier goal=({}, {}) world=({:.2}, {:.2}) path_len={} frontier_size={}",
                outcome.goal_cell_x,
                outcome.goal_cell_z,
                outcome.goal_x_m,
                outcome.goal_z_m,
                outcome.path_len,
                outcome.frontier_size
            ),
            None => println!("qualia-explore: no reachable frontier found"),
        }

        sleep(Duration::from_millis(config.interval_ms)).await;
    }
}

/// What `explore_once` accepted, in both cell and world coordinates.
struct GoalOutcome {
    goal_cell_x: i32,
    goal_cell_z: i32,
    goal_x_m: f32,
    goal_z_m: f32,
    path_len: usize,
    frontier_size: usize,
}

/// One exploration decision: `None` means nothing was published.
async fn explore_once(
    shm: &ShmRegion,
    compute_addr: &str,
    timeout_ms: u64,
    tuning: ExploreTuning,
    belief_risk: Option<PlannerBeliefRiskContext>,
) -> Option<GoalOutcome> {
    let pose = shm.world_model().robot_pose;
    if pose.timestamp_ns == 0 || pose.confidence <= 0.0 {
        return None;
    }

    let held = shm.world_model().nav_goal;
    if held.active != 0 && now_ns().saturating_sub(held.timestamp_ns) <= tuning.goal_hold_ns {
        return None;
    }

    let map = shm.binary_map();
    let seq = map.seq.load(AtomicOrdering::Acquire);
    if seq == 0 || map.width == 0 || map.height == 0 {
        return None;
    }

    let clusters = frontier_clusters(map, pose.x_m, pose.z_m, tuning);
    let mut coarse = downsample(map);
    let start = coarse.cell_of(pose.x_m, pose.z_m)?;
    coarse.clear_robot_footprint(start);
    println!("qualia-explore: frontier_clusters={}", clusters.len());
    if clusters.is_empty() {
        return None;
    }

    let mut candidates: Vec<GoalCandidate> = clusters
        .into_iter()
        .filter_map(|cluster| GoalCandidate::near(cluster, &coarse, start, tuning))
        .collect();
    candidates.sort_by(|a, b| a.rank.partial_cmp(&b.rank).unwrap_or(Ordering::Equal));

    let mut rejected = 0usize;
    let mut reasons = BTreeMap::<String, usize>::new();
    for candidate in candidates {
        let request = plan_request(
            &coarse,
            start,
            (candidate.cell_x, candidate.cell_z),
            belief_risk,
        );
        let reply = match ask_planner(compute_addr, timeout_ms, &request).await {
            Ok(reply) => reply,
            Err(err) => {
                rejected += 1;
                *reasons.entry(err).or_default() += 1;
                continue;
            }
        };

        if reply.status != "ok" {
            rejected += 1;
            let code = reply
                .error
                .as_ref()
                .map(|err| err.code.clone())
                .unwrap_or_else(|| "unknown".to_string());
            *reasons.entry(code).or_default() += 1;
            continue;
        }

        if reply.path.len() < tuning.min_path_len {
            rejected += 1;
            *reasons.entry("path_too_short".to_string()).or_default() += 1;
            continue;
        }

        let x_m = coarse.center_x(candidate.cell_x);
        let z_m = coarse.center_z(candidate.cell_z);
        shm.set_nav_goal(NavGoal {
            active: 1,
            _pad0: [0; 3],
            cell_x: candidate.cell_x,
            cell_z: candidate.cell_z,
            x_m,
            y_m: 0.0,
            z_m,
            yaw_rad: pose.yaw_rad,
            timestamp_ns: now_ns(),
        });
        return Some(GoalOutcome {
            goal_cell_x: candidate.cell_x,
            goal_cell_z: candidate.cell_z,
            goal_x_m: x_m,
            goal_z_m: z_m,
            path_len: reply.path.len(),
            frontier_size: candidate.cluster_size,
        });
    }

    println!(
        "qualia-explore: planner_rejected_candidates={rejected} reasons={reasons:?}"
    );
    None
}

/// A frontier cluster reduced to one stand-off goal with a ranking score.
struct GoalCandidate {
    cell_x: i32,
    cell_z: i32,
    rank: f32,
    cluster_size: usize,
}

impl GoalCandidate {
    /// Place a goal near the cluster and score it; lower ranks plan first.
    fn near(
        cluster: FrontierCluster,
        coarse: &PlanGrid,
        start: (i32, i32),
        tuning: ExploreTuning,
    ) -> Option<Self> {
        let frontier = coarse.cell_of(cluster.goal_x_m, cluster.goal_z_m)?;
        let (cell_x, cell_z) = pick_standoff(coarse, start, frontier, tuning)?;

        let dx = (cell_x - start.0) as f32;
        let dz = (cell_z - start.1) as f32;
        let distance = dx.hypot(dz);
        if distance < tuning.min_goal_distance_cells {
            return None;
        }

        let size = cluster.cells.len() as f32;
        let gain = cluster.unknown_gain as f32;
        let offset = ((cell_x - frontier.0) as f32).hypot((cell_z - frontier.1) as f32) * 0.35;
        let rank = -(size * 0.18) - (gain * 0.10) - distance * 0.45 + offset;

        Some(Self {
            cell_x,
            cell_z,
            rank,
            cluster_size: cluster.cells.len(),
        })
    }
}

/// The free coarse cell within `goal_search_radius_cells` of `frontier` that
/// sits closest to the frontier while staying `min_goal_distance_cells` away
/// from the robot. Ties keep the first cell in scan order.
fn pick_standoff(
    coarse: &PlanGrid,
    start: (i32, i32),
    frontier: (i32, i32),
    tuning: ExploreTuning,
) -> Option<(i32, i32)> {
    let radius = tuning.goal_search_radius_cells;
    let mut best: Option<((i32, i32), f32)> = None;

    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let gx = frontier.0 + dx;
            let gz = frontier.1 + dz;
            if gx < 0 || gz < 0 || gx >= PLAN_W as i32 || gz >= PLAN_H as i32 {
                continue;
            }
            if coarse.cell(gx, gz) != FREE {
                continue;
            }
            let start_distance = ((gx - start.0) as f32).hypot((gz - start.1) as f32);
            if start_distance < tuning.min_goal_distance_cells {
                continue;
            }
            let score = (dx as f32).hypot(dz as f32) + start_distance * 0.08;
            match best {
                Some((_, best_score)) if score >= best_score => {}
                _ => best = Some(((gx, gz), score)),
            }
        }
    }

    best.map(|(cell, _)| cell)
}

/// A connected group of frontier cells plus the margin of unknown space it
/// would reveal.
struct FrontierCluster {
    cells: Vec<(i32, i32)>,
    goal_x_m: f32,
    goal_z_m: f32,
    unknown_gain: usize,
}

/// Frontier cells in the fine map that are reachable from the robot, grouped by
/// 4-connectivity and filtered by size.
fn frontier_clusters(
    map: &BinaryMapGrid,
    pose_x: f32,
    pose_z: f32,
    tuning: ExploreTuning,
) -> Vec<FrontierCluster> {
    let start = match fine_cell_of(map, pose_x, pose_z) {
        Some(cell) => cell,
        None => return Vec::new(),
    };
    let reachable = reachable_free_cells(map, start);
    let width = map.width as i32;
    let height = map.height as i32;
    let mut visited = vec![false; map.width as usize * map.height as usize];
    let mut clusters = Vec::new();

    for z in 0..height {
        for x in 0..width {
            let index = z as usize * map.width as usize + x as usize;
            if visited[index] || !reachable[index] || !is_frontier_cell(map, x, z) {
                continue;
            }

            let mut queue = VecDeque::from([(x, z)]);
            let mut cells = Vec::new();
            visited[index] = true;
            while let Some((cx, cz)) = queue.pop_front() {
                cells.push((cx, cz));
                for (nx, nz) in neighbors(cx, cz, width, height, &NEIGHBORS4) {
                    let nindex = nz as usize * map.width as usize + nx as usize;
                    if visited[nindex] || !reachable[nindex] || !is_frontier_cell(map, nx, nz) {
                        continue;
                    }
                    visited[nindex] = true;
                    queue.push_back((nx, nz));
                }
            }

            if cells.len() < tuning.frontier_min_cluster {
                continue;
            }

            let mid = cells[cells.len() / 2];
            let gain = unknown_gain(map, &cells);
            clusters.push(FrontierCluster {
                cells,
                goal_x_m: fine_center_x(map, mid.0),
                goal_z_m: fine_center_z(map, mid.1),
                unknown_gain: gain,
            });
        }
    }

    clusters
}

/// Distinct unknown fine cells in the 8-neighbourhood of the cluster.
fn unknown_gain(map: &BinaryMapGrid, cells: &[(i32, i32)]) -> usize {
    let width = map.width as i32;
    let height = map.height as i32;
    let mut seen = vec![false; map.width as usize * map.height as usize];
    let mut total = 0usize;

    for &(x, z) in cells {
        for (nx, nz) in neighbors(x, z, width, height, &NEIGHBORS8) {
            if fine_cell(map, nx, nz) != UNKNOWN {
                continue;
            }
            let index = nz as usize * map.width as usize + nx as usize;
            if seen[index] {
                continue;
            }
            seen[index] = true;
            total += 1;
        }
    }

    total
}

/// A free fine cell that touches unknown space.
fn is_frontier_cell(map: &BinaryMapGrid, x: i32, z: i32) -> bool {
    if fine_cell(map, x, z) != FREE {
        return false;
    }
    neighbors(x, z, map.width as i32, map.height as i32, &NEIGHBORS8)
        .into_iter()
        .any(|(nx, nz)| fine_cell(map, nx, nz) == UNKNOWN)
}

/// Flood fill over free fine cells from the robot, 4-connected.
fn reachable_free_cells(map: &BinaryMapGrid, start: (i32, i32)) -> Vec<bool> {
    let mut reachable = vec![false; map.width as usize * map.height as usize];
    if fine_cell(map, start.0, start.1) == OCCUPIED {
        return reachable;
    }

    let mut queue = VecDeque::from([start]);
    reachable[start.1 as usize * map.width as usize + start.0 as usize] = true;
    while let Some((x, z)) = queue.pop_front() {
        for (nx, nz) in neighbors(x, z, map.width as i32, map.height as i32, &NEIGHBORS4) {
            let index = nz as usize * map.width as usize + nx as usize;
            if reachable[index] || fine_cell(map, nx, nz) != FREE {
                continue;
            }
            reachable[index] = true;
            queue.push_back((nx, nz));
        }
    }

    reachable
}

/// A fine cell value; anything outside the map reads as occupied.
fn fine_cell(map: &BinaryMapGrid, x: i32, z: i32) -> u8 {
    if x < 0 || z < 0 || x >= map.width as i32 || z >= map.height as i32 {
        OCCUPIED
    } else {
        map.cells[z as usize * map.width as usize + x as usize]
    }
}

fn fine_cell_of(map: &BinaryMapGrid, x_m: f32, z_m: f32) -> Option<(i32, i32)> {
    let gx = ((x_m - map.origin_x_m) / map.resolution_m).floor() as i32;
    let gz = ((z_m - map.origin_y_m) / map.resolution_m).floor() as i32;
    if gx < 0 || gz < 0 || gx >= map.width as i32 || gz >= map.height as i32 {
        None
    } else {
        Some((gx, gz))
    }
}

fn fine_center_x(map: &BinaryMapGrid, cell_x: i32) -> f32 {
    map.origin_x_m + (cell_x as f32 + 0.5) * map.resolution_m
}

fn fine_center_z(map: &BinaryMapGrid, cell_z: i32) -> f32 {
    map.origin_y_m + (cell_z as f32 + 0.5) * map.resolution_m
}

/// The coarse planner view: 32×32 cells, one per block of the fine map.
struct PlanGrid {
    cells: Vec<u8>,
    resolution_m: f32,
}

impl PlanGrid {
    fn cell(&self, x: i32, z: i32) -> u8 {
        if x < 0 || z < 0 || x >= PLAN_W as i32 || z >= PLAN_H as i32 {
            OCCUPIED
        } else {
            self.cells[z as usize * PLAN_W + x as usize]
        }
    }

    /// World coordinates to a coarse cell; the grid is centred on the origin.
    fn cell_of(&self, x_m: f32, z_m: f32) -> Option<(i32, i32)> {
        let half_w = PLAN_W as f32 * self.resolution_m * 0.5;
        let half_h = PLAN_H as f32 * self.resolution_m * 0.5;
        if x_m < -half_w || x_m >= half_w || z_m < -half_h || z_m >= half_h {
            return None;
        }
        Some((
            ((x_m + half_w) / self.resolution_m).floor() as i32,
            ((z_m + half_h) / self.resolution_m).floor() as i32,
        ))
    }

    fn center_x(&self, cell_x: i32) -> f32 {
        let half_w = PLAN_W as f32 * self.resolution_m * 0.5;
        -half_w + (cell_x as f32 + 0.5) * self.resolution_m
    }

    fn center_z(&self, cell_z: i32) -> f32 {
        let half_h = PLAN_H as f32 * self.resolution_m * 0.5;
        -half_h + (cell_z as f32 + 0.5) * self.resolution_m
    }

    /// The robot must never be treated as blocked by its own footprint.
    fn clear_robot_footprint(&mut self, start: (i32, i32)) {
        for dx in -1..=1 {
            for dz in -1..=1 {
                let x = start.0 + dx;
                let z = start.1 + dz;
                if x < 0 || z < 0 || x >= PLAN_W as i32 || z >= PLAN_H as i32 {
                    continue;
                }
                self.cells[z as usize * PLAN_W + x as usize] = FREE;
            }
        }
    }
}

/// Collapse blocks of fine cells into the coarse planner grid. A block holding
/// any obstacle is an obstacle; otherwise any free cell makes it free.
fn downsample(map: &BinaryMapGrid) -> PlanGrid {
    let scale_x = (map.width as usize / PLAN_W).max(1);
    let scale_y = (map.height as usize / PLAN_H).max(1);
    let mut cells = vec![UNKNOWN; PLAN_W * PLAN_H];

    for cz in 0..PLAN_H {
        for cx in 0..PLAN_W {
            let mut occupied = 0usize;
            let mut free = 0usize;
            for sy in 0..scale_y {
                for sx in 0..scale_x {
                    let x = cx * scale_x + sx;
                    let y = cz * scale_y + sy;
                    if x >= map.width as usize || y >= map.height as usize {
                        continue;
                    }
                    match map.cells[y * map.width as usize + x] {
                        OCCUPIED => occupied += 1,
                        FREE => free += 1,
                        _ => {}
                    }
                }
            }
            cells[cz * PLAN_W + cx] = if occupied > 0 {
                OCCUPIED
            } else if free > 0 {
                FREE
            } else {
                UNKNOWN
            };
        }
    }

    PlanGrid {
        cells,
        resolution_m: map.resolution_m * scale_x as f32,
    }
}

#[derive(Serialize)]
struct PlanRequest {
    schema_version: &'static str,
    request_id: String,
    request_type: &'static str,
    timestamp_ns: u64,
    goal: PlannerPose,
    start: PlannerPose,
    grid: PlannerGridRequest,
    constraints: PlannerConstraints,
    world_context: WorldContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    belief_risk: Option<PlannerBeliefRiskContext>,
}

/// The fly prior's coupling as the planner's risk context.
///
/// Both fields are clamped to `0.0..=1.0` by the sender: `runners/cuda-service`
/// scales `uncertainty_weight` by 12 and `semantic_novelty` by 4 and multiplies
/// by the grid resolution, so an out-of-range value would distort the cost map
/// instead of being rejected.
#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
struct PlannerBeliefRiskContext {
    uncertainty_weight: f32,
    semantic_novelty: f32,
}

impl PlannerBeliefRiskContext {
    /// The risk the loaded prior covers, or `None` when it is off.
    ///
    /// `uncertainty_weight` is the mean weight the prior applies per coupled
    /// type: [`CouplingPrior::couple`] returns the summed peak-normalised
    /// in-strength over the slots it is handed, times the dial the pair
    /// carries, and dividing that total by the type count keeps the value
    /// inside the unit range the planner contract fixes instead of saturating
    /// at the strongest type's unit weight — a uniformly innervated graph at
    /// the default dial reads `1.0`, one whose in-strength is concentrated in
    /// a few types reads lower. `semantic_novelty` is `0.0`
    /// because no runtime artifact carries per-type `dimorphism`; the dated
    /// resolution on issue #38 records that gap. With the prior off — or
    /// coupling nothing — the whole context is absent, so the request leaves
    /// `belief_risk` unset exactly as an ungoverned runner does.
    fn from_prior(prior: Option<(&CouplingPrior, f32)>) -> Option<Self> {
        let (prior, scale) = prior?;
        let type_count = prior.type_count as usize;
        if type_count == 0 {
            return None;
        }
        let mut belief = vec![1.0f32; type_count];
        let slots: Vec<(u32, usize)> = (0..prior.type_count)
            .map(|index| (index, index as usize))
            .collect();
        let total_applied = prior.couple(&mut belief, &slots, scale);
        let uncertainty_weight = (total_applied / type_count as f32).clamp(0.0, 1.0);
        if uncertainty_weight <= 0.0 {
            return None;
        }
        Some(Self {
            uncertainty_weight,
            semantic_novelty: 0.0,
        })
    }
}

#[derive(Serialize)]
struct PlannerPose {
    cell_x: i32,
    cell_z: i32,
    x_m: f32,
    y_m: f32,
    z_m: f32,
    yaw_rad: f32,
}

#[derive(Serialize)]
struct PlannerGridRequest {
    width: u32,
    depth: u32,
    resolution_m: f32,
    occupied: Vec<u8>,
    cost: Vec<u8>,
}

#[derive(Serialize)]
struct PlannerConstraints {
    robot_radius_cells: u32,
    allow_unknown: bool,
    max_iterations: u32,
}

#[derive(Serialize)]
struct WorldContext {
    nav_seq: u64,
    voxel_seq: u64,
    footprint_seq: u64,
}

#[derive(Deserialize, Debug)]
struct PlanResponse {
    status: String,
    #[serde(default)]
    path: Vec<PathPoint>,
    #[serde(default)]
    error: Option<PlanError>,
}

/// One waypoint of a returned plan; the fields mirror the wire contract.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct PathPoint {
    cell_x: i32,
    cell_z: i32,
    x_m: f32,
    z_m: f32,
}

#[derive(Deserialize, Debug)]
struct PlanError {
    code: String,
}

/// Build the `compute.v1` `plan_path` envelope the planner service expects.
fn plan_request(
    grid: &PlanGrid,
    start: (i32, i32),
    goal: (i32, i32),
    belief_risk: Option<PlannerBeliefRiskContext>,
) -> PlanRequest {
    let occupied = grid
        .cells
        .iter()
        .map(|&value| u8::from(value == OCCUPIED))
        .collect::<Vec<_>>();
    let cost = grid
        .cells
        .iter()
        .map(|&value| match value {
            OCCUPIED => 255,
            FREE => 0,
            _ => 12,
        })
        .collect::<Vec<_>>();

    PlanRequest {
        schema_version: "compute.v1",
        request_id: format!("explore_{:x}", now_ns()),
        request_type: "plan_path",
        timestamp_ns: now_ns(),
        goal: PlannerPose {
            cell_x: goal.0,
            cell_z: goal.1,
            x_m: grid.center_x(goal.0),
            y_m: 0.0,
            z_m: grid.center_z(goal.1),
            yaw_rad: 0.0,
        },
        start: PlannerPose {
            cell_x: start.0,
            cell_z: start.1,
            x_m: grid.center_x(start.0),
            y_m: 0.0,
            z_m: grid.center_z(start.1),
            yaw_rad: 0.0,
        },
        grid: PlannerGridRequest {
            width: PLAN_W as u32,
            depth: PLAN_H as u32,
            resolution_m: grid.resolution_m,
            occupied,
            cost,
        },
        constraints: PlannerConstraints {
            robot_radius_cells: 1,
            allow_unknown: false,
            max_iterations: 50_000,
        },
        world_context: WorldContext {
            nav_seq: 0,
            voxel_seq: 0,
            footprint_seq: 0,
        },
        belief_risk,
    }
}

/// Send one newline-delimited request and read one newline-delimited reply,
/// with every step bounded by `timeout_ms`.
async fn ask_planner(
    addr: &str,
    timeout_ms: u64,
    request: &PlanRequest,
) -> Result<PlanResponse, String> {
    let mut stream = timeout(
        Duration::from_millis(timeout_ms),
        PlannerStream::connect(addr),
    )
    .await
    .map_err(|_| "connect timeout".to_string())?
    .map_err(|err| format!("connect: {err}"))?;

    let body = serde_json::to_vec(request).map_err(|err| format!("encode: {err}"))?;
    timeout(Duration::from_millis(timeout_ms), async {
        stream.write_all(&body).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await
    })
    .await
    .map_err(|_| "write timeout".to_string())?
    .map_err(|err| format!("write: {err}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    timeout(
        Duration::from_millis(timeout_ms),
        reader.read_line(&mut line),
    )
    .await
    .map_err(|_| "read timeout".to_string())?
    .map_err(|err| format!("read: {err}"))?;

    serde_json::from_str(line.trim()).map_err(|err| format!("decode: {err}"))
}

/// Neighbour offsets for the 4- and 8-connected neighbourhoods, in scan order.
const NEIGHBORS4: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const NEIGHBORS8: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// The in-bounds neighbours of `(x, z)` under `offsets`.
fn neighbors(x: i32, z: i32, width: i32, height: i32, offsets: &[(i32, i32)]) -> Vec<(i32, i32)> {
    offsets
        .iter()
        .map(|&(dx, dz)| (x + dx, z + dz))
        .filter(|&(nx, nz)| nx >= 0 && nz >= 0 && nx < width && nz < height)
        .collect()
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Read `name` as `T`, falling back to `default` when it is unset or unparsable.
fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qualia_types::{NavPose, MAP_GRID_H, MAP_GRID_W};
    use serde_json::Value;
    #[cfg(not(windows))]
    use std::path::PathBuf;
    #[cfg(not(windows))]
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    #[cfg(windows)]
    use tokio::net::TcpListener;
    #[cfg(not(windows))]
    use tokio::net::UnixListener;

    const FINE_RES: f32 = 0.1;
    const FINE_ORIGIN: f32 = -12.8;

    fn region(tag: &str) -> ShmRegion {
        let name = format!("/qualia-explore-test-{}-{tag}", std::process::id());
        ShmRegion::create(&name).expect("create test shm region")
    }

    fn pose(x_m: f32, z_m: f32) -> NavPose {
        NavPose {
            x_m,
            y_m: 0.0,
            z_m,
            yaw_rad: 0.7,
            pitch_rad: 0.0,
            roll_rad: 0.0,
            confidence: 1.0,
            _pad0: 0.0,
            timestamp_ns: 1,
        }
    }

    fn publish_map(shm: &ShmRegion, fill: impl Fn(usize, usize) -> u8) {
        let map = shm.binary_map_mut();
        map.width = MAP_GRID_W as u32;
        map.height = MAP_GRID_H as u32;
        map.resolution_m = FINE_RES;
        map.origin_x_m = FINE_ORIGIN;
        map.origin_y_m = FINE_ORIGIN;
        for z in 0..MAP_GRID_H {
            for x in 0..MAP_GRID_W {
                map.cells[z * MAP_GRID_W + x] = fill(x, z);
            }
        }
        map.seq.store(1, AtomicOrdering::Release);
    }

    /// A 96-cell free block with one obstacle buried inside it. The free
    /// perimeter is one frontier ring reachable from the centre.
    fn block(x: usize, z: usize) -> u8 {
        if x == 100 && z == 100 {
            return OCCUPIED;
        }
        if (80..176).contains(&x) && (80..176).contains(&z) {
            FREE
        } else {
            UNKNOWN
        }
    }

    /// Two free blobs joined by a corridor whose walls are obstacles, so the
    /// corridor cells are not frontier cells and the blobs form two clusters.
    fn two_blobs(x: usize, z: usize) -> u8 {
        let a = (64..104).contains(&x) && (64..104).contains(&z);
        let b = (152..192).contains(&x) && (64..104).contains(&z);
        let corridor = (104..152).contains(&x) && (78..82).contains(&z);
        let wall = (104..152).contains(&x) && (z == 77 || z == 82);
        if a || b || corridor {
            FREE
        } else if wall {
            OCCUPIED
        } else {
            UNKNOWN
        }
    }

    /// The listener transport the runner dials. Selected by the same predicate
    /// as the runner's `PlannerStream`, so the fixture and the product speak
    /// the same protocol on every target.
    #[cfg(windows)]
    type PlannerListener = TcpListener;
    #[cfg(not(windows))]
    type PlannerListener = UnixListener;

    /// A bound fake-planner endpoint: the address the runner must dial, plus
    /// the Unix socket file to unlink when the test ends.
    struct PlannerSocket {
        addr: String,
        #[cfg(not(windows))]
        path: PathBuf,
    }

    #[cfg(not(windows))]
    impl Drop for PlannerSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Bind the platform's planner transport and report the endpoint the runner
    /// must dial.
    #[cfg(windows)]
    async fn bind_planner(_tag: &str) -> (PlannerListener, PlannerSocket) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake planner");
        let addr = listener.local_addr().expect("local addr").to_string();
        (listener, PlannerSocket { addr })
    }

    #[cfg(not(windows))]
    async fn bind_planner(tag: &str) -> (PlannerListener, PlannerSocket) {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, AtomicOrdering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "qualia-explore-{}-{tag}-{seq}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind fake planner");
        let addr = path.to_string_lossy().into_owned();
        (listener, PlannerSocket { addr, path })
    }

    struct FakePlanner {
        socket: PlannerSocket,
        lines: Arc<Mutex<Vec<String>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    async fn fake_planner(reply: impl Fn(usize) -> String + Send + 'static) -> FakePlanner {
        let (listener, socket) = bind_planner("planner").await;
        let lines = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&lines);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut line = String::new();
                {
                    let mut reader = BufReader::new(&mut socket);
                    if reader.read_line(&mut line).await.is_err() {
                        continue;
                    }
                }
                let index = {
                    let mut seen = seen.lock().expect("planner lines");
                    let index = seen.len();
                    seen.push(line);
                    index
                };
                if socket.write_all(reply(index).as_bytes()).await.is_err() {
                    continue;
                }
                let _ = socket.write_all(b"\n").await;
                let _ = socket.flush().await;
            }
        });
        FakePlanner {
            socket,
            lines,
            _task: task,
        }
    }

    /// Accepts and reads the request, then never answers.
    async fn silent_planner() -> PlannerSocket {
        let (listener, socket) = bind_planner("silent").await;
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut line = String::new();
                    {
                        let mut reader = BufReader::new(&mut socket);
                        let _ = reader.read_line(&mut line).await;
                    }
                    sleep(Duration::from_secs(60)).await;
                });
            }
        });
        socket
    }

    fn ok_reply(points: usize) -> String {
        let path: Vec<Value> = (0..points)
            .map(|i| {
                serde_json::json!({"cell_x": i as i32, "cell_z": 0, "x_m": i as f32 * 0.5, "z_m": 0.0})
            })
            .collect();
        serde_json::json!({"status": "ok", "path": path}).to_string()
    }

    fn error_reply(code: &str) -> String {
        serde_json::json!({"status": "error", "error": {"code": code}}).to_string()
    }

    fn request_for_test() -> PlanRequest {
        let grid = PlanGrid {
            cells: vec![UNKNOWN; PLAN_W * PLAN_H],
            resolution_m: 0.8,
        };
        plan_request(&grid, (16, 16), (10, 10), None)
    }

    #[test]
    fn frontier_mission_is_opened_at_start() {
        let mut braid = BraidState::default();
        open_default_mission(&mut braid).expect("the default mission folds into the braid");

        assert_eq!(
            braid.open_missions, 1,
            "the runner opens exactly one mission at stack start"
        );
        let wire = serde_json::to_value(default_mission_event()).expect("the event serialises");
        assert_eq!(wire["event"], "mission_opened");
        assert_eq!(wire["mission_id"], "explore-frontier");
    }

    #[test]
    fn prior_coupling_sets_a_mean_applied_weight_inside_the_unit_range() {
        // Two types coupled both ways with unequal weights: the peak type
        // couples at unit weight and the other at 1/7, so the mean applied
        // weight per type is (1 + 1/7) / 2 — inside the range, not the
        // saturated 1.0 a raw total would give.
        let skewed = CouplingPrior {
            type_count: 2,
            rowptr: vec![0, 1, 2],
            cols: vec![1, 0],
            weights: vec![1, 7],
        };
        let risk = PlannerBeliefRiskContext::from_prior(Some((
            &skewed,
            COUPLING_SCALE_DEFAULT,
        )))
        .expect("a loaded prior reports risk");
        assert!(
            risk.uncertainty_weight > 0.0 && risk.uncertainty_weight < 1.0,
            "a skewed prior reads strictly inside the unit range: {}",
            risk.uncertainty_weight
        );
        assert_eq!(risk.semantic_novelty, 0.0);

        // A uniformly innervated graph is the ceiling: every type couples at
        // unit weight, so the mean is 1.0 and the contract's clamp holds it.
        let uniform = CouplingPrior {
            type_count: 2,
            rowptr: vec![0, 2, 4],
            cols: vec![1, 0, 0, 1],
            weights: vec![5, 5, 5, 5],
        };
        let risk = PlannerBeliefRiskContext::from_prior(Some((
            &uniform,
            COUPLING_SCALE_DEFAULT,
        )))
        .expect("a loaded prior reports risk");
        assert!(
            (risk.uncertainty_weight - 1.0).abs() < 1e-6,
            "a uniform prior reads the unit ceiling: {}",
            risk.uncertainty_weight
        );
    }

    #[test]
    fn a_disabled_prior_never_stops_the_runner() {
        // Off by default, and every path that cannot load an artifact answers
        // `None` rather than failing: another mode, a missing path, a path
        // whose artifact does not exist.
        assert!(FlyPrior::from_settings(None, None, None).is_none(), "off by default");
        assert!(
            FlyPrior::from_settings(Some("sim".to_string()), Some("C:/tmp".to_string()), None).is_none(),
            "an unknown mode does not load a prior"
        );
        assert!(
            FlyPrior::from_settings(Some("prior".to_string()), None, None).is_none(),
            "prior mode without a path stays off"
        );
        assert!(
            FlyPrior::from_settings(
                Some("prior".to_string()),
                Some("C:/tmp/qualia-explore-no-such-prior".to_string()),
                None
            )
            .is_none(),
            "a rejected artifact stays off"
        );
    }

    /// The dial the manifest hands down is read at the coupling's bounds: a
    /// hand-edited manifest cannot drive the planner's prior to zero or to
    /// infinity.
    #[test]
    fn the_dial_is_read_at_the_coupling_bounds() {
        use qualia_jepa::prior::{COUPLING_SCALE_CEILING, COUPLING_SCALE_FLOOR};

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("brain")
            .join("prior")
            .to_string_lossy()
            .into_owned();
        for (raw, expected) in [
            ("0", COUPLING_SCALE_FLOOR),
            ("inf", COUPLING_SCALE_CEILING),
            ("nan", COUPLING_SCALE_DEFAULT),
            ("2.5", 2.5),
        ] {
            let prior = FlyPrior::from_settings(
                Some("prior".to_string()),
                Some(path.clone()),
                Some(raw.to_string()),
            )
            .unwrap_or_else(|| panic!("the committed prior loads for dial {raw:?}"));
            assert_eq!(prior.scale, expected, "dial {raw:?}");
        }

        let unset = FlyPrior::from_settings(Some("prior".to_string()), Some(path), None)
            .expect("the committed prior loads with no dial");
        assert_eq!(
            unset.scale, COUPLING_SCALE_DEFAULT,
            "an unset dial is the identity"
        );
    }

    #[test]
    fn belief_risk_is_written_only_with_a_prior() {
        let grid = PlanGrid {
            cells: vec![UNKNOWN; PLAN_W * PLAN_H],
            resolution_m: 0.8,
        };

        let ungoverned = serde_json::to_value(plan_request(&grid, (16, 16), (10, 10), None))
            .expect("request serialises");
        assert!(
            ungoverned.get("belief_risk").is_none(),
            "no prior leaves the field unset"
        );

        let governed = serde_json::to_value(plan_request(
            &grid,
            (16, 16),
            (10, 10),
            Some(PlannerBeliefRiskContext {
                uncertainty_weight: 0.25,
                semantic_novelty: 0.0,
            }),
        ))
        .expect("request serialises");
        assert_eq!(governed["belief_risk"]["uncertainty_weight"], 0.25);
        assert_eq!(governed["belief_risk"]["semantic_novelty"], 0.0);
    }

    #[test]
    fn no_prior_and_an_empty_prior_leave_the_context_absent() {
        assert_eq!(PlannerBeliefRiskContext::from_prior(None), None);
        let empty = CouplingPrior {
            type_count: 0,
            rowptr: vec![0],
            cols: Vec::new(),
            weights: Vec::new(),
        };
        assert_eq!(PlannerBeliefRiskContext::from_prior(Some((&empty, COUPLING_SCALE_DEFAULT))), None);
    }

    #[tokio::test]
    async fn accepted_plan_publishes_the_goal_and_the_contract_request() {
        let planner = fake_planner(|_| ok_reply(5)).await;
        let shm = region("happy");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, block);

        let outcome = explore_once(
            &shm,
            &planner.socket.addr,
            300,
            ExploreTuning::from_env(),
            Some(PlannerBeliefRiskContext {
                uncertainty_weight: 0.5,
                semantic_novelty: 0.0,
            }),
        )
        .await
        .expect("the frontier is plannable");

        assert_eq!(outcome.path_len, 5);
        assert_eq!(outcome.frontier_size, 4 * 96 - 4);

        let goal = shm.world_model().nav_goal;
        assert_eq!(goal.active, 1);
        assert_eq!(goal.cell_x, outcome.goal_cell_x);
        assert_eq!(goal.cell_z, outcome.goal_cell_z);
        assert_eq!(goal.x_m, outcome.goal_x_m);
        assert_eq!(goal.z_m, outcome.goal_z_m);
        assert_eq!(goal.y_m, 0.0);
        assert_eq!(goal.yaw_rad, 0.7);
        assert!(goal.timestamp_ns > 0);

        let lines = planner.lines.lock().expect("planner lines");
        assert_eq!(lines.len(), 1, "exactly one candidate was accepted");
        assert!(lines[0].ends_with('\n'), "requests are newline delimited");
        let request: Value = serde_json::from_str(lines[0].trim()).expect("request is JSON");

        assert_eq!(request["schema_version"], "compute.v1");
        assert_eq!(request["request_type"], "plan_path");
        assert!(request["request_id"]
            .as_str()
            .expect("request id")
            .starts_with("explore_"));
        assert!(request["timestamp_ns"].as_u64().expect("timestamp") > 0);

        assert_eq!(request["grid"]["width"], 32);
        assert_eq!(request["grid"]["depth"], 32);
        assert!(
            (request["grid"]["resolution_m"].as_f64().expect("resolution") - 0.8).abs() < 1e-6
        );
        let occupied = request["grid"]["occupied"].as_array().expect("occupied");
        let cost = request["grid"]["cost"].as_array().expect("cost");
        assert_eq!(occupied.len(), PLAN_W * PLAN_H);
        assert_eq!(cost.len(), PLAN_W * PLAN_H);
        assert_eq!(occupied[12 * 32 + 12], 1, "the buried obstacle is mapped");
        assert_eq!(cost[12 * 32 + 12], 255);
        assert_eq!(occupied[16 * 32 + 16], 0, "the robot cell is free");
        assert_eq!(cost[16 * 32 + 16], 0);
        assert_eq!(cost[0], 12, "unknown space is expensive, not blocked");

        assert_eq!(request["start"]["cell_x"], 16);
        assert_eq!(request["start"]["cell_z"], 16);
        assert_eq!(request["goal"]["cell_x"], outcome.goal_cell_x);
        assert_eq!(request["goal"]["cell_z"], outcome.goal_cell_z);
        assert_eq!(request["constraints"]["robot_radius_cells"], 1);
        assert_eq!(request["constraints"]["allow_unknown"], false);
        assert_eq!(request["constraints"]["max_iterations"], 50_000);
        assert_eq!(request["world_context"]["nav_seq"], 0);
        assert_eq!(request["world_context"]["voxel_seq"], 0);
        assert_eq!(request["world_context"]["footprint_seq"], 0);
        assert_eq!(request["belief_risk"]["uncertainty_weight"], 0.5);
        assert_eq!(request["belief_risk"]["semantic_novelty"], 0.0);
    }

    #[tokio::test]
    async fn a_rejected_frontier_falls_through_to_the_next() {
        let planner =
            fake_planner(|index| if index == 0 { error_reply("no_path") } else { ok_reply(5) }).await;
        let shm = region("fallthrough");
        // Fine cell (128, 80) sits inside the connecting corridor, so both
        // blobs are reachable while neither is within the stand-off radius.
        shm.set_robot_pose(pose(0.05, -4.75));
        publish_map(&shm, two_blobs);

        let outcome = explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .expect("the second candidate is planned");

        let lines = planner.lines.lock().expect("planner lines");
        assert_eq!(lines.len(), 2, "the loop moved on after the rejection");
        let first: Value = serde_json::from_str(lines[0].trim()).expect("first request");
        let second: Value = serde_json::from_str(lines[1].trim()).expect("second request");
        assert_ne!(
            (
                first["goal"]["cell_x"].clone(),
                first["goal"]["cell_z"].clone()
            ),
            (
                second["goal"]["cell_x"].clone(),
                second["goal"]["cell_z"].clone()
            ),
            "the retry targets a different frontier"
        );
        assert_eq!(second["goal"]["cell_x"], outcome.goal_cell_x);
        assert_eq!(shm.world_model().nav_goal.active, 1);
    }

    #[tokio::test]
    async fn a_fresh_goal_is_held_without_replanning() {
        let planner = fake_planner(|_| ok_reply(5)).await;
        let shm = region("hold");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, block);

        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_some());
        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_none());
        assert_eq!(
            planner.lines.lock().expect("planner lines").len(),
            1,
            "the held goal must not be replanned"
        );
    }

    #[tokio::test]
    async fn an_expired_goal_is_replanned() {
        let planner = fake_planner(|_| ok_reply(5)).await;
        let shm = region("stale");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, block);
        let mut tuning = ExploreTuning::from_env();
        tuning.goal_hold_ns = 0;

        assert!(explore_once(&shm, &planner.socket.addr, 300, tuning, None)
            .await
            .is_some());
        assert!(explore_once(&shm, &planner.socket.addr, 300, tuning, None)
            .await
            .is_some());
        assert_eq!(planner.lines.lock().expect("planner lines").len(), 2);
    }

    #[tokio::test]
    async fn an_empty_map_yields_no_goal() {
        let planner = fake_planner(|_| ok_reply(5)).await;
        let shm = region("empty");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, |_, _| UNKNOWN);

        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_none());
        assert_eq!(shm.world_model().nav_goal.active, 0);
        assert!(
            planner.lines.lock().expect("planner lines").is_empty(),
            "no candidate reaches the planner"
        );
    }

    #[tokio::test]
    async fn an_unlocalised_robot_plans_nothing() {
        let planner = fake_planner(|_| ok_reply(5)).await;
        let shm = region("nopose");
        let mut unlocalised = pose(0.0, 0.0);
        unlocalised.confidence = 0.0;
        shm.set_robot_pose(unlocalised);
        publish_map(&shm, block);

        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_none());
        assert!(planner.lines.lock().expect("planner lines").is_empty());
    }

    #[tokio::test]
    async fn a_pose_without_a_timestamp_plans_nothing() {
        let planner = fake_planner(|_| ok_reply(5)).await;
        let shm = region("notimestamp");
        let mut unstamped = pose(0.0, 0.0);
        unstamped.timestamp_ns = 0;
        shm.set_robot_pose(unstamped);
        publish_map(&shm, block);

        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_none());
        assert!(planner.lines.lock().expect("planner lines").is_empty());
    }

    #[tokio::test]
    async fn a_rejected_candidate_leaves_the_goal_untouched() {
        let planner = fake_planner(|_| error_reply("no_path")).await;
        let shm = region("reject");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, block);

        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_none());
        assert_eq!(shm.world_model().nav_goal.active, 0);
        assert_eq!(
            planner.lines.lock().expect("planner lines").len(),
            1,
            "the single frontier was tried"
        );
    }

    #[tokio::test]
    async fn a_too_short_path_is_rejected() {
        let planner = fake_planner(|_| ok_reply(1)).await;
        let shm = region("short");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, block);

        assert!(explore_once(&shm, &planner.socket.addr, 300, ExploreTuning::from_env(), None)
            .await
            .is_none());
        assert_eq!(shm.world_model().nav_goal.active, 0);
        assert_eq!(planner.lines.lock().expect("planner lines").len(), 1);
    }

    #[tokio::test]
    async fn frontier_clusters_require_reachability() {
        let shm = region("clusters");
        shm.set_robot_pose(pose(0.0, 0.0));
        publish_map(&shm, |x, z| {
            let block = (80..176).contains(&x) && (80..176).contains(&z);
            let island = (200..216).contains(&x) && (200..216).contains(&z);
            if block || island {
                FREE
            } else {
                UNKNOWN
            }
        });

        let map = shm.binary_map();
        let clusters = frontier_clusters(map, 0.0, 0.0, ExploreTuning::from_env());
        assert_eq!(
            clusters.len(),
            1,
            "the walled-off island cannot be reached from the robot"
        );
        assert_eq!(clusters[0].cells.len(), 4 * 96 - 4);
    }

    #[tokio::test]
    async fn a_silent_planner_hits_the_read_deadline() {
        let planner = silent_planner().await;
        let started = Instant::now();
        let err = ask_planner(&planner.addr, 150, &request_for_test())
            .await
            .expect_err("a silent planner must not satisfy the request");
        let elapsed = started.elapsed();

        assert_eq!(err, "read timeout");
        assert!(
            elapsed.as_millis() >= 100,
            "returned before the deadline: {elapsed:?}"
        );
        assert!(
            elapsed.as_millis() < 3_000,
            "returned far past the deadline: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn a_closed_compute_socket_is_a_connect_error() {
        let (listener, socket) = bind_planner("closed").await;
        drop(listener);
        let err = ask_planner(&socket.addr, 300, &request_for_test())
            .await
            .expect_err("a closed socket must not be used");
        assert!(err.starts_with("connect"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn a_garbage_reply_is_a_decode_error() {
        let planner = fake_planner(|_| "not json".to_string()).await;
        let err = ask_planner(&planner.socket.addr, 300, &request_for_test())
            .await
            .expect_err("garbage must not decode");
        assert!(err.starts_with("decode"), "unexpected error: {err}");
    }
}
