// evidence_layout.cu
//
// The device-side image of the append-only JEPA and applied-action ABI. The
// file has no behaviour worth running; its static assertions are the contract.
// NVRTC (or nvcc) compiles it and fails if a struct's size or alignment drifts
// away from the Rust declarations in qualia_types, which is the only way the
// shared-memory slots can be reinterpreted safely by any backend.
//
// NVRTC cannot put a field offset in a constant expression: it has no
// <cstddef>, no __builtin_offsetof, and it rejects the classic null-pointer
// `&((T*)0)->field` trick. The field offsets are therefore pinned from Rust, in
// `crates/cuda/tests/kernel_abi.rs`, against the same constants this file fixes
// for size and alignment.
//
// The probe kernel exists so a compiled module still has a symbol to load.

// A host-only compile still needs a definition for the kernel attribute.
#ifndef __CUDACC__
#define __global__
#endif

// qualia_types::AppliedActionSlot — one completed post-safety interval.
struct __attribute__((aligned(64))) AppliedActionSlotLayout {
    unsigned long long seq;
    unsigned long long producer_epoch;
    unsigned long long action_sequence;
    unsigned long long interval_start_ns;
    unsigned long long interval_end_ns;
    unsigned int requested_left;
    unsigned int requested_right;
    unsigned int clamped_left;
    unsigned int clamped_right;
    unsigned int applied_left;
    unsigned int applied_right;
    unsigned int speed_scale;
    unsigned int safety_flags;
    unsigned int authority;
    unsigned char valid;
    unsigned char armed;
    unsigned char deadman_active;
    unsigned char collision_clamped;
};

static_assert(sizeof(AppliedActionSlotLayout) == 128ULL, "AppliedActionSlot size");
static_assert(__alignof__(AppliedActionSlotLayout) == 64ULL, "AppliedActionSlot alignment");

static const unsigned int APPLIED_ACTION_HISTORY_CAPACITY = 4096;

// qualia_types::AppliedActionHistory — the lossless action ring.
struct __attribute__((aligned(64))) AppliedActionHistoryLayout {
    unsigned long long write_seq;
    unsigned char pad[56];
    unsigned long long entry_seq[APPLIED_ACTION_HISTORY_CAPACITY];
    AppliedActionSlotLayout entries[APPLIED_ACTION_HISTORY_CAPACITY];
};

static_assert(sizeof(AppliedActionHistoryLayout) == 557120ULL, "AppliedActionHistory size");
static_assert(__alignof__(AppliedActionHistoryLayout) == 64ULL, "AppliedActionHistory alignment");

// qualia_types::JepaEvidencePayload — one coherent inference result.
struct __attribute__((aligned(64))) JepaEvidencePayloadLayout {
    unsigned int abi_version;
    unsigned int backend;
    unsigned int mode;
    unsigned int flags;
    unsigned long long producer_epoch;
    unsigned long long runner_epoch;
    unsigned long long inference_seq;
    unsigned long long timestamp_ns;
    unsigned long long camera_seq;
    unsigned long long lidar_seq;
    unsigned long long pose_seq;
    unsigned long long action_seq;
    float camera_age_ms;
    float lidar_age_ms;
    float pose_age_ms;
    float action_age_ms;
    long long source_skew_ns;
    float observation_quality;
    float transition_nll;
    float occupancy_confidence;
    unsigned int latent_dim;
    unsigned int evidence_dim;
    unsigned int occupancy_dim;
    unsigned char model_id[64];
    unsigned char checkpoint_id[64];
    float latent[256];
    float predicted_mean[256];
    float predicted_log_variance[256];
    float evidence[1024];
    float occupancy_logits[4096];
};

static_assert(sizeof(JepaEvidencePayloadLayout) == 23808ULL, "JepaEvidencePayload size");
static_assert(__alignof__(JepaEvidencePayloadLayout) == 64ULL, "JepaEvidencePayload alignment");

// qualia_types::JepaTelemetryPayload — counters around the same inference.
struct __attribute__((aligned(64))) JepaTelemetryPayloadLayout {
    unsigned int abi_version;
    unsigned int backend;
    unsigned int mode;
    unsigned int flags;
    unsigned long long producer_epoch;
    unsigned long long runner_epoch;
    unsigned long long inference_count;
    unsigned long long dropped_frames;
    unsigned long long stale_frames;
    unsigned long long non_finite_outputs;
    unsigned long long hot_swaps;
    unsigned long long last_inference_ns;
    unsigned int latency_p50_us;
    unsigned int latency_p95_us;
    unsigned int latency_last_us;
    unsigned int latency_max_us;
    float camera_age_ms;
    float lidar_age_ms;
    float pose_age_ms;
    float action_age_ms;
    long long source_skew_ns;
    unsigned char model_id[64];
    unsigned char checkpoint_id[64];
    unsigned int last_error_code;
    unsigned int last_error_len;
    unsigned char last_error[256];
};

static_assert(sizeof(JepaTelemetryPayloadLayout) == 512ULL, "JepaTelemetryPayload size");
static_assert(__alignof__(JepaTelemetryPayloadLayout) == 64ULL, "JepaTelemetryPayload alignment");

// The seqlock header keeps the payload on its own cache line.
struct __attribute__((aligned(64))) JepaEvidenceSlotLayout {
    unsigned long long seq;
    unsigned char pad[56];
    JepaEvidencePayloadLayout payload;
};

struct __attribute__((aligned(64))) JepaTelemetrySlotLayout {
    unsigned long long seq;
    unsigned char pad[56];
    JepaTelemetryPayloadLayout payload;
};

static_assert(sizeof(JepaEvidenceSlotLayout) == 23872ULL, "JepaEvidenceSlot size");
static_assert(__alignof__(JepaEvidenceSlotLayout) == 64ULL, "JepaEvidenceSlot alignment");
static_assert(sizeof(JepaTelemetrySlotLayout) == 576ULL, "JepaTelemetrySlot size");
static_assert(__alignof__(JepaTelemetrySlotLayout) == 64ULL, "JepaTelemetrySlot alignment");

extern "C" __global__ void evidence_layout_probe() {}
