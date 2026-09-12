//! The device path: the same LIF recurrence, one kernel launch per tick.
//!
//! Shapes are fixed at load: the incoming-edge CSR is uploaded once as four
//! struct-of-arrays sections, and each tick launches
//! `ceil(neuron_count / 256)` blocks of 256 threads over `lif_step` in
//! `kernels/cns_lif.cu`. Nothing is copied per tick except, when a caller asks
//! for it, the 1-byte-per-neuron spike vector.

use std::sync::Arc;

use cudarc::driver::{
    CudaContext, CudaFunction, CudaModule, CudaSlice, CudaStream, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::{compile_ptx_with_opts, CompileOptions, Ptx};

use crate::{IncomingCsr, LifParams};

/// The kernel source, embedded so the crate carries its own device code.
pub const LIF_KERNEL: &str = include_str!("../../../kernels/cns_lif.cu");

/// The fatbin the build produced for `CUDAARCHS`, or empty when the build left
/// the source to NVRTC.
const LIF_FATBIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cns_lif.fatbin"));

/// Threads per block for `lif_step`.
pub const BLOCK: u32 = 256;

/// A device-resident network.
pub struct LifDevice {
    stream: Arc<CudaStream>,
    // The module owns the loaded image; the function handle must not outlive it.
    _module: Arc<CudaModule>,
    step: CudaFunction,
    device_name: String,
    pub rowptr: CudaSlice<u64>,
    pub cols: CudaSlice<u32>,
    pub sign: CudaSlice<i8>,
    pub weight: CudaSlice<u16>,
    pub v: CudaSlice<f32>,
    pub refractory: CudaSlice<u8>,
    pub spike: CudaSlice<u8>,
    pub spike_out: CudaSlice<u8>,
    pub external: CudaSlice<f32>,
    neuron_count: u32,
}

impl LifDevice {
    /// Open device 0, load the kernel and upload the network.
    pub fn new(graph: &IncomingCsr) -> Result<Self, String> {
        let context = CudaContext::new(0).map_err(|error| format!("CUDA init: {error:?}"))?;
        let device_name = context
            .name()
            .unwrap_or_else(|_| "unknown CUDA device".to_string());
        let stream = context.default_stream();

        let module = if LIF_FATBIN.is_empty() {
            let capability = context
                .compute_capability()
                .map_err(|error| format!("capability query: {error:?}"))?;
            let options = CompileOptions {
                arch: Some(ptx_architecture(capability)),
                ..Default::default()
            };
            let ptx = compile_ptx_with_opts(LIF_KERNEL, options)
                .map_err(|error| format!("cns_lif NVRTC compile: {error:?}"))?;
            context
                .load_module(ptx)
                .map_err(|error| format!("cns_lif load: {error:?}"))?
        } else {
            context
                .load_module(Ptx::from_binary(LIF_FATBIN.to_vec()))
                .map_err(|error| format!("cns_lif fatbin load: {error:?}"))?
        };
        let step = module
            .load_function("lif_step")
            .map_err(|error| format!("lif_step missing: {error:?}"))?;
        let neurons = graph.neuron_count();
        let rowptr = stream
            .clone_htod(graph.rowptr.as_slice())
            .map_err(|error| format!("H2D rowptr: {error:?}"))?;
        let cols = stream
            .clone_htod(graph.cols.as_slice())
            .map_err(|error| format!("H2D cols: {error:?}"))?;
        let sign = stream
            .clone_htod(graph.sign.as_slice())
            .map_err(|error| format!("H2D sign: {error:?}"))?;
        let weight = stream
            .clone_htod(graph.weight.as_slice())
            .map_err(|error| format!("H2D weight: {error:?}"))?;
        let v = stream
            .alloc_zeros::<f32>(neurons)
            .map_err(|error| format!("alloc v: {error:?}"))?;
        let refractory = stream
            .alloc_zeros::<u8>(neurons)
            .map_err(|error| format!("alloc refractory: {error:?}"))?;
        let spike = stream
            .alloc_zeros::<u8>(neurons)
            .map_err(|error| format!("alloc spike: {error:?}"))?;
        let spike_out = stream
            .alloc_zeros::<u8>(neurons)
            .map_err(|error| format!("alloc spike_out: {error:?}"))?;
        let external = stream
            .alloc_zeros::<f32>(neurons)
            .map_err(|error| format!("alloc external: {error:?}"))?;

        Ok(Self {
            stream,
            _module: module,
            step,
            device_name,
            rowptr,
            cols,
            sign,
            weight,
            v,
            refractory,
            spike,
            spike_out,
            external,
            neuron_count: neurons as u32,
        })
    }

    /// The CUDA device name this context opened.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Device bytes resident for this network.
    pub fn bytes_resident(&self) -> usize {
        let neurons = self.neuron_count as usize;
        let edges = self.cols.len();
        // rowptr is per neuron; cols/sign/weight and the five per-neuron state
        // arrays are per edge and per neuron respectively.
        8 * (neurons + 1) + edges * (4 + 1 + 2) + neurons * (4 + 1 + 1 + 1 + 4)
    }

    /// Neuron count.
    pub fn neuron_count(&self) -> u32 {
        self.neuron_count
    }

    /// Edge count.
    pub fn edge_count(&self) -> usize {
        self.cols.len()
    }

    /// Upload one tick's external current (the sensory drive).
    pub fn set_external(&mut self, external: &[f32]) -> Result<(), String> {
        if external.len() != self.neuron_count as usize {
            return Err(format!(
                "external drive is {} values, the network has {} neurons",
                external.len(),
                self.neuron_count
            ));
        }
        self.stream
            .memcpy_htod(external, &mut self.external)
            .map_err(|error| format!("H2D external: {error:?}"))
    }

    /// Launch one tick, leaving the result in `spike_out`.
    ///
    /// One kernel serves both cases: a free-running network simply leaves the
    /// external buffer zeroed, because a `__global__` function cannot call
    /// another one without dynamic parallelism (nvcc rejects it).
    pub fn step(&mut self, params: &LifParams, driven: bool) -> Result<(), String> {
        let config = LaunchConfig {
            grid_dim: (self.neuron_count.div_ceil(BLOCK), 1, 1),
            block_dim: (BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        let count = self.neuron_count;
        let decay = params.decay;
        let threshold = params.threshold;
        let reset = params.reset;
        let refractory_ticks = params.refractory_ticks;
        let stream = self.stream.clone();
        // SAFETY: the argument order and types match kernels/cns_lif.cu, both
        // spike buffers are distinct device allocations, and the stream is
        // synchronized before either is read back.
        let _ = driven;
        // SAFETY: the argument order and types match kernels/cns_lif.cu, both
        // spike buffers are distinct device allocations, and the stream is
        // synchronized before either is read back.
        unsafe {
            stream
                .launch_builder(&self.step)
                .arg(&self.rowptr)
                .arg(&self.cols)
                .arg(&self.sign)
                .arg(&self.weight)
                .arg(&self.spike)
                .arg(&self.external)
                .arg(&mut self.v)
                .arg(&mut self.refractory)
                .arg(&mut self.spike_out)
                .arg(&count)
                .arg(&decay)
                .arg(&threshold)
                .arg(&reset)
                .arg(&refractory_ticks)
                .launch(config)
                .map_err(|error| format!("lif_step launch: {error:?}"))?;
        }
        std::mem::swap(&mut self.spike, &mut self.spike_out);
        Ok(())
    }

    /// Wait for every launched tick to finish.
    pub fn synchronize(&self) -> Result<(), String> {
        self.stream
            .synchronize()
            .map_err(|error| format!("synchronize: {error:?}"))
    }

    /// Copy the current spike vector back and return the firing node ids.
    pub fn fired(&self) -> Result<Vec<u32>, String> {
        let spikes: Vec<u8> = self
            .stream
            .clone_dtoh(&self.spike)
            .map_err(|error| format!("D2H spike: {error:?}"))?;
        Ok(spikes
            .iter()
            .enumerate()
            .filter(|(_, value)| **value != 0)
            .map(|(index, _)| index as u32)
            .collect())
    }
}

/// The PTX target NVRTC should emit for a device capability.
fn ptx_architecture(capability: (i32, i32)) -> &'static str {
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
    let mut chosen = "compute_87";
    for (major, minor, name) in ARCHITECTURES {
        if (major, minor) <= capability {
            chosen = name;
        }
    }
    chosen
}
