// costmap_stats.cu
//
// The occupancy/cost reduction behind the compute service's `costmap_stats`
// request: four counters over a row-major cell pair.
//
// Launch shape: one thread per cell, grid sized by the caller.
//
// NVRTC compiles this at process start and nvcc may compile it at build time
// (`CUDAARCHS`), so it may not include any CUDA header.

extern "C" __global__ void costmap_stats(
    const unsigned char* occupied,
    const unsigned char* cost,
    unsigned int length,
    unsigned int* counters)
{
    const unsigned int cell = blockIdx.x * blockDim.x + threadIdx.x;
    if (cell >= length) return;

    if (occupied[cell] != 0) {
        atomicAdd(&counters[0], 1u);
        atomicAdd(&counters[3], 1u);
    }
    if (cost[cell] >= 200u) {
        atomicAdd(&counters[1], 1u);
    }
    atomicAdd(&counters[2], (unsigned int)cost[cell]);
}
