#include <metal_stdlib>
using namespace metal;

// One thread per state dimension of a cognition layer (L3-L6). The dense
// weight matrix is row-major: weights[row * STATE_DIM + column] maps the
// previous state onto row's prediction.

constant uint STATE_DIM = 1024;
constant uint WEIGHT_STRIDE = 1024;

constant float STATE_BOUND = 4.0f;
constant float WEIGHT_BOUND = 2.0f;
constant float BIAS_BOUND = 1.0f;
constant float ERROR_BOUND = 10.0f;
constant float GRADIENT_BOUND = 0.001f;

// params[0] = source precision
// params[1] = top-down precision
// params[2] = state learning rate
// params[3] = bounded weight learning rate
// params[4] = evidence-derived learning signal, already clamped to 0..1
kernel void cognition_update(
    device float*       state    [[buffer(0)]],
    const device float* lower    [[buffer(1)]],
    const device float* top_down [[buffer(2)]],
    device float*       weights  [[buffer(3)]],
    device float*       bias     [[buffer(4)]],
    const device float* params   [[buffer(5)]],
    uint tid [[thread_position_in_grid]])
{
    if (tid >= STATE_DIM) {
        return;
    }

    threadgroup float prior_state[STATE_DIM];
    threadgroup float bottom_up_error[STATE_DIM];

    prior_state[tid] = state[tid];
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Predict this dimension from every dimension of the previous state.
    float prediction = bias[tid];
    for (uint column = 0; column < STATE_DIM; ++column) {
        prediction += weights[tid * WEIGHT_STRIDE + column] * prior_state[column];
    }
    bottom_up_error[tid] = clamp(lower[tid] - prediction, -ERROR_BOUND, ERROR_BOUND);
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Predictive coding: descend Wᵀ·ε_bottom while resisting the top-down
    // prediction, then take one bounded step on this row of the weights.
    float transposed_error = 0.0f;
    for (uint row = 0; row < STATE_DIM; ++row) {
        transposed_error += weights[row * WEIGHT_STRIDE + tid] * bottom_up_error[row];
    }
    const float top_down_error = prior_state[tid] - top_down[tid];
    const float next = prior_state[tid]
        + params[2] * params[0] * transposed_error
        - params[2] * params[1] * top_down_error;
    state[tid] = clamp(next, -STATE_BOUND, STATE_BOUND);

    const float row_gradient = clamp(
        params[3] * params[4] * params[0] * bottom_up_error[tid],
        -GRADIENT_BOUND,
        GRADIENT_BOUND);
    for (uint column = 0; column < STATE_DIM; ++column) {
        const uint index = tid * WEIGHT_STRIDE + column;
        weights[index] = clamp(weights[index] + row_gradient * prior_state[column], -WEIGHT_BOUND, WEIGHT_BOUND);
    }
    bias[tid] = clamp(bias[tid] + row_gradient, -BIAS_BOUND, BIAS_BOUND);
}
