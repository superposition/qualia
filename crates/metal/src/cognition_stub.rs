//! The CPU fallback used on every platform without Metal.
//!
//! This is not a GPU backend and does not pretend to be one. It is a scalar
//! port of the same dense predictive-coding update the Metal kernel runs, so a
//! host with neither Metal nor CUDA can still drive the agent's cognition
//! layers. It therefore computes real numbers — it just does so one `f32` at a
//! time — and it is honest about what it is: [`MetalCognitionStack::backend_name`]
//! reports `cpu-fallback`, never a device name.
//!
//! Layout matches the GPU path so a checkpoint written here restores there:
//! four layers, each with a `STATE_DIM × STATE_DIM` weight matrix, a
//! `STATE_DIM` bias vector and a `STATE_DIM` state vector.

use qualia_types::{STATE_DIM, WEIGHT_COUNT};

/// Layers the cognition stack carries, matching the GPU stack.
const LAYER_COUNT: usize = 4;

/// State integration rate, `params[2]` in the Metal kernel.
const STATE_RATE: f32 = 0.00012;
/// Weight integration rate, `params[3]` in the Metal kernel.
const WEIGHT_RATE: f32 = 0.00008;
/// Largest per-row weight gradient the kernel applies.
const GRADIENT_CLAMP: f32 = 0.001;
/// Bounds the kernel clamps its buffers to.
const STATE_CLAMP: f32 = 4.0;
const WEIGHT_CLAMP: f32 = 2.0;
const BIAS_CLAMP: f32 = 1.0;
const BOTTOM_UP_CLAMP: f32 = 10.0;
/// Seed for the diagonal of a freshly constructed stack.
const DIAGONAL_SEED: f32 = 0.75;
const DIAGONAL_STEP: f32 = 0.05;

struct Layer {
    state: Vec<f32>,
    weights: Vec<f32>,
    bias: Vec<f32>,
}

impl Layer {
    fn seeded(layer: usize) -> Self {
        let mut weights = vec![0.0_f32; WEIGHT_COUNT];
        let diagonal = DIAGONAL_SEED + layer as f32 * DIAGONAL_STEP;
        for index in 0..STATE_DIM {
            weights[index * STATE_DIM + index] = diagonal;
        }
        Self {
            state: vec![0.0_f32; STATE_DIM],
            weights,
            bias: vec![0.0_f32; STATE_DIM],
        }
    }
}

/// Dense cognition parameters backed by host memory.
///
/// The public surface is identical to the Metal stack's, so a consumer does not
/// care which one it holds; only [`Self::backend_name`] reveals the difference.
pub struct MetalCognitionStack {
    layers: Vec<Layer>,
}

impl MetalCognitionStack {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            layers: (0..LAYER_COUNT).map(Layer::seeded).collect(),
        })
    }

    pub fn backend_name(&self) -> String {
        "cpu-fallback".to_string()
    }

    /// An independent copy of every parameter and state vector.
    pub fn fork(&self) -> Result<Self, String> {
        Ok(Self {
            layers: self
                .layers
                .iter()
                .map(|layer| Layer {
                    state: layer.state.clone(),
                    weights: layer.weights.clone(),
                    bias: layer.bias.clone(),
                })
                .collect(),
        })
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
        let slot = &mut self.layers[layer].weights[row * STATE_DIM + column];
        *slot = (*slot + delta).clamp(-WEIGHT_CLAMP, WEIGHT_CLAMP);
        Ok(*slot)
    }

    /// A `side × side` block-RMS summary of one weight matrix.
    ///
    /// Each cell samples an 8×8 lattice inside its source block, so an operator
    /// view never has to move the whole 4 MiB matrix.
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
        let weights = &self.layers[layer].weights;
        let mut sketch = Vec::with_capacity(side * side);
        for block_row in 0..side {
            for block_column in 0..side {
                let row_start = block_row * block;
                let column_start = block_column * block;
                let mut sum_squares = 0.0_f32;
                let mut count = 0_usize;
                for row in (row_start..row_start + block).step_by(stride) {
                    for column in (column_start..column_start + block).step_by(stride) {
                        let value = weights[row * STATE_DIM + column];
                        sum_squares += value * value;
                        count += 1;
                    }
                }
                sketch.push((sum_squares / count.max(1) as f32).sqrt());
            }
        }
        Ok(sketch)
    }

    /// Runs one scalar predictive-coding step and returns the new state vector.
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
        let entry = &mut self.layers[layer];
        let previous = entry.state.clone();
        let signal = learning_signal.clamp(0.0, 1.0);

        let mut bottom_up = vec![0.0_f32; STATE_DIM];
        for row in 0..STATE_DIM {
            let start = row * STATE_DIM;
            let mut prediction = entry.bias[row];
            for column in 0..STATE_DIM {
                prediction += entry.weights[start + column] * previous[column];
            }
            bottom_up[row] = (lower[row] - prediction).clamp(-BOTTOM_UP_CLAMP, BOTTOM_UP_CLAMP);
        }

        for row in 0..STATE_DIM {
            // Wᵀ applied to the bottom-up error.
            let mut transposed_error = 0.0_f32;
            for source in 0..STATE_DIM {
                transposed_error += entry.weights[source * STATE_DIM + row] * bottom_up[source];
            }
            let top_down_error = previous[row] - top_down[row];
            let next = previous[row] + STATE_RATE * source_precision * transposed_error
                - STATE_RATE * top_down_precision * top_down_error;
            entry.state[row] = next.clamp(-STATE_CLAMP, STATE_CLAMP);

            let row_gradient = (WEIGHT_RATE * signal * source_precision * bottom_up[row])
                .clamp(-GRADIENT_CLAMP, GRADIENT_CLAMP);
            let start = row * STATE_DIM;
            for column in 0..STATE_DIM {
                let weight = &mut entry.weights[start + column];
                *weight = (*weight + row_gradient * previous[column]).clamp(-WEIGHT_CLAMP, WEIGHT_CLAMP);
            }
            entry.bias[row] = (entry.bias[row] + row_gradient).clamp(-BIAS_CLAMP, BIAS_CLAMP);
        }

        Ok(entry.state.clone())
    }

    pub fn checkpoint_parameters(&self) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        (
            self.layers.iter().map(|layer| layer.weights.clone()).collect(),
            self.layers.iter().map(|layer| layer.bias.clone()).collect(),
        )
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
        for index in 0..self.layers.len() {
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
        for (index, layer) in self.layers.iter_mut().enumerate() {
            layer.weights.copy_from_slice(&weights[index]);
            layer.bias.copy_from_slice(&biases[index]);
            layer.state.copy_from_slice(&states[index]);
        }
        Ok(())
    }
}
