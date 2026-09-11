//! The macOS cognition stack: dense L3-L6 parameters kept GPU-resident.
//!
//! Each layer owns its state, its two error inputs, its weight matrix and its
//! bias as Metal buffers. Between ticks nothing crosses the bus; a checkpoint
//! reads the buffers back, and a fork copies them into an independent stack so
//! a research candidate can be evaluated without touching the canonical one.

use qualia_types::{STATE_DIM, WEIGHT_COUNT};

use metal::*;

use crate::gpu::{copy, download, upload};

const COGNITION_KERNEL_SRC: &str = include_str!("../kernels/cognition_update.metal");

/// Layers this stack carries, matching `qualia-agent`'s L3-L6 band.
const LAYER_COUNT: usize = 4;

/// State and weight integration rates passed to the kernel.
const STATE_RATE: f32 = 0.00012;
const WEIGHT_RATE: f32 = 0.00008;
/// The kernel clamps a stored weight to this magnitude.
const WEIGHT_CLAMP: f32 = 2.0;
/// Seed for a fresh layer's diagonal.
const DIAGONAL_SEED: f32 = 0.75;
const DIAGONAL_STEP: f32 = 0.05;

struct LayerBuffers {
    state: Buffer,
    lower: Buffer,
    top_down: Buffer,
    weights: Buffer,
    bias: Buffer,
    params: Buffer,
}

/// Floats the cognition kernel reads from its params buffer.
const PARAM_COUNT: usize = 5;

/// Allocates one layer's buffer set at the lengths the kernel expects.
fn allocate_layer(device: &Device, vector_bytes: u64, weight_bytes: u64) -> LayerBuffers {
    let shared = MTLResourceOptions::StorageModeShared;
    LayerBuffers {
        state: device.new_buffer(vector_bytes, shared),
        lower: device.new_buffer(vector_bytes, shared),
        top_down: device.new_buffer(vector_bytes, shared),
        weights: device.new_buffer(weight_bytes, shared),
        bias: device.new_buffer(vector_bytes, shared),
        params: device.new_buffer((PARAM_COUNT * std::mem::size_of::<f32>()) as u64, shared),
    }
}

/// Zeroes a fresh layer and seeds its diagonal so it starts near identity.
///
/// # Safety
/// Every buffer must have been allocated for `STATE_DIM` floats — weights for
/// `WEIGHT_COUNT`.
unsafe fn seed_layer(buffers: &LayerBuffers, layer: usize) {
    let weights = buffers.weights.contents() as *mut f32;
    std::ptr::write_bytes(weights, 0, WEIGHT_COUNT);
    let diagonal = DIAGONAL_SEED + layer as f32 * DIAGONAL_STEP;
    for index in 0..STATE_DIM {
        *weights.add(index * STATE_DIM + index) = diagonal;
    }
    std::ptr::write_bytes(buffers.state.contents() as *mut f32, 0, STATE_DIM);
    std::ptr::write_bytes(buffers.lower.contents() as *mut f32, 0, STATE_DIM);
    std::ptr::write_bytes(buffers.top_down.contents() as *mut f32, 0, STATE_DIM);
    std::ptr::write_bytes(buffers.bias.contents() as *mut f32, 0, STATE_DIM);
}

/// Dense cognition parameters resident on the GPU.
pub struct MetalCognitionStack {
    device_name: String,
    queue: CommandQueue,
    pipeline: ComputePipelineState,
    layers: Vec<LayerBuffers>,
}

impl MetalCognitionStack {
    pub fn new() -> Result<Self, String> {
        let device = Device::system_default().ok_or("No Metal device found")?;
        let queue = device.new_command_queue();

        let options = CompileOptions::new();
        let library = device
            .new_library_with_source(COGNITION_KERNEL_SRC, &options)
            .map_err(|error| format!("Metal cognition compile error: {error}"))?;
        let function = library
            .get_function("cognition_update", None)
            .map_err(|error| format!("Metal cognition function missing: {error}"))?;
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|error| format!("Metal cognition pipeline error: {error}"))?;

        let vector_bytes = (STATE_DIM * std::mem::size_of::<f32>()) as u64;
        let weight_bytes = (WEIGHT_COUNT * std::mem::size_of::<f32>()) as u64;
        let layers: Vec<LayerBuffers> = (0..LAYER_COUNT)
            .map(|layer| {
                let buffers = allocate_layer(&device, vector_bytes, weight_bytes);
                // SAFETY: `allocate_layer` sized every buffer for this payload.
                unsafe { seed_layer(&buffers, layer) };
                buffers
            })
            .collect();

        Ok(Self {
            device_name: device.name().to_string(),
            queue,
            pipeline,
            layers,
        })
    }

    pub fn backend_name(&self) -> String {
        format!("metal:{}", self.device_name)
    }

    /// An independent stack holding the same state, weights and biases.
    pub fn fork(&self) -> Result<Self, String> {
        let duplicate = Self::new()?;
        for (source, destination) in self.layers.iter().zip(&duplicate.layers) {
            // SAFETY: both sides were allocated at the same lengths.
            unsafe {
                copy::<f32>(&source.state, &destination.state, STATE_DIM);
                copy::<f32>(&source.weights, &destination.weights, WEIGHT_COUNT);
                copy::<f32>(&source.bias, &destination.bias, STATE_DIM);
            }
        }
        Ok(duplicate)
    }

    /// Nudges one weight and returns the stored value after clamping.
    pub fn apply_weight_delta(
        &mut self,
        layer: usize,
        row: usize,
        column: usize,
        delta: f32,
    ) -> Result<f32, String> {
        if layer >= self.layers.len() || row >= STATE_DIM || column >= STATE_DIM {
            return Err("invalid Metal cognition weight coordinate".to_string());
        }
        if !delta.is_finite() {
            return Err("cognition weight delta must be finite".to_string());
        }
        let index = row * STATE_DIM + column;
        // SAFETY: `index` is inside the `WEIGHT_COUNT`-element weight buffer.
        let updated = unsafe {
            let weights = self.layers[layer].weights.contents() as *mut f32;
            let value = (*weights.add(index) + delta).clamp(-WEIGHT_CLAMP, WEIGHT_CLAMP);
            *weights.add(index) = value;
            value
        };
        Ok(updated)
    }

    /// A `side × side` block-RMS summary of one weight matrix.
    ///
    /// Each cell samples an 8×8 lattice inside its source block, so a status
    /// refresh never transfers the full 4 MiB matrix.
    pub fn weight_sketch(&self, layer: usize, side: usize) -> Result<Vec<f32>, String> {
        if layer >= self.layers.len()
            || side == 0
            || side > STATE_DIM
            || !STATE_DIM.is_multiple_of(side)
        {
            return Err("invalid Metal cognition sketch shape".to_string());
        }
        let block = STATE_DIM / side;
        let stride = (block / 8).max(1);
        let weights = self.layers[layer].weights.contents() as *const f32;
        let mut sketch = Vec::with_capacity(side * side);
        for block_row in 0..side {
            for block_column in 0..side {
                let row_start = block_row * block;
                let column_start = block_column * block;
                let mut sum_squares = 0.0_f32;
                let mut count = 0_usize;
                for row in (row_start..row_start + block).step_by(stride) {
                    for column in (column_start..column_start + block).step_by(stride) {
                        // SAFETY: row and column are inside the matrix.
                        let value = unsafe { *weights.add(row * STATE_DIM + column) };
                        sum_squares += value * value;
                        count += 1;
                    }
                }
                sketch.push((sum_squares / count.max(1) as f32).sqrt());
            }
        }
        Ok(sketch)
    }

    /// Runs one predictive-coding step on the GPU and returns the new state.
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
            return Err("invalid Metal cognition layer or vector size".to_string());
        }
        let buffers = &self.layers[layer];
        let params = [
            source_precision,
            top_down_precision,
            STATE_RATE,
            WEIGHT_RATE,
            learning_signal.clamp(0.0, 1.0),
        ];
        // SAFETY: the slices match the buffer lengths and `params` is exactly
        // the five floats the kernel reads.
        unsafe {
            upload(&buffers.lower, lower);
            upload(&buffers.top_down, top_down);
            upload(&buffers.params, &params[..]);
        }

        let command = self.queue.new_command_buffer();
        let encoder = command.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&self.pipeline);
        encoder.set_buffer(0, Some(&buffers.state), 0);
        encoder.set_buffer(1, Some(&buffers.lower), 0);
        encoder.set_buffer(2, Some(&buffers.top_down), 0);
        encoder.set_buffer(3, Some(&buffers.weights), 0);
        encoder.set_buffer(4, Some(&buffers.bias), 0);
        encoder.set_buffer(5, Some(&buffers.params), 0);
        encoder.dispatch_threads(
            MTLSize::new(STATE_DIM as u64, 1, 1),
            MTLSize::new(STATE_DIM as u64, 1, 1),
        );
        encoder.end_encoding();
        command.commit();
        command.wait_until_completed();

        let mut state = vec![0.0_f32; STATE_DIM];
        // SAFETY: the kernel wrote exactly `STATE_DIM` floats into the buffer.
        unsafe {
            download(&buffers.state, &mut state);
        }
        Ok(state)
    }

    pub fn checkpoint_parameters(&self) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut weights = Vec::with_capacity(self.layers.len());
        let mut biases = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            let mut layer_weights = vec![0.0_f32; WEIGHT_COUNT];
            let mut layer_biases = vec![0.0_f32; STATE_DIM];
            // SAFETY: each destination is exactly the source buffer's length.
            unsafe {
                download(&layer.weights, &mut layer_weights);
                download(&layer.bias, &mut layer_biases);
            }
            weights.push(layer_weights);
            biases.push(layer_biases);
        }
        (weights, biases)
    }

    pub fn restore_parameters(
        &mut self,
        weights: &[Vec<f32>],
        biases: &[Vec<f32>],
        states: &[Vec<f32>],
    ) -> Result<(), String> {
        if weights.len() != self.layers.len()
            || biases.len() != self.layers.len()
            || states.len() != self.layers.len()
        {
            return Err("cognition checkpoint layer count mismatch".to_string());
        }
        for (index, layer) in self.layers.iter().enumerate() {
            if weights[index].len() != WEIGHT_COUNT
                || biases[index].len() != STATE_DIM
                || states[index].len() != STATE_DIM
                || weights[index].iter().any(|value| !value.is_finite())
                || biases[index].iter().any(|value| !value.is_finite())
                || states[index].iter().any(|value| !value.is_finite())
            {
                return Err("cognition checkpoint parameter shape is invalid".to_string());
            }
        }
        for (index, layer) in self.layers.iter().enumerate() {
            // SAFETY: lengths were validated against the buffer allocations.
            unsafe {
                upload(&layer.weights, &weights[index]);
                upload(&layer.bias, &biases[index]);
                upload(&layer.state, &states[index]);
            }
        }
        Ok(())
    }
}
