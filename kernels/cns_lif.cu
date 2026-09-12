// lif_step.cu
//
// One tick of the sparse leaky integrate-and-fire network, neuron-parallel.
//
// The artifact stores pre→post CSR; the runner transposes it once and hands the
// device the incoming-edge CSR, so this kernel gathers instead of scattering:
// each thread owns one neuron, walks its incoming edges, and adds the signed
// synapse count of every presynaptic partner that fired on the previous tick.
// No atomics, no ordering between threads, one kernel launch per tick.
//
// Both compilers that build this crate read this file: NVRTC at run time and
// nvcc at build time when CUDAARCHS is set, so it may not include any CUDA
// header.
//
// Launch shape: one thread per neuron, 256 threads per block.

extern "C" __global__ void lif_step(
    const unsigned long long* __restrict__ rowptr,
    const unsigned int* __restrict__ cols,
    const signed char* __restrict__ sign,
    const unsigned short* __restrict__ weight,
    const unsigned char* __restrict__ spike_in,
    const float* __restrict__ external,
    float* __restrict__ v,
    unsigned char* __restrict__ refractory,
    unsigned char* __restrict__ spike_out,
    const unsigned int neuron_count,
    const float decay,
    const float threshold,
    const float reset,
    const unsigned int refractory_ticks)
{
    const unsigned int neuron = blockIdx.x * blockDim.x + threadIdx.x;
    if (neuron >= neuron_count) return;

    if (refractory[neuron] > 0) {
        --refractory[neuron];
        v[neuron] = reset;
        spike_out[neuron] = 0;
        return;
    }

    float current = 0.0f;
    const unsigned long long start = rowptr[neuron];
    const unsigned long long end = rowptr[neuron + 1];
    for (unsigned long long edge = start; edge < end; ++edge) {
        if (spike_in[cols[edge]] != 0) {
            current += (float)sign[edge] * (float)weight[edge];
        }
    }
    if (external != 0) current += external[neuron];

    const float next = v[neuron] * decay + current;
    if (next >= threshold) {
        spike_out[neuron] = 1;
        v[neuron] = reset;
        refractory[neuron] = (unsigned char)refractory_ticks;
    } else {
        spike_out[neuron] = 0;
        v[neuron] = next;
    }
}

// The free-running variant needs no external buffer, so a caller with no
// sensory drive is not obliged to allocate one.
extern "C" __global__ void lif_step_free(
    const unsigned long long* __restrict__ rowptr,
    const unsigned int* __restrict__ cols,
    const signed char* __restrict__ sign,
    const unsigned short* __restrict__ weight,
    const unsigned char* __restrict__ spike_in,
    float* __restrict__ v,
    unsigned char* __restrict__ refractory,
    unsigned char* __restrict__ spike_out,
    const unsigned int neuron_count,
    const float decay,
    const float threshold,
    const float reset,
    const unsigned int refractory_ticks)
{
    lif_step(rowptr, cols, sign, weight, spike_in, (const float*)0, v, refractory,
             spike_out, neuron_count, decay, threshold, reset, refractory_ticks);
}
