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

pub mod bjt;
pub mod bsim4;
pub mod diode;
pub mod dual;
pub mod junction;
mod linear;
pub mod model;
pub mod mos1;
mod sources;
pub mod va;
pub mod waveform;

pub use bjt::Bjts;
pub use diode::Diodes;
pub use linear::{Capacitors, Cccs, Ccvs, Inductors, Resistors, Vccs, Vcvs};
pub use model::{ModelParams, ParamSpec};
pub use mos1::Mos1s;
pub use sources::{CurrentSources, VoltageSources, dc_value};

use std::collections::HashMap;

use spx_diag::Diagnostic;

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
#[derive(Debug, Clone, Default)]
pub struct Unknowns {
    names: Vec<String>,
    kinds: Vec<UnknownKind>,
    branches: HashMap<String, usize>,
}

impl Unknowns {
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
        let idx = self.names.len();
        self.names.push(format!("i({name})"));
        self.kinds.push(UnknownKind::Current);
        self.branches.insert(name.to_string(), idx);
        idx
    }

    /// Allocate an internal node voltage `v(device#suffix)` (e.g. behind a series resistance).
    pub fn alloc_internal_node(&mut self, device: &str, suffix: &str) -> usize {
        let idx = self.names.len();
        self.names.push(format!("v({device}#{suffix})"));
        self.kinds.push(UnknownKind::Voltage);
        idx
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
#[derive(Debug, Clone, Default)]
pub struct StampLayout {
    /// Entries of `G = ∂f/∂x`.
    pub g: Vec<(Node, Node)>,
    /// Entries of `C = ∂q/∂x`.
    pub c: Vec<(Node, Node)>,
    /// Rows of `f`.
    pub f: Vec<Node>,
    /// Rows of `q`.
    pub q: Vec<Node>,
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
    fn va_stamp(&mut self, at: &mut SlotOffsets, n: usize) -> spx_va::EvalStamp<'_> {
        let stamp = spx_va::EvalStamp {
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

/// All instances of one device type.
pub trait DeviceGroup: Send + Sync + std::fmt::Debug {
    /// Device type name, e.g. `resistor`.
    fn type_name(&self) -> &'static str;

    /// Number of instances.
    fn len(&self) -> usize;

    /// Whether the group has no instances.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `f` and `q` are linear in `x` (enables single-solve Newton).
    fn is_linear(&self) -> bool;

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

    /// Write the AC excitation into the `b` slots. Default: no excitation.
    fn eval_ac(&self, out: &mut AcSlots) {
        out.re.iter_mut().for_each(|v| *v = 0.0);
        out.im.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Append transient breakpoints in `[0, tstop]`.
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
