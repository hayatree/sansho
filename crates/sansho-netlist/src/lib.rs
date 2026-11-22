//! SPICE netlist lexer and parser producing a span-annotated, typed netlist.
//!
//! The supported subset grows milestone by milestone; unsupported constructs produce
//! structured diagnostics rather than being silently ignored.
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975) and
//! general SPICE3 conventions. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

pub mod control;
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
mod xspice;
pub use statistics::Sample;
pub use xspice::{CodePort, SourceTable, analog_model, digital_code, digital_model};

pub use expr::{
    Expr, current_probe_branches, eval as eval_expr, eval_const, parse_behavior, parse_expr,
    voltage_probe_nodes,
};
pub use lexer::{Lexed, Line, Token, lex, lex_source};
pub use meas::parse_control_meas;
pub use number::parse_number;
pub use parser::{parse, parse_parameters, parse_with, prepare_parameters};
pub use preproc::{FsLoader, NoFiles, SourceLoader};
pub use sansho_diag::{Diagnostic, Severity, Span};
pub use writer::{write, write_analysis, write_measure, write_model, write_model_with};

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
    /// Batch control blocks, in encounter order.
    pub controls: Vec<Vec<control::Statement>>,
    pub csparams: std::collections::BTreeMap<String, f64>,
    /// Original input for re-elaboration of global parameters by alterparam/reset.
    #[serde(skip)]
    pub parameter_source: Option<String>,
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
    /// Optional device observables referenced by a running control script.
    #[serde(skip)]
    pub control_probes: Vec<String>,
    /// Requested output vector names across all `.save` cards (empty means all).
    pub saves: Vec<String>,
    /// Batch output selections.
    pub prints: Vec<PrintCard>,
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
    /// Ordered instance initial conditions, used with transient UIC.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ic: Vec<f64>,
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
    /// XSPICE analog code model.
    CodeModel { ports: Vec<CodePort>, model: String },
    /// Compiled Verilog-A instance: Nname nodes... model [key=value ...].
    Va {
        /// Ordered terminal connections.
        nodes: Vec<String>,
        /// Model card name.
        model: String,
        /// Numeric instance overrides.
        params: Vec<(String, f64)>,
    },
    /// Distributed RC ladder with common substrate.
    Urc {
        ports: [String; 3],
        model: String,
        params: Vec<(String, f64)>,
    },
    /// TXL or CPL with a reference terminal after each group of conductors.
    Kspice {
        ports: Vec<String>,
        model: String,
        params: Vec<(String, f64)>,
    },
    /// Convolutional lossy two-port.
    Ltra { ports: [String; 4], model: String },
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
    /// Statz metal-semiconductor field-effect transistor.
    Mesfet {
        d: String,
        g: String,
        s: String,
        model: String,
        params: Vec<(String, f64)>,
        off: bool,
    },
    /// Three-terminal vertical double-diffused MOS transistor.
    Vdmos {
        thermal_pins: Option<[String; 2]>,
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
        /// Numeric modifiers on signal-dependent passives.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        params: Vec<(String, f64)>,
        /// Optional endpoint-clamped E/G lookup.
        table: Vec<(f64, f64)>,
    },
    /// `Rxxx n+ n- value`
    Resistor {
        /// Parallel instances represented by the effective value.
        mult: f64,
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
    /// `Qxxx nc nb ne [ns [tj]] model [area] [off] [key=value ...]`
    Bjt {
        /// Collector.
        c: String,
        /// Base.
        b: String,
        /// Emitter.
        e: String,
        /// Substrate (ground when omitted).
        s: Option<String>,
        /// Compact bipolar junction temperature rise (optional fifth terminal).
        #[serde(skip_serializing_if = "Option::is_none")]
        tj: Option<String>,
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
        /// Bulk, or the back gate of an SOI device.
        b: String,
        /// Optional SOI contact, body and thermal terminals in card order.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        extra: Vec<String>,
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
            Self::CodeModel { model, .. }
            | Self::Switch { model, .. }
            | Self::Va { model, .. }
            | Self::Urc { model, .. }
            | Self::Kspice { model, .. }
            | Self::Ltra { model, .. }
            | Self::Jfet { model, .. }
            | Self::Mesfet { model, .. }
            | Self::Vdmos { model, .. }
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
    /// Ordered physical continuation cards of a numerical semiconductor model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cider: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<SourceTable>,
    /// Ordered numeric arrays on code-model and CPL cards.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arrays: Vec<(String, Vec<f64>)>,
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

impl ModelCard {
    /// Silicon-on-insulator MOSFET levels in the ngspice 44 dialect.
    pub fn is_bsimsoi(&self) -> bool {
        matches!(self.kind.as_str(), "nmos" | "pmos")
            && self
                .params
                .iter()
                .rfind(|(k, _)| k == "level")
                .is_some_and(|(_, v)| matches!(v, 10. | 58.))
    }

    /// True for a native level-four bipolar card, honoring the last LEVEL.
    pub fn is_vbic(&self) -> bool {
        self.bipolar_level(4.)
    }

    /// Native HICUM/L2 card.
    pub fn is_hicum(&self) -> bool {
        self.bipolar_level(8.)
    }

    pub(crate) fn thermal_bipolar(&self) -> bool {
        self.is_vbic() || self.is_hicum()
    }

    fn bipolar_level(&self, level: f64) -> bool {
        matches!(self.kind.as_str(), "npn" | "pnp")
            && self
                .params
                .iter()
                .rev()
                .find(|(k, _)| k == "level")
                .is_some_and(|(_, v)| *v == level)
    }
}

impl ElementKind {
    /// Terminal nodes of the element (output and controlling nodes).
    pub fn nodes(&self) -> Vec<&str> {
        match self {
            Self::CodeModel { ports, .. } => ports
                .iter()
                .filter(|p| p.modifier != "vnam")
                .flat_map(|p| p.nodes.iter().map(String::as_str))
                .collect(),
            Self::Coupling { .. } => vec![],
            Self::Kspice { ports, .. } => ports.iter().map(String::as_str).collect(),
            Self::Urc { ports, .. } => ports.iter().map(String::as_str).collect(),
            Self::Line { ports, .. } | Self::Ltra { ports, .. } => {
                ports.iter().map(String::as_str).collect()
            }
            Self::Vdmos {
                d,
                g,
                s,
                thermal_pins,
                ..
            } => {
                let mut nodes = vec![d.as_str(), g.as_str(), s.as_str()];
                nodes.extend(thermal_pins.iter().flatten().map(String::as_str));
                nodes
            }
            Self::Mesfet { d, g, s, .. } | Self::Jfet { d, g, s, .. } => vec![d, g, s],
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
            Self::Bjt { c, b, e, s, tj, .. } => {
                let mut v: Vec<&str> = vec![c, b, e];
                if let Some(s) = s {
                    v.push(s);
                }
                v.extend(tj.as_deref());
                v
            }
            Self::Mosfet {
                d, g, s, b, extra, ..
            } => {
                let mut nodes: Vec<&str> = vec![d, g, s, b];
                nodes.extend(extra.iter().map(String::as_str));
                nodes
            }
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
    /// Whether the card carries an `ac` specification, even `ac 0`. Pole-zero
    /// analysis treats such a voltage source as the open excitation port.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ac_given: bool,
    /// One-based RF port number and real reference resistance in ohms.
    pub port: Option<(usize, f64)>,
    /// DISTOF1 and DISTOF2 peak magnitudes and phases in degrees.
    pub disto: [Option<(f64, f64)>; 2],
    /// Transient waveform, if given.
    pub tran: Option<TranFn>,
}

/// Transient source functions. Parameters are stored as written; defaults that depend on the
/// analysis (e.g. `tstep`, `tstop`) are resolved by the device implementation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "fn", content = "params", rename_all = "snake_case")]
pub enum TranFn {
    /// NA, NT, NALPHA, NAMP, RTSAM, RTSCAPT, RTSEMT.
    TrNoise([f64; 7]),
    /// TYPE, TS, TD, PARAM1, PARAM2.
    TrRandom([f64; 5]),
    /// `PULSE(v1 v2 [td [tr [tf [pw [per [np]]]]]])`
    Pulse(Vec<f64>),
    /// `SIN(vo va [freq [td [theta [phase]]]])`
    Sin(Vec<f64>),
    /// `SFFM(vo va [fc mdi fm td phasem phasec])`
    Sffm(Vec<f64>),
    /// `AM(vo vmo [vma fm fc td phasem phasec])`
    Am(Vec<f64>),
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

/// One axis of a DC sweep, traversed in netlist order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DcSweep {
    pub source: String,
    pub start: f64,
    pub stop: f64,
    pub step: f64,
}

impl DcSweep {
    pub fn count(&self) -> Option<usize> {
        let span = (self.stop - self.start) / self.step;
        if !self.step.is_finite() || self.step == 0. || span < 0. {
            return None;
        }
        let intervals = (span + 1e-9).floor();
        (intervals >= 0. && intervals < (isize::MAX as usize / 8) as f64)
            .then(|| intervals as usize + 1)
    }
    pub fn value(&self, index: usize) -> f64 {
        self.start + index as f64 * self.step
    }
}

/// Periodic shooting controls (ngspice 44 §11.3.12).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PssSpec {
    pub gfreq: f64,
    pub tstab: f64,
    pub oscnob: String,
    pub points: usize,
    pub harmonics: usize,
    pub iterations: usize,
    pub steadycoeff: f64,
    pub uic: bool,
    pub driven: bool,
}

/// Analysis requests.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "analysis", rename_all = "snake_case")]
pub enum AnalysisCard {
    Pss {
        span: Span,
        spec: PssSpec,
    },
    Sp {
        span: Span,
        grid: FrequencySweep,
        noise: bool,
    },
    Disto {
        span: Span,
        grid: FrequencySweep,
        ratio: Option<f64>,
    },
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
        #[serde(flatten)]
        inner: DcSweep,
        #[serde(skip_serializing_if = "Option::is_none")]
        outer: Option<DcSweep>,
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
    /// Noise spectrum.
    Noise,
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
    /// A bare vector selector without a voltage or current function.
    pub bare: bool,
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
        /// Optional signal used as a varying threshold.
        reference: Option<Signal>,
        /// Direction.
        dir: CrossDir,
        /// 1-based crossing number, `None` for `LAST`.
        n: Option<u32>,
        /// Delay before crossings are counted.
        td: Option<f64>,
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
    MaxAt,
    MinAt,
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
    Param {
        expression: String,
    },
    Deriv {
        signal: Signal,
        event: MeasEvent,
    },
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
    fn signals(&self) -> Vec<&Signal> {
        match self {
            Self::Cross {
                signal, reference, ..
            } => std::iter::once(signal).chain(reference).collect(),
            Self::At { .. } => Vec::new(),
        }
    }
}

impl MeasKind {
    /// Every signal the measurement reads.
    pub fn signals(&self) -> Vec<&Signal> {
        match self {
            Self::Param { .. } => Vec::new(),
            Self::TrigTarg { trig, targ } => {
                trig.signals().into_iter().chain(targ.signals()).collect()
            }
            Self::When { event } => event.signals(),
            Self::Deriv { signal, event } | Self::Find { signal, event } => {
                std::iter::once(signal).chain(event.signals()).collect()
            }
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

/// Batch table requested by one .print card.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PrintCard {
    pub analysis: String,
    pub signals: Vec<Signal>,
    pub span: sansho_diag::Span,
}

impl Signal {
    /// Formatted netlist representation.
    pub fn expression(&self) -> String {
        writer::signal(self)
    }

    /// Bare vector selector, including a device operating-point quantity.
    pub fn is_vector(&self) -> bool {
        self.bare
    }
}
