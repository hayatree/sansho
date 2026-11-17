//! Simulator adapter for interpreted compiled Verilog-A modules.
//! Provenance: original implementation from the repository DAE interface and Accellera LRM 2.4.

use crate::{
    DeviceGroup, EvalCtx, Mode, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns, volt,
};
use spx_diag::Diagnostic;
use spx_va::{Analysis, Bound, CompiledModule, EvalInput, EvalOutput, StateIo};
use std::sync::Arc;

#[derive(Debug)]
struct Instance {
    name: String,
    nodes: Vec<Node>,
    model: Arc<Bound>,
    bound: Bound,
    params: Vec<(String, f64)>,
}

/// All instances of one compiled module, in fixed netlist order.
#[derive(Debug, Default)]
pub struct VaGroup {
    module: Option<Arc<CompiledModule>>,
    instances: Vec<Instance>,
}

/// Resolve SPICE's case-insensitive parameter names to the declared VA spelling.
pub fn parameter_names(module: &CompiledModule, params: &[(String, f64)]) -> Vec<(String, f64)> {
    params
        .iter()
        .map(|(name, value)| {
            let declared = module
                .params
                .iter()
                .flat_map(|p| std::iter::once(&p.name).chain(p.aliases.iter()))
                .find(|p| p.eq_ignore_ascii_case(name))
                .unwrap_or(name);
            (declared.clone(), *value)
        })
        .collect()
}

impl VaGroup {
    /// Bind an instance; the model binding supplies defaults overridden by instance parameters.
    pub fn push(
        &mut self,
        module: Arc<CompiledModule>,
        name: &str,
        nodes: Vec<Node>,
        model: Arc<Bound>,
        params: &[(String, f64)],
    ) -> Result<(), Vec<Diagnostic>> {
        if nodes.len() != module.ports.len() {
            return Err(vec![Diagnostic::error(
                "E-ELAB-PARAM",
                format!(
                    "{name}: module {} needs {} ports, found {}",
                    module.name,
                    module.ports.len(),
                    nodes.len()
                ),
            )]);
        }
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
        let bound = module.bind_instance(&model, &params)?;
        self.module = Some(module);
        self.instances.push(Instance {
            name: name.into(),
            nodes,
            model,
            bound,
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
            for name in &m.internal_nodes {
                i.nodes.push(Some(u.alloc_internal_node(&i.name, name)));
            }
            for name in &m.branch_unknowns {
                i.nodes
                    .push(Some(u.alloc_branch(&format!("{}#{name}", i.name))));
            }
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
        let mut values = Vec::with_capacity(m.unknown_len());
        let mut result = EvalOutput::default();
        for (k, i) in self.instances.iter().enumerate() {
            values.clear();
            values.extend(i.nodes.iter().map(|&n| volt(x, n)));
            let start = k * m.state_len();
            let end = start + m.state_len();
            let input = EvalInput {
                x: &values,
                temperature: ctx.temp,
                analysis: match ctx.mode {
                    Mode::Dc => Analysis::Dc,
                    Mode::Tran => Analysis::Tran,
                },
                initial_step: ctx.init_junctions || (ctx.mode == Mode::Tran && ctx.time == 0.),
                abstime: ctx.time,
                gmin: ctx.gmin,
                limit: ctx.limit,
                simparams: &|_| None,
                mfactor: 1.,
            };
            // Each binding owns its reusable interpreter variables. No implicit gmin stamp:
            // models opt in through $simparam("gmin").
            m.eval(
                &i.bound,
                &input,
                StateIo {
                    accepted: &state.accepted[start..end],
                    current: &mut state.current[start..end],
                },
                &mut result,
            );
            if let Some(error) = &result.error {
                out.error = Some(Diagnostic::error(
                    "E-VA-EVAL",
                    format!("{}: {error}", i.name),
                ));
                return;
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
        match m.bind_instance(&i.model, &params) {
            Ok(bound) => {
                i.bound = bound;
                i.params = params;
                true
            }
            Err(_) => false,
        }
    }
    fn param(&self, instance: &str, param: &str) -> Option<f64> {
        let m = self.module.as_ref()?;
        let i = self.instances.iter().find(|i| i.name == instance)?;
        let names = parameter_names(m, &[(param.into(), 0.)]);
        m.param(&i.bound, &names[0].0)
    }
}
