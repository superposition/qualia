// belief_couple.cu
//
// The connectome prior's weighted slot update, the device twin of
// `qualia_jepa::prior::CouplingPrior::couple`.
//
// A type's in-strength is the summed weight of the edges that end on it; the
// most strongly innervated type couples at unit weight and every other type
// scales down in proportion, so the prior can attenuate a belief but never
// amplify it. One thread per mapped slot records its own factor, so the host
// can sum the applied weights exactly as the CPU reference returns them.
//
// Launch shape: one block, one thread per mapped slot. The block first
// accumulates every edge's weight into the shared in-strength table (a
// cooperative stride over the edge list, the same edges the host loops over
// once), then each thread reads its own type's strength and the block-wide
// peak and records its factor. The reduction is order-independent because
// integer addition is associative and the peak is a max, so the recorded
// factors do not depend on how the block is scheduled.
//
// Two mapped slots may name one belief index, so the update is serialised by
// slot: of the lanes that map an index, the last one applies every valid factor
// for it in slot order — the reference's sequential loop, split by slot — and
// the others write nothing. Applying the factors directly from every lane would
// let concurrent read-modify-write pairs drop all but one of them.
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

    unsigned int peak = 0u;
    for (unsigned int type = 0; type < type_count; ++type) {
        peak = max(peak, in_strength[type]);
    }

    unsigned int type = 0u;
    unsigned int slot = 0u;
    if (lane < slot_count) {
        type = slot_types[lane];
        slot = slot_indices[lane];
        // The reference ignores a pair that names a type outside the graph or a
        // slot outside the belief, and an all-zero prior applies nothing.
        const bool ignored = peak == 0u || type >= type_count || slot >= belief_len;
        slot_factors[lane] = ignored ? 0.0f : (float)in_strength[type] / (float)peak;
    }
    __syncthreads();

    if (lane >= slot_count) return;
    if (peak == 0u || type >= type_count || slot >= belief_len) return;

    // One lane per belief index writes it: a later valid lane mapping the same
    // index owns the update, and it applies every valid factor for the index in
    // slot order. A repeated index therefore sees all of its factors, as the
    // reference's sequential loop gives it.
    for (unsigned int other = lane + 1u; other < slot_count; ++other) {
        if (slot_indices[other] == slot && slot_types[other] < type_count) return;
    }
    for (unsigned int other = 0u; other < slot_count; ++other) {
        if (slot_indices[other] == slot && slot_types[other] < type_count) {
            belief[slot] *= slot_factors[other];
        }
    }
}
