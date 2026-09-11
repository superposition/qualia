// smoke.cu
//
// The one-kernel smoke test the compute service exposes as `cuda_smoke`: it
// proves the device path is alive without touching shared memory, so a
// deployment can tell "no device" from "wrong maths".
//
// Launch shape: one block of four lanes, one lane per element.
//
// Both compilers that build this crate read this file: NVRTC at run time and
// nvcc at build time when `CUDAARCHS` is set, so it may not include any CUDA
// header.

extern "C" __global__ void add_one(float* values) {
    const unsigned int lane = threadIdx.x;
    if (lane < 4) values[lane] += 1.0f;
}
