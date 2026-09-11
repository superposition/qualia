// belief_couple.cu
//
// The connectome prior's weighted slot update, the device twin of
// `qualia_jepa::prior::CouplingPrior::couple`.
//
// A type's in-strength is the summed weight of the edges that end on it; the
// most strongly innervated type couples at unit weight and every other type
// scales down in proportion, so the prior can attenuate a belief but never
// amplify it. One thread per mapped slot applies the factor and records it, so
// the host can sum the applied weights exactly as the CPU reference returns
// them.
//
// Launch shape: one block, one thread per mapped slot. The block first
// accumulates every edge's weight into the shared in-strength table (a
// cooperative stride over the edge list, the same edges the host loops over
// once), then each thread reads its own type's strength and the block-wide
// peak and multiplies its belief slot. The reduction is order-independent
// because integer addition is associative and the peak is a max, so the
// result does not depend on how the block is scheduled.
//
// NVRTC compiles this at process start and supplies the device builtins, so
// the file includes no CUDA header.

extern "C" __global__ void belief_couple(
    float* belief,                    // belief slots, read and written
    const unsigned int* cols,         // edge_count post-synaptic type indices
    const unsigned int* weights,      // edge_count aggregated edge weights
    const unsigned int* slot_types,   // slot_count type index per mapped slot
    const unsigned int* slot_indices, // slot_count belief index per mapped slot
    float* slot_factors,              // slot_count applied factor, per slot
    unsigned int slot_count,
    unsigned int edge_count,
    unsigned int type_count,
    unsigned int belief_len)
{
    // One table entry per type; the caller sizes the dynamic shared memory.
    extern __shared__ unsigned int in_strength[];

    for (unsigned int type = threadIdx.x; type < type_count; type += blockDim.x) {
        in_strength[type] = 0u;
    }
    __syncthreads();

    // Scatter each edge into its post-synaptic type. A column outside the
    // graph cannot occur in a validated prior, but it is skipped rather than
    // allowed to write past the table.
    for (unsigned int edge = threadIdx.x; edge < edge_count; edge += blockDim.x) {
        const unsigned int type = cols[edge];
        if (type < type_count) {
            atomicAdd(&in_strength[type], weights[edge]);
        }
    }
    __syncthreads();

    const unsigned int lane = threadIdx.x;
    if (lane >= slot_count) return;

    unsigned int peak = 0u;
    for (unsigned int type = 0; type < type_count; ++type) {
        peak = max(peak, in_strength[type]);
    }

    const unsigned int type = slot_types[lane];
    const unsigned int slot = slot_indices[lane];

    // The reference ignores a pair that names a type outside the graph or a
    // slot outside the belief, and an all-zero prior applies nothing.
    if (peak == 0u || type >= type_count || slot >= belief_len) {
        slot_factors[lane] = 0.0f;
        return;
    }

    const float factor = (float)in_strength[type] / (float)peak;
    belief[slot] *= factor;
    slot_factors[lane] = factor;
}
