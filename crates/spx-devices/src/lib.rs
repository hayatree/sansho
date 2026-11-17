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

mod linear;
mod sources;
pub mod waveform;

pub use linear::{Capacitors, Cccs, Ccvs, Inductors, Resistors, Vccs, Vcvs};
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

    /// Finish and return the layout.
    pub fn finish(self) -> StampLayout {
        self.layout
    }
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
}

impl Slots {
    /// Allocate zeroed slots matching a layout.
    pub fn for_layout(l: &StampLayout) -> Self {
        Self {
            g: vec![0.0; l.g.len()],
            c: vec![0.0; l.c.len()],
            f: vec![0.0; l.f.len()],
            q: vec![0.0; l.q.len()],
            b: vec![0.0; l.b.len()],
        }
    }
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
        }
    }
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

    /// Allocate branch-current unknowns. Called for all groups before any [`Self::setup`].
    fn alloc_unknowns(&mut self, _u: &mut Unknowns) {}

    /// Declare contributions. Called once.
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic>;

    /// Evaluate `G`, `C`, `f`, `q`, `b` at `x` into `out` (slot order from `setup`).
    fn eval(&self, ctx: &EvalCtx, x: &[f64], out: &mut Slots);

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
