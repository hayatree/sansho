//! Simulator options (`.options`) with SPICE-compatible defaults.
//!
//! Provenance: original work. Option names and default values follow the SPICE2/SPICE3
//! conventions documented in Nagel (1975). See `docs/PROVENANCE.md`.

use spx_diag::Diagnostic;
use spx_netlist::OptionCard;

/// Numerical options.
#[derive(Debug, Clone, PartialEq)]
pub struct SimOptions {
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
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            reltol: 1e-3,
            abstol: 1e-12,
            vntol: 1e-6,
            chgtol: 1e-14,
            trtol: 7.0,
            gmin: 1e-12,
            itl1: 100,
            itl4: 10,
            rshunt: None,
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
