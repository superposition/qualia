#include <metal_stdlib>
using namespace metal;

// One thread per belief dimension. The struct mirrors qualia_types::BeliefSlot,
// which is repr(C, align(64)); every field must stay in the order declared
// there because the host and the GPU share the mapped bytes verbatim.

constant uint STATE_DIM = 1024;
constant uint WEIGHT_STRIDE = 1024;

struct BeliefSlot {
    float mean[STATE_DIM];
    float precision[STATE_DIM];
    float vfe;
    float prediction[STATE_DIM];
    float residual[STATE_DIM];
    float challenge_vfe;
    uint  confirm_streak;
    uchar compression;
    uchar layer;
    uchar pad0[2];
    ulong timestamp_ns;
    uint  cycle_us;
    uchar pad1[4];
};

constant float RESIDUAL_BOUND = 10.0f;
constant float PRECISION_CEILING = 10.0f;
constant float PRECISION_FLOOR = 0.01f;
constant float BELIEF_BOUND = 10.0f;
constant float WEIGHT_BOUND = 10.0f;
constant float STEP_BOUND = 0.1f;
constant float GRADIENT_BOUND = 0.01f;

// params[0] = VFE threshold
// params[1] = belief learning rate
// params[2] = layer id (unused by the arithmetic, kept for traceability)
// params[3] = weight decay
kernel void belief_update(
    device BeliefSlot*       me      [[buffer(0)]],
    const device BeliefSlot* below   [[buffer(1)]],
    const device float*      params  [[buffer(2)]],
    device float*            weights [[buffer(3)]],
    device float*            bias    [[buffer(4)]],
    uint tid [[thread_position_in_grid]])
{
    if (tid >= STATE_DIM) {
        return;
    }

    const float threshold = params[0];
    const float belief_rate = params[1];
    const float weight_decay = params[3];
    const float weight_rate = belief_rate * 0.1f;

    threadgroup float prior_mean[STATE_DIM];
    threadgroup float squared_error[STATE_DIM];

    prior_mean[tid] = me->mean[tid];
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // 1. Predict this layer's mean from the previous mean of the layer below.
    float prediction = bias[tid];
    for (uint column = 0; column < STATE_DIM; ++column) {
        prediction += weights[tid * WEIGHT_STRIDE + column] * prior_mean[column];
    }
    me->prediction[tid] = prediction;

    // 2. Prediction error against the layer below.
    const float residual = below->mean[tid] - prediction;
    me->residual[tid] = residual;

    // 3. Precision-weighted squared error, bounded so one wild sample cannot
    //    dominate the free-energy sum.
    const float precision = me->precision[tid];
    const float bounded_residual = clamp(residual, -RESIDUAL_BOUND, RESIDUAL_BOUND);
    squared_error[tid] = bounded_residual * min(precision, PRECISION_CEILING) * bounded_residual;
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // 4. Reduce the per-dimension errors into the layer's free energy.
    for (uint stride = STATE_DIM / 2; stride > 0; stride >>= 1) {
        if (tid < stride) {
            squared_error[tid] += squared_error[tid + stride];
        }
        threadgroup_barrier(mem_flags::mem_threadgroup);
    }
    const float total_vfe = squared_error[0];

    // 5. One thread updates the scalar bookkeeping so the fields stay coherent.
    if (tid == 0) {
        me->vfe = total_vfe;
        me->challenge_vfe = total_vfe;
        if (total_vfe <= threshold) {
            me->confirm_streak += 1;
            if (me->compression < 255 && me->confirm_streak > 100) {
                me->compression += 1;
            }
        } else {
            me->confirm_streak = 0;
            if (me->compression > 0) {
                me->compression -= 1;
            }
        }
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // 6. Below threshold the layer is settled; above it, beliefs move toward
    //    the layer below and the generative weights take one gradient step.
    if (total_vfe > threshold) {
        const float effective_precision = min(precision, 1.0f);
        const float belief_step = clamp(
            belief_rate * effective_precision * residual,
            -STEP_BOUND,
            STEP_BOUND);
        me->mean[tid] = clamp(me->mean[tid] + belief_step, -BELIEF_BOUND, BELIEF_BOUND);

        const float gradient = clamp(
            weight_rate * effective_precision * residual,
            -GRADIENT_BOUND,
            GRADIENT_BOUND);
        for (uint column = 0; column < STATE_DIM; ++column) {
            const uint index = tid * WEIGHT_STRIDE + column;
            float weight = weights[index] * (1.0f - weight_decay);
            weight += gradient * prior_mean[column];
            weights[index] = clamp(weight, -WEIGHT_BOUND, WEIGHT_BOUND);
        }
        const float bias_step = clamp(
            weight_rate * effective_precision * residual,
            -GRADIENT_BOUND,
            GRADIENT_BOUND);
        bias[tid] = clamp(bias[tid] * (1.0f - weight_decay) + bias_step, -WEIGHT_BOUND, WEIGHT_BOUND);
    }

    // 7. A non-finite belief would poison every later tick; reset that
    //    dimension to the observation and lose confidence in it.
    if (isnan(me->mean[tid]) || isinf(me->mean[tid])) {
        me->mean[tid] = below->mean[tid];
        me->precision[tid] = PRECISION_FLOOR;
    }

    // 8. Confidence tracks how well the last prediction held up.
    if (fabs(residual) < 0.01f) {
        me->precision[tid] = min(precision * 1.001f, PRECISION_CEILING);
    } else {
        me->precision[tid] = max(precision * 0.999f, PRECISION_FLOOR);
    }
}
