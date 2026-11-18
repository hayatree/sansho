//! Simulator options (`.options`) with SPICE-compatible defaults.
//!
//! Provenance: original work. Option names and default values follow the SPICE2/SPICE3
//! conventions documented in Nagel (1975). See `docs/PROVENANCE.md`.

use spx_diag::Diagnostic;
use spx_netlist::OptionCard;

/// Numerical options.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SimOptions {
    /// Enable native Verilog-A kernels in JIT builds.
    pub va_jit: bool,
    /// Relative tolerance.
    pub reltol: f64,
    /// Absolute current tolerance (A).
    pub abstol: f64,
    /// Absolute voltage tolerance (V).
    pub vntol: f64,
    /// Charge tolerance (C); used by LTE control (later milestone).
    pub chgtol: f64,
    /// Truncation error overestimation factor; used by LTE control (later milestone).
    pub trtol: f64,
    /// Minimum conductance (S).
    pub gmin: f64,
    /// Newton iteration limit for DC analyses.
    pub itl1: usize,
    /// Newton iteration limit per transient timepoint.
    pub itl4: usize,
    /// Optional shunt resistance from every node to ground (Ω).
    pub rshunt: Option<f64>,
    /// Circuit temperature (°C).
    pub temp: f64,
    /// Nominal model temperature (°C).
    pub tnom: f64,
    /// Default MOSFET channel length (m).
    pub defl: Option<f64>,
    /// Default MOSFET channel width (m).
    pub defw: Option<f64>,
    /// Default MOSFET drain area (m²).
    pub defad: Option<f64>,
    /// Default MOSFET source area (m²).
    pub defas: Option<f64>,
    /// Integration method for transient analysis.
    pub method: Method,
    /// Maximum number of gmin-stepping solves before giving up.
    pub gmin_steps: usize,
    /// Maximum number of source-stepping solves before giving up.
    pub src_steps: usize,
}

/// Transient integration method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// Trapezoidal rule (default).
    Trap,
    /// Second-order backward differentiation formula (`method=gear`).
    Gear,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            va_jit: cfg!(feature = "jit"),
            reltol: 1e-3,
            abstol: 1e-12,
            vntol: 1e-6,
            chgtol: 1e-14,
            trtol: 7.0,
            gmin: 1e-12,
            itl1: 100,
            itl4: 10,
            rshunt: None,
            temp: 27.0,
            tnom: 27.0,
            defl: None,
            defw: None,
            defad: None,
            defas: None,
            method: Method::Trap,
            gmin_steps: 100,
            src_steps: 100,
        }
    }
}

impl SimOptions {
    /// Build options from `.options` cards, returning warnings for unknown or invalid entries.
    pub fn from_cards(cards: &[OptionCard]) -> (Self, Vec<Diagnostic>) {
        let mut o = Self::default();
        let mut warnings = Vec::new();
        for c in cards {
            let ok = match (c.name.as_str(), c.value) {
                ("va_jit", Some(v @ (0.0 | 1.0))) => {
                    o.va_jit = v == 1.;
                    true
                }
                ("reltol", Some(v)) if v > 0.0 => {
                    o.reltol = v;
                    true
                }
                ("abstol", Some(v)) if v > 0.0 => {
                    o.abstol = v;
                    true
                }
                ("vntol", Some(v)) if v > 0.0 => {
                    o.vntol = v;
                    true
                }
                ("chgtol", Some(v)) if v > 0.0 => {
                    o.chgtol = v;
                    true
                }
                ("trtol", Some(v)) if v > 0.0 => {
                    o.trtol = v;
                    true
                }
                ("gmin", Some(v)) if v >= 0.0 => {
                    o.gmin = v;
                    true
                }
                ("itl1", Some(v)) if v >= 1.0 => {
                    o.itl1 = v as usize;
                    true
                }
                ("itl4", Some(v)) if v >= 1.0 => {
                    o.itl4 = v as usize;
                    true
                }
                ("rshunt", Some(v)) if v > 0.0 => {
                    o.rshunt = Some(v);
                    true
                }
                ("temp", Some(v)) => {
                    o.temp = v;
                    true
                }
                ("tnom", Some(v)) => {
                    o.tnom = v;
                    true
                }
                ("defl", Some(v)) if v > 0.0 => {
                    o.defl = Some(v);
                    true
                }
                ("defw", Some(v)) if v > 0.0 => {
                    o.defw = Some(v);
                    true
                }
                ("defad", Some(v)) if v >= 0.0 => {
                    o.defad = Some(v);
                    true
                }
                ("defas", Some(v)) if v >= 0.0 => {
                    o.defas = Some(v);
                    true
                }
                ("gminsteps", Some(v)) if v >= 0.0 => {
                    o.gmin_steps = v as usize;
                    true
                }
                ("srcsteps", Some(v)) if v >= 0.0 => {
                    o.src_steps = v as usize;
                    true
                }
                ("trap", None) => {
                    o.method = Method::Trap;
                    true
                }
                ("gear", None) => {
                    o.method = Method::Gear;
                    true
                }
                // Output-formatting flags that have no effect here.
                ("nopage" | "nomod" | "noacct" | "noinit" | "acct" | "list" | "node", _) => true,
                _ => false,
            };
            if !ok {
                warnings.push(
                    Diagnostic::warning(
                        "W-OPT-IGNORED",
                        format!(
                            "option `{}` is unknown or has an invalid value; ignored",
                            c.name
                        ),
                    )
                    .with_span(c.span),
                );
            }
        }
        (o, warnings)
    }
}
