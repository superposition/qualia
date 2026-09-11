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

    __shared__ float prior_state[1024];
    __shared__ float local_error[1024];

    prior_state[tid] = state[tid];
    __syncthreads();

    float prediction = bias[tid];
    for (unsigned int column = 0; column < 1024; ++column) {
        prediction += weights[tid * 1024 + column] * prior_state[column];
    }
    local_error[tid] = fminf(10.0f, fmaxf(-10.0f, lower[tid] - prediction));
    __syncthreads();

    float transposed_grad = 0.0f;
    for (unsigned int row = 0; row < 1024; ++row) {
        transposed_grad += weights[row * 1024 + tid] * local_error[row];
    }
    const float candidate = prior_state[tid]
        + params[2] * params[0] * transposed_grad
        - params[2] * params[1] * (prior_state[tid] - top_down[tid]);
    state[tid] = fminf(4.0f, fmaxf(-4.0f, candidate));

    float row_step = params[3] * params[4] * params[0] * local_error[tid];
    row_step = fminf(0.001f, fmaxf(-0.001f, row_step));
    for (unsigned int column = 0; column < 1024; ++column) {
        const unsigned int index = tid * 1024 + column;
        const float moved = weights[index] + row_step * prior_state[column];
        weights[index] = fminf(2.0f, fmaxf(-2.0f, moved));
    }
    bias[tid] = fminf(1.0f, fmaxf(-1.0f, bias[tid] + row_step));
}

extern "C" __global__ void cognition_patch(float* weights, unsigned int index, float delta) {
    if (threadIdx.x == 0) {
        weights[index] = fminf(2.0f, fmaxf(-2.0f, weights[index] + delta));
    }
}
