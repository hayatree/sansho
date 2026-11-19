//! SPICE netlist lexer and parser producing a span-annotated, typed netlist.
//!
//! The supported subset grows milestone by milestone; unsupported constructs produce
//! structured diagnostics rather than being silently ignored.
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975) and
//! general SPICE3 conventions. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod expr;
mod lexer;
mod meas;
mod number;
mod parameters;
mod parser;
pub use parameters::ParameterPlan;
mod preproc;
mod statistics;
mod writer;
pub use statistics::Sample;

pub use expr::{Expr, eval_const, parse_behavior, parse_expr};
pub use lexer::{Lexed, Line, Token, lex, lex_source};
pub use number::parse_number;
pub use parser::{parse, parse_with, prepare_parameters};
pub use preproc::{FsLoader, NoFiles, SourceLoader};
pub use spx_diag::{Diagnostic, Severity, Span};
pub use writer::{write, write_analysis, write_measure};

use serde::Serialize;

/// Parse a netlist file; `.include` / `.lib` paths resolve relative to the including file.
///
/// I/O errors on the main file are reported as `E-PARSE-INCLUDE`.
pub fn parse_file(path: &std::path::Path) -> Result<Netlist, Vec<Diagnostic>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![Diagnostic::error(
            "E-PARSE-INCLUDE",
            format!("cannot read `{}`: {e}", path.display()),
        )]
    })?;
    let base = path
        .parent()
        .map_or_else(std::path::PathBuf::new, std::path::Path::to_path_buf);
    let mut nl = parse_with(&text, &FsLoader { base })?;
    if let Some(main) = nl.sources.first_mut() {
        *main = path.display().to_string();
    }
    Ok(nl)
}

/// Name of the ground node after normalization.
pub const GROUND: &str = "0";

/// A parsed netlist.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Netlist {
    /// Title line.
    pub title: String,
    /// Circuit elements in source order.
    pub elements: Vec<Element>,
    /// Loaded Verilog-A translation units.
    pub hdl: Vec<HdlSource>,
    /// Analysis requests in source order.
    pub analyses: Vec<AnalysisCard>,
    /// `.options` entries.
    pub options: Vec<OptionCard>,
    /// `.model` cards in source order.
    pub models: Vec<ModelCard>,
    /// Requested output vector names across all `.save` cards (empty means all).
    pub saves: Vec<String>,
    /// `.meas` cards in source order.
    pub measures: Vec<MeasureCard>,
    /// `.ic` node voltages.
    pub initial_conditions: Vec<NodeValue>,
    /// `.nodeset` node voltages (initial guesses for the operating point).
    pub nodesets: Vec<NodeValue>,
    /// Source files: index 0 is the main netlist (empty name), others are included files
    /// (referenced by `Span::file`).
    pub sources: Vec<String>,
    /// Non-fatal diagnostics (warnings).
    pub warnings: Vec<Diagnostic>,
}

/// A Verilog-A source loaded through the netlist source loader.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HdlSource {
    /// Resolved source path.
    pub path: String,
    /// Source text retained for compilation.
    pub text: String,
    /// Location of the HDL card in the netlist.
    pub span: Span,
}

/// A node voltage from `.ic` or `.nodeset`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeValue {
    /// Node name (lower-cased).
    pub node: String,
    /// Voltage.
    pub value: f64,
    /// Location.
    pub span: Span,
}

/// A circuit element instance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Element {
    /// Lower-cased instance name, e.g. `r1`.
    pub name: String,
    /// Location of the instance name.
    pub span: Span,
    /// Element type and connections.
    pub kind: ElementKind,
}

/// Control quantity of an ideal switch.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum SwitchControl {
    Voltage(String, String),
    Current(String),
}

/// Element types and their parameters. Node names are lower-cased; ground is [`GROUND`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElementKind {
    /// Compiled Verilog-A instance: Nname nodes... model [key=value ...].
    Va {
        /// Ordered terminal connections.
        nodes: Vec<String>,
        /// Model card name.
        model: String,
        /// Numeric instance overrides.
        params: Vec<(String, f64)>,
    },
    /// Ideal lossless line, port order [+1, -1, +2, -2].
    Line {
        ports: [String; 4],
        z0: f64,
        td: f64,
    },
    /// SPICE2 polynomial in differential voltages or branch currents.
    Polynomial {
        p: String,
        n: String,
        voltage: bool,
        controls: Vec<SwitchControl>,
        coefficients: Vec<f64>,
    },
    /// Gate-junction field-effect transistor.
    Jfet {
        d: String,
        g: String,
        s: String,
        model: String,
        params: Vec<(String, f64)>,
        off: bool,
    },
    /// Symmetric mutual inductance, with dots at the first winding terminals.
    Coupling {
        first: String,
        second: String,
        k: f64,
    },
    /// SW or CSW controlled conductance.
    Switch {
        p: String,
        n: String,
        control: SwitchControl,
        model: String,
        on: bool,
    },
    /// Nonlinear controlled voltage or current source.
    Behavioral {
        p: String,
        n: String,
        voltage: bool,
        expr: String,
        /// Optional endpoint-clamped E/G lookup.
        table: Vec<(f64, f64)>,
    },
    /// `Rxxx n+ n- value`
    Resistor {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Resistance in ohms.
        value: f64,
        /// Optional passive model providing nominal temperature and coefficients.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Instance parameters, including temperature coefficients and initial condition.
        params: Vec<(String, f64)>,
    },
    /// `Cxxx n+ n- value`
    Capacitor {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Capacitance in farads.
        value: f64,
        /// Optional passive model providing nominal temperature and coefficients.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Instance parameters, including temperature coefficients and initial condition.
        params: Vec<(String, f64)>,
    },
    /// `Lxxx n+ n- value`
    Inductor {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Inductance in henries.
        value: f64,
        /// Optional passive model providing nominal temperature and coefficients.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Instance parameters, including temperature coefficients and initial condition.
        params: Vec<(String, f64)>,
    },
    /// `Vxxx n+ n- [DC v] [AC mag [phase]] [tran-fn]`
    VoltageSource {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Source specification.
        source: SourceSpec,
    },
    /// `Ixxx n+ n- [DC v] [AC mag [phase]] [tran-fn]` (current flows from n+ through the source
    /// to n-).
    CurrentSource {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Source specification.
        source: SourceSpec,
    },
    /// `Exxx n+ n- nc+ nc- gain` (voltage-controlled voltage source)
    Vcvs {
        /// Positive output node.
        p: String,
        /// Negative output node.
        n: String,
        /// Positive controlling node.
        cp: String,
        /// Negative controlling node.
        cn: String,
        /// Voltage gain.
        gain: f64,
    },
    /// `Gxxx n+ n- nc+ nc- gm` (voltage-controlled current source)
    Vccs {
        /// Positive output node.
        p: String,
        /// Negative output node.
        n: String,
        /// Positive controlling node.
        cp: String,
        /// Negative controlling node.
        cn: String,
        /// Transconductance in siemens.
        gm: f64,
    },
    /// `Fxxx n+ n- vname gain` (current-controlled current source)
    Cccs {
        /// Positive output node.
        p: String,
        /// Negative output node.
        n: String,
        /// Controlling voltage source (lower-cased name).
        control: String,
        /// Current gain.
        gain: f64,
    },
    /// `Hxxx n+ n- vname r` (current-controlled voltage source)
    Ccvs {
        /// Positive output node.
        p: String,
        /// Negative output node.
        n: String,
        /// Controlling voltage source (lower-cased name).
        control: String,
        /// Transresistance in ohms.
        r: f64,
    },
    /// `Dxxx n+ n- model [area] [off] [key=value ...]`
    Diode {
        /// Anode.
        p: String,
        /// Cathode.
        n: String,
        /// Model name (lower-cased).
        model: String,
        /// Instance parameters (`area` included when given positionally).
        params: Vec<(String, f64)>,
        /// `off` flag (initial guess hint for the operating point).
        off: bool,
    },
    /// `Qxxx nc nb ne [ns] model [area] [off] [key=value ...]`
    Bjt {
        /// Collector.
        c: String,
        /// Base.
        b: String,
        /// Emitter.
        e: String,
        /// Substrate (ground when omitted).
        s: Option<String>,
        /// Model name (lower-cased).
        model: String,
        /// Instance parameters.
        params: Vec<(String, f64)>,
        /// `off` flag.
        off: bool,
    },
    /// `Mxxx nd ng ns nb model [key=value ...] [off]`
    Mosfet {
        /// Drain.
        d: String,
        /// Gate.
        g: String,
        /// Source.
        s: String,
        /// Bulk.
        b: String,
        /// Model name (lower-cased).
        model: String,
        /// Instance parameters (`l`, `w`, `ad`, `as`, `pd`, `ps`, ...).
        params: Vec<(String, f64)>,
        /// `off` flag.
        off: bool,
    },
}

impl ElementKind {
    /// Model name for semiconductor devices.
    pub fn model(&self) -> Option<&str> {
        match self {
            Self::Resistor { model, .. }
            | Self::Capacitor { model, .. }
            | Self::Inductor { model, .. } => model.as_deref(),
            Self::Switch { model, .. }
            | Self::Va { model, .. }
            | Self::Jfet { model, .. }
            | Self::Diode { model, .. }
            | Self::Bjt { model, .. }
            | Self::Mosfet { model, .. } => Some(model),
            _ => None,
        }
    }
}

/// A `.model name type (params)` card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelCard {
    /// Multi-component version labels that cannot be represented by an f64.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Lower-cased model name.
    pub name: String,
    /// Lower-cased model type (`d`, `npn`, `pnp`, `nmos`, `pmos`).
    pub kind: String,
    /// Parameters in source order (lower-cased names).
    pub params: Vec<(String, f64)>,
    /// Card location.
    pub span: Span,
}

impl ElementKind {
    /// Terminal nodes of the element (output and controlling nodes).
    pub fn nodes(&self) -> Vec<&str> {
        match self {
            Self::Coupling { .. } => vec![],
            Self::Line { ports, .. } => ports.iter().map(String::as_str).collect(),
            Self::Jfet { d, g, s, .. } => vec![d, g, s],
            Self::Polynomial { p, n, controls, .. } => {
                let mut nodes = vec![p.as_str(), n.as_str()];
                for control in controls {
                    if let SwitchControl::Voltage(a, b) = control {
                        nodes.extend([a.as_str(), b.as_str()]);
                    }
                }
                nodes
            }
            Self::Switch {
                p,
                n,
                control: SwitchControl::Voltage(a, b),
                ..
            } => vec![p, n, a, b],
            Self::Switch { p, n, .. } | Self::Behavioral { p, n, .. } => vec![p, n],
            Self::Va { nodes, .. } => nodes.iter().map(String::as_str).collect(),
            Self::Resistor { p, n, .. }
            | Self::Capacitor { p, n, .. }
            | Self::Inductor { p, n, .. }
            | Self::VoltageSource { p, n, .. }
            | Self::CurrentSource { p, n, .. }
            | Self::Cccs { p, n, .. }
            | Self::Ccvs { p, n, .. } => vec![p, n],
            Self::Vcvs { p, n, cp, cn, .. } | Self::Vccs { p, n, cp, cn, .. } => {
                vec![p, n, cp, cn]
            }
            Self::Diode { p, n, .. } => vec![p, n],
            Self::Bjt { c, b, e, s, .. } => {
                let mut v: Vec<&str> = vec![c, b, e];
                if let Some(s) = s {
                    v.push(s);
                }
                v
            }
            Self::Mosfet { d, g, s, b, .. } => vec![d, g, s, b],
        }
    }
}

/// Independent source specification.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SourceSpec {
    /// Explicit DC value, if given.
    pub dc: Option<f64>,
    /// AC magnitude (0 if not given).
    pub ac_mag: f64,
    /// AC phase in degrees.
    pub ac_phase_deg: f64,
    /// Transient waveform, if given.
    pub tran: Option<TranFn>,
}

/// Transient source functions. Parameters are stored as written; defaults that depend on the
/// analysis (e.g. `tstep`, `tstop`) are resolved by the device implementation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "fn", content = "params", rename_all = "snake_case")]
pub enum TranFn {
    /// `PULSE(v1 v2 [td [tr [tf [pw [per [np]]]]]])`
    Pulse(Vec<f64>),
    /// `SIN(vo va [freq [td [theta [phase]]]])`
    Sin(Vec<f64>),
    /// `EXP(v1 v2 [td1 [tau1 [td2 [tau2]]]])`
    Exp(Vec<f64>),
    /// `PWL(t1 v1 t2 v2 ...)`
    Pwl(Vec<(f64, f64)>),
}

/// Frequency sweep type for `.ac`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AcSweep {
    /// Points per decade.
    Dec,
    /// Points per octave.
    Oct,
    /// Total number of linearly spaced points.
    Lin,
}

/// Frequency grid for a small-signal parameter sweep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FrequencySweep {
    pub sweep: AcSweep,
    pub points: u32,
    pub fstart: f64,
    pub fstop: f64,
}

/// A differential voltage or the branch current of a voltage source.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Probe {
    Voltage(String, String),
    Current(String),
}
impl std::fmt::Display for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Voltage(p, n) if n == GROUND => write!(f, "v({p})"),
            Self::Voltage(p, n) => write!(f, "v({p},{n})"),
            Self::Current(v) => write!(f, "i({v})"),
        }
    }
}

/// Analysis requests.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "analysis", rename_all = "snake_case")]
pub enum AnalysisCard {
    Noise {
        span: Span,
        output: Probe,
        source: String,
        sweep: AcSweep,
        points: u32,
        fstart: f64,
        fstop: f64,
        summary: u32,
    },
    Tf {
        span: Span,
        output: Probe,
        source: String,
    },
    Sens {
        ac: Option<FrequencySweep>,
        span: Span,
        output: Probe,
    },
    Four {
        span: Span,
        frequency: f64,
        outputs: Vec<Probe>,
    },
    Pz {
        span: Span,
        nodes: [String; 4],
        voltage: bool,
        poles: bool,
        zeros: bool,
    },
    /// `.op`
    Op {
        /// Card location.
        span: Span,
    },
    /// `.dc srcname start stop incr`
    Dc {
        /// Card location.
        span: Span,
        /// Swept independent source (lower-cased).
        source: String,
        /// Start value.
        start: f64,
        /// Stop value.
        stop: f64,
        /// Increment (non-zero).
        step: f64,
    },
    /// `.ac dec|oct|lin n fstart fstop`
    Ac {
        /// Card location.
        span: Span,
        /// Sweep type.
        sweep: AcSweep,
        /// Number of points (per decade/octave, or total).
        points: u32,
        /// Start frequency in Hz.
        fstart: f64,
        /// Stop frequency in Hz.
        fstop: f64,
    },
    /// `.tran tstep tstop [tstart [tmax]] [uic]`
    Tran {
        /// Card location.
        span: Span,
        /// Suggested print increment.
        tstep: f64,
        /// Stop time.
        tstop: f64,
        /// Time from which results are stored.
        tstart: f64,
        /// Maximum internal step, if given.
        tmax: Option<f64>,
        /// Skip the initial operating point.
        uic: bool,
    },
}

/// A `.options` entry. Flags without a value have `value == None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OptionCard {
    /// Lower-cased option name.
    pub name: String,
    /// Numeric value, if any.
    pub value: Option<f64>,
    /// Location of the option name.
    pub span: Span,
}

/// Analysis a `.meas` card applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasAnalysis {
    /// `.meas dc`
    Dc,
    /// `.meas ac`
    Ac,
    /// `.meas tran`
    Tran,
}

/// How a complex (AC) or real value is reduced to a real number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalFn {
    /// Plain `v(..)` / `i(..)`: the real value, or the magnitude for AC results.
    Plain,
    /// `vm`, `im`: magnitude.
    Mag,
    /// `vdb`, `idb`: 20·log10 of the magnitude.
    Db,
    /// `vp`, `ip`: phase in radians.
    Phase,
    /// `vr`, `ir`: real part.
    Re,
    /// `vi`, `ii`: imaginary part.
    Im,
}

/// A measured signal such as `v(out)`, `v(a,b)`, `vdb(out)` or `i(v1)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Signal {
    /// Reduction function.
    pub func: SignalFn,
    /// `true` for branch currents `i(..)`.
    pub current: bool,
    /// Node (or voltage source for currents), lower-cased.
    pub pos: String,
    /// Optional reference node for differential voltages.
    pub neg: Option<String>,
}

/// Which crossing of a threshold counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CrossDir {
    /// Rising crossings.
    Rise,
    /// Falling crossings.
    Fall,
    /// Any crossing.
    Cross,
}

/// A point on the sweep axis defined by a threshold crossing or a fixed value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MeasEvent {
    /// The `n`-th crossing (`None` = last) of `value` by `signal`, ignoring crossings before
    /// `td`.
    Cross {
        /// Signal.
        signal: Signal,
        /// Threshold.
        value: f64,
        /// Direction.
        dir: CrossDir,
        /// 1-based crossing number, `None` for `LAST`.
        n: Option<u32>,
        /// Delay before crossings are counted.
        td: f64,
    },
    /// A fixed sweep value (`AT=`).
    At {
        /// Sweep value.
        at: f64,
    },
}

/// Statistic computed over an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasStat {
    /// Maximum.
    Max,
    /// Minimum.
    Min,
    /// Peak-to-peak.
    Pp,
    /// Average (integral divided by the interval length).
    Avg,
    /// Root mean square.
    Rms,
    /// Trapezoidal integral.
    Integ,
}

/// What a `.meas` card computes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeasKind {
    /// `TRIG ... TARG ...`: sweep distance between two events.
    TrigTarg {
        /// Trigger event.
        trig: MeasEvent,
        /// Target event.
        targ: MeasEvent,
    },
    /// `WHEN ...`: sweep value of an event.
    When {
        /// Event.
        event: MeasEvent,
    },
    /// `FIND sig WHEN ...` / `FIND sig AT=...`: value of a signal at an event.
    Find {
        /// Signal.
        signal: Signal,
        /// Event.
        event: MeasEvent,
    },
    /// `MAX|MIN|PP|AVG|RMS|INTEG sig [FROM=] [TO=]`
    Stat {
        /// Statistic.
        stat: MeasStat,
        /// Signal.
        signal: Signal,
        /// Interval start.
        from: Option<f64>,
        /// Interval end.
        to: Option<f64>,
    },
}

impl MeasEvent {
    fn signal(&self) -> Option<&Signal> {
        match self {
            Self::Cross { signal, .. } => Some(signal),
            Self::At { .. } => None,
        }
    }
}

impl MeasKind {
    /// Every signal the measurement reads.
    pub fn signals(&self) -> Vec<&Signal> {
        match self {
            Self::TrigTarg { trig, targ } => {
                trig.signal().into_iter().chain(targ.signal()).collect()
            }
            Self::When { event } => event.signal().into_iter().collect(),
            Self::Find { signal, event } => std::iter::once(signal).chain(event.signal()).collect(),
            Self::Stat { signal, .. } => vec![signal],
        }
    }
}

/// A `.meas` card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeasureCard {
    /// Lower-cased result name.
    pub name: String,
    /// Analysis the measurement applies to.
    pub analysis: MeasAnalysis,
    /// Measurement definition.
    pub kind: MeasKind,
    /// Card location.
    pub span: Span,
}
