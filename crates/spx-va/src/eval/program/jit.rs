//! Shared native specializations and per-instance retained numeric history.
//! Provenance: original implementation from the repository register evaluator and SSA literature.

mod abi;
mod arithmetic;
mod guards;
mod lower;
mod split;

use super::*;
use std::io::Write;

#[derive(PartialEq, Eq, Hash, Debug, serde::Serialize)]
struct Key {
    parameters: Vec<(u64, Option<Arc<str>>)>,
    given: Vec<bool>,
    layout: Vec<Option<usize>>,
    connected: usize,
    temperature: u64,
    multiplier: u64,
    tnom: u64,
    gmin: u64,
}

type Cell = Arc<std::sync::OnceLock<Option<Arc<Kernel>>>>;

#[derive(Debug, Default)]
pub(crate) struct Cache(Mutex<HashMap<Key, Cell>>);

#[derive(Debug)]
struct Kernel {
    code: abi::Code,
    constants: Vec<f64>,
    queries: Vec<String>,
    stores: Vec<(usize, Ty)>,
    regular: bool,
}

impl Bound {
    /// Identify instances that share a native specialization within this module.
    pub fn specialization_key(
        &self,
        temperature: f64,
        mfactor: f64,
    ) -> impl std::hash::Hash + Eq + use<> {
        self.native_key(temperature, mfactor)
    }
    fn native_key(&self, temperature: f64, mfactor: f64) -> Key {
        Key {
            tnom: self.sim.tnom.to_bits(),
            gmin: self.sim.gmin.to_bits(),
            parameters: self
                .values
                .iter()
                .map(|v| (v.f.v.to_bits(), v.text.clone()))
                .collect(),
            given: self.given.clone(),
            layout: self.layout.map.clone(),
            connected: self.connected_ports,
            temperature: temperature.to_bits(),
            multiplier: mfactor.to_bits(),
        }
    }
}

impl CompiledModule {
    /// Prepare the native specialization during elaboration. Evaluation also prepares it lazily.
    pub fn prepare_jit(&self, b: &Bound, temperature: f64, mfactor: f64) -> Option<Diagnostic> {
        if !Arc::ptr_eq(&self.identity, &b.identity) || !crate::jit_enabled() {
            return None;
        }
        let input = EvalInput {
            x: &[],
            temperature,
            mfactor,
            analysis: Analysis::Dc,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin: b.sim.gmin,
            limit: false,
            simparams: &|_| None,
        };
        let mut diagnostic = None;
        self.kernel(b, &input, |d| diagnostic = Some(d));
        diagnostic
    }

    fn kernel(
        &self,
        b: &Bound,
        input: &EvalInput<'_>,
        report: impl FnOnce(Diagnostic),
    ) -> Option<Arc<Kernel>> {
        let key = b.native_key(input.temperature, input.mfactor);
        let mut cache = self.kernels.0.lock().unwrap_or_else(|e| e.into_inner());
        let cell = cache.entry(key);
        if let std::collections::hash_map::Entry::Occupied(ref occupied) = cell
            && let Some(result) = occupied.get().get()
        {
            return result.clone();
        }
        let disk = self.artifact.as_ref().and_then(|artifact| {
            serde_json::to_vec(cell.key())
                .ok()
                .map(|bytes| artifact.child(&[&bytes]))
        });
        let cell = cell.or_default().clone();
        drop(cache);
        cell.get_or_init(|| {
            let result =
                self.specialize_native(b, input, disk.as_ref())
                    .and_then(|(spec, native)| {
                        let _phase = spx_diag::Phase::start("jit");
                        Kernel::compile(
                            spec.program,
                            b.layout.unknown_len(),
                            self.slots,
                            self.states,
                            spec.regular,
                            native.as_ref(),
                        )
                    });
            match result {
                Ok(kernel) => Some(Arc::new(kernel)),
                Err(reason) => {
                    report(Diagnostic::information(
                        "I-VA-JIT",
                        format!("{} uses the interpreter: {reason}", self.name),
                    ));
                    None
                }
            }
        })
        .clone()
    }

    fn specialize_native(
        &self,
        b: &Bound,
        input: &EvalInput<'_>,
        disk: Option<&crate::cache::Artifact>,
    ) -> Result<(Specialization, Option<crate::cache::Artifact>), String> {
        let _phase = spx_diag::Phase::start("specialize");
        if let Some(entry) = disk
            && let Some(bytes) = entry.read()
            && let Ok(spec) = serde_json::from_slice(&bytes)
        {
            return Ok((spec, Some(entry.child(&[b"native", &bytes]))));
        }
        let p = Program::compile(self, b, input, self.states == 0)
            .ok_or("binding requires the general evaluator")?;
        let regular = self.states == 0
            && !p
                .ops
                .iter()
                .any(|op| matches!(op, Op::Ddx(..) | Op::Remember(..)));
        let program = if regular {
            Program::compile_channels(self, b, input, true, true)
                .ok_or("charge specialization failed")?
        } else {
            p
        };
        let spec = Specialization { regular, program };
        let native = disk.and_then(|entry| {
            let bytes = serde_json::to_vec(&spec).ok()?;
            entry.write(&bytes);
            Some(entry.child(&[b"native", &bytes]))
        });
        Ok((spec, native))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Specialization {
    regular: bool,
    program: Program,
}

impl Kernel {
    fn compile(
        p: Program,
        n: usize,
        slots: usize,
        states: usize,
        regular: bool,
        disk: Option<&crate::cache::Artifact>,
    ) -> Result<Self, String> {
        // Every native displacement is a signed 32-bit byte offset.
        for elements in [
            n.saturating_mul(n),
            slots.saturating_mul(2),
            states,
            p.values.len(),
        ] {
            if elements > i32::MAX as usize / 8 {
                return Err("native buffer offset exceeds i32".into());
            }
        }
        let queries = p
            .ops
            .iter()
            .filter_map(|op| {
                if let Op::Simparam(_, k, _) = op {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        let stores = p
            .ops
            .iter()
            .filter_map(|op| {
                if let Op::Remember(i, _, ty) = *op {
                    Some((i, ty))
                } else {
                    None
                }
            })
            .collect();

        // Some backend resource/ISA failures are reported by Cranelift as unwinds.
        let code = std::panic::catch_unwind(|| lower::compile(&p, n, regular, disk))
            .map_err(|_| "native backend failed during compilation".to_string())??;
        Ok(Self {
            code,
            regular,
            constants: p.values,
            queries,
            stores,
        })
    }
}

pub(super) enum Dispatch {
    Finished,
    Interpreter,
    Replay,
}

#[derive(Default, Debug)]
pub(super) struct Instance {
    key: Option<(u64, u64)>,
    kernel: Option<std::sync::Weak<Kernel>>,
    history: Vec<f64>,
    bridge: Vec<f64>,
    pub(super) replay: Vec<Option<f64>>,
}

impl Machine {
    pub(in crate::eval) fn used_jit(&self) -> bool {
        self.native.kernel.is_some()
    }
    pub(in crate::eval) fn leave_jit(&mut self, slots: &mut [Slot]) {
        self.native.flush(slots);
        self.native.key = None;
    }
}

impl Instance {
    fn flush(&mut self, slots: &mut [Slot]) {
        if let Some(kernel) = self.kernel.take().and_then(|k| k.upgrade()) {
            for &(i, ty) in &kernel.stores {
                let slot = &mut slots[i];
                if slot.values.is_empty() {
                    slot.ty = ty;
                    slot.values.push(Value::num(0.));
                }
                slot.values[0].f.v = self.history[2 * i];
                slot.values[0].q.v = self.history[2 * i + 1];
            }
        }
    }
    pub(super) fn evaluate(
        &mut self,
        m: &CompiledModule,
        b: &Bound,
        input: &EvalInput<'_>,
        slots: &mut [Slot],
        out: &mut EvalOutput,
        state: &mut StateIo<'_>,
    ) -> Dispatch {
        let key = (input.temperature.to_bits(), input.mfactor.to_bits());
        if self.key != Some(key) {
            self.flush(slots);

            self.key = Some(key);
            self.kernel = if crate::jit_enabled() {
                m.kernel(b, input, |d| {
                    let _ = writeln!(std::io::stderr().lock(), "{d}");
                })
                .as_ref()
                .map(Arc::downgrade)
            } else {
                None
            };
            if self.kernel.is_some() {
                self.history.clear();
                self.history.extend(
                    slots
                        .iter()
                        .flat_map(|s| s.values.first().map_or([0., 0.], |v| [v.f.v, v.q.v])),
                );
            }
        }
        let Some(kernel) = self.kernel.as_ref().and_then(std::sync::Weak::upgrade) else {
            return Dispatch::Interpreter;
        };
        self.replay.clear();
        let mut queries = abi::Queries {
            keys: &kernel.queries,
            callback: input.simparams,
            log: &mut self.replay,
            panic: None,
        };
        let ok = kernel.code.run(
            &mut abi::Frame::new(
                input,
                b,
                state,
                out,
                &kernel.constants,
                &mut self.history,
                &mut queries,
            ),
            &mut self.bridge,
        );
        if let Some(payload) = queries.panic {
            std::panic::resume_unwind(payload)
        }
        out.noncon = ok & 2 != 0;
        let regular = kernel.regular;
        if ok & 1 != 0
            && (!regular
                || out
                    .df
                    .iter()
                    .chain(&out.dq)
                    .chain(&out.q)
                    .chain(&out.f)
                    .all(|g| g.is_finite()))
        {
            return Dispatch::Finished;
        }
        // Only stateless programs have charge guards. Their retained stores are
        // after the success branch, so a retry starts with the old history.
        self.flush(slots);
        out.f.fill(0.);
        out.q.fill(0.);
        out.df.fill(0.);
        out.dq.fill(0.);
        Dispatch::Replay
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn equal_bindings_share_pages_owned_by_the_module() {
        let ast = crate::parse_va(
            "module r(p); electrical p; parameter real a=2; analog I(p)<+a*V(p); endmodule",
            &crate::NoVaFiles,
        )
        .unwrap();
        let m = crate::compile_module(&ast, None).unwrap();
        let a = m.bind(&[]).unwrap();
        let b = m.bind(&[]).unwrap();
        let input = EvalInput {
            x: &[1.],
            temperature: 300.,
            mfactor: 1.,
            analysis: Analysis::Dc,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin: 0.,
            limit: false,
            simparams: &|_| None,
        };
        let first = m.kernel(&a, &input, |_| {}).unwrap();
        let second = m.kernel(&b, &input, |_| {}).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let mut scratch = Scratch::default();
        for _ in 0..2 {
            m.eval_with_scratch(
                &a,
                &input,
                StateIo {
                    accepted: &[],
                    current: &mut [],
                },
                &mut scratch,
                &mut EvalOutput::default(),
            );
        }
        let weak = Arc::downgrade(&first);
        drop((first, second, m));
        assert!(weak.upgrade().is_none());
    }
}
