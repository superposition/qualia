// cognition_update.cu
//
// The persistent L3-L6 cognition stack's two kernels: one predictive-coding
// update over a 1024-dimensional state and its 1024x1024 generative weights,
// and a single-weight patch the CRDT learning path applies.
//
// Launch shape: one block of 1024 threads, one thread per state dimension,
// with both per-block scratch arrays static.
//
// NVRTC compiles this at process start and nvcc may compile it at build time
// (`CUDAARCHS`), so it may not include any CUDA header.

/// One element of the cognition row step: the reference's fused sum, then its
/// bound. `fmaf` pins the contraction the scalar walk chose, so the vector walk
/// reproduces its values bit for bit.
static __device__ __forceinline__ float cognition_weight_element(
    float value,
    float step,
    float state)
{
    const float moved = fmaf(step, state, value);
    return fminf(2.0f, fmaxf(-2.0f, moved));
}

extern "C" __global__ void cognition_update(
    float* state,
    const float* lower,
    const float* top_down,
    float* weights,
    float* bias,
    const float* params)
{
    const unsigned int tid = threadIdx.x;
    if (tid >= 1024) return;

    // Both snapshots are read back four floats at a time below.
    __shared__ __align__(16) float prior_state[1024];
    __shared__ __align__(16) float local_error[1024];

    prior_state[tid] = state[tid];
    __syncthreads();

    float prediction = bias[tid];
    // Do not re-associate: ascending column order and the fmaf contraction are
    // the twin's contract (crates/cuda/src/cpu.rs) and D-008/D-009 make the
    // emitted bits interface.
    const float4* row4 = reinterpret_cast<const float4*>(weights + tid * 1024);
    const float4* state4 = reinterpret_cast<const float4*>(prior_state);
#pragma unroll 4
    for (unsigned int chunk = 0; chunk < 256; ++chunk) {
        const float4 weight = row4[chunk];
        const float4 snapshot = state4[chunk];
        prediction += weight.x * snapshot.x;
        prediction += weight.y * snapshot.y;
        prediction += weight.z * snapshot.z;
        prediction += weight.w * snapshot.w;
    }
    local_error[tid] = fminf(10.0f, fmaxf(-10.0f, lower[tid] - prediction));
    __syncthreads();

    // Each thread walks one column in ascending row order. The lanes already
    // read that column coalesced; four rows of errors now come back per
    // shared-memory read instead of one.
    float transposed_grad = 0.0f;
    const float4* error4 = reinterpret_cast<const float4*>(local_error);
#pragma unroll 4
    for (unsigned int tile = 0; tile < 256; ++tile) {
        const float4 error = error4[tile];
        const unsigned int row = tile * 4;
        transposed_grad += weights[row * 1024 + tid] * error.x;
        transposed_grad += weights[(row + 1) * 1024 + tid] * error.y;
        transposed_grad += weights[(row + 2) * 1024 + tid] * error.z;
        transposed_grad += weights[(row + 3) * 1024 + tid] * error.w;
    }
    const float candidate = prior_state[tid]
        + params[2] * params[0] * transposed_grad
        - params[2] * params[1] * (prior_state[tid] - top_down[tid]);
    state[tid] = fminf(4.0f, fmaxf(-4.0f, candidate));

    float row_step = params[3] * params[4] * params[0] * local_error[tid];
    row_step = fminf(0.001f, fmaxf(-0.001f, row_step));
    // The step is row-local, but it exists only once this row's dot product has
    // completed, so the row is walked a second time; four columns at a time,
    // and the value each chunk already loaded is the value it writes.
    // Do not re-associate: ascending column order and the fmaf contraction are
    // the twin's contract (crates/cuda/src/cpu.rs) and D-008/D-009 make the
    // emitted bits interface.
    float4* row4w = reinterpret_cast<float4*>(weights + tid * 1024);
#pragma unroll 4
    for (unsigned int chunk = 0; chunk < 256; ++chunk) {
        const float4 snapshot = state4[chunk];
        float4 weight = row4w[chunk];
        weight.x = cognition_weight_element(weight.x, row_step, snapshot.x);
        weight.y = cognition_weight_element(weight.y, row_step, snapshot.y);
        weight.z = cognition_weight_element(weight.z, row_step, snapshot.z);
        weight.w = cognition_weight_element(weight.w, row_step, snapshot.w);
        row4w[chunk] = weight;
    }
    bias[tid] = fminf(1.0f, fmaxf(-1.0f, bias[tid] + row_step));
}

extern "C" __global__ void cognition_patch(float* weights, unsigned int index, float delta) {
    if (threadIdx.x == 0) {
        weights[index] = fminf(2.0f, fmaxf(-2.0f, weights[index] + delta));
    }
}
