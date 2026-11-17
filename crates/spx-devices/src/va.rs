//! Simulator adapter for interpreted compiled Verilog-A modules.
//! Provenance: original implementation from the repository DAE interface and Accellera LRM 2.4.

use crate::{
    DeviceGroup, EvalCtx, Mode, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns, volt,
};
use spx_diag::Diagnostic;
use spx_va::{Analysis, Bound, CompiledModule, EvalInput, EvalOutput, Scratch, StateIo};
use std::sync::Arc;

#[derive(Debug)]
struct Instance {
    name: String,
    nodes: Vec<Node>,
    ports: Vec<Node>,
    model: Arc<Bound>,
    bound: Bound,
    copies: f64,
    port_count: usize,
    swept: bool,
    params: Vec<(String, f64)>,
}

/// All instances of one compiled module, in fixed netlist order.
#[derive(Debug, Default)]
pub struct VaGroup {
    buffers: std::sync::Mutex<Buffers>,
    module: Option<Arc<CompiledModule>>,
    instances: Vec<Instance>,
}

#[derive(Debug, Default)]
struct Buffers {
    scratch: Vec<Scratch>,
    values: Vec<f64>,
    result: EvalOutput,
}

/// Resolve SPICE's case-insensitive parameter names to the declared VA spelling.
pub fn parameter_names(module: &CompiledModule, params: &[(String, f64)]) -> Vec<(String, f64)> {
    params
        .iter()
        .map(|(name, value)| {
            let declared = module.parameter_info(name).map_or(name, |p| &p.name);
            (declared.clone(), *value)
        })
        .collect()
}

impl VaGroup {
    /// Reserve every possible branch equation for an in-place DC parameter sweep.
    pub fn prepare_sweep(&mut self, name: &str) {
        if let Some(i) = self.instances.iter_mut().find(|i| i.name == name) {
            i.swept = true;
            i.bound.retain_sweep_layout();
        }
    }
    /// Report connections between external nets implied by binding-time branch collapse.
    pub fn terminal_shorts(&self) -> impl Iterator<Item = (Node, Node)> + '_ {
        self.instances.iter().flat_map(|i| {
            i.ports.iter().enumerate().filter_map(|(k, &node)| {
                let layout = i.bound.layout();
                let representative = layout.index(k).map(|j| layout.representatives()[j]);
                (representative != Some(k)).then(|| (node, representative.and_then(|j| i.ports[j])))
            })
        })
    }
    /// Apply the circuit-wide net equivalences before internal unknown allocation.
    pub fn remap_ports(&mut self, indices: &[Node]) -> Result<(), Diagnostic> {
        for i in &mut self.instances {
            for n in &mut i.ports {
                if let Some(k) = *n {
                    *n = *indices.get(k).ok_or_else(|| {
                        Diagnostic::error("E-ELAB-INTERNAL", "invalid VA terminal index")
                    })?;
                }
            }
        }
        Ok(())
    }
    /// Bind an instance; the model binding supplies defaults overridden by instance parameters.
    pub fn push(
        &mut self,
        module: Arc<CompiledModule>,
        name: &str,
        nodes: Vec<Node>,
        model: Arc<Bound>,
        params: &[(String, f64)],
    ) -> Result<(), Vec<Diagnostic>> {
        if self
            .module
            .as_ref()
            .is_some_and(|m| !Arc::ptr_eq(m, &module))
        {
            return Err(vec![Diagnostic::error(
                "E-ELAB-PARAM",
                "VA group contains different compiled modules",
            )]);
        }
        let params = parameter_names(&module, params);
        let port_count = nodes.len();
        let bound = module.bind_connected_instance(&model, &params, port_count)?;
        let copies = module.parallel_copies(&bound);
        self.module = Some(module);
        self.instances.push(Instance {
            name: name.into(),
            ports: nodes,
            nodes: vec![],
            model,
            bound,
            port_count,
            copies,
            swept: false,
            params,
        });
        Ok(())
    }
}

impl DeviceGroup for VaGroup {
    fn type_name(&self) -> &'static str {
        "verilog-a"
    }
    fn len(&self) -> usize {
        self.instances.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        self.module
            .as_ref()
            .map_or(0, |m| m.state_len() * self.len())
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        let Some(m) = &self.module else { return };
        for i in &mut self.instances {
            let ports = &i.ports;
            i.nodes = i
                .bound
                .layout()
                .representatives()
                .iter()
                .map(|&k| {
                    if k < ports.len() {
                        ports[k]
                    } else if let Some(name) = m.ports.get(k) {
                        Some(u.alloc_internal_node(&i.name, name))
                    } else if let Some(name) = m.internal_nodes.get(k - m.ports.len()) {
                        Some(u.alloc_internal_node(&i.name, name))
                    } else {
                        Some(u.alloc_branch(&format!(
                            "{}#{}",
                            i.name,
                            m.branch_unknowns[k - m.ports.len() - m.internal_nodes.len()]
                        )))
                    }
                })
                .collect();
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for i in &self.instances {
            b.core(&i.nodes);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        let Some(m) = &self.module else { return };
        let mut at = SlotOffsets::default();
        let Ok(mut buffers) = self.buffers.lock() else {
            out.error = Some(Diagnostic::error(
                "E-VA-EVAL",
                "VA group workspace is poisoned",
            ));
            return;
        };
        let Buffers {
            values,
            result,
            scratch,
        } = &mut *buffers;
        scratch.resize_with(self.len(), Scratch::default);
        for (k, i) in self.instances.iter().enumerate() {
            values.clear();
            values.extend(i.nodes.iter().map(|&n| volt(x, n)));
            let start = k * m.state_len();
            let end = start + m.state_len();
            let input = EvalInput {
                x: values,
                temperature: ctx.temp,
                analysis: match ctx.mode {
                    Mode::Dc => Analysis::Dc,
                    Mode::Ac => Analysis::Ac,
                    Mode::Tran => Analysis::Tran,
                },
                final_step: ctx.final_step,
                initial_step: ctx.init_junctions || (ctx.mode == Mode::Tran && ctx.time == 0.),
                abstime: ctx.time,
                gmin: ctx.gmin,
                limit: ctx.limit,
                simparams: &|_| None,
                mfactor: 1.,
            };
            // Per-instance variables and specialization live in the chunk workspace. No implicit gmin stamp:
            // models opt in through $simparam("gmin").
            m.eval_with_scratch(
                &i.bound,
                &input,
                StateIo {
                    accepted: &state.accepted[start..end],
                    current: &mut state.current[start..end],
                },
                &mut scratch[k],
                result,
            );
            if let Some(d) = result.diagnostic.take() {
                out.error = Some(d.with_note(format!("VA instance {}", i.name)));
                return;
            }
            if let Some(error) = &result.error {
                out.error = Some(Diagnostic::error(
                    "E-VA-EVAL",
                    format!("{}: {error}", i.name),
                ));
                return;
            }
            // Branch currents and internal equations describe one copy. External KCL
            // sums its terminal flows, including displacement current, over all copies.
            let copies = i.copies;
            if copies != 1. {
                let n = i.nodes.len();
                for (row, &rep) in i.bound.layout().representatives().iter().enumerate() {
                    if rep < i.port_count {
                        result.f[row] *= copies;
                        result.q[row] *= copies;
                        for x in result.df[row * n..(row + 1) * n]
                            .iter_mut()
                            .chain(&mut result.dq[row * n..(row + 1) * n])
                        {
                            *x *= copies;
                        }
                    }
                }
            }
            out.noncon |= result.noncon;
            out.core_dyn(&mut at, &result.f, &result.q, &result.df, &result.dq);
        }
    }
    fn set_param(&mut self, instance: &str, param: &str, value: f64) -> bool {
        let Some(m) = &self.module else { return false };
        let Some(i) = self.instances.iter_mut().find(|i| i.name == instance) else {
            return false;
        };
        let mut params = i.params.clone();
        for (name, value) in parameter_names(m, &[(param.into(), value)]) {
            if let Some(p) = params.iter_mut().find(|p| p.0 == name) {
                p.1 = value;
            } else {
                params.push((name, value));
            }
        }
        match m.bind_connected_instance(&i.model, &params, i.port_count) {
            Ok(mut bound) if i.swept || bound.layout() == i.bound.layout() => {
                if i.swept {
                    bound.retain_sweep_layout();
                }
                i.copies = m.parallel_copies(&bound);
                i.bound = bound;
                i.params = params;
                true
            }
            _ => false,
        }
    }
    fn param(&self, instance: &str, param: &str) -> Option<f64> {
        let m = self.module.as_ref()?;
        let i = self.instances.iter().find(|i| i.name == instance)?;
        let names = parameter_names(m, &[(param.into(), 0.)]);
        m.param(&i.bound, &names[0].0)
    }
}
