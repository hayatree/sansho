//! Prepare and invoke two-lane specializations for simulator-owned circuit inputs.
//! Provenance: original work using the repository specialization cache and native frame ABI.

use super::*;

/// Prepared two-instance kernel for the circuit adapter's default simulator queries.
#[derive(Debug, Clone)]
pub struct CircuitPair {
    kernel: Arc<Kernel>,
    stamp: CircuitStamp,
    tnom: f64,
}

#[derive(Debug)]
pub(super) struct BatchCode {
    code: abi::Code,
    constants: Vec<[f64; 2]>,
}

impl CompiledModule {
    /// Prepare a packed evaluation for a pure, bias-dependent circuit specialization.
    pub fn prepare_circuit_pair(&self, b: &Bound, temperature: f64) -> Option<CircuitPair> {
        if !crate::jit_enabled() || !self.direct_start || !Arc::ptr_eq(&self.identity, &b.identity)
        {
            return None;
        }
        let stamp = b.circuit.as_ref()?;
        let input = EvalInput {
            x: &[],
            temperature,
            mfactor: 1.,
            analysis: Analysis::Tran,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin: b.sim.gmin,
            limit: false,
            simparams: &|_| None,
        };
        if stamp.dc {
            return None;
        }
        let kernel = self.kernel(b, &input, |_| {})?;
        if !kernel.bias_only
            || stamp.wires.len().saturating_mul(stamp.wires.len()) > i32::MAX as usize / 16
        {
            return None;
        }
        kernel
            .batch
            .get_or_init(|| {
                let disk = self.artifact.as_ref().and_then(|a| {
                    serde_json::to_vec(&b.native_key(temperature, 1.))
                        .ok()
                        .map(|bytes| a.child(&[&bytes]))
                });
                let (spec, native) = self.specialize_native(b, &input, disk.as_ref()).ok()?;
                // Share one packed entry per bit pattern, in first-register order.
                let mut indices = HashMap::new();
                let mut constants = Vec::new();
                for &x in &spec.program.values {
                    indices.entry(x.to_bits()).or_insert_with(|| {
                        let i = constants.len();
                        constants.push([x, x]);
                        i
                    });
                }
                if constants.len() > i32::MAX as usize / 16 {
                    return None;
                }
                std::panic::catch_unwind(|| {
                    lower::compile(
                        &spec.program,
                        stamp.wires.len(),
                        true,
                        native.as_ref(),
                        Some(stamp),
                        Some(&indices),
                    )
                })
                .ok()?
                .inspect_err(|error| {
                    if std::env::var_os("SPX_PROFILE").is_some() {
                        let _ = writeln!(std::io::stderr().lock(), "spx-pair-compile {error}");
                    }
                })
                .ok()
                .map(|code| BatchCode { code, constants })
            })
            .as_ref()?;
        Some(CircuitPair {
            kernel,
            stamp: stamp.clone(),
            tnom: b.sim.tnom,
        })
    }
}

impl CircuitPair {
    /// Evaluate two independent biases in interleaved order. On a guard failure
    /// the caller discards the partial result and evaluates both scalar instances.
    pub fn evaluate(&self, x: &[f64], gmin: f64, out: &mut EvalOutput) -> bool {
        let n = self.stamp.wires.len();
        if x.len() != 2 * n
            || self.stamp.wires.iter().enumerate().any(|(i, &r)| {
                (0..2).any(|k| match r {
                    None => x[2 * i + k].to_bits() != 0,
                    Some(j) if j != i => {
                        !x[2 * i + k].is_finite()
                            || x[2 * i + k].to_bits() != x[2 * j + k].to_bits()
                    }
                    _ => false,
                })
            })
        {
            return false;
        }
        for (v, len) in [
            (&mut out.f, 2 * n),
            (&mut out.q, 2 * n),
            (&mut out.df, 2 * n * n),
            (&mut out.dq, 2 * n * n),
        ] {
            v.clear();
            v.resize(len, 0.);
        }
        let Some(batch) = self.kernel.batch.get().and_then(Option::as_ref) else {
            return false;
        };
        let input = EvalInput {
            x,
            temperature: 0.,
            mfactor: 1.,
            analysis: Analysis::Tran,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin,
            limit: false,
            simparams: &|_| None,
        };
        let mut stamp = EvalStamp {
            f: &mut out.f,
            q: &mut out.q,
            df: &mut out.df,
            dq: &mut out.dq,
            ..Default::default()
        };
        let mut state = StateIo {
            accepted: &[],
            current: &mut [],
        };
        let mut queries = abi::Queries {
            keys: &[],
            callback: &|_| None,
            log: &mut Vec::new(),
            panic: None,
        };
        let mut frame = abi::Frame::new(
            &input,
            self.tnom,
            &mut state,
            &mut stamp,
            batch.constants.as_flattened(),
            &mut [],
            &mut queries,
        );
        let ok = batch.code.run(&mut frame, &mut Vec::new());
        ok == 1 && finite_output(out)
    }
}
