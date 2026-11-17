//! SPICE netlist lexer and parser producing a span-annotated, typed netlist.
//!
//! The supported subset grows milestone by milestone; unsupported constructs produce
//! structured diagnostics rather than being silently ignored.
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975) and
//! general SPICE3 conventions. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod lexer;
mod number;
mod parser;

pub use lexer::{Lexed, Line, Token, lex};
pub use number::parse_number;
pub use parser::parse;
pub use spx_diag::{Diagnostic, Severity, Span};

use serde::Serialize;

/// Name of the ground node after normalization.
pub const GROUND: &str = "0";

/// A parsed netlist.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Netlist {
    /// Title line.
    pub title: String,
    /// Circuit elements in source order.
    pub elements: Vec<Element>,
    /// Analysis requests in source order.
    pub analyses: Vec<AnalysisCard>,
    /// `.options` entries.
    pub options: Vec<OptionCard>,
    /// Non-fatal diagnostics (warnings).
    pub warnings: Vec<Diagnostic>,
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

/// Element types and their parameters. Node names are lower-cased; ground is [`GROUND`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElementKind {
    /// `Rxxx n+ n- value`
    Resistor {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Resistance in ohms.
        value: f64,
    },
    /// `Cxxx n+ n- value`
    Capacitor {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Capacitance in farads.
        value: f64,
    },
    /// `Lxxx n+ n- value`
    Inductor {
        /// Positive node.
        p: String,
        /// Negative node.
        n: String,
        /// Inductance in henries.
        value: f64,
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
}

impl ElementKind {
    /// Terminal nodes of the element (output and controlling nodes).
    pub fn nodes(&self) -> Vec<&str> {
        match self {
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

/// Analysis requests.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "analysis", rename_all = "snake_case")]
pub enum AnalysisCard {
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
