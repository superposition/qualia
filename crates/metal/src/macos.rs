//! The macOS belief path: a Metal compute pipeline plus the layer runner.
//!
//! `BeliefSlot`, the layer's weight matrix and its bias all live in shared
//! memory; the kernel reads and writes them in place, so one dispatch advances
//! both the belief and the generative model without a parameter round trip.

use qualia_types::*;

use metal::*;

use crate::gpu::{download, upload};

const BELIEF_KERNEL_SRC: &str = include_str!("../kernels/belief_update.metal");

/// Thought kinds recorded in the shared thought ring.
const THOUGHT_OBSERVE: u8 = 0;
const THOUGHT_PREDICT: u8 = 1;
const THOUGHT_SURPRISE: u8 = 2;
const THOUGHT_LEARN: u8 = 3;
const THOUGHT_RESOLVE: u8 = 4;
const THOUGHT_ESCALATE: u8 = 5;

/// One line of narration per layer, used when composing thoughts.
const LAYER_DESCRIPTIONS: [&str; NUM_LAYERS] = [
    "raw sensation",
    "motor patterns",
    "local structure",
    "visual patterns",
    "short-term behavior",
    "deep patterns",
    "long-horizon context",
    "senses",
];

/// The Metal belief-update pipeline and its persistent buffers.
///
/// One context belongs to one layer: the buffers are sized for a single
/// [`BeliefSlot`] and re-used on every tick.
pub struct MetalContext {
    device_name: String,
    queue: CommandQueue,
    pipeline: ComputePipelineState,
    params_buf: Buffer,
    belief_buf: Buffer,
    below_buf: Buffer,
    weights_buf: Buffer,
    bias_buf: Buffer,
}

impl MetalContext {
    pub fn new(params: &LayerParams) -> Result<Self, String> {
        let device = Device::system_default().ok_or("No Metal device found")?;
        let queue = device.new_command_queue();

        let options = CompileOptions::new();
        let library = device
            .new_library_with_source(BELIEF_KERNEL_SRC, &options)
            .map_err(|error| format!("Metal compile error: {error}"))?;
        let function = library
            .get_function("belief_update", None)
            .map_err(|error| format!("Kernel function 'belief_update' not found: {error}"))?;
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|error| format!("Pipeline error: {error}"))?;

        // The kernel reads threshold, belief learning rate, layer id and weight
        // decay in that order.
        let params_data: [f32; 4] = [
            params.threshold,
            params.learning_rate,
            params.layer_id as f32,
            params.weight_decay,
        ];
        let params_buf = device.new_buffer_with_data(
            params_data.as_ptr() as *const _,
            (4 * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::StorageModeShared,
        );

        let belief_bytes = std::mem::size_of::<BeliefSlot>() as u64;
        let weight_bytes = (WEIGHT_COUNT * std::mem::size_of::<f32>()) as u64;
        let bias_bytes = (STATE_DIM * std::mem::size_of::<f32>()) as u64;
        let belief_buf = device.new_buffer(belief_bytes, MTLResourceOptions::StorageModeShared);
        let below_buf = device.new_buffer(belief_bytes, MTLResourceOptions::StorageModeShared);
        let weights_buf = device.new_buffer(weight_bytes, MTLResourceOptions::StorageModeShared);
        let bias_buf = device.new_buffer(bias_bytes, MTLResourceOptions::StorageModeShared);

        Ok(Self {
            device_name: device.name().to_string(),
            queue,
            pipeline,
            params_buf,
            belief_buf,
            below_buf,
            weights_buf,
            bias_buf,
        })
    }

    pub fn device_name(&self) -> String {
        self.device_name.clone()
    }

    /// Runs one belief update on the GPU, updating `belief`, `weights` and
    /// `bias` from `below` in place.
    pub fn dispatch_belief_update(
        &mut self,
        belief: &mut BeliefSlot,
        below: &BeliefSlot,
        weights: &mut [f32; WEIGHT_COUNT],
        bias: &mut [f32; STATE_DIM],
    ) {
        // SAFETY: each destination buffer was allocated for exactly the payload
        // copied into it, and the caller's slices are live for the call.
        unsafe {
            upload(&self.belief_buf, std::slice::from_ref(&*belief));
            upload(&self.below_buf, std::slice::from_ref(below));
            upload(&self.weights_buf, &weights[..]);
            upload(&self.bias_buf, &bias[..]);
        }

        let command = self.queue.new_command_buffer();
        let encoder = command.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&self.pipeline);
        encoder.set_buffer(0, Some(&self.belief_buf), 0);
        encoder.set_buffer(1, Some(&self.below_buf), 0);
        encoder.set_buffer(2, Some(&self.params_buf), 0);
        encoder.set_buffer(3, Some(&self.weights_buf), 0);
        encoder.set_buffer(4, Some(&self.bias_buf), 0);
        encoder.dispatch_threads(
            MTLSize::new(STATE_DIM as u64, 1, 1),
            MTLSize::new(STATE_DIM as u64, 1, 1),
        );
        encoder.end_encoding();
        command.commit();
        command.wait_until_completed();

        // SAFETY: the kernel wrote complete `BeliefSlot`, weight and bias
        // values, and each destination is the matching buffer's length.
        unsafe {
            download(&self.belief_buf, std::slice::from_mut(belief));
            download(&self.weights_buf, &mut weights[..]);
            download(&self.bias_buf, &mut bias[..]);
        }
    }
}

/// Run the belief loop for one layer until the process stops.
///
/// The runner owns its layer slot: it is the only writer of `my_slot`, and the
/// buffer it reads is the layer below (wrapping to the sensor plane for L0).
pub fn run_layer(layer_id: u8, name: &str) {
    use qualia_shm::{LayerReader, LayerWriter, ShmRegion};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let shm_name = std::env::var("QUALIA_SHM_NAME").unwrap_or_else(|_| "/qualia_body".to_string());
    eprintln!("qualia-{name}: opening shm '{shm_name}', layer {layer_id}");

    let shm = ShmRegion::open(&shm_name).unwrap_or_else(|error| {
        panic!("qualia-{name}: failed to open shm: {error}");
    });

    let params = default_params(layer_id);
    let mut metal = MetalContext::new(&params).unwrap_or_else(|error| {
        panic!("qualia-{name}: Metal init failed: {error}");
    });
    eprintln!("qualia-{name}: Metal device: {}", metal.device_name());

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

    // Identity generative model: predict that the layer below is unchanged.
    // SAFETY: this runner is the sole writer of its own layer slot and the
    // weight and bias arrays are disjoint from the belief buffers.
    unsafe {
        let slot = my_slot as *const LayerSlot as *mut LayerSlot;
        for row in 0..STATE_DIM {
            for column in 0..STATE_DIM {
                (*slot).weights[row * STATE_DIM + column] = if row == column { 1.0 } else { 0.0 };
            }
            (*slot).bias[row] = 0.0;
        }
    }

    // Seed both belief buffers with precision 1.0. Starting from an untouched
    // zeroed back buffer would pin precision at its floor and slow convergence.
    for _ in 0..2 {
        let buffer = writer.back_buffer();
        buffer.layer = layer_id;
        for index in 0..STATE_DIM {
            buffer.mean[index] = 0.0;
            buffer.precision[index] = 1.0;
            buffer.prediction[index] = 0.0;
            buffer.residual[index] = 0.0;
        }
        buffer.vfe = 0.0;
        buffer.challenge_vfe = 0.0;
        buffer.confirm_streak = 0;
        buffer.compression = 0;
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
            "{description} init: dim={STATE_DIM}, freq={:.1}Hz",
            params.freq_hz
        ),
    );
    eprintln!(
        "qualia-{name}: running at {:.1} Hz with 64×64 generative model",
        params.freq_hz
    );

    // Lower layers change faster, so they narrate less often.
    let thought_cooldown = Duration::from_millis(match layer_id {
        0 => 2_000,
        1..=3 => 1_000,
        4..=5 => 500,
        6 => 300,
        _ => 2_000,
    });

    let mut last_thought = Instant::now();
    let mut previous_vfe = 0.0_f32;
    let mut previous_compression = 0_u8;
    let mut previous_streak = 0_u32;
    let mut cycle: u64 = 0;

    loop {
        let cycle_start = Instant::now();
        cycle += 1;

        let below = *reader.read();

        // SAFETY: as above — the weights and bias live in this layer's slot,
        // which this runner alone writes.
        let buffer = writer.back_buffer();
        unsafe {
            let slot = my_slot as *const LayerSlot as *mut LayerSlot;
            metal.dispatch_belief_update(buffer, &below, &mut (*slot).weights, &mut (*slot).bias);
        }

        buffer.cycle_us = cycle_start.elapsed().as_micros() as u32;
        buffer.timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);

        let vfe = buffer.vfe;
        let compression = buffer.compression;
        let streak = buffer.confirm_streak;
        let challenge = buffer.challenge_vfe > params.threshold;

        if challenge {
            my_slot.challenge_flag.store(true, Ordering::Release);
            my_slot.challenge_total.fetch_add(1, Ordering::Relaxed);
        } else {
            my_slot.confirm_flag.store(true, Ordering::Release);
            my_slot.confirm_total.fetch_add(1, Ordering::Relaxed);
        }

        writer.publish();

        if last_thought.elapsed() >= thought_cooldown {
            if let Some((kind, text)) = narrate(
                layer_id,
                description,
                vfe,
                previous_vfe,
                compression,
                previous_compression,
                streak,
                previous_streak,
                challenge,
                cycle,
                &params,
            ) {
                shm.emit_thought(layer_id, kind, vfe, &text);
                last_thought = Instant::now();
            }
        }

        previous_vfe = vfe;
        previous_compression = compression;
        previous_streak = streak;

        let compute = cycle_start.elapsed();
        if compute < tick {
            std::thread::sleep(tick - compute);
        }
    }
}

/// Composes at most one thought per cooldown from what the last tick did.
///
/// No semantic injection is applied at this layer: the agent feeds typed,
/// evidence-backed priors to L6 instead, so the narration only reflects the
/// layer's own prediction error.
#[allow(clippy::too_many_arguments)]
fn narrate(
    layer_id: u8,
    description: &str,
    vfe: f32,
    previous_vfe: f32,
    compression: u8,
    previous_compression: u8,
    streak: u32,
    previous_streak: u32,
    challenge: bool,
    cycle: u64,
    params: &LayerParams,
) -> Option<(u8, String)> {
    let ratio = if previous_vfe > 0.001 {
        vfe / previous_vfe
    } else {
        1.0
    };

    if challenge && ratio > 2.0 && vfe > params.threshold * 2.0 {
        return Some((
            THOUGHT_SURPRISE,
            format!(
                "{description} VFE {previous_vfe:.4} -> {vfe:.4} ({ratio:.1}x spike, thresh={:.4})",
                params.threshold
            ),
        ));
    }

    if challenge && previous_streak > 20 {
        return Some((
            THOUGHT_SURPRISE,
            format!("{description} broke {previous_streak}-cycle streak, VFE={vfe:.4}"),
        ));
    }

    if challenge {
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

    if vfe - previous_vfe < -0.01 && previous_vfe > params.threshold && vfe < params.threshold {
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

    if streak > 50 && cycle % 200 == 0 {
        return Some((
            THOUGHT_OBSERVE,
            format!("{description} stable: streak={streak}, VFE={vfe:.4}, comp={compression}"),
        ));
    }

    if cycle <= 3 {
        return Some((
            THOUGHT_PREDICT,
            format!(
                "Layer {layer_id} predicting {description}: the generative model starts as identity, so my first guess is that the layer below is already what I believe."
            ),
        ));
    }

    None
}
