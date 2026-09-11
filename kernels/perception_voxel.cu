// perception_voxel.cu
//
// Projects a planar LiDAR sweep into the world voxel lattice as occupancy
// logits: one thread per voxel, one logit per cell.
//
// The input shape mirrors the flat coordinate array the Leash voxel kernel
// takes: coordinates are interleaved, `points[2 * i]` and `points[2 * i + 1]`
// are one return's lateral and forward metres, so a sweep of n returns is a
// `2 * n` float array. LiDAR is planar (Pinkie's scanner measures no obstacle
// height), so a return marks its whole voxel column: every height at that
// (x, z) cell shares the count, which is the extruded projection the
// localization contract calls `projected-occupancy`.
//
// Each cell's logit is the prior logit plus one hit's log-odds per return in
// the cell, clamped to the same [-20, 20] band the belief kernels use so a
// dense column cannot saturate the downstream sigmoid.
//
// The lattice is qualia_types' `WorldVoxels`: a 32 x 32 x 12 volume spanning
// roughly 8 m x 8 m x 3 m, with `index = vx * VOXEL_D * VOXEL_H + vz *
// VOXEL_H + vy`. The lateral axis is centred on the room and the forward axis
// starts at the near wall, matching the reference projection.
//
// Launch shape: a grid-stride loop over the cell count, one thread per voxel.
// NVRTC compiles this at process start, so the file includes no CUDA header.

constexpr unsigned int VOXEL_W = 32; // cells along the lateral axis
constexpr unsigned int VOXEL_D = 32; // cells along the forward axis
constexpr unsigned int VOXEL_H = 12; // cells along the vertical axis

static_assert(VOXEL_W == 32u, "voxel lattice width");
static_assert(VOXEL_D == 32u, "voxel lattice depth");
static_assert(VOXEL_H == 12u, "voxel lattice height");

extern "C" __global__ void perception_voxel(
    const float* points,     // flat [x, y, ...] metres, ground plane
    unsigned int point_count,
    float resolution_m,      // metres per cell, uniform across the volume
    float prior_logit,       // logit of a cell with no return
    float hit_log_odds,      // log-odds added per return in a cell
    float* logits)           // VOXEL_W * VOXEL_D * VOXEL_H occupancy logits
{
    const unsigned int cells = VOXEL_W * VOXEL_D * VOXEL_H;
    const float half = 0.5f * resolution_m;

    for (unsigned int index = blockIdx.x * blockDim.x + threadIdx.x; index < cells;
         index += gridDim.x * blockDim.x) {
        // The vertical index only selects which height of the column this
        // thread writes; the return's evidence is the same for the column.
        const unsigned int vx = index / (VOXEL_D * VOXEL_H);
        const unsigned int vz = (index % (VOXEL_D * VOXEL_H)) / VOXEL_H;

        const float centre_x = ((float)vx + 0.5f - 0.5f * (float)VOXEL_W) * resolution_m;
        const float centre_y = ((float)vz + 0.5f) * resolution_m;

        unsigned int hits = 0u;
        for (unsigned int point = 0; point < point_count; ++point) {
            const float x = points[2u * point];
            const float y = points[2u * point + 1u];
            if (fabsf(x - centre_x) <= half && fabsf(y - centre_y) <= half) {
                ++hits;
            }
        }

        const float logit = prior_logit + hit_log_odds * (float)hits;
        logits[index] = fminf(20.0f, fmaxf(-20.0f, logit));
    }
}
