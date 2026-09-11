//! The CUDA backend, compiled only when the `cuda` feature is enabled.
//!
//! Kernels are embedded as source and compiled by NVRTC at process start, so
//! the crate carries no build-time CUDA dependency and the same binary can run
//! on the Orin Nano (sm_87) and the development 4090 (sm_89). The device
//! architecture is read from the adapter rather than hard-coded.
//!
//! `run_layer` mirrors the Metal layer loop exactly; only the dispatch calls
//! differ, so a layer behaves identically on either backend.

pub use crate::cpu::CostmapStats;

use qualia_shm::{
    LayerReader, LayerWriter, ShmRegion, LAYER_SLOTS_OFFSET, LAYER_SLOT_SIZE,
};
use qualia_types::{
    default_params, BeliefSlot, LayerParams, LayerSlot, MAX_QUESTION_TEXT, NUM_LAYERS, STATE_DIM,
    WEIGHT_COUNT,
};

use cudarc::driver::{
    CudaContext as Adapter, CudaFunction, CudaModule, CudaSlice, CudaStream, LaunchConfig,
    PushKernelArg,
};
use cudarc::nvrtc::{compile_ptx_with_opts, CompileOptions};

use std::mem::size_of;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Embedded kernel sources ──────────────────────────────────────────────────

/// The belief kernel lives at the workspace root: the Metal backend compiles
/// the same algorithm for macOS, so the ABI and the maths stay together.
const BELIEF_KERNEL: &str = include_str!("../../../kernels/belief_update.cu");

const SMOKE_KERNEL: &str = r#"
extern "C" __global__ void add_one(float* values) {
    const unsigned int lane = threadIdx.x;
    if (lane < 4) values[lane] += 1.0f;
}
"#;

const COSTMAP_KERNEL: &str = r#"
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
"#;

const COGNITION_KERNEL: &str = r#"
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
"#;

// ── Belief thought kinds ─────────────────────────────────────────────────────

const THOUGHT_OBSERVE: u8 = 0;
const THOUGHT_PREDICT: u8 = 1;
const THOUGHT_SURPRISE: u8 = 2;
const THOUGHT_LEARN: u8 = 3;
const THOUGHT_RESOLVE: u8 = 4;
const THOUGHT_ESCALATE: u8 = 5;

const LAYER_DESCRIPTIONS: [&str; NUM_LAYERS] = [
    "sensation",
    "body motion",
    "local space",
    "visual form",
    "short behaviour",
    "deep behaviour",
    "semantic context",
    "sensor input",
];

// ── Shared helpers ───────────────────────────────────────────────────────────

/// NVRTC wants a named PTX target. CUDA 12.x accepts the whole list below, and
/// compiling for the newest architecture at or below the adapter lets the
/// driver JIT the PTX forward onto the device. The two shipped targets are the
/// Orin Nano (sm_87) and the 4090 (sm_89).
const ARCHITECTURES: [(i32, i32, &str); 10] = [
    (7, 0, "compute_70"),
    (7, 2, "compute_72"),
    (7, 5, "compute_75"),
    (8, 0, "compute_80"),
    (8, 6, "compute_86"),
    (8, 7, "compute_87"),
    (8, 9, "compute_89"),
    (9, 0, "compute_90"),
    (10, 0, "compute_100"),
    (12, 0, "compute_120"),
];

fn ptx_architecture(capability: (i32, i32)) -> &'static str {
    let mut chosen = "compute_87";
    for (major, minor, name) in ARCHITECTURES {
        if (major, minor) <= capability {
            chosen = name;
        }
    }
    chosen
}

/// Compiles one embedded kernel source for the adapter's architecture.
fn compile_for(
    adapter: &Arc<Adapter>,
    source: &str,
) -> Result<(Arc<CudaModule>, (i32, i32)), String> {
    let capability = adapter
        .compute_capability()
        .map_err(|error| format!("CUDA capability query failed: {error:?}"))?;
    let options = CompileOptions {
        arch: Some(ptx_architecture(capability)),
        ..Default::default()
    };
    let ptx = compile_ptx_with_opts(source, options)
        .map_err(|error| format!("NVRTC compile failed: {error:?}"))?;
    let module = adapter
        .load_module(ptx)
        .map_err(|error| format!("PTX load failed: {error:?}"))?;
    Ok((module, capability))
}

fn open_adapter() -> Result<(Arc<Adapter>, Arc<CudaStream>, String), String> {
    // One GPU per deployment: the Orin Nano and the 4090 both enumerate as 0.
    let adapter = Adapter::new(0).map_err(|error| format!("CUDA context init failed: {error:?}"))?;
    let device_name = adapter
        .name()
        .unwrap_or_else(|_| "unknown CUDA device".to_string());
    let stream = adapter.default_stream();
    Ok((adapter, stream, device_name))
}

/// A raw pointer to one of this process's own layer slots.
///
/// Layer slots are single-writer by construction: `run_layer` is spawned once
/// per layer. The pointer is taken from the region's base address rather than
/// from a shared reference, so the mutable fields (weights, bias, question)
/// can be written without invalidating the shared view the belief writer uses.
fn layer_slot_ptr(shm: &ShmRegion, layer: usize) -> *mut LayerSlot {
    let offset = LAYER_SLOTS_OFFSET + layer * LAYER_SLOT_SIZE;
    // SAFETY: the offset lies inside the mapping the region owns.
    unsafe { shm.as_ptr().add(offset).cast::<LayerSlot>() }
}

// ── CudaContext: one layer's belief kernel ───────────────────────────────────

pub struct CudaContext {
    stream: Arc<CudaStream>,
    // The module owns the loaded PTX; the function handle must not outlive it.
    _module: Arc<CudaModule>,
    function: CudaFunction,
    params: [f32; 4],
}

impl CudaContext {
    pub fn new(params: &LayerParams) -> Result<Self, String> {
        let (adapter, stream, _) = open_adapter()?;
        let (module, _) = compile_for(&adapter, BELIEF_KERNEL)?;
        let function = module
            .load_function("belief_update")
            .map_err(|error| format!("belief_update function missing: {error:?}"))?;

        Ok(Self {
            stream,
            _module: module,
            function,
            params: [
                params.threshold,
                params.learning_rate,
                f32::from(params.layer_id),
                params.weight_decay,
            ],
        })
    }

    pub fn device_name(&self) -> String {
        self.stream
            .context()
            .name()
            .unwrap_or_else(|_| "unknown CUDA device".to_string())
    }

    /// Copies one layer's state to the device, runs the update, and copies the
    /// mutated belief, weights and bias back. Synchronous.
    pub fn dispatch_belief_update(
        &self,
        belief: &mut BeliefSlot,
        below: &BeliefSlot,
        weights: &mut [f32; WEIGHT_COUNT],
        bias: &mut [f32; STATE_DIM],
    ) {
        let belief_bytes = size_of::<BeliefSlot>();
        let stream = &self.stream;

        // SAFETY: BeliefSlot is repr(C, align(64)) with no pointer members, so
        // its bytes are a valid host buffer of exactly `belief_bytes`.
        let host_belief = unsafe {
            std::slice::from_raw_parts(belief as *const BeliefSlot as *const u8, belief_bytes)
        };
        let host_below = unsafe {
            std::slice::from_raw_parts(below as *const BeliefSlot as *const u8, belief_bytes)
        };

        let mut device_belief = stream.clone_htod(host_belief).expect("H2D belief");
        let device_below = stream.clone_htod(host_below).expect("H2D below");
        let device_params = stream
            .clone_htod(self.params.as_slice())
            .expect("H2D params");
        let mut device_weights = stream.clone_htod(weights.as_slice()).expect("H2D weights");
        let mut device_bias = stream.clone_htod(bias.as_slice()).expect("H2D bias");

        // One block of STATE_DIM threads; both __shared__ arrays are static.
        let config = LaunchConfig {
            grid_dim: (1, 1, 1),
            block_dim: (STATE_DIM as u32, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: argument order and types match belief_update.cu, and the
        // device buffers live until after the synchronization below.
        unsafe {
            stream
                .launch_builder(&self.function)
                .arg(&mut device_belief)
                .arg(&device_below)
                .arg(&device_params)
                .arg(&mut device_weights)
                .arg(&mut device_bias)
                .launch(config)
                .expect("belief_update launch");
        }
        stream.synchronize().expect("belief_update synchronize");

        let belief_out = stream.clone_dtoh(&device_belief).expect("D2H belief");
        let weights_out: Vec<f32> = stream.clone_dtoh(&device_weights).expect("D2H weights");
        let bias_out: Vec<f32> = stream.clone_dtoh(&device_bias).expect("D2H bias");

        // SAFETY: the read is unaligned because a byte buffer has alignment 1;
        // BeliefSlot is Copy, so the value is owned from here on.
        *belief = unsafe { std::ptr::read_unaligned(belief_out.as_ptr() as *const BeliefSlot) };
        weights.copy_from_slice(&weights_out);
        bias.copy_from_slice(&bias_out);
    }
}

// ── SmokeContext: a single trivial kernel round trip ─────────────────────────

pub struct SmokeContext {
    stream: Arc<CudaStream>,
    _module: Arc<CudaModule>,
    function: CudaFunction,
    buffer: Mutex<CudaSlice<f32>>,
}

impl SmokeContext {
    pub fn new() -> Result<Self, String> {
        let (adapter, stream, _) = open_adapter()?;
        let (module, _) = compile_for(&adapter, SMOKE_KERNEL)?;
        let function = module
            .load_function("add_one")
            .map_err(|error| format!("add_one function missing: {error:?}"))?;
        let buffer = stream
            .alloc_zeros::<f32>(4)
            .map_err(|error| format!("smoke buffer allocation failed: {error:?}"))?;
        Ok(Self {
            stream,
            _module: module,
            function,
            buffer: Mutex::new(buffer),
        })
    }

    pub fn device_name(&self) -> String {
        self.stream
            .context()
            .name()
            .unwrap_or_else(|_| "unknown CUDA device".to_string())
    }

    pub fn run_add_one(&self, input: [f32; 4]) -> Result<[f32; 4], String> {
        let mut buffer = self
            .buffer
            .lock()
            .map_err(|_| "smoke buffer mutex poisoned".to_string())?;
        self.stream
            .memcpy_htod(&input, &mut *buffer)
            .map_err(|error| format!("H2D smoke input failed: {error:?}"))?;

        let config = LaunchConfig {
            grid_dim: (1, 1, 1),
            block_dim: (4, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: add_one takes one mutable float buffer and reads four lanes.
        unsafe {
            self.stream
                .launch_builder(&self.function)
                .arg(&mut *buffer)
                .launch(config)
                .map_err(|error| format!("smoke launch failed: {error:?}"))?;
        }
        self.stream
            .synchronize()
            .map_err(|error| format!("smoke synchronize failed: {error:?}"))?;

        let mut output = [0.0f32; 4];
        self.stream
            .memcpy_dtoh(&*buffer, &mut output)
            .map_err(|error| format!("D2H smoke output failed: {error:?}"))?;
        self.stream
            .synchronize()
            .map_err(|error| format!("smoke readback synchronize failed: {error:?}"))?;
        Ok(output)
    }
}

// ── CostmapStatsContext: the occupancy/cost reduction ────────────────────────

struct CostmapBuffers {
    occupied: CudaSlice<u8>,
    cost: CudaSlice<u8>,
    counters: CudaSlice<u32>,
    capacity: usize,
}

pub struct CostmapStatsContext {
    stream: Arc<CudaStream>,
    _module: Arc<CudaModule>,
    function: CudaFunction,
    buffers: Mutex<CostmapBuffers>,
}

impl CostmapStatsContext {
    pub fn new() -> Result<Self, String> {
        let (adapter, stream, _) = open_adapter()?;
        let (module, _) = compile_for(&adapter, COSTMAP_KERNEL)?;
        let function = module
            .load_function("costmap_stats")
            .map_err(|error| format!("costmap_stats function missing: {error:?}"))?;
        let occupied = stream
            .alloc_zeros::<u8>(1)
            .map_err(|error| format!("costmap occupied allocation failed: {error:?}"))?;
        let cost = stream
            .alloc_zeros::<u8>(1)
            .map_err(|error| format!("costmap cost allocation failed: {error:?}"))?;
        let counters = stream
            .alloc_zeros::<u32>(4)
            .map_err(|error| format!("costmap counters allocation failed: {error:?}"))?;
        Ok(Self {
            stream,
            _module: module,
            function,
            buffers: Mutex::new(CostmapBuffers {
                occupied,
                cost,
                counters,
                capacity: 1,
            }),
        })
    }

    pub fn device_name(&self) -> String {
        self.stream
            .context()
            .name()
            .unwrap_or_else(|_| "unknown CUDA device".to_string())
    }

    /// Reduces one costmap pair. Empty input is answered on the host; a length
    /// mismatch is refused before anything reaches the device.
    pub fn run(&self, occupied: &[u8], cost: &[u8]) -> Result<CostmapStats, String> {
        crate::cpu::validate_costmap_pair(occupied, cost)?;
        if occupied.is_empty() {
            return Ok(CostmapStats::default());
        }

        let mut buffers = self
            .buffers
            .lock()
            .map_err(|_| "costmap buffer mutex poisoned".to_string())?;
        if buffers.capacity < occupied.len() {
            buffers.occupied = self
                .stream
                .alloc_zeros::<u8>(occupied.len())
                .map_err(|error| format!("costmap occupied resize failed: {error:?}"))?;
            buffers.cost = self
                .stream
                .alloc_zeros::<u8>(cost.len())
                .map_err(|error| format!("costmap cost resize failed: {error:?}"))?;
            buffers.capacity = occupied.len();
        }

        self.stream
            .memcpy_htod(occupied, &mut buffers.occupied)
            .map_err(|error| format!("H2D occupied failed: {error:?}"))?;
        self.stream
            .memcpy_htod(cost, &mut buffers.cost)
            .map_err(|error| format!("H2D cost failed: {error:?}"))?;
        self.stream
            .memcpy_htod(&[0u32; 4], &mut buffers.counters)
            .map_err(|error| format!("costmap counter reset failed: {error:?}"))?;

        let length = occupied.len() as u32;
        let block = 256u32;
        let config = LaunchConfig {
            grid_dim: (length.div_ceil(block), 1, 1),
            block_dim: (block, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: the kernel reads `length` cells from each input and
        // accumulates into four counters; all three buffers cover that. The
        // destructuring splits one mutable borrow into disjoint fields.
        let CostmapBuffers {
            occupied: device_occupied,
            cost: device_cost,
            counters: device_counters,
            ..
        } = &mut *buffers;
        unsafe {
            self.stream
                .launch_builder(&self.function)
                .arg(device_occupied)
                .arg(device_cost)
                .arg(&length)
                .arg(device_counters)
                .launch(config)
                .map_err(|error| format!("costmap_stats launch failed: {error:?}"))?;
        }
        self.stream
            .synchronize()
            .map_err(|error| format!("costmap stats synchronize failed: {error:?}"))?;

        let mut counters = [0u32; 4];
        self.stream
            .memcpy_dtoh(&buffers.counters, &mut counters)
            .map_err(|error| format!("D2H counters failed: {error:?}"))?;
        self.stream
            .synchronize()
            .map_err(|error| format!("costmap readback synchronize failed: {error:?}"))?;

        Ok(CostmapStats {
            occupied_count: counters[0],
            high_cost_count: counters[1],
            cost_sum: counters[2],
            blocked_count: counters[3],
        })
    }
}

// ── CudaCognitionStack: the L3-L6 persistent cognition model ─────────────────

struct CognitionLayer {
    state: CudaSlice<f32>,
    lower: CudaSlice<f32>,
    top_down: CudaSlice<f32>,
    weights: CudaSlice<f32>,
    bias: CudaSlice<f32>,
    params: CudaSlice<f32>,
}

/// Persistent device buffers for the four cognition layers.
///
/// The dense 1024x1024 weights stay resident on the GPU between inference
/// cycles; they cross to the host only for checkpoints, restores and the
/// compact operator sketches.
pub struct CudaCognitionStack {
    stream: Arc<CudaStream>,
    _module: Arc<CudaModule>,
    update: CudaFunction,
    patch: CudaFunction,
    layers: Vec<CognitionLayer>,
    device_name: String,
    capability: (i32, i32),
}

impl CudaCognitionStack {
    pub fn new() -> Result<Self, String> {
        let (adapter, stream, device_name) = open_adapter()?;
        let capability = adapter
            .compute_capability()
            .map_err(|error| format!("CUDA capability query failed: {error:?}"))?;
        let options = CompileOptions {
            arch: Some(ptx_architecture(capability)),
            ..Default::default()
        };
        let ptx = compile_ptx_with_opts(COGNITION_KERNEL, options)
            .map_err(|error| format!("NVRTC cognition compile failed: {error:?}"))?;
        let module = adapter
            .load_module(ptx)
            .map_err(|error| format!("cognition PTX load failed: {error:?}"))?;
        let update = module
            .load_function("cognition_update")
            .map_err(|error| format!("cognition_update function missing: {error:?}"))?;
        let patch = module
            .load_function("cognition_patch")
            .map_err(|error| format!("cognition_patch function missing: {error:?}"))?;

        let mut layers = Vec::with_capacity(4);
        for layer in 0..4usize {
            let state = stream
                .alloc_zeros::<f32>(STATE_DIM)
                .map_err(|error| format!("cognition state allocation failed: {error:?}"))?;
            let lower = stream
                .alloc_zeros::<f32>(STATE_DIM)
                .map_err(|error| format!("cognition lower allocation failed: {error:?}"))?;
            let top_down = stream
                .alloc_zeros::<f32>(STATE_DIM)
                .map_err(|error| format!("cognition top-down allocation failed: {error:?}"))?;
            let mut initial = vec![0.0f32; WEIGHT_COUNT];
            for index in 0..STATE_DIM {
                initial[index * STATE_DIM + index] = 0.75 + layer as f32 * 0.05;
            }
            let weights = stream
                .clone_htod(&initial)
                .map_err(|error| format!("cognition weight allocation failed: {error:?}"))?;
            let bias = stream
                .alloc_zeros::<f32>(STATE_DIM)
                .map_err(|error| format!("cognition bias allocation failed: {error:?}"))?;
            let params = stream
                .alloc_zeros::<f32>(5)
                .map_err(|error| format!("cognition params allocation failed: {error:?}"))?;
            layers.push(CognitionLayer {
                state,
                lower,
                top_down,
                weights,
                bias,
                params,
            });
        }

        Ok(Self {
            stream,
            _module: module,
            update,
            patch,
            layers,
            device_name,
            capability,
        })
    }

    pub fn backend_name(&self) -> String {
        format!(
            "cuda:{}:sm_{}{}",
            self.device_name, self.capability.0, self.capability.1
        )
    }

    /// A second stack initialised with this one's parameters and states.
    pub fn fork(&self) -> Result<Self, String> {
        let mut forked = Self::new()?;
        let (weights, biases) = self.checkpoint_parameters();
        let states = self
            .layers
            .iter()
            .map(|layer| {
                self.stream
                    .clone_dtoh(&layer.state)
                    .map_err(|error| format!("cognition state copy failed: {error:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        forked.restore_parameters(&weights, &biases, &states)?;
        Ok(forked)
    }

    /// Adds `delta` to one weight and returns the clamped result.
    pub fn apply_weight_delta(
        &mut self,
        layer: usize,
        row: usize,
        column: usize,
        delta: f32,
    ) -> Result<f32, String> {
        if layer >= self.layers.len() || row >= STATE_DIM || column >= STATE_DIM {
            return Err("cognition weight coordinate out of range".to_string());
        }
        if !delta.is_finite() {
            return Err("cognition weight delta must be finite".to_string());
        }
        let index = (row * STATE_DIM + column) as u32;
        let config = LaunchConfig {
            grid_dim: (1, 1, 1),
            block_dim: (1, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: cognition_patch writes exactly one element of `weights`.
        unsafe {
            self.stream
                .launch_builder(&self.patch)
                .arg(&mut self.layers[layer].weights)
                .arg(&index)
                .arg(&delta)
                .launch(config)
                .map_err(|error| format!("cognition patch launch failed: {error:?}"))?;
        }
        self.stream
            .synchronize()
            .map_err(|error| format!("cognition patch synchronize failed: {error:?}"))?;

        let weights = self
            .stream
            .clone_dtoh(&self.layers[layer].weights)
            .map_err(|error| format!("cognition patch readback failed: {error:?}"))?;
        Ok(weights[index as usize])
    }

    /// Root-mean-square magnitude of each `side` x `side` block of one layer's
    /// weight matrix, row-major.
    pub fn weight_sketch(&self, layer: usize, side: usize) -> Result<Vec<f32>, String> {
        if layer >= self.layers.len()
            || side == 0
            || side > STATE_DIM
            || !STATE_DIM.is_multiple_of(side)
        {
            return Err("cognition sketch shape is invalid".to_string());
        }
        let weights = self
            .stream
            .clone_dtoh(&self.layers[layer].weights)
            .map_err(|error| format!("cognition sketch copy failed: {error:?}"))?;

        let block = STATE_DIM / side;
        let stride = (block / 8).max(1);
        let mut sketch = Vec::with_capacity(side * side);
        for block_row in 0..side {
            for block_column in 0..side {
                let mut sum_squares = 0.0f32;
                let mut samples = 0usize;
                for row in (block_row * block..(block_row + 1) * block).step_by(stride) {
                    for column in (block_column * block..(block_column + 1) * block).step_by(stride)
                    {
                        let value = weights[row * STATE_DIM + column];
                        sum_squares += value * value;
                        samples += 1;
                    }
                }
                sketch.push((sum_squares / samples.max(1) as f32).sqrt());
            }
        }
        Ok(sketch)
    }

    /// Runs one cognition tick for `layer` and returns the new activation.
    pub fn dispatch(
        &mut self,
        layer: usize,
        lower: &[f32],
        top_down: &[f32],
        source_precision: f32,
        top_down_precision: f32,
        learning_signal: f32,
    ) -> Result<Vec<f32>, String> {
        if layer >= self.layers.len() || lower.len() != STATE_DIM || top_down.len() != STATE_DIM {
            return Err("cognition layer or vector shape is invalid".to_string());
        }

        let params = [
            source_precision,
            top_down_precision,
            0.00012,
            0.00008,
            learning_signal.clamp(0.0, 1.0),
        ];
        let buffers = &mut self.layers[layer];
        self.stream
            .memcpy_htod(lower, &mut buffers.lower)
            .map_err(|error| format!("cognition lower copy failed: {error:?}"))?;
        self.stream
            .memcpy_htod(top_down, &mut buffers.top_down)
            .map_err(|error| format!("cognition top-down copy failed: {error:?}"))?;
        self.stream
            .memcpy_htod(&params, &mut buffers.params)
            .map_err(|error| format!("cognition params copy failed: {error:?}"))?;

        let config = LaunchConfig {
            grid_dim: (1, 1, 1),
            block_dim: (STATE_DIM as u32, 1, 1),
            shared_mem_bytes: 0,
        };
        // SAFETY: cognition_update reads 1024-element vectors and writes the
        // state, weight and bias buffers held by this layer.
        unsafe {
            self.stream
                .launch_builder(&self.update)
                .arg(&mut buffers.state)
                .arg(&buffers.lower)
                .arg(&buffers.top_down)
                .arg(&mut buffers.weights)
                .arg(&mut buffers.bias)
                .arg(&buffers.params)
                .launch(config)
                .map_err(|error| format!("cognition launch failed: {error:?}"))?;
        }
        self.stream
            .synchronize()
            .map_err(|error| format!("cognition synchronize failed: {error:?}"))?;

        self.stream
            .clone_dtoh(&buffers.state)
            .map_err(|error| format!("cognition state readback failed: {error:?}"))
    }

    /// The dense weights and biases of every layer, in layer order.
    pub fn checkpoint_parameters(&self) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let weights = self
            .layers
            .iter()
            .map(|layer| {
                self.stream
                    .clone_dtoh(&layer.weights)
                    .expect("cognition weight checkpoint")
            })
            .collect();
        let biases = self
            .layers
            .iter()
            .map(|layer| {
                self.stream
                    .clone_dtoh(&layer.bias)
                    .expect("cognition bias checkpoint")
            })
            .collect();
        (weights, biases)
    }

    /// Restores a full checkpoint: one weight matrix, one bias vector and one
    /// state vector per layer. A malformed checkpoint is refused untouched.
    pub fn restore_parameters(
        &mut self,
        weights: &[Vec<f32>],
        biases: &[Vec<f32>],
        states: &[Vec<f32>],
    ) -> Result<(), String> {
        let expected = self.layers.len();
        let shaped = weights.len() == expected
            && biases.len() == expected
            && states.len() == expected
            && weights.iter().all(|layer| layer.len() == WEIGHT_COUNT)
            && biases.iter().all(|layer| layer.len() == STATE_DIM)
            && states.iter().all(|layer| layer.len() == STATE_DIM);
        let finite = weights
            .iter()
            .chain(biases.iter())
            .chain(states.iter())
            .all(|layer| layer.iter().all(|value| value.is_finite()));
        if !shaped || !finite {
            return Err("cognition checkpoint shape is invalid".to_string());
        }

        for (index, layer) in self.layers.iter_mut().enumerate() {
            self.stream
                .memcpy_htod(&weights[index], &mut layer.weights)
                .map_err(|error| format!("cognition weight restore failed: {error:?}"))?;
            self.stream
                .memcpy_htod(&biases[index], &mut layer.bias)
                .map_err(|error| format!("cognition bias restore failed: {error:?}"))?;
            self.stream
                .memcpy_htod(&states[index], &mut layer.state)
                .map_err(|error| format!("cognition state restore failed: {error:?}"))?;
        }
        self.stream
            .synchronize()
            .map_err(|error| format!("cognition restore synchronize failed: {error:?}"))
    }
}

// ── run_layer: the belief layer loop ─────────────────────────────────────────

/// Runs one belief layer until the process is stopped.
///
/// The loop is identical to the Metal backend's: read the layer below, run the
/// generative update, optionally inject the semantic embedding, publish the
/// belief, and emit thoughts and questions on their own cooldowns.
pub fn run_layer(layer_id: u8, name: &str) {
    let shm_name =
        std::env::var("QUALIA_SHM_NAME").unwrap_or_else(|_| "/qualia_body".to_string());
    eprintln!("qualia-{name}: opening shm '{shm_name}', layer {layer_id}");

    let shm = ShmRegion::open(&shm_name).unwrap_or_else(|error| {
        panic!("qualia-{name}: failed to open shm: {error}");
    });

    let params = default_params(layer_id);
    let context = CudaContext::new(&params).unwrap_or_else(|error| {
        panic!("qualia-{name}: CUDA init failed: {error}");
    });
    eprintln!("qualia-{name}: CUDA device: {}", context.device_name());

    let my_slot = shm.layer_slot(layer_id as usize);
    let writer = LayerWriter::new(my_slot);
    let below_slot = if layer_id > 0 {
        shm.layer_slot(layer_id as usize - 1)
    } else {
        shm.layer_slot(NUM_LAYERS - 1)
    };
    let reader = LayerReader::new(below_slot);

    let tick = if params.freq_hz > 0.0 {
        Duration::from_secs_f64(1.0 / params.freq_hz)
    } else {
        Duration::from_secs(60)
    };

    // A fresh layer starts as an identity predictor with a flat belief.
    {
        // SAFETY: this process is the only writer for its own layer slot.
        let slot = unsafe { &mut *layer_slot_ptr(&shm, layer_id as usize) };
        for row in 0..STATE_DIM {
            for column in 0..STATE_DIM {
                slot.weights[row * STATE_DIM + column] = if row == column { 1.0 } else { 0.0 };
            }
            slot.bias[row] = 0.0;
        }
    }
    {
        let belief = writer.back_buffer();
        belief.layer = layer_id;
        for index in 0..STATE_DIM {
            belief.mean[index] = 0.0;
            belief.precision[index] = 1.0;
            belief.prediction[index] = 0.0;
            belief.residual[index] = 0.0;
        }
        belief.vfe = 0.0;
        belief.challenge_vfe = 0.0;
        belief.confirm_streak = 0;
        belief.compression = 0;
        writer.publish();
    }

    let description = LAYER_DESCRIPTIONS
        .get(layer_id as usize)
        .copied()
        .unwrap_or("unknown");
    shm.emit_thought(
        layer_id,
        THOUGHT_OBSERVE,
        0.0,
        &format!(
            "{description} init (CUDA): dim={STATE_DIM}, freq={:.1}Hz",
            params.freq_hz
        ),
    );

    let semantic_alpha: f32 = match layer_id {
        6 => 0.50,
        5 => 0.30,
        4 => 0.15,
        3 => 0.05,
        2 => 0.02,
        _ => 0.0,
    };
    let injects = semantic_alpha > 0.0;
    if injects {
        eprintln!("qualia-{name}: semantic injection alpha={semantic_alpha}");
    }
    eprintln!(
        "qualia-{name}: running at {:.1} Hz with 1024×1024 generative model (CUDA)",
        params.freq_hz
    );

    let question_cooldown = Duration::from_secs(match layer_id {
        5 => 60,
        4 => 120,
        3 => 300,
        _ => 600,
    });
    let thought_cooldown = Duration::from_millis(match layer_id {
        0 => 2000,
        1..=3 => 1000,
        4..=5 => 500,
        6 => 300,
        _ => 2000,
    });

    let mut last_thought = Instant::now();
    let mut last_question = Instant::now();
    let mut previous_vfe = 0.0f32;
    let mut previous_compression = 0u8;
    let mut previous_streak = 0u32;
    let mut cycle = 0u64;
    let mut seen_world_seq = 0u64;
    let mut high_vfe_streak = 0u32;
    let mut compression_stall = 0u32;

    loop {
        let cycle_start = Instant::now();
        cycle += 1;

        let below = *reader.read();
        let belief = writer.back_buffer();
        belief.layer = layer_id;
        // SAFETY: the layer is single-writer, and weights/bias are disjoint
        // from the belief buffer the writer owns.
        let (weights, bias) = unsafe {
            let slot = &mut *layer_slot_ptr(&shm, layer_id as usize);
            (&mut slot.weights, &mut slot.bias)
        };
        context.dispatch_belief_update(belief, &below, weights, bias);

        if injects {
            let world = shm.world_model();
            let world_seq = world.update_seq.load(Ordering::Relaxed);
            let alpha = if world_seq != seen_world_seq && world_seq > 0 {
                seen_world_seq = world_seq;
                let burst = (semantic_alpha * 3.0).min(0.95);
                if last_thought.elapsed() >= thought_cooldown {
                    shm.emit_thought(
                        layer_id,
                        THOUGHT_SURPRISE,
                        belief.vfe,
                        &format!(
                            "{description} semantic inject: alpha={burst:.2}, VFE={:.4}, seq={}",
                            belief.vfe, world_seq
                        ),
                    );
                    last_thought = Instant::now();
                }
                burst
            } else {
                semantic_alpha
            };

            for index in 0..STATE_DIM {
                let embedding = world.scene_embedding[index];
                let precision_scale = 1.0 / (1.0 + belief.precision[index] * 0.1);
                belief.mean[index] =
                    belief.mean[index] * (1.0 - alpha * precision_scale) + embedding * alpha * precision_scale;
            }
            for index in 0..STATE_DIM {
                let strength = world.scene_embedding[index].abs();
                if strength > 0.3 {
                    belief.precision[index] =
                        (belief.precision[index] * (1.0 + alpha * 0.1 * strength)).min(100.0);
                }
            }
        }

        let elapsed = cycle_start.elapsed();
        belief.cycle_us = elapsed.as_micros() as u32;
        belief.timestamp_ns = now_ns();

        let vfe = belief.vfe;
        let compression = belief.compression;
        let streak = belief.confirm_streak;
        let challenged = belief.challenge_vfe > params.threshold;

        if challenged {
            my_slot.challenge_flag.store(true, Ordering::Release);
            my_slot.challenge_total.fetch_add(1, Ordering::Relaxed);
        } else {
            my_slot.confirm_flag.store(true, Ordering::Release);
            my_slot.confirm_total.fetch_add(1, Ordering::Relaxed);
        }

        writer.publish();

        if last_thought.elapsed() >= thought_cooldown {
            if let Some((kind, text)) = generate_thought(
                description,
                vfe,
                previous_vfe,
                compression,
                previous_compression,
                streak,
                previous_streak,
                challenged,
                cycle,
                &params,
            ) {
                shm.emit_thought(layer_id, kind, vfe, &text);
                last_thought = Instant::now();
            }
        }

        if challenged && vfe > params.threshold * 3.0 {
            high_vfe_streak += 1;
        } else {
            high_vfe_streak = 0;
        }
        if compression == previous_compression && compression > 0 {
            compression_stall += 1;
        } else {
            compression_stall = 0;
        }

        if injects
            && last_question.elapsed() >= question_cooldown
            && !my_slot.question.pending.load(Ordering::Relaxed)
        {
            if let Some((reason, text)) = generate_question(
                layer_id,
                description,
                vfe,
                high_vfe_streak,
                compression,
                compression_stall,
                challenged,
                cycle,
                &params,
            ) {
                // SAFETY: this process is the only writer for its own layer,
                // and the question slot is disjoint from the belief buffers.
                let slot = unsafe { &mut *layer_slot_ptr(&shm, layer_id as usize) };
                slot.question.text.fill(0);
                let bytes = text.as_bytes();
                let length = bytes.len().min(MAX_QUESTION_TEXT - 1);
                slot.question.text[..length].copy_from_slice(&bytes[..length]);
                slot.question.layer = layer_id;
                slot.question.reason = reason;
                slot.question.vfe = vfe;
                slot.question.timestamp_ns = now_ns();
                slot.question.pending.store(true, Ordering::Release);

                shm.emit_thought(
                    layer_id,
                    THOUGHT_ESCALATE,
                    vfe,
                    &format!("QUESTION VFE={vfe:.3}: {text}"),
                );
                last_question = Instant::now();
            }
        }

        previous_vfe = vfe;
        previous_compression = compression;
        previous_streak = streak;

        let compute_time = cycle_start.elapsed();
        if compute_time < tick {
            std::thread::sleep(tick - compute_time);
        }
    }
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Picks the one thought worth recording for this cycle, if any.
fn generate_thought(
    description: &str,
    vfe: f32,
    previous_vfe: f32,
    compression: u8,
    previous_compression: u8,
    streak: u32,
    previous_streak: u32,
    challenged: bool,
    cycle: u64,
    params: &LayerParams,
) -> Option<(u8, String)> {
    let vfe_delta = vfe - previous_vfe;
    let vfe_ratio = if previous_vfe > 0.001 {
        vfe / previous_vfe
    } else {
        1.0
    };

    if challenged && vfe_ratio > 2.0 && vfe > params.threshold * 2.0 {
        return Some((
            THOUGHT_SURPRISE,
            format!(
                "{description} VFE {previous_vfe:.4} -> {vfe:.4} ({vfe_ratio:.1}x spike, thresh={:.4})",
                params.threshold
            ),
        ));
    }
    if challenged && previous_streak > 20 {
        return Some((
            THOUGHT_SURPRISE,
            format!("{description} broke a {previous_streak}-cycle streak, VFE={vfe:.4}"),
        ));
    }
    if challenged && vfe > params.threshold {
        return Some((
            THOUGHT_LEARN,
            format!(
                "{description} VFE={vfe:.4} (thresh={:.4}, {:.1}x), dW active",
                params.threshold,
                vfe / params.threshold
            ),
        ));
    }
    if compression > previous_compression && compression % 5 == 0 {
        return Some((
            THOUGHT_RESOLVE,
            format!("{description} compression {previous_compression} -> {compression}"),
        ));
    }
    if streak > 0 && previous_streak > 0 && streak % 100 == 0 && streak != previous_streak {
        return Some((
            THOUGHT_RESOLVE,
            format!("{description} streak={streak}, VFE={vfe:.4}, comp={compression}"),
        ));
    }
    if vfe_delta < -0.01 && previous_vfe > params.threshold && vfe < params.threshold {
        return Some((
            THOUGHT_RESOLVE,
            format!(
                "{description} VFE {previous_vfe:.4} -> {vfe:.4} (below thresh={:.4})",
                params.threshold
            ),
        ));
    }
    if vfe > params.threshold * 10.0 && cycle % 10 == 0 {
        return Some((
            THOUGHT_ESCALATE,
            format!(
                "{description} VFE={vfe:.3} ({:.0}x thresh), cycle {cycle}",
                vfe / params.threshold
            ),
        ));
    }
    if !challenged && streak > 50 && cycle % 200 == 0 {
        return Some((
            THOUGHT_OBSERVE,
            format!("{description} stable: streak={streak}, VFE={vfe:.4}, comp={compression}"),
        ));
    }
    if cycle <= 3 {
        return Some((
            THOUGHT_PREDICT,
            format!(
                "Making my first predictions about {description} (CUDA). Identity init — predicting the layer below."
            ),
        ));
    }
    None
}

/// Raises a question when the layer cannot explain its own error.
fn generate_question(
    layer_id: u8,
    description: &str,
    vfe: f32,
    high_vfe_streak: u32,
    compression: u8,
    compression_stall: u32,
    challenged: bool,
    cycle: u64,
    params: &LayerParams,
) -> Option<(u8, String)> {
    if high_vfe_streak > 50 && vfe > params.threshold * 5.0 {
        let text = match layer_id {
            5 => format!(
                "I process {description} but my predictions keep failing (VFE={vfe:.4} for \
                 {high_vfe_streak} cycles). What deep pattern or regularity exists in this \
                 environment that I'm missing?"
            ),
            4 => format!(
                "My {description} predictions are wrong (VFE={vfe:.4}, {high_vfe_streak} cycles \
                 stuck). What is currently happening that I haven't accounted for?"
            ),
            3 => format!(
                "I can't predict what I'm seeing in {description} (VFE={vfe:.4}). \
                 What visual pattern should I expect here?"
            ),
            _ => format!(
                "Layer {layer_id} ({description}) can't converge (VFE={vfe:.4}, \
                 {high_vfe_streak} stuck cycles). What structure exists here that I should learn?"
            ),
        };
        return Some((0, text));
    }
    if compression_stall > 200 && compression > 20 && challenged {
        let text = match layer_id {
            5 => format!(
                "I've learned a pattern in {description} (compression={compression}) but can't \
                 simplify it. Is there a higher-level concept that unifies what I'm seeing?"
            ),
            4 => format!(
                "My {description} model plateaued at compression={compression}. \
                 What category or label describes this behavioral pattern?"
            ),
            _ => format!(
                "Compression stalled at {compression} for {description}. \
                 What abstraction am I missing?"
            ),
        };
        return Some((1, text));
    }
    if challenged && vfe > params.threshold * 8.0 && cycle > 100 {
        return Some((
            2,
            format!(
                "Something new appeared in {description}. VFE spiked to {vfe:.4}. \
                 What changed in the environment?"
            ),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The evidence-layout fixture is a compile-time ABI check. NVRTC compiles
    /// it here so the static assertions run as part of the suite. Without a
    /// device the test reports and returns.
    #[test]
    fn evidence_layout_fixture_compiles() {
        let Ok(adapter) = Adapter::new(0) else {
            eprintln!("skipping evidence layout ABI check: no CUDA device");
            return;
        };
        let source = include_str!("../../../kernels/evidence_layout.cu");
        let result = compile_for(&adapter, source);
        assert!(result.is_ok(), "ABI fixture failed to compile: {result:?}");
    }

    /// Every kernel this crate embeds must compile under NVRTC, which is the
    /// runtime path on the target. NVRTC needs no device, so this runs on a
    /// plain build host; a host without the CUDA toolkit is reported and
    /// skipped rather than failed.
    #[test]
    fn embedded_kernels_compile_under_nvrtc() {
        let options = || CompileOptions {
            arch: Some("compute_87"),
            ..Default::default()
        };
        let probe = "extern \"C\" __global__ void qualia_nvrtc_probe() {}";
        if compile_ptx_with_opts(probe, options()).is_err() {
            eprintln!("skipping kernel compile check: NVRTC is unavailable");
            return;
        }
        for (label, source) in [
            ("belief_update.cu", BELIEF_KERNEL),
            ("smoke kernel", SMOKE_KERNEL),
            ("costmap kernel", COSTMAP_KERNEL),
            ("cognition kernel", COGNITION_KERNEL),
            (
                "evidence_layout.cu",
                include_str!("../../../kernels/evidence_layout.cu"),
            ),
        ] {
            let result = compile_ptx_with_opts(source, options());
            assert!(
                result.is_ok(),
                "{label} failed NVRTC compilation: {result:?}"
            );
        }
    }
}
