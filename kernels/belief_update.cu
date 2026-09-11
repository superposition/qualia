// belief_update.cu
//
// The per-layer predictive-coding update for the qualia belief stack.
// Two front ends read this file: NVRTC at process start, and nvcc at build
// time when `CUDAARCHS` asks for a fatbin. It may not include any CUDA header;
// device builtins and the device math functions are supplied by the compiler.
//
// Launch shape: one block of 1024 threads, one thread per belief dimension.
// Each thread predicts the layer below from the shared belief, forms a
// precision-weighted residual, contributes one term to the block-wide VFE sum,
// and then applies the belief, weight and precision updates. Everything is
// deterministic given the inputs: the only cross-thread communication is the
// VFE reduction, and that reduction is a fixed tree.
//
// The struct below is the device-side image of qualia_types::BeliefSlot. It is
// `repr(C, align(64))` on the Rust side. NVRTC has no `<cstddef>`, no
// `__builtin_offsetof`, and rejects the classic null-pointer
// `&((T*)0)->field` form in a constant expression, so this file pins the total
// size and the alignment — all NVRTC can assert. The per-field offsets are
// pinned from Rust, against these same constants, in
// `crates/cuda/tests/kernel_abi.rs`.

// `alignas(64)` is the standard spelling of the alignment: nvcc's MSVC host
// pass rejects the GNU `__attribute__((aligned(64)))` form, and both front
// ends are C++11 or newer.
struct alignas(64) QualiaBeliefSlot {
    float mean[1024];
    float precision[1024];
    float vfe;
    float prediction[1024];
    float residual[1024];
    float challenge_vfe;
    unsigned int confirm_streak;
    unsigned char compression;
    unsigned char layer;
    unsigned char pad0[2];
    unsigned long long timestamp_ns;
    unsigned int cycle_us;
    unsigned char pad1[4];
};

static_assert(sizeof(QualiaBeliefSlot) == 16448ULL, "BeliefSlot size");
static_assert(__alignof__(QualiaBeliefSlot) == 64ULL, "BeliefSlot alignment");

extern "C" __global__ void belief_update(
    QualiaBeliefSlot* belief,       // this layer, read and written
    const QualiaBeliefSlot* below,  // the layer underneath, read only
    const float* params,            // {threshold, learning_rate, layer_id, weight_decay}
    float* weights,                 // 1024 x 1024 generative matrix, row major
    float* bias)                    // 1024 generative biases
{
    const int tid = threadIdx.x;
    if (tid >= 1024) return;

    const float threshold = params[0];
    const float learning_rate = params[1];
    const float weight_decay = params[3];
    const float weight_rate = learning_rate * 0.1f;

    __shared__ float prior_mean[1024];
    __shared__ float vfe_terms[1024];

    // Snapshot the belief before any update: the weight update below is a
    // Hebbian step against the pre-update state, so it must read this copy.
    prior_mean[tid] = belief->mean[tid];
    __syncthreads();

    // 1. Predict the layer below: bias + W * mean. The row is written by the
    // weight step in (7), so it is not const.
    float* row = weights + tid * 1024;
    float prediction = bias[tid];
    for (int column = 0; column < 1024; ++column) {
        prediction += row[column] * prior_mean[column];
    }
    belief->prediction[tid] = prediction;

    // 2. Residual against the observed layer below.
    const float residual = below->mean[tid] - prediction;
    belief->residual[tid] = residual;

    // 3. Precision-weighted, clamped error term.
    const float precision = belief->precision[tid];
    const float clipped = fminf(fmaxf(residual, -10.0f), 10.0f);
    vfe_terms[tid] = clipped * fminf(precision, 10.0f) * clipped;
    __syncthreads();

    // 4. Block-wide VFE: a fixed binary tree over the 1024 terms.
    for (int span = 512; span > 0; span >>= 1) {
        if (tid < span) vfe_terms[tid] += vfe_terms[tid + span];
        __syncthreads();
    }
    const float vfe = vfe_terms[0];

    // 5. Thread zero owns the scalar fields.
    if (tid == 0) {
        belief->vfe = vfe;
        belief->challenge_vfe = vfe;
        if (vfe <= threshold) {
            belief->confirm_streak += 1;
            if (belief->compression < 255 && belief->confirm_streak > 100) {
                belief->compression += 1;
            }
        } else {
            belief->confirm_streak = 0;
            if (belief->compression > 0) belief->compression -= 1;
        }
    }
    __syncthreads();

    if (vfe > threshold) {
        const float gain = fminf(precision, 1.0f);

        // 6. Belief step toward the residual, bounded per tick.
        const float belief_step =
            fminf(fmaxf(learning_rate * gain * residual, -0.1f), 0.1f);
        const float moved = belief->mean[tid] + belief_step;
        belief->mean[tid] = fminf(fmaxf(moved, -10.0f), 10.0f);

        // 7. Weight and bias step, bounded and decayed.
        const float weight_step =
            fminf(fmaxf(weight_rate * gain * residual, -0.01f), 0.01f);
        for (int column = 0; column < 1024; ++column) {
            float updated = row[column] * (1.0f - weight_decay);
            updated += weight_step * prior_mean[column];
            row[column] = fminf(fmaxf(updated, -10.0f), 10.0f);
        }
        float updated_bias = bias[tid] * (1.0f - weight_decay) + weight_step;
        bias[tid] = fminf(fmaxf(updated_bias, -10.0f), 10.0f);
    }

    // 8. A non-finite belief is replaced by the observation rather than kept.
    if (isnan(belief->mean[tid]) || isinf(belief->mean[tid])) {
        belief->mean[tid] = below->mean[tid];
        belief->precision[tid] = 0.01f;
    }

    // 9. Precision follows the residual: firming up when predictable,
    // loosening when surprised.
    if (fabsf(residual) < 0.01f) {
        belief->precision[tid] = fminf(precision * 1.001f, 10.0f);
    } else {
        belief->precision[tid] = fmaxf(precision * 0.999f, 0.01f);
    }
}
