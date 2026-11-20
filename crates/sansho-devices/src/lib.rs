//! Device model interface and built-in device models.
//!
//! Every device is expressed in DAE form `d/dt q(x) + f(x) = b(t)` (architecture D1). A
//! [`DeviceGroup`] holds all instances of one model type (D3). During setup it declares, in a
//! fixed order, the matrix entries and vector rows it contributes to; during evaluation it writes
//! values into a private [`Slots`] buffer in exactly that order (D4). The engine sums slots into
//! the shared matrix in a fixed order, which makes results independent of scheduling.
//!
//! Provenance: original work. MNA stamps follow Ho, Ruehli & Brennan (1975). See
//! `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

pub mod behavioral;
pub mod bjt;
pub mod bsim3;
pub mod bsim4;
mod bsim_body;
pub mod controlled;
pub mod diode;
pub mod distortion;
pub mod dual;
pub mod jfet;
pub mod junction;
pub mod line;
mod linear;
pub mod ltra;
pub mod model;
pub mod mos1;
mod mos23;
mod observables;
mod random;
mod sources;
pub mod temperature;
pub mod urc;
pub mod va;
pub mod vdmos;
pub mod waveform;
pub mod xspice;

pub use bjt::Bjts;
pub use diode::Diodes;
pub use linear::{Capacitors, Cccs, Ccvs, Inductors, Resistors, Vccs, Vcvs};
pub use model::{ModelParams, ParamSpec};
pub use mos1::Mos1s;
pub use sources::{CurrentSources, VoltageSources, dc_value};

use std::collections::HashMap;

use sansho_diag::Diagnostic;

/// Elaboration inputs for one transistor. Temperatures in degrees Celsius.
#[derive(Debug)]
pub struct MosBinding<'a> {
    /// Evaluated model card.
    pub model: &'a ModelParams,
    /// Evaluated instance parameters.
    pub instance: &'a ModelParams,
    /// Geometry after the netlist scale factor.
    pub geometry: mos1::Mos1Geometry,
    /// +1 for NMOS, -1 for PMOS.
    pub polarity: f64,
    /// Circuit temperature.
    pub temp: f64,
    /// Geometric scale for additional layout distances.
    pub scale: f64,
    /// Global nominal temperature.
    pub tnom: f64,
}
/// Index of an unknown in the solution vector; `None` is the ground node.
pub type Node = Option<usize>;

/// Kind of an unknown, used to choose the absolute tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownKind {
    /// Node voltage (uses `vntol`).
    Voltage,
    /// Branch current (uses `abstol`).
    Current,
}

/// Registry of unknowns (node voltages first, then branch currents).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Unknowns {
    offset: usize,
    names: Vec<String>,
    kinds: Vec<UnknownKind>,
    branches: HashMap<String, usize>,
    operator_states: Vec<usize>,
}

impl Unknowns {
    /// Record a group's internal unknowns at its existing absolute offset.
    pub fn at_offset(offset: usize) -> Self {
        Self {
            offset,
            ..Self::default()
        }
    }

    /// Create a registry pre-populated with node voltages `v(name)`.
    pub fn with_nodes<'a>(nodes: impl IntoIterator<Item = &'a str>) -> Self {
        let mut u = Self::default();
        for n in nodes {
            u.names.push(format!("v({n})"));
            u.kinds.push(UnknownKind::Voltage);
        }
        u
    }

    /// Allocate a branch-current unknown `i(name)` for the device `name`.
    pub fn alloc_branch(&mut self, name: &str) -> usize {
        let idx = self.offset.saturating_add(self.names.len());
        self.names.push(format!("i({name})"));
        self.kinds.push(UnknownKind::Current);
        self.branches.insert(name.to_string(), idx);
        idx
    }

    /// Allocate an internal node voltage `v(device#suffix)` (e.g. behind a series resistance).
    pub fn alloc_internal_node(&mut self, device: &str, suffix: &str) -> usize {
        let idx = self.offset.saturating_add(self.names.len());
        self.names.push(format!("v({device}#{suffix})"));
        self.kinds.push(UnknownKind::Voltage);
        idx
    }

    /// Allocate a private analog-operator equation with voltage convergence scaling.
    pub fn alloc_operator_state(&mut self, device: &str, suffix: &str) -> usize {
        let idx = self.alloc_internal_node(device, suffix);
        self.operator_states.push(idx);
        idx
    }

    /// A physical node can take a shunt conductance in its KCL equation.
    pub fn is_physical_node(&self, index: usize) -> bool {
        self.kinds.get(index) == Some(&UnknownKind::Voltage)
            && self.operator_states.binary_search(&index).is_err()
    }

    /// Branch-current unknown of device `name`, if it has one.
    pub fn branch(&self, name: &str) -> Option<usize> {
        self.branches.get(name).copied()
    }

    /// Number of unknowns.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether there are no unknowns.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Display names (`v(node)` / `i(device)`).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Unknown kinds.
    pub fn kinds(&self) -> &[UnknownKind] {
        &self.kinds
    }
}

/// Declared contributions of one group, in slot order. `None` positions touch ground and are
/// discarded during assembly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StampLayout {
    /// Entries of `G = ∂f/∂x`.
    pub g: Vec<(Node, Node)>,
    /// Entries of `C = ∂q/∂x`.
    pub c: Vec<(Node, Node)>,
    /// Rows of `f`.
    pub f: Vec<Node>,
    /// Rows of `q`.
    pub q: Vec<Node>,
    /// Charge slots also monitored before summation for transient error control.
    pub lte_q: Vec<usize>,
    /// Individual device charges participate in timestep and breakpoint control.
    pub has_independent_charges: bool,
    /// Internal RC storage nodes whose relaxation times bound a transient restart.
    pub relaxation_nodes: Vec<usize>,
    /// Rows of `b` (independent sources).
    pub b: Vec<Node>,
}

/// Builder passed to [`DeviceGroup::setup`].
#[derive(Debug)]
pub struct StampBuilder<'a> {
    unknowns: &'a Unknowns,
    layout: StampLayout,
}

impl<'a> StampBuilder<'a> {
    /// Create a builder.
    pub fn new(unknowns: &'a Unknowns) -> Self {
        Self {
            unknowns,
            layout: StampLayout::default(),
        }
    }

    /// Declare a `G` entry.
    pub fn g(&mut self, row: Node, col: Node) {
        self.layout.g.push((row, col));
    }

    /// Declare the four `G` entries of a conductance-like stamp between `p` and `n`
    /// (order: pp, pn, np, nn).
    pub fn g2(&mut self, p: Node, n: Node) {
        self.g(p, p);
        self.g(p, n);
        self.g(n, p);
        self.g(n, n);
    }

    /// Declare a `C` entry.
    pub fn c(&mut self, row: Node, col: Node) {
        self.layout.c.push((row, col));
    }

    /// Declare an `f` row.
    pub fn f(&mut self, row: Node) {
        self.layout.f.push(row);
    }

    /// Declare a `q` row.
    pub fn q(&mut self, row: Node) {
        self.layout.q.push(row);
    }

    /// Declare a charge whose individual value participates in truncation control.
    /// A row of `None` tracks a device state without adding it to the MNA residual.
    pub fn tracked_q(&mut self, row: Node) {
        self.independent_charges();
        self.layout.lte_q.push(self.layout.q.len());
        self.q(row);
    }

    /// Declare a `b` row.
    pub fn b(&mut self, row: Node) {
        self.layout.b.push(row);
    }

    /// Look up the branch-current unknown of another device (e.g. the controlling source of an
    /// F or H element).
    pub fn branch(&self, name: &str) -> Option<usize> {
        self.unknowns.branch(name)
    }

    /// Current number of declared entries `(g, c, f, q, b)`; used by groups to record slot
    /// offsets of instances whose stamp size varies.
    pub fn counts(&self) -> SlotOffsets {
        SlotOffsets {
            g: self.layout.g.len(),
            c: self.layout.c.len(),
            f: self.layout.f.len(),
            q: self.layout.q.len(),
        }
    }

    /// Declare the stamp of a linear resistor between `p` and `n` (4 `G`, 2 `f`).
    pub fn resistor(&mut self, p: Node, n: Node) {
        self.g2(p, n);
        self.f(p);
        self.f(n);
    }

    /// Declare the dense stamp of a nonlinear core with terminals `nodes`: `N²` entries of `G`
    /// and of `C` (row-major), `N` rows of `f` and of `q`.
    pub fn core(&mut self, nodes: &[Node]) {
        for &r in nodes {
            for &c in nodes {
                self.g(r, c);
            }
        }
        for &r in nodes {
            for &c in nodes {
                self.c(r, c);
            }
        }
        for &r in nodes {
            self.f(r);
        }
        for &r in nodes {
            self.q(r);
        }
    }

    /// Declare a core whose terminal charges must also be tested individually for LTE.
    pub fn charge_core(&mut self, nodes: &[Node]) {
        let first = self.layout.q.len();
        self.core(nodes);
        self.layout.lte_q.extend(first..self.layout.q.len());
        self.independent_charges();
    }

    /// Request charge-aware startup and breakpoint handling. A private DAE
    /// row already supplies its own LTE sample through the assembled history.
    pub fn independent_charges(&mut self) {
        self.layout.has_independent_charges = true;
    }

    /// Register a local capacitive node with a resistive discharge path.
    pub fn relaxation_node(&mut self, node: Node) {
        self.layout.relaxation_nodes.extend(node);
    }

    /// Finish and return the layout.
    pub fn finish(self) -> StampLayout {
        self.layout
    }
}

/// Slot offsets of one instance, recorded with [`StampBuilder::counts`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotOffsets {
    /// Offset into `G` slots.
    pub g: usize,
    /// Offset into `C` slots.
    pub c: usize,
    /// Offset into `f` slots.
    pub f: usize,
    /// Offset into `q` slots.
    pub q: usize,
}

/// Values written by [`DeviceGroup::eval`], parallel to the [`StampLayout`].
#[derive(Debug, Clone, Default)]
pub struct Slots {
    /// Device quantities requested during a separate, unlimited output evaluation.
    pub outputs: Vec<Vec<(&'static str, f64)>>,
    /// Local startup biases, present only while sampling the first UIC charge.
    pub initial_conditions: Vec<Vec<f64>>,
    /// `G` entry values.
    pub g: Vec<f64>,
    /// `C` entry values.
    pub c: Vec<f64>,
    /// `f` row values.
    pub f: Vec<f64>,
    /// `q` row values.
    pub q: Vec<f64>,
    /// `b` row values.
    pub b: Vec<f64>,
    /// Set by nonlinear devices when Newton limiting changed a controlling voltage, which means
    /// the iteration must not be declared converged.
    pub noncon: bool,
    /// Runtime model failure, consumed by the Newton solver.
    pub error: Option<Diagnostic>,
}

impl Slots {
    pub(crate) fn initial_bias(&self, i: usize) -> &[f64] {
        self.initial_conditions.get(i).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn mos_voltages(&self, i: usize, actual: [f64; 4]) -> [f64; 4] {
        if let [ds, gs, bs] = self.initial_bias(i) {
            let s = actual[2];
            [s + ds, s + gs, s, s + bs]
        } else {
            actual
        }
    }
    /// Write a linear resistor stamp declared with [`StampBuilder::resistor`]; advances `at`.
    pub fn resistor(&mut self, at: &mut SlotOffsets, g: f64, vp: f64, vn: f64) {
        let i = g * (vp - vn);
        self.g[at.g..at.g + 4].copy_from_slice(&[g, -g, -g, g]);
        self.f[at.f] = i;
        self.f[at.f + 1] = -i;
        at.g += 4;
        at.f += 2;
    }

    /// Write a nonlinear core declared with [`StampBuilder::core`]; advances `at`.
    ///
    /// `i[t]` / `q[t]` are the current / charge flowing into terminal `t` evaluated at the
    /// linearization point `lin`, with gradients with respect to the terminal voltages. The
    /// written residual is the first-order expansion at the actual terminal voltages `act`.
    pub fn core<const N: usize>(
        &mut self,
        at: &mut SlotOffsets,
        i: &[dual::Dual<N>; N],
        q: &[dual::Dual<N>; N],
        act: &[f64; N],
        lin: &[f64; N],
    ) {
        let mut dv = [0.0; N];
        for k in 0..N {
            dv[k] = act[k] - lin[k];
        }
        for t in 0..N {
            let mut fi = i[t].v;
            let mut qi = q[t].v;
            for u in 0..N {
                self.g[at.g + t * N + u] = i[t].d[u];
                self.c[at.c + t * N + u] = q[t].d[u];
                fi += i[t].d[u] * dv[u];
                qi += q[t].d[u] * dv[u];
            }
            self.f[at.f + t] = fi;
            self.q[at.q + t] = qi;
        }
        at.g += N * N;
        at.c += N * N;
        at.f += N;
        at.q += N;
    }

    /// Write an already linearized dynamic dense core in row-major order.
    pub fn core_dyn(&mut self, at: &mut SlotOffsets, f: &[f64], q: &[f64], df: &[f64], dq: &[f64]) {
        let n = f.len();
        self.f[at.f..at.f + n].copy_from_slice(f);
        self.q[at.q..at.q + n].copy_from_slice(q);
        self.g[at.g..at.g + n * n].copy_from_slice(df);
        self.c[at.c..at.c + n * n].copy_from_slice(dq);
        at.f += n;
        at.q += n;
        at.g += n * n;
        at.c += n * n;
    }

    /// Borrow the next VA instance's final contribution slots.
    fn va_stamp(&mut self, at: &mut SlotOffsets, n: usize) -> sansho_va::EvalStamp<'_> {
        let stamp = sansho_va::EvalStamp {
            f: &mut self.f[at.f..at.f + n],
            q: &mut self.q[at.q..at.q + n],
            df: &mut self.g[at.g..at.g + n * n],
            dq: &mut self.c[at.c..at.c + n * n],
            ..Default::default()
        };
        at.f += n;
        at.q += n;
        at.g += n * n;
        at.c += n * n;
        stamp
    }

    /// Allocate zeroed slots matching a layout.
    pub fn for_layout(l: &StampLayout) -> Self {
        Self {
            outputs: Vec::new(),
            initial_conditions: Vec::new(),
            g: vec![0.0; l.g.len()],
            c: vec![0.0; l.c.len()],
            f: vec![0.0; l.f.len()],
            q: vec![0.0; l.q.len()],
            b: vec![0.0; l.b.len()],
            noncon: false,
            error: None,
        }
    }
}

/// Per-instance state of a group (e.g. last limited junction voltages, charge history).
///
/// `accepted` is the state at the last accepted solution (operating point or timepoint) and is
/// read-only during a Newton solve; `current` is read and written by every evaluation. The
/// engine copies `current` into `accepted` when a solution is accepted.
#[derive(Debug)]
pub struct StateRef<'a> {
    /// State at the last accepted solution.
    pub accepted: &'a [f64],
    /// Working state of the current Newton iteration.
    pub current: &'a mut [f64],
}

/// AC excitation written by [`DeviceGroup::eval_ac`], parallel to [`StampLayout::b`].
#[derive(Debug, Clone, Default)]
pub struct AcSlots {
    /// Real parts.
    pub re: Vec<f64>,
    /// Imaginary parts.
    pub im: Vec<f64>,
}

/// Which source values to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// DC operating point / DC sweep: sources use their DC value.
    Dc,
    /// AC small-signal linearization at the DC operating point.
    Ac,
    /// Small-signal noise linearization.
    Noise,
    /// Transient (including its initial operating point): sources use their waveform at `time`.
    Tran,
}

/// Transient analysis parameters that some source defaults depend on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranParams {
    /// `.tran` tstep.
    pub tstep: f64,
    /// `.tran` tstop.
    pub tstop: f64,
}

/// Evaluation context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvalCtx {
    /// Sample instance initial charges for the first UIC history point.
    pub initial_charge: bool,
    /// Source mode.
    pub mode: Mode,
    /// Simulation time (transient only).
    pub time: f64,
    /// Transient parameters (transient only; otherwise unused).
    pub tran: TranParams,
    /// Scale factor applied to all independent sources (source stepping).
    pub source_factor: f64,
    /// Conductance placed in parallel with every pn junction (S).
    pub gmin: f64,
    /// Device temperature (K).
    pub temp: f64,
    /// Whether Newton limiting of junction voltages is enabled. Disabled for evaluations at a
    /// converged solution (e.g. AC linearization, charge history updates).
    pub limit: bool,
    /// First iteration of a fresh DC solve: nonlinear devices ignore `x` and linearize around
    /// their built-in initial junction voltages.
    pub init_junctions: bool,
    /// Last evaluation point of the current analysis.
    pub final_step: bool,
}

impl EvalCtx {
    /// Context for DC analyses.
    pub fn dc() -> Self {
        Self {
            initial_charge: false,
            mode: Mode::Dc,
            time: 0.0,
            tran: TranParams {
                tstep: 0.0,
                tstop: 0.0,
            },
            source_factor: 1.0,
            gmin: 1e-12,
            temp: REFTEMP,
            limit: true,
            init_junctions: false,
            final_step: false,
        }
    }
}

/// Default nominal temperature, 27 °C in kelvin.
pub const REFTEMP: f64 = 300.15;

/// Boltzmann constant (J/K).
pub const BOLTZMANN: f64 = 1.380_649e-23;

/// Elementary charge (C).
pub const CHARGE: f64 = 1.602_176_634e-19;

/// Thermal voltage `kT/q` at temperature `temp` (K).
#[inline]
pub fn thermal_voltage(temp: f64) -> f64 {
    BOLTZMANN * temp / CHARGE
}

/// An independent random source with one-sided PSD power / frequency^exponent.
/// Each MNA row receives the same realization through its f and q coefficients.
#[derive(Debug)]
pub struct NoiseSource {
    pub device: String,
    pub power: f64,
    pub exponent: f64,
    pub table: Vec<(f64, f64)>,
    pub rows: Vec<(usize, f64, f64)>,
}
impl NoiseSource {
    pub fn spectral_power(&self, f: f64) -> f64 {
        if self.table.is_empty() {
            return self.power / f.powf(self.exponent);
        }
        let j = self.table.partition_point(|&(x, _)| x < f);
        if j == 0 {
            return self.table[0].1;
        }
        let (lo, a) = self.table[j - 1];
        let Some(&(hi, b)) = self.table.get(j) else {
            return a;
        };
        if a == 0. || b == 0. {
            return a + (b - a) * (f - lo) / (hi - lo);
        }
        (a.ln() + (b.ln() - a.ln()) * (f / lo).ln() / (hi / lo).ln()).exp()
    }
    pub fn branch(device: &str, p: Node, n: Node, power: f64, exponent: f64) -> Self {
        Self {
            device: device.into(),
            power,
            exponent,
            table: Vec::new(),
            rows: [(p, 1.), (n, -1.)]
                .into_iter()
                .filter_map(|(node, w)| node.map(|i| (i, w, 0.)))
                .collect(),
        }
    }
}

/// All instances of one device type.
pub trait DeviceGroup: Send + Sync + std::fmt::Debug {
    /// Device type name, e.g. `resistor`.
    fn type_name(&self) -> &'static str;

    /// Flattened name at this group's instance index, for parameter rebinding.
    fn instance_name(&self, _index: usize) -> Option<&str> {
        None
    }

    /// Number of instances.
    fn len(&self) -> usize;

    /// Whether the group has no instances.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `f` and `q` are linear in `x` (enables single-solve Newton).
    fn is_linear(&self) -> bool;
    /// Collect second- and third-order nonlinear current and charge coefficients.
    fn distortion(
        &self,
        _ctx: &EvalCtx,
        _bias: &[f64],
        _out: &mut Vec<distortion::Term>,
    ) -> Result<(), Diagnostic> {
        if self.is_linear() {
            Ok(())
        } else {
            Err(Diagnostic::error(
                "E-ANALYSIS",
                format!(
                    "DISTO derivatives unavailable for {} ({})",
                    self.instance_name(0).unwrap_or("device"),
                    self.type_name()
                ),
            ))
        }
    }
    /// Distributed element whose delayed waves require accepted transient samples.
    fn line(&self) -> Option<&line::Line> {
        None
    }

    fn ltra(&self) -> Option<&ltra::Ltra> {
        None
    }

    /// Apply one explicit UIC tuple using intrinsic terminals.
    fn instance_ic(
        &self,
        _index: usize,
        _ic: &[f64],
        _x: &mut [f64],
        _voltages: &mut Vec<(Node, Node, f64)>,
    ) {
    }
    /// Charge slots whose time derivatives contribute to a reported current.
    fn output_charges(&self, _index: usize, _param: &str) -> Vec<(usize, f64)> {
        Vec::new()
    }

    /// Supply UIC branch currents and differential voltage constraints.
    fn initial_values(&self, _x: &mut [f64], _voltages: &mut Vec<(Node, Node, f64)>) {}
    /// Source span for diagnostics emitted during setup.
    fn instance_span(&self) -> Option<sansho_diag::Span> {
        None
    }
    /// Node voltages read by expressions in addition to explicit terminals.
    fn probe_nodes(&self) -> Vec<usize> {
        Vec::new()
    }
    /// Update discrete controls after a converged continuous solve.
    fn settle(&self, _x: &[f64], _state: &mut [f64]) -> bool {
        false
    }
    /// Whether this group can locate a transition within a transient step.
    fn has_crossings(&self) -> bool {
        false
    }
    /// Fraction of a proposed step at which a transition occurred.
    fn crossing(
        &self,
        _old: &[f64],
        _new: &[f64],
        _accepted: &[f64],
        _current: &[f64],
    ) -> Option<f64> {
        None
    }
    /// Allocate branch-current and internal-node unknowns. Called for all groups before any
    /// [`Self::setup`].
    fn alloc_unknowns(&mut self, _u: &mut Unknowns) {}

    /// Number of state values (see [`StateRef`]).
    fn state_len(&self) -> usize {
        0
    }

    /// Declare contributions. Called once.
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic>;

    /// Evaluate `G`, `C`, `f`, `q`, `b` at `x` into `out` (slot order from `setup`).
    ///
    /// Nonlinear devices may linearize around a limited point `x̃` instead of `x`; they must
    /// then write `f(x̃) + G·(x − x̃)` (and likewise for `q`) so that the Newton step stays
    /// consistent, and set [`Slots::noncon`].
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots);

    /// Evaluate with a job-local linearization cache dedicated to this group.
    /// A zero tolerance permits only bit-exact input reuse.
    fn eval_cached(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        _cache: &mut bsim4::CoreCache,
        _tolerance: f64,
    ) {
        self.eval(ctx, x, state, out);
    }

    /// Optionally sample exact transient charges and advance limiter history at an
    /// unlimited solution. Return false to request full device evaluation.
    fn sample_charges(
        &self,
        _x: &[f64],
        _state: &mut StateRef<'_>,
        _out: &mut Slots,
        _cache: &mut bsim4::CoreCache,
    ) -> bool {
        false
    }

    /// Evaluate a Newton assembly whose specified voltage increment is exactly zero.
    fn eval_fixed_voltage(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        _node: usize,
    ) {
        self.eval(ctx, x, state, out);
    }

    /// Prepare transient pair kernels after the matrix ordering is known.
    #[cfg(feature = "jit")]
    fn prepare_newton_pairs(&mut self, _temperature: f64, _fixed_node: Node) {}

    /// Collect independent noise generators at the converged small-signal bias.
    fn noise(
        &self,
        _ctx: &EvalCtx,
        _x: &[f64],
        _slots: &Slots,
        _out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        Ok(())
    }

    /// Forward transport terms whose small-signal gain acquires exp(-jωτ).
    fn transport_delays(&self, _state: &[f64], _emit: &mut dyn FnMut(f64, Node, Node, f64)) {}

    /// Write the AC excitation into the `b` slots.
    /// Linearized current of an element without an MNA branch unknown. Emit
    /// coefficients of (G + jwC)x and return its independent AC excitation.
    fn ac_current(
        &self,
        _i: usize,
        _slots: &Slots,
        _emit: &mut dyn FnMut(Node, f64, f64),
    ) -> Option<(f64, f64)> {
        None
    }

    fn eval_ac(&self, out: &mut AcSlots) {
        out.re.iter_mut().for_each(|v| *v = 0.0);
        out.im.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Append (activation time, maximum step) pairs for transient sampling.
    fn step_limits(&self, _tran: &TranParams, _out: &mut Vec<(f64, f64)>) {}

    /// Append transient breakpoints in `[0, tstop]`.
    fn has_random_waveform(&self) -> bool {
        false
    }
    fn next_random_breakpoint(&self, _time: f64) -> f64 {
        f64::INFINITY
    }
    fn breakpoints(&self, _tran: &TranParams, _out: &mut Vec<f64>) {}

    /// Change an instance parameter (e.g. the DC value of a swept source). Returns `true` if the
    /// instance was found and the parameter changed.
    fn set_param(&mut self, _instance: &str, _param: &str, _value: f64) -> bool {
        false
    }

    /// Current value of an instance parameter.
    fn param(&self, _instance: &str, _param: &str) -> Option<f64> {
        None
    }
}

/// Read the voltage of `n` from `x` (ground is 0).
#[inline]
pub fn volt(x: &[f64], n: Node) -> f64 {
    n.map_or(0.0, |i| x[i])
}
