//! Analog tree interpreter and public evaluation API.
//! Provenance: original implementation from Accellera LRM 2.4 and forward-mode AD literature.
mod generated;
pub(crate) mod program;
mod topology;
use crate::{
    dual::{Dual, Gradients},
    ir::*,
    limit,
};
#[cfg(feature = "jit")]
pub use program::CircuitPair;
use spx_diag::Diagnostic;
use std::sync::{Arc, Mutex};
pub use topology::Layout;
const ITERATION_CAP: usize = 100_000;
const K_OVER_Q: f64 = 8.617333262145179e-5;
/// Analysis mode visible to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Analysis {
    /// DC operating point or sweep.
    Dc,
    /// Time-domain evaluation.
    Tran,
    /// Small-signal AC linearization.
    Ac,
    /// Noise linearization (noise generators currently return zero).
    Noise,
}
/// Values and simulator context for one evaluation.
pub struct EvalInput<'a> {
    /// Ports, internal voltages, then branch currents.
    pub x: &'a [f64],
    /// Absolute temperature in kelvin.
    pub temperature: f64,
    /// Active analysis.
    pub analysis: Analysis,
    /// Execute initialization event blocks.
    pub initial_step: bool,
    /// Execute final-step event blocks at the last analysis point.
    pub final_step: bool,
    /// Simulation time in seconds.
    pub abstime: f64,
    /// Simulator minimum conductance, also available as `$simparam("gmin")`.
    pub gmin: f64,
    /// Enable voltage limiting.
    pub limit: bool,
    /// Query simulator parameters by name.
    pub simparams: &'a dyn Fn(&str) -> Option<f64>,
    /// Instance multiplicity visible through `$mfactor`.
    pub mfactor: f64,
}
impl std::fmt::Debug for EvalInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvalInput")
            .field("x", &self.x)
            .field("analysis", &self.analysis)
            .finish_non_exhaustive()
    }
}
/// Limiter history supplied by the simulator. Initialize `current` from `accepted` on rollback.
#[derive(Debug)]
pub struct StateIo<'a> {
    /// Last accepted state (fallback history if current is not finite).
    pub accepted: &'a [f64],
    /// Newton iteration history, read and updated in evaluation order.
    pub current: &'a mut [f64],
}
/// Device residuals and dense, row-major Jacobians.
#[derive(Debug, Default, Clone)]
pub struct StampBuffers<T> {
    /// Resistive residual, one entry per unknown.
    pub f: T,
    /// Charge residual, one entry per unknown.
    pub q: T,
    /// Derivative of `f` with respect to all unknowns.
    pub df: T,
    /// Derivative of `q` with respect to all unknowns.
    pub dq: T,
    /// A limiter changed a voltage.
    pub noncon: bool,
    /// Runtime failure, including model error tasks or iteration exhaustion.
    pub error: Option<String>,
    /// Source diagnostic for an unsupported operation reached during evaluation.
    pub diagnostic: Option<Diagnostic>,
}
/// Owned device residuals and Jacobians for standalone evaluation.
pub type EvalOutput = StampBuffers<Vec<f64>>;
/// Borrowed stamp slots supplied by a circuit assembler.
pub type EvalStamp<'a> = StampBuffers<&'a mut [f64]>;

#[derive(Debug, Clone)]
struct Value {
    f: Dual,
    q: Dual,
    text: Option<Arc<str>>,
}
impl Value {
    fn num(v: f64) -> Self {
        Self {
            f: Dual::constant(v),
            q: Dual::constant(0.),
            text: None,
        }
    }
    fn dual(f: Dual) -> Self {
        Self {
            f,
            q: Dual::constant(0.),
            text: None,
        }
    }
    fn string(s: Arc<str>) -> Self {
        Self {
            text: Some(s),
            ..Self::num(0.)
        }
    }
    fn cast(mut self, t: Ty, g: &mut Gradients) -> Self {
        if t == Ty::Integer {
            self.f = self.f.map(self.f.v.trunc(), 0., g);
            self.q = self.q.map(self.q.v.trunc(), 0., g);
        }
        self
    }
    fn truth(&self) -> bool {
        self.f.v != 0.
    }
    fn equal(&self, b: &Self) -> bool {
        match (&self.text, &b.text) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.f.v == b.f.v,
            _ => false,
        }
    }
}
#[derive(Debug, Clone)]
struct Slot {
    values: Vec<Value>,
    lower: i64,
    ty: Ty,
}
/// Reusable, per-instance interpreter workspace. Do not share between instances.
#[derive(Debug, Default)]
pub struct Scratch {
    slots: Vec<Slot>,
    initialized: bool,
    steps: usize,
    binding_key: Option<Arc<()>>,
    width: usize,
    machine: program::Machine,
    #[cfg(feature = "jit")]
    disable_jit: bool,
    gradients: Gradients,
    drives: Vec<Drive>,
    operator_rows: Vec<Option<Value>>,
    frames: Vec<CallFrame>,
    calls: Vec<usize>,
}
#[cfg(feature = "jit")]
impl Scratch {
    /// Select native specialization for subsequent evaluations in this workspace.
    pub fn set_jit(&mut self, enabled: bool) {
        if !enabled && !self.disable_jit {
            self.machine.leave_jit(&mut self.slots);
        }
        self.disable_jit = !enabled;
    }

    /// Whether the last native result can be reused for bit-identical unknowns,
    /// binding, temperature, multiplier, gmin and side-effect-free simulator queries.
    pub fn bias_only(&self) -> bool {
        self.machine.bias_only()
    }

    /// Whether the last specialized evaluation used a native kernel.
    pub fn used_jit(&self) -> bool {
        self.machine.used_jit()
    }
}
/// Validated parameter values and explicit-supply flags.
/// The convenience evaluator owns one instance workspace here; use `eval_with_scratch`
/// for separate instances sharing these parameters.
#[derive(Debug)]
pub struct Bound {
    #[cfg(feature = "jit")]
    run_kernels: Option<Arc<program::Cache>>,
    #[cfg(feature = "jit")]
    circuit: Option<CircuitStamp>,
    sim: BindOptions,
    layout: Layout,
    generated: Arc<generated::Derivatives>,
    connected_ports: usize,
    values: Vec<Value>,
    given: Vec<bool>,
    scratch: Mutex<Scratch>,
    identity: Arc<()>,
    workspace_key: Arc<()>,
}
/// The simulator discards grounded rows and columns during assembly. Equal
/// entries name the same unknown, but retain separate stamp columns and order.
#[cfg(feature = "jit")]
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize)]
struct CircuitStamp {
    fixed_columns: Vec<usize>,
    bias: Option<(Vec<Option<u64>>, [u64; 2])>,
    wires: Vec<Option<usize>>,
    dc: bool,
}

impl Bound {
    /// Keep specializations of a statistical model and its instances for this run.
    /// The compiled module and its nominal kernel cache remain shareable.
    pub fn use_run_cache(&mut self) {
        #[cfg(feature = "jit")]
        {
            self.run_kernels = Some(Default::default());
        }
    }

    /// Start an independent instance with the same validated parameters and port count.
    /// Circuit connections and evaluation history are instance-specific.
    pub fn new_instance(&self) -> Self {
        Self {
            #[cfg(feature = "jit")]
            run_kernels: self.run_kernels.clone(),
            #[cfg(feature = "jit")]
            circuit: None,
            sim: self.sim,
            layout: self.layout.clone(),
            generated: self.generated.clone(),
            connected_ports: self.connected_ports,
            values: self.values.clone(),
            given: self.given.clone(),
            scratch: Mutex::new(Scratch::default()),
            identity: Arc::clone(&self.identity),
            workspace_key: Arc::new(()),
        }
    }

    /// Specialize native stamps for a circuit adapter. Grounded rows and columns
    /// are unused, and DC-only clients also discard the reactive stamp.
    #[cfg(feature = "jit")]
    pub fn specialize_stamp(&mut self, ports: &[Option<usize>], dc_only: bool) {
        let wires = self
            .layout
            .representatives()
            .iter()
            .enumerate()
            .map(|(row, &rep)| {
                if let Some(net) = ports.get(rep) {
                    net.and_then(|n| {
                        self.layout
                            .representatives()
                            .iter()
                            .position(|&r| ports.get(r) == Some(&Some(n)))
                    })
                } else {
                    Some(row)
                }
            })
            .collect();
        self.circuit = Some(CircuitStamp {
            fixed_columns: Vec::new(),
            wires,
            dc: dc_only,
            bias: None,
        });
        self.workspace_key = Arc::new(());
    }

    /// Supply nominal port voltages and supply range for predicting branches.
    /// The packed kernel validates predictions against each actual bias.
    #[cfg(feature = "jit")]
    pub fn predict_bias(&mut self, ports: &[Option<f64>], range: [f64; 2]) {
        if let Some(stamp) = &mut self.circuit {
            let known = self
                .layout
                .representatives()
                .iter()
                .map(|&i| ports.get(i).copied().flatten().map(f64::to_bits))
                .collect();
            stamp.bias = Some((known, range.map(f64::to_bits)));
            self.workspace_key = Arc::new(());
        }
    }

    /// Keep the module-wide unknown set when parameters will change within one matrix factorization pattern.
    pub fn retain_sweep_layout(&mut self) {
        self.layout.retain_all();
        #[cfg(feature = "jit")]
        {
            self.circuit = None;
        }
        self.workspace_key = Arc::new(());
    }

    /// Unknown layout specialized for these model and instance parameters.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }
}
/// Simulator settings available while evaluating parameter defaults.
#[derive(Debug, Clone, Copy)]
pub struct BindOptions {
    /// Nominal temperature in degrees Celsius.
    pub tnom: f64,
    /// Junction conductance in siemens.
    pub gmin: f64,
}
impl Default for BindOptions {
    fn default() -> Self {
        Self {
            tnom: 27.,
            gmin: 1e-12,
        }
    }
}
fn distance(a: &str, b: &str) -> usize {
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.bytes().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, y) in b.bytes().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (diag + usize::from(!x.eq_ignore_ascii_case(&y)))
                .min(old + 1)
                .min(row[j] + 1);
            diag = old;
        }
    }
    row[b.len()]
}
impl CompiledModule {
    /// Validate external numeric overrides and evaluate defaults in declaration order.
    /// Integer parameter overrides round to the nearest integer, ties away from zero.
    pub fn bind(&self, values: &[(String, f64)]) -> Result<Bound, Vec<Diagnostic>> {
        self.bind_overrides(
            None,
            values,
            BindOptions::default(),
            false,
            self.ports.len(),
        )
    }
    /// Bind a SPICE model card, checking the model/instance parameter boundary.
    pub fn bind_model(
        &self,
        values: &[(String, f64)],
        options: BindOptions,
    ) -> Result<Bound, Vec<Diagnostic>> {
        self.bind_overrides(None, values, options, true, self.ports.len())
    }
    /// SPICE model cards may supply defaults for tagged instance parameters.
    pub fn bind_spice_model(
        &self,
        values: &[(String, f64)],
        options: BindOptions,
    ) -> Result<Bound, Vec<Diagnostic>> {
        self.bind_overrides(None, values, options, false, self.ports.len())
    }

    /// Number of identical parallel copies for simulator terminal stamping.
    pub fn parallel_copies(&self, bound: &Bound) -> f64 {
        if self.implicit_m {
            self.param(bound, "m").unwrap_or(1.)
        } else {
            1.
        }
    }

    /// Apply instance overrides to a model binding. Explicit-supply flags are preserved.
    pub fn bind_instance(
        &self,
        model: &Bound,
        values: &[(String, f64)],
    ) -> Result<Bound, Vec<Diagnostic>> {
        self.bind_overrides(Some(model), values, model.sim, false, self.ports.len())
    }
    /// Bind an instance whose trailing optional terminals may be left unconnected.
    pub fn bind_connected_instance(
        &self,
        model: &Bound,
        values: &[(String, f64)],
        ports: usize,
    ) -> Result<Bound, Vec<Diagnostic>> {
        if ports > self.ports.len()
            || self.optional_ports[ports..]
                .iter()
                .any(|optional| !*optional)
        {
            return Err(vec![Diagnostic::error(
                "E-ELAB-PARAM",
                format!(
                    "module {} declares {} ports, found {ports}",
                    self.name,
                    self.ports.len()
                ),
            )]);
        }
        self.bind_overrides(Some(model), values, model.sim, false, ports)
    }
    /// Resolve a unique SPICE spelling or alias to the parameter declaration.
    pub fn parameter_info(&self, name: &str) -> Option<&ParamInfo> {
        self.param_index
            .get(&name.to_ascii_lowercase())
            .copied()
            .flatten()
            .map(|i| &self.params[i])
    }
    fn bind_overrides(
        &self,
        model: Option<&Bound>,
        values: &[(String, f64)],
        sim: BindOptions,
        model_card: bool,
        connected_ports: usize,
    ) -> Result<Bound, Vec<Diagnostic>> {
        let mut diagnostics = vec![];
        let mut overrides = vec![None; self.params.len()];
        if let Some(m) = model
            && !Arc::ptr_eq(&m.identity, &self.identity)
        {
            return Err(vec![Diagnostic::error(
                "E-VA-PARAM",
                "binding belongs to another module",
            )]);
        }
        for (name, value) in values {
            if let Some(entry) = self.param_index.get(&name.to_ascii_lowercase()) {
                let Some(&i) = entry.as_ref() else {
                    diagnostics.push(Diagnostic::error(
                        "E-VA-PARAM",
                        format!("ambiguous parameter {name}"),
                    ));
                    continue;
                };
                let p = &self.params[i];
                if model_card && p.placement == Some(crate::ParamKind::Instance) {
                    diagnostics.push(
                        Diagnostic::error("E-VA-PARAM", format!("{name} is an instance parameter"))
                            .with_span(p.default.span),
                    );
                } else if model.is_some() && p.placement == Some(crate::ParamKind::Model) {
                    diagnostics.push(
                        Diagnostic::error("E-VA-PARAM", format!("{name} is a model parameter"))
                            .with_span(p.default.span),
                    );
                } else if self.implicit_m && i == 0 && *value <= 0. {
                    diagnostics.push(
                        Diagnostic::error("E-VA-PARAM", "instance multiplier must be positive")
                            .with_span(p.default.span),
                    );
                } else if p.local || p.ty == "string" || !value.is_finite() {
                    diagnostics.push(
                        Diagnostic::error("E-VA-PARAM", format!("invalid override for {name}"))
                            .with_span(p.default.span),
                    );
                } else {
                    overrides[i] = Some(*value);
                }
            } else {
                let suggestion = self
                    .params
                    .iter()
                    .filter(|p| !p.local)
                    .flat_map(|p| std::iter::once(&p.name).chain(&p.aliases))
                    .min_by_key(|s| distance(name, s))
                    .map(|s| format!(", did you mean {s}?"))
                    .unwrap_or_default();
                diagnostics.push(Diagnostic::error(
                    "E-VA-PARAM",
                    format!("unknown parameter {name}{suggestion}"),
                ));
            }
        }
        let mut bound = Bound {
            #[cfg(feature = "jit")]
            run_kernels: model.and_then(|m| m.run_kernels.clone()),
            #[cfg(feature = "jit")]
            circuit: None,
            connected_ports,
            layout: Layout::default(),
            generated: Default::default(),
            sim,
            values: vec![],
            given: vec![],
            scratch: Mutex::new(Scratch::default()),
            identity: Arc::clone(&self.identity),
            workspace_key: Arc::new(()),
        };
        let no_params = |_: &str| None;
        let input = EvalInput {
            x: &[],
            temperature: 300.,
            analysis: Analysis::Dc,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin: sim.gmin,
            limit: false,
            simparams: &no_params,
            mfactor: 1.,
        };
        for (i, p) in self.params.iter().enumerate() {
            let inherited = model.filter(|m| m.given.get(i).copied().unwrap_or(false));
            let value = if let Some(v) = overrides[i] {
                Value::num(if p.ty == "integer" { v.round() } else { v })
            } else if let Some(m) = inherited {
                m.values[i].clone()
            } else {
                let mut scratch = Scratch::default();
                let mut current = [];
                let mut out = EvalStamp::default();
                let mut runtime = Runtime {
                    drives: vec![],
                    operator_rows: vec![],
                    module: self,
                    bound: &bound,
                    input: &input,
                    state: StateIo {
                        accepted: &[],
                        current: &mut current,
                    },
                    out: &mut out,
                    scratch: &mut scratch,
                    n: 0,
                    depth: 0,
                };
                let v = runtime.expr(&p.value);
                if let Some(e) = out.error {
                    diagnostics.push(Diagnostic::error("E-VA-PARAM", e).with_span(p.default.span));
                }
                if p.ty == "integer" {
                    Value::num(v.f.v.round())
                } else {
                    v
                }
            };
            bound.values.push(value);
            bound
                .given
                .push(overrides[i].is_some() || inherited.is_some());
        }
        // Ranges can depend on any preceding parameters, and are checked after all overrides.
        for (i, p) in self.params.iter().enumerate() {
            let mut scratch = Scratch::default();
            let mut current = [];
            let mut out = EvalStamp::default();
            let mut r = Runtime {
                drives: vec![],
                operator_rows: vec![],
                module: self,
                bound: &bound,
                input: &input,
                state: StateIo {
                    accepted: &[],
                    current: &mut current,
                },
                out: &mut out,
                scratch: &mut scratch,
                n: 0,
                depth: 0,
            };
            let value = bound.values[i].f.v;
            let mut has_from = false;
            let mut in_from = false;
            let mut excluded = false;
            for (exclude, a, b, lc, uc) in &p.limits {
                let lo = r.expr(a).f.v;
                let inside = if let Some(b) = b {
                    let hi = r.expr(b).f.v;
                    (if *lc { value >= lo } else { value > lo })
                        && (if *uc { value <= hi } else { value < hi })
                } else {
                    value == lo
                };
                if *exclude {
                    excluded |= inside;
                } else {
                    has_from = true;
                    in_from |= inside;
                }
            }
            if excluded || (has_from && !in_from) {
                diagnostics.push(
                    Diagnostic::error(
                        "E-VA-PARAM",
                        format!("parameter {} violates its declared range", p.name),
                    )
                    .with_span(p.default.span),
                );
            }
            if let Some(e) = out.error {
                diagnostics.push(Diagnostic::error("E-VA-PARAM", e).with_span(p.default.span));
            }
            if bound.values[i].text.is_none() && !value.is_finite() {
                diagnostics.push(
                    Diagnostic::error("E-VA-PARAM", format!("parameter {} is not finite", p.name))
                        .with_span(p.default.span),
                );
            }
        }
        if diagnostics.is_empty() {
            bound.layout = topology::layout(self, &bound, &input);
            if !self.derivative_loops.is_empty() {
                let d = generated::Derivatives::bind(self, &bound, &input).map_err(|d| vec![d])?;
                bound.layout.append(d.extra);
                bound.generated = Arc::new(d);
            }
            Ok(bound)
        } else {
            Err(diagnostics)
        }
    }
    /// Numeric value of a bound parameter, including aliases.
    pub fn param(&self, bound: &Bound, name: &str) -> Option<f64> {
        if !Arc::ptr_eq(&bound.identity, &self.identity) {
            return None;
        }
        self.param_index
            .get(&name.to_ascii_lowercase())
            .copied()
            .flatten()
            .and_then(|i| bound.values.get(i))
            .filter(|v| v.text.is_none())
            .map(|v| v.f.v)
    }
    /// Evaluate with the binding's convenience per-instance workspace.
    pub fn eval(&self, b: &Bound, input: &EvalInput<'_>, state: StateIo<'_>, out: &mut EvalOutput) {
        match b.scratch.try_lock() {
            Ok(mut scratch) => self.eval_with_scratch(b, input, state, &mut scratch, out),
            Err(_) => {
                out.error = Some(
                    "binding workspace is busy or poisoned; use separate scratch workspaces".into(),
                );
            }
        }
    }
    /// Evaluate with an explicit per-instance workspace, reusable across Newton iterations.
    pub fn eval_with_scratch(
        &self,
        b: &Bound,
        input: &EvalInput<'_>,
        state: StateIo<'_>,
        scratch: &mut Scratch,
        out: &mut EvalOutput,
    ) {
        let n = b.layout.unknown_len();
        out.f.resize(n, 0.);
        out.q.resize(n, 0.);
        out.df.resize(n * n, 0.);
        out.dq.resize(n * n, 0.);
        let mut stamp = EvalStamp {
            f: &mut out.f[..],
            q: &mut out.q[..],
            df: &mut out.df[..],
            dq: &mut out.dq[..],
            ..Default::default()
        };
        self.eval_into(b, input, state, scratch, &mut stamp);
        out.error = stamp.error;
        out.diagnostic = stamp.diagnostic;
        out.noncon = stamp.noncon;
    }

    /// Evaluate directly into caller-owned dense stamp slots.
    pub fn eval_into(
        &self,
        b: &Bound,
        input: &EvalInput<'_>,
        mut state: StateIo<'_>,
        scratch: &mut Scratch,
        out: &mut EvalStamp<'_>,
    ) {
        let n = b.layout.unknown_len();
        out.f.fill(0.);
        out.q.fill(0.);
        out.df.fill(0.);
        out.dq.fill(0.);
        out.diagnostic = None;
        out.error = None;
        out.noncon = false;
        if out.f.len() != n
            || out.q.len() != n
            || out.df.len() != n * n
            || out.dq.len() != n * n
            || input.x.len() != n
            || state.current.len() < self.states
            || state.accepted.len() < self.states
            || !Arc::ptr_eq(&b.identity, &self.identity)
        {
            out.error = Some("incompatible input, state, or binding shape".into());
            return;
        }
        if scratch.slots.len() != self.slots
            || scratch
                .binding_key
                .as_ref()
                .is_none_or(|key| !Arc::ptr_eq(key, &b.workspace_key))
            || scratch.width != n
        {
            scratch.slots = vec![
                Slot {
                    values: vec![],
                    lower: 0,
                    ty: Ty::Real
                };
                self.slots
            ];
            scratch.initialized = false;
            scratch.binding_key = Some(Arc::clone(&b.workspace_key));
            scratch.width = n;
            scratch.machine = program::Machine::default();
        }
        // Keep initialized false until the general evaluator has initialized its
        // own slots. A later unsupported specialization can still enter it cold.
        #[cfg(feature = "jit")]
        let direct_start = self.direct_start && !scratch.disable_jit && b.circuit.is_some();
        #[cfg(not(feature = "jit"))]
        let direct_start = false;
        if (scratch.initialized || direct_start)
            && program::evaluate(self, b, input, scratch, out, &mut state)
        {
            return;
        }
        // Retained numeric variables are history, independent of this call's unknowns.
        for slot in &mut scratch.slots {
            for v in &mut slot.values {
                v.f = Dual::constant(v.f.v);
                v.q = Dual::constant(v.q.v);
            }
        }
        scratch.gradients.reset(n);
        scratch.steps = 0;
        let mut r = Runtime {
            drives: std::mem::take(&mut scratch.drives),
            operator_rows: std::mem::take(&mut scratch.operator_rows),
            module: self,
            bound: b,
            input,
            state,
            out,
            scratch,
            n,
            depth: 0,
        };
        r.operator_rows.clear();
        r.operator_rows
            .resize(self.operator_rows.len() + b.generated.extra, None);
        if !r.scratch.initialized {
            for (i, v) in &self.variables {
                r.init(*i, v);
            }
            if r.out.error.is_some() {
                return;
            }
        }
        r.drives.clear();
        r.drives.resize_with(self.branches.len(), Drive::default);
        for s in &self.body {
            r.stmt(s);
        }
        r.scratch.initialized = r.out.error.is_none();
        for (k, original) in self.operator_nodes(b).enumerate() {
            let state = r.voltage(Some(original), None);
            let equation = r.operator_rows[k].take().unwrap_or(state);
            r.stamp(b.layout.index(original), &equation, 1.);
        }
        for (k, branch) in self.branches.iter().enumerate() {
            if let Some(i) = branch.current.and_then(|id| b.layout.index(id)) {
                let current = Value::dual(Dual::variable(input.x[i], i, &mut r.scratch.gradients));
                r.stamp(branch.a.and_then(|id| b.layout.index(id)), &current, 1.);
                r.stamp(branch.b.and_then(|id| b.layout.index(id)), &current, -1.);
                let lhs = if r.drives[k].voltage || !branch.contributed {
                    r.voltage(branch.a, branch.b)
                } else {
                    current
                };
                r.stamp(Some(i), &lhs, 1.);
                if let Some(rhs) = r.drives[k].sum.take() {
                    r.stamp(Some(i), &rhs, -1.);
                }
            }
        }
        r.scratch.drives = r.drives;
        r.scratch.operator_rows = r.operator_rows;
    }
}
#[derive(Default, Debug)]
struct Drive {
    voltage: bool,
    sum: Option<Value>,
}
#[derive(Default, Debug)]
struct CallFrame {
    inputs: Vec<Value>,
    locations: Vec<Option<(usize, usize)>>,
    outputs: Vec<Option<Value>>,
}

struct Runtime<'a, 'b, 's> {
    drives: Vec<Drive>,
    operator_rows: Vec<Option<Value>>,
    module: &'a CompiledModule,
    bound: &'a Bound,
    input: &'a EvalInput<'a>,
    state: StateIo<'b>,
    out: &'b mut EvalStamp<'s>,
    scratch: &'b mut Scratch,
    n: usize,
    depth: usize,
}
impl Runtime<'_, '_, '_> {
    fn fail(&mut self, msg: impl Into<String>) {
        if self.out.error.is_none() {
            self.out.error = Some(msg.into());
        }
    }
    fn unsupported(&mut self, op: &str, span: spx_diag::Span) -> Value {
        let message = format!("{op} is unsupported in this analysis");
        self.fail(&message);
        self.out.diagnostic = Some(Diagnostic::error("E-VA-UNSUPPORTED", message).with_span(span));
        self.zero()
    }
    fn tick(&mut self) -> bool {
        self.scratch.steps += 1;
        if self.scratch.steps > ITERATION_CAP {
            self.fail("analog evaluation iteration budget exceeded");
            false
        } else {
            self.out.error.is_none()
        }
    }
    fn zero(&self) -> Value {
        Value::num(0.)
    }
    fn init(&mut self, i: usize, v: &Variable) {
        let mut lower = 0;
        let mut len = 1;
        if let Some((a, b)) = v.dimensions.first() {
            let a = self.expr(a).f.v;
            let b = self.expr(b).f.v;
            if !a.is_finite()
                || !b.is_finite()
                || a.fract() != 0.
                || b.fract() != 0.
                || (a - b).abs() > 100_000.
            {
                self.fail("invalid or excessive array bounds");
                return;
            }
            lower = a.min(b) as i64;
            len = (a - b).abs() as usize + 1;
        }
        let value = v
            .init
            .as_ref()
            .map(|e| self.expr(e))
            .unwrap_or_else(|| self.zero())
            .cast(v.ty, &mut self.scratch.gradients);
        if let Some(s) = self.scratch.slots.get_mut(i) {
            s.lower = lower;
            s.ty = v.ty;
            s.values.resize(len, value.clone());
            s.values.fill(value);
        }
    }
    fn index(&mut self, i: usize, e: Option<&Expr>) -> Option<usize> {
        let index = e.map(|e| self.expr(e).f.v);
        let Some(slot) = self.scratch.slots.get(i) else {
            self.fail("invalid variable slot");
            return None;
        };
        let k = if let Some(x) = index {
            if !x.is_finite() || x.fract() != 0. {
                self.fail("array index must be an integer");
                return None;
            }
            let k = x as i64;
            let Some(k) = k
                .checked_sub(slot.lower)
                .and_then(|k| usize::try_from(k).ok())
            else {
                self.fail("array index out of bounds");
                return None;
            };
            k
        } else {
            if slot.values.len() != 1 {
                self.fail("array requires an index");
                return None;
            }
            0
        };
        if k >= slot.values.len() {
            self.fail("array index out of bounds");
            None
        } else {
            Some(k)
        }
    }
    fn load(&mut self, i: usize, e: Option<&Expr>) -> Value {
        if let Some(k) = self.index(i, e) {
            self.scratch.slots[i].values[k].clone()
        } else {
            self.zero()
        }
    }
    fn location(&mut self, e: &Expr) -> Option<(usize, usize)> {
        let (i, idx) = match &e.kind {
            E::Var(i) => (*i, None),
            E::Index(i, e) => (*i, Some(e.as_ref())),
            _ => {
                self.fail("invalid assignment target");
                return None;
            }
        };
        self.index(i, idx).map(|k| (i, k))
    }
    fn store(&mut self, e: &Expr, value: Value) {
        if let Some((i, k)) = self.location(e) {
            self.write(i, k, value);
        }
    }
    fn write(&mut self, i: usize, k: usize, value: Value) {
        let ty = self.scratch.slots[i].ty;
        self.scratch.slots[i].values[k] = value.cast(ty, &mut self.scratch.gradients);
    }
    fn voltage(&mut self, a: Option<usize>, b: Option<usize>) -> Value {
        let mut get = |i: Option<usize>| {
            i.and_then(|id| self.bound.layout.index(id))
                .map(|i| {
                    Dual::variable(
                        self.input.x.get(i).copied().unwrap_or(0.),
                        i,
                        &mut self.scratch.gradients,
                    )
                })
                .unwrap_or_else(|| Dual::constant(0.))
        };
        Value::dual(get(a).sub(get(b), &mut self.scratch.gradients))
    }
    fn stamp(&mut self, row: Option<usize>, v: &Value, sign: f64) {
        if let Some(i) = row {
            self.out.f[i] += sign * (v.f.v + v.f.t);
            self.out.q[i] += sign * (v.q.v + v.q.t);
            for (dual, jac) in [(v.f, &mut self.out.df), (v.q, &mut self.out.dq)] {
                for (x, &d) in jac[i * self.n..(i + 1) * self.n]
                    .iter_mut()
                    .zip(self.scratch.gradients.row(dual))
                {
                    *x += sign * d;
                }
            }
        }
    }
    fn stmt(&mut self, s: &Stmt) {
        // At a statement boundary only variables, branch drives and operator
        // equations can outlive the statement in this invocation.
        let g = &mut self.scratch.gradients;
        if g.needs_collection() {
            for v in self
                .scratch
                .slots
                .iter_mut()
                .flat_map(|s| &mut s.values)
                .chain(self.drives.iter_mut().filter_map(|d| d.sum.as_mut()))
                .chain(self.operator_rows.iter_mut().flatten())
            {
                g.retain(&mut v.f);
                g.retain(&mut v.q);
            }
            g.finish_collection();
        }
        if !self.tick() {
            return;
        }
        match s {
            Stmt::Empty => {}
            Stmt::Block(vars, body) => {
                for (i, v) in vars {
                    if self.depth > 0 || self.scratch.slots[*i].values.is_empty() {
                        self.init(*i, v);
                    }
                }
                for s in body {
                    self.stmt(s);
                }
            }
            Stmt::Assign(a, b) => {
                let v = self.expr(b);
                self.store(a, v);
            }
            Stmt::Contrib(i, voltage, sign, e) => {
                let v = self.expr(e);
                let b = &self.module.branches[*i];
                if b.current
                    .and_then(|id| self.bound.layout.index(id))
                    .is_some()
                {
                    let drive = &mut self.drives[*i];
                    if drive.voltage != *voltage {
                        drive.sum = None;
                        drive.voltage = *voltage;
                    }
                    let sum = drive.sum.get_or_insert_with(|| Value::num(0.));
                    sum.f = sum.f.combine(
                        v.f,
                        sum.f.v + sign * v.f.v,
                        1.,
                        *sign,
                        &mut self.scratch.gradients,
                    );
                    sum.q = sum.q.combine(
                        v.q,
                        sum.q.v + sign * v.q.v,
                        1.,
                        *sign,
                        &mut self.scratch.gradients,
                    );
                } else if !*voltage {
                    self.stamp(b.a.and_then(|id| self.bound.layout.index(id)), &v, *sign);
                    self.stamp(b.b.and_then(|id| self.bound.layout.index(id)), &v, -sign);
                }
            }
            Stmt::If(c, a, b) => {
                if self.expr(c).truth() {
                    self.stmt(a);
                } else if let Some(b) = b {
                    self.stmt(b);
                }
            }
            Stmt::Case(e, arms) => {
                let v = self.expr(e);
                let mut default = None;
                for (labels, s) in arms {
                    if labels.is_empty() {
                        default = Some(s);
                    } else if labels.iter().any(|e| v.equal(&self.expr(e))) {
                        self.stmt(s);
                        return;
                    }
                }
                if let Some(s) = default {
                    self.stmt(s);
                }
            }
            Stmt::For(init, c, step, body) => {
                self.stmt(init);
                while self.tick() && self.expr(c).truth() {
                    self.stmt(body);
                    self.stmt(step);
                }
            }
            Stmt::While(c, body) => {
                while self.tick() && self.expr(c).truth() {
                    self.stmt(body);
                }
            }
            Stmt::Repeat(count, body) => {
                let count = self.expr(count).f.v.trunc().max(0.);
                if !count.is_finite() || count > ITERATION_CAP as f64 {
                    self.fail("repeat iteration budget exceeded");
                    return;
                }
                for _ in 0..count as usize {
                    if !self.tick() {
                        break;
                    }
                    self.stmt(body);
                }
            }
            Stmt::Event {
                initial,
                startup,
                last,
                body,
            } => {
                if (*startup && !self.scratch.initialized)
                    || (*initial && self.input.initial_step)
                    || (*last && self.input.final_step)
                {
                    self.stmt(body);
                }
            }
            Stmt::Task(name, args) => {
                if ["$error", "$finish", "$fatal"].contains(&name.as_str()) {
                    let msg = args
                        .iter()
                        .map(|e| {
                            let v = self.expr(e);
                            v.text.map_or_else(|| v.f.v.to_string(), |s| s.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    self.fail(format!("{name}: {msg}"));
                }
            }
        }
    }
    fn expr(&mut self, e: &Expr) -> Value {
        if self.out.error.is_some() {
            return self.zero();
        }
        let value = match &e.kind {
            E::Num(v) => Value::num(*v),
            E::Str(s) => Value::string(s.clone()),
            E::Param(i) => {
                let p = &self.bound.values[*i];
                if let Some(s) = &p.text {
                    Value::string(s.clone())
                } else {
                    Value::num(p.f.v)
                }
            }
            E::Var(i) => self.load(*i, None),
            E::Index(i, k) => self.load(*i, Some(k)),
            E::Voltage(a, b) => self.voltage(*a, *b),
            E::Current(i) => {
                if let Some(j) = self.module.branches[*i]
                    .current
                    .and_then(|id| self.bound.layout.index(id))
                {
                    Value::dual(Dual::variable(
                        self.input.x[j],
                        j,
                        &mut self.scratch.gradients,
                    ))
                } else {
                    self.zero()
                }
            }
            E::Given(i) => Value::num(f64::from(
                self.bound.given.get(*i).copied().unwrap_or(false),
            )),
            E::Connected(i) => Value::num(f64::from(*i < self.bound.connected_ports)),
            E::Ddx(a, i) => {
                let v = self.expr(a);
                Value::num(
                    self.bound
                        .layout
                        .index(*i)
                        .and_then(|j| self.scratch.gradients.row(v.f).get(j))
                        .copied()
                        .unwrap_or(0.),
                )
            }
            E::Select(c, a, b) => {
                if self.expr(c).truth() {
                    self.expr(a)
                } else {
                    self.expr(b)
                }
            }
            E::Unary(op, a) => {
                let a = self.expr(a);
                match op.as_str() {
                    "+" => a,
                    "-" => Value {
                        f: a.f.map(-a.f.v, -1., &mut self.scratch.gradients),
                        q: a.q.map(-a.q.v, -1., &mut self.scratch.gradients),
                        text: None,
                    },
                    "!" => Value::num(f64::from(!a.truth())),
                    _ => {
                        let x = a.f.v as i64;
                        let v = match op.as_str() {
                            "~" => !x,
                            "&" => i64::from(x == -1),
                            "~&" => i64::from(x != -1),
                            "|" => i64::from(x != 0),
                            "~|" => i64::from(x == 0),
                            "^" => (x.count_ones() % 2) as i64,
                            "~^" | "^~" => 1 - (x.count_ones() % 2) as i64,
                            _ => 0,
                        };
                        Value::num(v as f64)
                    }
                }
            }
            E::Binary(op, a, b) => {
                let x = self.expr(a);
                if op == "&&" && !x.truth() {
                    Value::num(0.)
                } else if op == "||" && x.truth() {
                    Value::num(1.)
                } else {
                    let y = self.expr(b);
                    self.binary(op, x, y, a.ty == Ty::Integer && b.ty == Ty::Integer)
                }
            }
            E::Unsupported(name, span) => self.unsupported(name, *span),
            E::Derivative(None, operand, _) => Value {
                f: Dual::constant(0.),
                q: self.expr(operand).f,
                text: None,
            },
            E::Derivative(Some(template), operand, span) => {
                let slot = self.bound.generated.slot(*template, |i| {
                    self.scratch.slots[i].values.first().map(|v| v.f.v)
                });
                let Some(k) = slot else {
                    return self.unsupported("ddt outside its elaborated genvar range", *span);
                };
                if self.operator_rows[k].is_some() {
                    return self.unsupported("repeated evaluation of one ddt operator", *span);
                }
                let state = self.voltage(Some(self.module.operator_node(k)), None);
                let q = self.expr(operand).f;
                self.operator_rows[k] = Some(Value {
                    f: state.f,
                    q: q.map(-q.v, -1., &mut self.scratch.gradients),
                    text: None,
                });
                state
            }
            E::Integral(k, integrand, ic, span) => {
                if self.operator_rows[*k].is_some() {
                    return self.unsupported("repeated evaluation of one idt operator", *span);
                }
                let state = self.voltage(Some(self.module.operator_rows.start + k), None);
                let initial = self.input.analysis == Analysis::Dc
                    || (self.input.analysis == Analysis::Tran && self.input.abstime == 0.);
                // In DC the explicit initial condition fixes the state. In the time domain
                // d(state)/dt = integrand becomes q = state and f = -integrand.
                let residual = if initial {
                    state.f.sub(self.expr(ic).f, &mut self.scratch.gradients)
                } else {
                    let u = self.expr(integrand);
                    u.f.map(-u.f.v, -1., &mut self.scratch.gradients)
                };
                self.operator_rows[*k] = Some(Value {
                    f: residual,
                    q: state.f,
                    text: None,
                });
                state
            }
            E::Call(name, _)
                if matches!(
                    name.as_str(),
                    "white_noise" | "flicker_noise" | "noise_table"
                ) =>
            {
                self.zero()
            }
            E::Call(name, args) => match args.as_slice() {
                [] => self.call(name, &[]),
                [a] => {
                    let a = self.expr(a);
                    self.call(name, &[a])
                }
                [a, b] => {
                    let a = self.expr(a);
                    let b = self.expr(b);
                    self.call(name, &[a, b])
                }
                _ => {
                    self.fail("invalid built-in arity");
                    self.zero()
                }
            },
            E::Function(i, args) => {
                let mut frame = self.scratch.frames.pop().unwrap_or_default();
                frame.inputs.clear();
                frame.locations.clear();
                for (e, (_, direction)) in args.iter().zip(&self.module.functions[*i].args) {
                    if direction == "input" {
                        frame.inputs.push(self.expr(e));
                        frame.locations.push(None);
                    } else {
                        let location = self.location(e);
                        frame.inputs.push(if direction == "inout" {
                            location
                                .map(|(i, k)| self.scratch.slots[i].values[k].clone())
                                .unwrap_or_else(|| self.zero())
                        } else {
                            self.zero()
                        });
                        frame.locations.push(location);
                    }
                }
                let v = self.function(*i, &frame.inputs, &mut frame.outputs);
                for (location, value) in frame.locations.drain(..).zip(frame.outputs.drain(..)) {
                    if let (Some((i, k)), Some(value)) = (location, value) {
                        self.write(i, k, value);
                    }
                }
                self.scratch.frames.push(frame);
                v
            }
            E::Limit(slot, name, args, func) => {
                let mut frame = self.scratch.frames.pop().unwrap_or_default();
                let values = &mut frame.inputs;
                values.clear();
                values.extend(args.iter().map(|e| self.expr(e)));
                let mut v = values[0].clone();
                // The polarity-aware junction limiter operates in forward-junction coordinates.
                if name == "typedpnjlim_new" {
                    v.f = v.f.mul(values[3].f, &mut self.scratch.gradients);
                }
                let new = v.f.v;
                let previous = self.state.current[*slot];
                let old = if previous.is_finite() {
                    previous
                } else {
                    self.state.accepted[*slot]
                };
                let limited = if !self.input.limit || name == "dummy" {
                    new
                } else if let Some(i) = func {
                    values.insert(1, Value::num(old));
                    self.function(*i, values, &mut frame.outputs).f.v
                } else if matches!(name.as_str(), "pnjlim" | "typedpnjlim_new") {
                    limit::pnjlim(new, old, values[1].f.v, values[2].f.v)
                } else {
                    limit::fetlim(new, old, values[1].f.v)
                };
                self.scratch.frames.push(frame);
                self.state.current[*slot] = limited;
                if (limited - new).abs() > 1e-14 * (1. + new.abs()) {
                    self.out.noncon = true;
                }
                v.f.t += new - limited;
                v.f.v = limited;
                v
            }
        };
        if e.ty == Ty::Integer {
            value.cast(Ty::Integer, &mut self.scratch.gradients)
        } else {
            value
        }
    }
    fn binary(&mut self, op: &str, a: Value, b: Value, integers: bool) -> Value {
        let x = a.f.v;
        let y = b.f.v;
        let g = &mut self.scratch.gradients;
        match op {
            "+" => Value {
                f: a.f.add(b.f, g),
                q: a.q.add(b.q, g),
                text: None,
            },
            "-" => Value {
                f: a.f.sub(b.f, g),
                q: a.q.sub(b.q, g),
                text: None,
            },
            "*" => Value {
                f: a.f.mul(b.f, g),
                q: a.q.mul(b.f, g).add(a.f.mul(b.q, g), g),
                text: None,
            },
            "/" => {
                if integers {
                    if y == 0. {
                        self.fail("integer division by zero");
                    }
                    Value::num((x / y).trunc())
                } else {
                    Value {
                        f: a.f.div(b.f, g),
                        q: a.q.div(b.f, g),
                        text: None,
                    }
                }
            }
            "**" => self.call("pow", &[a, b]),
            "%" => {
                if y == 0. {
                    self.fail("integer remainder by zero");
                }
                Value::num(x % y)
            }
            "==" | "===" => Value::num(f64::from(a.equal(&b))),
            "!=" | "!==" => Value::num(f64::from(!a.equal(&b))),
            "<" | ">" | "<=" | ">=" => {
                let v = if let (Some(x), Some(y)) = (&a.text, &b.text) {
                    match op {
                        "<" => x < y,
                        ">" => x > y,
                        "<=" => x <= y,
                        _ => x >= y,
                    }
                } else {
                    match op {
                        "<" => x < y,
                        ">" => x > y,
                        "<=" => x <= y,
                        _ => x >= y,
                    }
                };
                Value::num(f64::from(v))
            }
            "&&" => Value::num(f64::from(a.truth() && b.truth())),
            "||" => Value::num(f64::from(a.truth() || b.truth())),
            _ => {
                let x = x as i64;
                let y = y as i64;
                let v = match op {
                    "&" => x & y,
                    "|" => x | y,
                    "^" => x ^ y,
                    "~^" | "^~" => !(x ^ y),
                    "<<" | "<<<" => x.wrapping_shl(y as u32),
                    ">>" | ">>>" => x.wrapping_shr(y as u32),
                    _ => 0,
                };
                Value::num(v as f64)
            }
        }
    }
    fn function(&mut self, i: usize, values: &[Value], outputs: &mut Vec<Option<Value>>) -> Value {
        outputs.clear();
        if self.depth >= 128 {
            self.fail("function call depth exceeds 128");
            return self.zero();
        }
        self.depth += 1;
        let floor = self.scratch.gradients.enter();
        let f = &self.module.functions[i];
        // Functions have statically distinct slots; save them so nested calls preserve caller locals.
        let reentered = self.scratch.calls.contains(&i);
        self.scratch.calls.push(i);
        let saved = f
            .locals
            .iter()
            .filter(|_| reentered)
            .map(|(i, _)| (*i, self.scratch.slots[*i].clone()))
            .collect::<Vec<_>>();
        for (i, v) in &f.locals {
            self.init(*i, v);
        }
        for ((i, dir), v) in f.args.iter().zip(values) {
            if dir != "output" {
                let ty = self.scratch.slots[*i].ty;
                self.scratch.slots[*i].values[0] = v.clone().cast(ty, &mut self.scratch.gradients);
            }
        }
        for s in &f.body {
            self.stmt(s);
        }
        let result = self.load(f.result, None);
        outputs.extend(f.args.iter().map(|(i, dir)| {
            if dir == "input" {
                None
            } else {
                Some(self.load(*i, None))
            }
        }));
        for (i, s) in saved {
            self.scratch.slots[i] = s;
        }
        self.scratch.gradients.leave(floor);
        self.scratch.calls.pop();
        self.depth -= 1;
        result
    }
    fn call(&mut self, name: &str, a: &[Value]) -> Value {
        let x = a.first().map_or(0., |a| a.f.v);
        let y = a.get(1).map_or(0., |a| a.f.v);
        match name {
            "$temperature" => Value::num(self.input.temperature),
            "$vt" => {
                if a.is_empty() {
                    Value::num(K_OVER_Q * self.input.temperature)
                } else {
                    Value::dual(
                        a[0].f
                            .map(x * K_OVER_Q, K_OVER_Q, &mut self.scratch.gradients),
                    )
                }
            }
            "$abstime" => Value::num(self.input.abstime),
            "$mfactor" => Value::num(
                self.input.mfactor
                    * if self.module.implicit_m {
                        self.bound.values[0].f.v
                    } else {
                        1.
                    },
            ),
            "$simparam" => {
                let key = a[0].text.as_deref().unwrap_or("");
                let v = (self.input.simparams)(key)
                    .or(match key {
                        "gmin" => Some(self.input.gmin),
                        "tnom" => Some(self.bound.sim.tnom),
                        "scale" => Some(1.),
                        "shrink" => Some(0.),
                        "imelt" | "imax" if a.len() == 1 => Some(1.),
                        _ => None,
                    })
                    .unwrap_or(y);
                Value::num(v)
            }
            "analysis" => {
                let key = a[0].text.as_deref().unwrap_or("");
                Value::num(f64::from(match key {
                    "dc" | "static" => self.input.analysis == Analysis::Dc,
                    "tran" => self.input.analysis == Analysis::Tran,
                    "ac" => self.input.analysis == Analysis::Ac,
                    "noise" => self.input.analysis == Analysis::Noise,
                    "ic" => self.input.initial_step,
                    _ => false,
                }))
            }
            "white_noise" | "flicker_noise" | "noise_table" => Value::num(0.),
            "min" | "max" => {
                if (name == "min" && x <= y) || (name == "max" && x >= y) {
                    a[0].clone()
                } else {
                    a[1].clone()
                }
            }
            _ => {
                let Some(op) = crate::math::Math::named(name) else {
                    self.fail(format!("unknown runtime function {name}"));
                    return self.zero();
                };
                let (v, dx, dy) = op.apply(x, y);
                Value::dual(if let Some(b) = a.get(1) {
                    a[0].f.combine(b.f, v, dx, dy, &mut self.scratch.gradients)
                } else {
                    a[0].f.map(v, dx, &mut self.scratch.gradients)
                })
            }
        }
    }
}
