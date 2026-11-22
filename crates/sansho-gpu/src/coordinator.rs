//! Lock-step collection of device evaluations from concurrently running batch members.
//!
//! Provenance: original implementation. See `docs/PROVENANCE.md`.

use crate::Jacobian;
use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, DeviceRepr, LaunchConfig, PushKernelArg,
};
use sansho_devices::diode::{DIODE_BATCH_PARAMS, DiodeBatchTopology};
use sansho_devices::{EvalCtx, Mode, Slots, StateRef};
use sansho_diag::Diagnostic;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

const SOURCE: &str = include_str!("diode.cu");
const BLOCK: u32 = 64;

/// One wrapped device group of the batch topology.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupPlan {
    /// Per-instance connectivity, identical for every run.
    pub topology: Vec<DiodeBatchTopology>,
    /// Slot counts `G`, `C`, `f`, `q` of the group's layout.
    pub slots: [usize; 4],
}

impl GroupPlan {
    fn output_len(&self) -> usize {
        self.slots.iter().sum::<usize>() + 2 * self.topology.len()
    }
}

/// Launch counters of a finished batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Kernel launches.
    pub launches: u64,
    /// Group evaluations served by those launches.
    pub requests: u64,
    /// Launches that used the single-precision Jacobian kernel.
    pub f32_launches: u64,
    /// Wall time spent packing, transferring and running launches (ns).
    pub launch_ns: u64,
}

struct Request {
    lane: usize,
    group: usize,
    ctx: [f64; 4],
    f32: bool,
    input: Vec<f64>,
}

#[derive(Debug)]
struct Buffer<T: DeviceRepr> {
    data: Option<CudaSlice<T>>,
}

impl<T: DeviceRepr + cudarc::driver::ValidAsZeroBits> Buffer<T> {
    fn upload(&mut self, stream: &Arc<CudaStream>, host: &[T]) -> Result<(), String> {
        let len = host.len().max(1);
        if self.data.as_ref().is_none_or(|d| d.len() < len) {
            self.data = Some(
                stream
                    .alloc_zeros::<T>(len.next_power_of_two())
                    .map_err(text)?,
            );
        }
        if let Some(d) = self.data.as_mut()
            && !host.is_empty()
        {
            stream
                .memcpy_htod(host, &mut d.slice_mut(..host.len()))
                .map_err(text)?;
        }
        Ok(())
    }
    fn ensure(&mut self, stream: &Arc<CudaStream>, len: usize) -> Result<(), String> {
        let len = len.max(1);
        if self.data.as_ref().is_none_or(|d| d.len() < len) {
            self.data = Some(
                stream
                    .alloc_zeros::<T>(len.next_power_of_two())
                    .map_err(text)?,
            );
        }
        Ok(())
    }
    fn get(&self) -> Result<&CudaSlice<T>, String> {
        self.data
            .as_ref()
            .ok_or_else(|| "GPU buffer not allocated".to_string())
    }
}

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct Device {
    stream: Arc<CudaStream>,
    f64_kernel: CudaFunction,
    f32_kernel: CudaFunction,
    groups: CudaSlice<i64>,
    instances: CudaSlice<i64>,
    params: CudaSlice<f64>,
    req: Buffer<i64>,
    ctx: Buffer<f64>,
    input: Buffer<f64>,
    output: Buffer<f64>,
}

struct Shared {
    live: usize,
    pending: Vec<Request>,
    results: Vec<Option<Result<Vec<f64>, String>>>,
    device: Device,
    stats: Stats,
}

/// Collects evaluation requests of all lanes and launches them together.
pub struct Coordinator {
    shared: Mutex<Shared>,
    ready: Condvar,
    plans: Vec<GroupPlan>,
    first: Vec<usize>,
    instances: usize,
    dim: usize,
    jacobian: Jacobian,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator")
            .field("groups", &self.plans.len())
            .field("instances", &self.instances)
            .field("dim", &self.dim)
            .finish_non_exhaustive()
    }
}

fn gpu_error(message: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error("E-GPU", format!("GPU batch evaluation failed: {message}"))
}

/// Initialize CUDA, compile the kernels and upload the shared topology.
fn open(
    config: &crate::Config,
    plans: &[GroupPlan],
    first: &[usize],
    instances: usize,
) -> Result<Device, String> {
    let ctx = CudaContext::new(config.device).map_err(text)?;
    let stream = ctx.default_stream();
    let (major, minor) = ctx.compute_capability().map_err(text)?;
    let compile = |dt: &str| {
        let opts = cudarc::nvrtc::CompileOptions {
            fmad: Some(false),
            options: vec![
                format!("--gpu-architecture=compute_{major}{minor}"),
                format!("-DDT={dt}"),
            ],
            ..Default::default()
        };
        let ptx = cudarc::nvrtc::compile_ptx_with_opts(SOURCE, opts).map_err(text)?;
        ctx.load_module(ptx)
            .and_then(|m| m.load_function("diode_eval"))
            .map_err(text)
    };
    let f64_kernel = compile("double")?;
    let f32_kernel = compile("float")?;
    let mut group_table = Vec::with_capacity(plans.len() * 8);
    for (plan, &start) in plans.iter().zip(first) {
        let [g, c, f, q] = plan.slots.map(|n| n as i64);
        group_table.extend([start as i64, plan.topology.len() as i64, g, c, f, q, 0, 0]);
    }
    let instance_table: Vec<i64> = plans
        .iter()
        .flat_map(|p| p.topology.iter().flatten().copied())
        .collect();
    let groups = stream.clone_htod(&group_table).map_err(text)?;
    let instances_buf = stream
        .clone_htod(&if instance_table.is_empty() {
            vec![0]
        } else {
            instance_table
        })
        .map_err(text)?;
    let params = stream
        .alloc_zeros::<f64>((config.lanes * instances * DIODE_BATCH_PARAMS).max(1))
        .map_err(text)?;
    Ok(Device {
        stream,
        f64_kernel,
        f32_kernel,
        groups,
        instances: instances_buf,
        params,
        req: Buffer { data: None },
        ctx: Buffer { data: None },
        input: Buffer { data: None },
        output: Buffer { data: None },
    })
}

impl Coordinator {
    /// Open the device and prepare `config.lanes` lanes for the given groups.
    ///
    /// Every lane counts as live until it calls [`Self::leave`].
    pub fn new(
        config: &crate::Config,
        plans: Vec<GroupPlan>,
        dim: usize,
    ) -> Result<Arc<Self>, Diagnostic> {
        let mut first = Vec::with_capacity(plans.len());
        let mut instances = 0;
        for p in &plans {
            first.push(instances);
            instances += p.topology.len();
        }
        // The dynamic loader reports a missing CUDA installation by panicking.
        let device = std::panic::catch_unwind(|| open(config, &plans, &first, instances))
            .map_err(|_| gpu_error("CUDA driver or NVRTC library not found"))?
            .map_err(gpu_error)?;
        Ok(Arc::new(Self {
            shared: Mutex::new(Shared {
                live: config.lanes,
                pending: Vec::new(),
                results: (0..config.lanes).map(|_| None).collect(),
                device,
                stats: Stats::default(),
            }),
            ready: Condvar::new(),
            plans,
            first,
            instances,
            dim,
            jacobian: config.jacobian,
        }))
    }

    /// Wrapped groups of the batch topology.
    pub fn plans(&self) -> &[GroupPlan] {
        &self.plans
    }

    /// Launch counters so far.
    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Upload one run's model rows for plan group `group`.
    pub fn bind(&self, lane: usize, group: usize, params: &[f64]) -> Result<(), Diagnostic> {
        let plan = self
            .plans
            .get(group)
            .ok_or_else(|| gpu_error("unknown group"))?;
        if params.len() != plan.topology.len() * DIODE_BATCH_PARAMS {
            return Err(gpu_error("parameter table size mismatch"));
        }
        let mut shared = self.lock();
        if lane >= shared.results.len() {
            return Err(gpu_error("lane out of range"));
        }
        let start = (lane * self.instances + self.first[group]) * DIODE_BATCH_PARAMS;
        let device = &mut shared.device;
        let stream = device.stream.clone();
        let mut view = device
            .params
            .try_slice_mut(start..start + params.len())
            .ok_or_else(|| gpu_error("parameter buffer too small"))?;
        stream.memcpy_htod(params, &mut view).map_err(gpu_error)?;
        Ok(())
    }

    /// Mark `lane` as finished. Must be called exactly once per lane.
    pub fn leave(&self) {
        let mut shared = self.lock();
        shared.live = shared.live.saturating_sub(1);
        if shared.live > 0 && shared.pending.len() == shared.live {
            self.launch(&mut shared);
            self.ready.notify_all();
        }
    }

    /// Evaluate plan group `group` for `lane`, blocking until the batch launch completes.
    pub(crate) fn eval(
        &self,
        lane: usize,
        group: usize,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
    ) {
        let Some(plan) = self.plans.get(group) else {
            out.error = Some(gpu_error("unknown group"));
            return;
        };
        let n = plan.topology.len();
        if x.len() != self.dim
            || state.current.len() != n
            || [out.g.len(), out.c.len(), out.f.len(), out.q.len()] != plan.slots
        {
            out.error = Some(gpu_error("slot layout mismatch"));
            return;
        }
        let newton = ctx.limit && matches!(ctx.mode, Mode::Dc | Mode::Tran);
        let mut input = Vec::with_capacity(self.dim + n);
        input.extend_from_slice(x);
        input.extend_from_slice(state.current);
        let request = Request {
            lane,
            group,
            ctx: [
                ctx.gmin,
                ctx.temp,
                f64::from(u8::from(ctx.limit)),
                f64::from(u8::from(ctx.init_junctions)),
            ],
            f32: newton && self.jacobian == Jacobian::F32,
            input,
        };
        let mut shared = self.lock();
        shared.pending.push(request);
        if shared.pending.len() >= shared.live {
            self.launch(&mut shared);
            self.ready.notify_all();
        }
        let result = loop {
            if let Some(r) = shared.results.get_mut(lane).and_then(Option::take) {
                break r;
            }
            shared = self
                .ready
                .wait(shared)
                .unwrap_or_else(PoisonError::into_inner);
        };
        drop(shared);
        match result {
            Ok(values) => {
                let [g, c, f, q] = plan.slots;
                let mut at = 0;
                for (dst, len) in [
                    (&mut out.g, g),
                    (&mut out.c, c),
                    (&mut out.f, f),
                    (&mut out.q, q),
                ] {
                    dst.copy_from_slice(&values[at..at + len]);
                    at += len;
                }
                state.current.copy_from_slice(&values[at..at + n]);
                out.noncon = values[at + n..at + 2 * n].iter().any(|&v| v != 0.);
                out.error = None;
            }
            Err(e) => out.error = Some(gpu_error(e)),
        }
    }

    /// Run all pending requests and publish their results.
    fn launch(&self, shared: &mut Shared) {
        let pending = std::mem::take(&mut shared.pending);
        if pending.is_empty() {
            return;
        }
        for f32 in [false, true] {
            let batch: Vec<&Request> = pending.iter().filter(|r| r.f32 == f32).collect();
            if batch.is_empty() {
                continue;
            }
            let started = std::time::Instant::now();
            let outcome = self.run(&mut shared.device, &batch, f32);
            shared.stats.launch_ns += started.elapsed().as_nanos() as u64;
            shared.stats.launches += 1;
            shared.stats.requests += batch.len() as u64;
            if f32 {
                shared.stats.f32_launches += 1;
            }
            match outcome {
                Ok(mut outputs) => {
                    for (r, values) in batch.iter().zip(outputs.drain(..)) {
                        shared.results[r.lane] = Some(Ok(values));
                    }
                }
                Err(e) => {
                    for r in &batch {
                        shared.results[r.lane] = Some(Err(e.clone()));
                    }
                }
            }
        }
    }

    fn run(&self, d: &mut Device, batch: &[&Request], f32: bool) -> Result<Vec<Vec<f64>>, String> {
        let mut req = Vec::with_capacity(batch.len() * 4);
        let mut ctx = Vec::with_capacity(batch.len() * 4);
        let mut input = Vec::new();
        let mut output_len = 0;
        let mut widest = 0;
        for r in batch {
            let plan = &self.plans[r.group];
            req.extend([
                r.lane as i64,
                r.group as i64,
                input.len() as i64,
                output_len as i64,
            ]);
            ctx.extend(r.ctx);
            input.extend_from_slice(&r.input);
            output_len += plan.output_len();
            widest = widest.max(plan.topology.len());
        }
        let stream = d.stream.clone();
        d.req.upload(&stream, &req)?;
        d.ctx.upload(&stream, &ctx)?;
        d.input.upload(&stream, &input)?;
        d.output.ensure(&stream, output_len)?;
        let count = i32::try_from(batch.len()).map_err(text)?;
        let dim = i32::try_from(self.dim).map_err(text)?;
        let instances = self.instances as i64;
        let config = LaunchConfig {
            grid_dim: (
                u32::try_from(widest.div_ceil(BLOCK as usize).max(1)).map_err(text)?,
                u32::try_from(batch.len()).map_err(text)?,
                1,
            ),
            block_dim: (BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        let kernel = if f32 { &d.f32_kernel } else { &d.f64_kernel };
        let (req_buf, ctx_buf, in_buf) = (d.req.get()?, d.ctx.get()?, d.input.get()?);
        let out_buf = d
            .output
            .data
            .as_mut()
            .ok_or_else(|| "GPU buffer not allocated".to_string())?;
        {
            let mut launch = stream.launch_builder(kernel);
            launch
                .arg(req_buf)
                .arg(ctx_buf)
                .arg(&d.groups)
                .arg(&d.instances)
                .arg(&d.params)
                .arg(in_buf)
                .arg(&mut *out_buf)
                .arg(&count)
                .arg(&instances)
                .arg(&dim);
            // SAFETY: the argument list matches `diode_eval` in diode.cu. Every request
            // offset indexes buffers sized above, and lane/group indices were validated on
            // submission.
            unsafe { launch.launch(config) }.map_err(text)?;
        }
        let mut host = vec![0.0; output_len];
        let view = out_buf
            .try_slice(..output_len)
            .ok_or_else(|| "GPU output buffer too small".to_string())?;
        stream.memcpy_dtoh(&view, &mut host).map_err(text)?;
        stream.synchronize().map_err(text)?;
        let mut at = 0;
        Ok(batch
            .iter()
            .map(|r| {
                let len = self.plans[r.group].output_len();
                let v = host[at..at + len].to_vec();
                at += len;
                v
            })
            .collect())
    }
}
