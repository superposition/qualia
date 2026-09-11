//! CPU/CUDA parity for the three perception/action kernels.
//!
//! This is `crates/jepa-model/src/parity.rs`'s pattern carried to the device
//! path: for each kernel the CPU oracle in `qualia_cuda::cpu` and the CUDA
//! context run the same input, and every value the device produces has to agree
//! with the oracle within `1e-4` relative error for f32. Each kernel is
//! exercised over four input regimes — fixed, empty, maximum-size and
//! deterministic-random — so a divergence that only appears in one shape of
//! input cannot hide behind a single fixture.
//!
//! Every test here requires the `cuda` feature and opens the device itself; a
//! host with no CUDA adapter skips with a message rather than failing, the same
//! contract `gpu.rs` uses.

#![cfg(feature = "cuda")]

use qualia_cuda::{
    cpu, ActionScoreContext, BeliefCoupleContext, PerceptionVoxelContext, JEPA_OCCUPANCY_CELLS,
    STATE_DIM, VOXEL_D, VOXEL_W,
};

/// Largest relative error a compared device/host f32 pair may show.
const PARITY_TOLERANCE: f32 = 1.0e-4;

/// Relative error of one pair: `|device - host| / max(|device|, |host|)`.
///
/// The two sides run the same arithmetic and usually agree bit-for-bit — an
/// exact zero, which no ratio can divide, is the common case — so a pair that
/// is equal has no error. Anything else is measured against the larger
/// magnitude, so a value that legitimately sits near zero still has to be
/// reproduced rather than merely approximated in absolute terms.
fn relative_error(device: f32, host: f32) -> f32 {
    if device == host {
        return 0.0;
    }
    (device - host).abs() / device.abs().max(host.abs())
}

/// Assert every device value lies within `PARITY_TOLERANCE` of its oracle value.
fn assert_parity(label: &str, device: &[f32], host: &[f32]) {
    assert_eq!(
        device.len(),
        host.len(),
        "{label}: device has {} values, host has {}",
        device.len(),
        host.len()
    );
    let mut worst = 0.0f32;
    let mut worst_index = 0usize;
    for (index, (device_value, host_value)) in device.iter().zip(host.iter()).enumerate() {
        assert!(
            device_value.is_finite() && host_value.is_finite(),
            "{label}[{index}]: non-finite device {device_value} host {host_value}"
        );
        let error = relative_error(*device_value, *host_value);
        if error > worst {
            worst = error;
            worst_index = index;
        }
        assert!(
            error <= PARITY_TOLERANCE,
            "{label}[{index}]: device {device_value} host {host_value} relative error \
             {error:e} exceeds {PARITY_TOLERANCE:e}"
        );
    }
    println!(
        "{label}: {} values, worst relative error {worst:e} at [{worst_index}]",
        device.len()
    );
}

#[test]
fn parity_within_tolerance() {
    let contexts = (
        BeliefCoupleContext::new(),
        PerceptionVoxelContext::new(),
        ActionScoreContext::new(),
    );
    let (couple, voxel, score) = match contexts {
        (Ok(couple), Ok(voxel), Ok(score)) => (couple, voxel, score),
        (couple, voxel, score) => {
            let error = [couple.err(), voxel.err(), score.err()]
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_else(|| "unknown CUDA error".to_string());
            eprintln!("skipping CUDA parity test: {error}");
            return;
        }
    };

    assert!(!couple.device_name().is_empty());
    assert!(!voxel.device_name().is_empty());
    assert!(!score.device_name().is_empty());

    parity_belief_couple(&couple);
    parity_perception_voxel(&voxel);
    parity_action_score(&score);
}

// ── Small deterministic generator so the tests do not depend on `rand` ───────

/// A fixed-seed xorshift generator; the same sequence on every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u32(&mut self) -> u32 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        (state >> 32) as u32
    }

    /// A value in `[0, 1)` with 24 bits of entropy, the f32 mantissa.
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u32() as usize) % bound
    }
}

// ── belief_couple ────────────────────────────────────────────────────────────

/// One belief-couple case: the prior graph, the belief and the mapped slots.
struct CoupleCase {
    belief: Vec<f32>,
    cols: Vec<u32>,
    weights: Vec<u32>,
    slots: Vec<(u32, usize)>,
}

/// The connectome graph from `cpu_fallback.rs`: in-strength 4 / 8 / 2 against a
/// peak of 8, so factors 0.5 / 1.0 / 0.25 and a type no edge reaches, which
/// couples at zero. Slots include a repeat and two out-of-range pairs the
/// oracle and the kernel both ignore.
fn couple_fixed() -> CoupleCase {
    CoupleCase {
        belief: vec![10.0, 20.0, 40.0, -8.0],
        cols: vec![1, 1, 2, 0],
        weights: vec![3, 5, 2, 4],
        slots: vec![(0, 1), (1, 2), (2, 3), (3, 0), (1, 0), (7, 2), (2, 9)],
    }
}

/// An empty edge list names no type, so nothing couples.
fn couple_empty_graph() -> CoupleCase {
    CoupleCase {
        belief: vec![1.0, -2.0, 3.5],
        cols: Vec::new(),
        weights: Vec::new(),
        slots: vec![(0, 0), (1, 1), (2, 2)],
    }
}

/// A graph with no mapped slots applies nothing.
fn couple_empty_slots() -> CoupleCase {
    CoupleCase {
        belief: vec![1.0, -2.0, 3.5],
        cols: vec![0, 1, 2, 1],
        weights: vec![3, 1, 4, 1],
        slots: Vec::new(),
    }
}

/// The largest dispatch the context accepts: one 1024-thread block, a 2048-entry
/// type table in shared memory, and more edges than types to reduce.
fn couple_max() -> CoupleCase {
    const SLOTS: usize = 1024;
    const TYPES: usize = 2048;
    const EDGES: usize = 8192;
    let mut rng = Rng::new(0xbe11_ef0f);
    let belief: Vec<f32> = (0..STATE_DIM).map(|_| rng.range(-4.0, 4.0)).collect();
    let cols: Vec<u32> = (0..EDGES)
        .map(|edge| ((edge * 7 + 3) % TYPES) as u32)
        .collect();
    let weights: Vec<u32> = (0..EDGES).map(|edge| (edge % 9) as u32 + 1).collect();
    let slots: Vec<(u32, usize)> = (0..SLOTS).map(|slot| ((slot % TYPES) as u32, slot)).collect();
    CoupleCase {
        belief,
        cols,
        weights,
        slots,
    }
}

/// A deterministic-random graph, belief and slot mapping, with one pair outside
/// the graph and one outside the belief so the ignore rule is part of parity.
fn couple_random() -> CoupleCase {
    const TYPES: usize = 37;
    let mut rng = Rng::new(0x517e_c0de);
    let belief: Vec<f32> = (0..128).map(|_| rng.range(-4.0, 4.0)).collect();
    let cols: Vec<u32> = (0..512).map(|_| rng.below(TYPES) as u32).collect();
    let weights: Vec<u32> = (0..512).map(|_| rng.below(9) as u32 + 1).collect();
    let mut slots: Vec<(u32, usize)> = (0..96)
        .map(|_| (rng.below(TYPES) as u32, rng.below(128)))
        .collect();
    slots.push((TYPES as u32 + 4, 3));
    slots.push((1, 128 + 7));
    CoupleCase {
        belief,
        cols,
        weights,
        slots,
    }
}

fn parity_belief_couple(context: &BeliefCoupleContext) {
    let cases = [
        ("fixed", couple_fixed()),
        ("empty-graph", couple_empty_graph()),
        ("empty-slots", couple_empty_slots()),
        ("maximum-size", couple_max()),
        ("deterministic-random", couple_random()),
    ];
    for (regime, case) in cases {
        let mut host = case.belief.clone();
        let applied = cpu::belief_couple(&mut host, &case.cols, &case.weights, &case.slots);
        let (device, factors) = context
            .run(&case.belief, &case.cols, &case.weights, &case.slots)
            .unwrap_or_else(|error| panic!("belief_couple/{regime}: {error}"));
        assert_parity(&format!("belief_couple/{regime}/belief"), &device, &host);
        let factor_sum: f32 = factors.iter().sum();
        assert_parity(
            &format!("belief_couple/{regime}/applied"),
            &[factor_sum],
            &[applied],
        );
    }
}

// ── perception_voxel ─────────────────────────────────────────────────────────

/// One perception-voxel case: the flat sweep and the projection parameters.
struct VoxelCase {
    points: Vec<f32>,
    resolution_m: f32,
    prior_logit: f32,
    hit_log_odds: f32,
}

/// A handful of returns, including one at a cell centre.
fn voxel_fixed() -> VoxelCase {
    VoxelCase {
        // (0.125, 2.625) is the centre of ground cell vx = 16, vz = 10; the
        // second return falls in a different column.
        points: vec![0.125, 2.625, -3.875, 0.125],
        resolution_m: 0.25,
        prior_logit: -2.0,
        hit_log_odds: 1.5,
    }
}

/// An empty sweep marks no cell.
fn voxel_empty() -> VoxelCase {
    VoxelCase {
        points: Vec::new(),
        resolution_m: 0.25,
        prior_logit: -2.0,
        hit_log_odds: 1.5,
    }
}

fn voxel_cell_centre(voxel_x: usize, voxel_z: usize, resolution_m: f32) -> (f32, f32) {
    let centre_x = (voxel_x as f32 + 0.5 - 0.5 * VOXEL_W as f32) * resolution_m;
    let centre_z = (voxel_z as f32 + 0.5) * resolution_m;
    (centre_x, centre_z)
}

/// One return at every ground cell centre, so the whole `VOXEL_W * VOXEL_D`
/// plane is covered, plus enough returns in one cell to reach the +20 logit
/// clamp; a denser sweep only raises per-cell counts.
fn voxel_max() -> VoxelCase {
    const CLAMP_EXTRA: usize = 18;
    let resolution_m = 0.25f32;
    let mut points = Vec::with_capacity(2 * (VOXEL_W * VOXEL_D + CLAMP_EXTRA));
    for voxel_x in 0..VOXEL_W {
        for voxel_z in 0..VOXEL_D {
            let (x, z) = voxel_cell_centre(voxel_x, voxel_z, resolution_m);
            points.push(x);
            points.push(z);
        }
    }
    for _ in 0..CLAMP_EXTRA {
        let (x, z) = voxel_cell_centre(7, 19, resolution_m);
        points.push(x);
        points.push(z);
    }
    VoxelCase {
        points,
        resolution_m,
        prior_logit: -3.0,
        hit_log_odds: 1.25,
    }
}

/// A deterministic-random sweep over the ground plane.
fn voxel_random() -> VoxelCase {
    let mut rng = Rng::new(0x70b1_e1de);
    let mut points = Vec::with_capacity(2 * 500);
    for _ in 0..500 {
        points.push(rng.range(-4.0, 4.0));
        points.push(rng.range(0.0, 8.0));
    }
    VoxelCase {
        points,
        resolution_m: 0.25,
        prior_logit: -2.0,
        hit_log_odds: 1.5,
    }
}

fn parity_perception_voxel(context: &PerceptionVoxelContext) {
    let cases = [
        ("fixed", voxel_fixed()),
        ("empty", voxel_empty()),
        ("maximum-size", voxel_max()),
        ("deterministic-random", voxel_random()),
    ];
    for (regime, case) in cases {
        let host = cpu::perception_voxel(
            &case.points,
            case.resolution_m,
            case.prior_logit,
            case.hit_log_odds,
        )
        .unwrap_or_else(|error| panic!("perception_voxel/{regime} oracle: {error}"));
        let device = context
            .run(&case.points, case.resolution_m, case.prior_logit, case.hit_log_odds)
            .unwrap_or_else(|error| panic!("perception_voxel/{regime}: {error}"));
        assert_parity(&format!("perception_voxel/{regime}"), &device, &host);
    }
}

// ── action_score ─────────────────────────────────────────────────────────────

/// One action-score case: the candidates, their grounding maps and the goal.
struct ScoreCase {
    steps: Vec<[f32; 4]>,
    candidate_steps: Vec<usize>,
    occupancy: Vec<f32>,
    config: cpu::ActionScoreConfig,
}

fn score_config(goal_lateral_m: f32, goal_forward_m: f32) -> cpu::ActionScoreConfig {
    cpu::ActionScoreConfig {
        max_wheel_speed_mps: 1.4,
        track_width_m: 0.5,
        resolution_m: 0.25,
        collision_radius_m: 0.5,
        goal_lateral_m,
        goal_forward_m,
        goal_tolerance_m: 0.1,
    }
}

/// One grounding map per step: the prior everywhere, two cells inside the
/// central footprint raised, so the sigmoid and its reduction are exercised.
fn score_occupancy(steps: usize, peak: f32) -> Vec<f32> {
    let mut occupancy = vec![-3.0f32; steps * JEPA_OCCUPANCY_CELLS];
    for step in 0..steps {
        let base = step * JEPA_OCCUPANCY_CELLS;
        occupancy[base + 32 * 64 + 32] = peak;
        occupancy[base + 32 * 64 + 33] = peak * 0.5;
    }
    occupancy
}

/// Two hand-built candidates.
fn score_fixed() -> ScoreCase {
    let steps = vec![
        [0.6f32, 0.6, 0.9, 0.10],
        [0.4, -0.4, 0.7, 0.15],
        [0.5, 0.5, 1.0, 0.05],
        [0.3, 0.6, 0.8, 0.20],
        [-0.2, 0.5, 0.6, 0.12],
    ];
    ScoreCase {
        candidate_steps: vec![3, 2],
        occupancy: score_occupancy(steps.len(), 1.5),
        config: score_config(0.2, 3.0),
        steps,
    }
}

/// A candidate with no steps still scores its standing pose.
fn score_zero_steps() -> ScoreCase {
    ScoreCase {
        steps: Vec::new(),
        candidate_steps: vec![0],
        occupancy: Vec::new(),
        config: score_config(0.2, 3.0),
    }
}

/// The maximum a dispatch carries: the planner's 64 candidates of at most 16
/// steps each (`crates/jepa-model/src/planner.rs`), one grounding map per step.
fn score_max() -> ScoreCase {
    const CANDIDATES: usize = 64;
    const STEPS_PER_CANDIDATE: usize = 16;
    let mut rng = Rng::new(0xac71_0ac5);
    let steps: Vec<[f32; 4]> = (0..CANDIDATES * STEPS_PER_CANDIDATE)
        .map(|_| {
            [
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(0.3, 1.0),
                rng.range(0.05, 0.2),
            ]
        })
        .collect();
    let occupancy: Vec<f32> = (0..steps.len() * JEPA_OCCUPANCY_CELLS)
        .map(|_| rng.range(-6.0, 6.0))
        .collect();
    ScoreCase {
        steps,
        candidate_steps: vec![STEPS_PER_CANDIDATE; CANDIDATES],
        occupancy,
        config: score_config(0.0, 12.0),
    }
}

/// Deterministic-random candidates of 3 to 7 steps each.
fn score_random() -> ScoreCase {
    let mut rng = Rng::new(0x5c0e_e0de);
    let candidate_steps: Vec<usize> = (0..5).map(|_| rng.below(5) + 3).collect();
    let total: usize = candidate_steps.iter().sum();
    let steps: Vec<[f32; 4]> = (0..total)
        .map(|_| {
            [
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(0.3, 1.0),
                rng.range(0.05, 0.2),
            ]
        })
        .collect();
    let occupancy: Vec<f32> = (0..total * JEPA_OCCUPANCY_CELLS)
        .map(|_| rng.range(-6.0, 6.0))
        .collect();
    ScoreCase {
        steps,
        candidate_steps,
        occupancy,
        config: score_config(0.0, 12.0),
    }
}

/// The oracle's score for every candidate, in dispatch order.
fn host_scores(case: &ScoreCase) -> (Vec<f32>, Vec<f32>) {
    let mut distances = Vec::with_capacity(case.candidate_steps.len());
    let mut collisions = Vec::with_capacity(case.candidate_steps.len());
    let mut start = 0usize;
    for &count in &case.candidate_steps {
        let end = start + count;
        let flat: Vec<f32> = case.steps[start..end].iter().flatten().copied().collect();
        let maps = &case.occupancy[start * JEPA_OCCUPANCY_CELLS..end * JEPA_OCCUPANCY_CELLS];
        let score = cpu::action_score(&flat, maps, &case.config).expect("host action_score");
        distances.push(score.terminal_goal_distance_m);
        collisions.push(score.max_collision_probability);
        start = end;
    }
    (distances, collisions)
}

fn parity_action_score(context: &ActionScoreContext) {
    let cases = [
        ("fixed", score_fixed()),
        ("empty-zero-steps", score_zero_steps()),
        ("maximum-size", score_max()),
        ("deterministic-random", score_random()),
    ];
    for (regime, case) in cases {
        let device = context
            .run(&case.steps, &case.candidate_steps, &case.occupancy, &case.config)
            .unwrap_or_else(|error| panic!("action_score/{regime}: {error}"));
        let (host_distances, host_collisions) = host_scores(&case);
        assert_eq!(
            device.len(),
            case.candidate_steps.len(),
            "action_score/{regime}: one score per candidate"
        );
        let device_distances: Vec<f32> = device.iter().map(|s| s.terminal_goal_distance_m).collect();
        let device_collisions: Vec<f32> =
            device.iter().map(|s| s.max_collision_probability).collect();
        assert_parity(
            &format!("action_score/{regime}/terminal-goal-distance"),
            &device_distances,
            &host_distances,
        );
        assert_parity(
            &format!("action_score/{regime}/max-collision"),
            &device_collisions,
            &host_collisions,
        );
    }

    // No candidates at all is an empty dispatch on both paths.
    let case = ScoreCase {
        steps: Vec::new(),
        candidate_steps: Vec::new(),
        occupancy: Vec::new(),
        config: score_config(0.2, 3.0),
    };
    let device = context
        .run(&case.steps, &case.candidate_steps, &case.occupancy, &case.config)
        .expect("action_score/empty dispatch");
    assert!(
        device.is_empty(),
        "action_score/empty: {} scores for no candidates",
        device.len()
    );
    let (host_distances, host_collisions) = host_scores(&case);
    assert!(host_distances.is_empty() && host_collisions.is_empty());
}
