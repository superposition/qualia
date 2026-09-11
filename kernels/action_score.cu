// action_score.cu
//
// Scores rollout candidates on the two terms the planner's cost struct exposes
// first: `terminal_goal_distance_m` and `max_collision_probability`. It is the
// device twin of the per-step midpoint integration and central-collision
// footprint in `qualia_jepa_model`'s rollout planner.
//
// One block per candidate. A candidate's steps are its own action sequence
// (left, right, speed_scale, dt each) and one 64 x 64 grounding map of
// occupancy logits per step. The block walks the steps in order because the
// pose is stateful, but it splits each step's grounding map across its threads
// and reduces the worst central collision with a fixed shared-memory tree, so
// the block does the same deterministic work regardless of scheduling.
//
// The pose update is the reference's: wheel speeds from the action, linear and
// angular rates, then a half-step yaw midpoint for the translation so a
// turning step does not cut the corner. The footprint is the reference's:
// cells whose centre is within `collision_radius_m` of the grounding origin,
// their logits through the numerically stable sigmoid. Terminal distance is
// the reference's too: the L2 error against the goal, less the goal's own
// tolerance, floored at zero.
//
// Launch shape: one block of `ACTION_SCORE_BLOCK` threads per candidate.
// NVRTC compiles this at process start, so the file includes no CUDA header.

constexpr unsigned int GROUNDING_W = 64;
constexpr unsigned int GROUNDING_H = 64;
constexpr unsigned int GROUNDING_CELLS = GROUNDING_W * GROUNDING_H;
constexpr unsigned int ACTION_SCORE_BLOCK = 256;

static_assert(GROUNDING_W == 64u, "grounding map width");
static_assert(GROUNDING_H == 64u, "grounding map height");

// The reference's branch on the sign keeps expf from overflowing.
__device__ float sigmoid(float value) {
    if (value >= 0.0f) {
        return 1.0f / (1.0f + expf(-value));
    }
    const float exponent = expf(value);
    return exponent / (1.0f + exponent);
}

__device__ float wrap_angle(float angle) {
    const float pi = 3.14159265358979323846f;
    const float tau = 6.28318530717958647692f;
    while (angle > pi) angle -= tau;
    while (angle < -pi) angle += tau;
    return angle;
}

extern "C" __global__ void action_score(
    const float* steps,                     // [total_steps * 4] left, right, speed_scale, dt
    const unsigned int* candidate_offsets,  // [candidate_count + 1] first step of each candidate
    const float* occupancy,                 // [total_steps * GROUNDING_CELLS]
    float* terminal_goal_distance_m,        // [candidate_count]
    float* max_collision_probability,       // [candidate_count]
    unsigned int candidate_count,
    float max_wheel_speed_mps,
    float track_width_m,
    float resolution_m,
    float collision_radius_m,
    float goal_lateral_m,
    float goal_forward_m,
    float goal_tolerance_m)
{
    const unsigned int candidate = blockIdx.x;
    if (candidate >= candidate_count) return;

    const unsigned int lane = threadIdx.x;
    const unsigned int begin = candidate_offsets[candidate];
    const unsigned int end = candidate_offsets[candidate + 1u];

    __shared__ float step_collision[ACTION_SCORE_BLOCK];

    float lateral_m = 0.0f;
    float forward_m = 0.0f;
    float yaw_rad = 0.0f;
    float worst = 0.0f;

    for (unsigned int step = begin; step < end; ++step) {
        const float* logits = occupancy + (unsigned long long)step * GROUNDING_CELLS;

        float local = 0.0f;
        for (unsigned int cell = lane; cell < GROUNDING_CELLS; cell += blockDim.x) {
            const unsigned int row = cell / GROUNDING_W;
            const unsigned int column = cell % GROUNDING_W;
            const float x_m = ((float)column + 0.5f - 0.5f * (float)GROUNDING_W) * resolution_m;
            const float z_m = ((float)row + 0.5f - 0.5f * (float)GROUNDING_H) * resolution_m;
            if (sqrtf(x_m * x_m + z_m * z_m) <= collision_radius_m) {
                local = fmaxf(local, sigmoid(logits[cell]));
            }
        }

        step_collision[lane] = local;
        __syncthreads();
        for (unsigned int span = blockDim.x >> 1; span > 0u; span >>= 1) {
            if (lane < span) {
                step_collision[lane] = fmaxf(step_collision[lane], step_collision[lane + span]);
            }
            __syncthreads();
        }

        if (lane == 0u) {
            worst = fmaxf(worst, step_collision[0]);

            const float left = steps[4u * step];
            const float right = steps[4u * step + 1u];
            const float speed_scale = steps[4u * step + 2u];
            const float dt_seconds = steps[4u * step + 3u];

            const float left_mps = left * speed_scale * max_wheel_speed_mps;
            const float right_mps = right * speed_scale * max_wheel_speed_mps;
            const float linear_mps = (left_mps + right_mps) * 0.5f;
            const float angular_rps = (right_mps - left_mps) / track_width_m;
            const float midpoint_yaw = yaw_rad + angular_rps * dt_seconds * 0.5f;
            lateral_m += linear_mps * dt_seconds * sinf(midpoint_yaw);
            forward_m += linear_mps * dt_seconds * cosf(midpoint_yaw);
            yaw_rad = wrap_angle(yaw_rad + angular_rps * dt_seconds);
        }
        // Keep the next step's write to the shared tree from racing this one.
        __syncthreads();
    }

    if (lane == 0u) {
        const float dx = lateral_m - goal_lateral_m;
        const float dy = forward_m - goal_forward_m;
        const float distance = sqrtf(dx * dx + dy * dy) - goal_tolerance_m;
        terminal_goal_distance_m[candidate] = fmaxf(0.0f, distance);
        max_collision_probability[candidate] = worst;
    }
}
