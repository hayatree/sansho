//! Simulation results, the ngspice-compatible raw file writer/reader and JSON serialization.
//!
//! Results are stored column-wise: one vector per variable.
//!
//! Provenance: original work. The ASCII raw layout follows the publicly documented
//! SPICE3/ngspice "rawfile" format (header lines, `Variables:` and `Values:` sections).
//! See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod raw_reader;

pub use raw_reader::{RawError, read_raw_ascii};

use std::io::{self, Write};

use serde::Serialize;

/// Physical quantity of a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantity {
    VoltageDensity,
    CurrentDensity,
    Resistance,
    Admittance,
    Scalar,
    Phase,
    PoleZero,
    Percent,
    /// Degrees Celsius.
    Temperature,
    /// Seconds.
    Time,
    /// Hertz.
    Frequency,
    /// Volts.
    Voltage,
    /// Amperes.
    Current,
}

impl Quantity {
    fn raw_name(self) -> &'static str {
        match self {
            Self::VoltageDensity => "voltage-density",
            Self::CurrentDensity => "current-density",
            Self::Resistance => "resistance",
            Self::Admittance => "admittance",
            Self::Scalar => "notype",
            Self::Phase => "phase",
            Self::PoleZero => "pole",
            Self::Percent => "percent",
            Self::Temperature => "temperature",
            Self::Time => "time",
            Self::Frequency => "frequency",
            Self::Voltage => "voltage",
            Self::Current => "current",
        }
    }

    /// SI unit symbol.
    pub fn unit(self) -> &'static str {
        match self {
            Self::VoltageDensity => "V/√Hz",
            Self::CurrentDensity => "A/√Hz",
            Self::Resistance => "Ω",
            Self::Admittance => "S",
            Self::Scalar => "",
            Self::Phase => "deg",
            Self::PoleZero => "rad/s",
            Self::Percent => "%",
            Self::Temperature => "°C",
            Self::Time => "s",
            Self::Frequency => "Hz",
            Self::Voltage => "V",
            Self::Current => "A",
        }
    }
}

/// A named result variable.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Variable {
    /// Name, e.g. `v(out)`, `i(v1)`, `time`.
    pub name: String,
    /// Quantity.
    pub quantity: Quantity,
}

/// Kind of analysis that produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisKind {
    Noise,
    IntegratedNoise,
    Tf,
    Sens,
    Four,
    Pz,
    /// DC operating point.
    Op,
    /// DC sweep.
    Dc,
    /// AC small-signal sweep.
    Ac,
    /// Transient.
    Tran,
}

impl AnalysisKind {
    /// ngspice plot name.
    pub fn plot_name(self) -> &'static str {
        match self {
            Self::Noise => "Noise Spectral Density Curves",
            Self::IntegratedNoise => "Integrated Noise",
            Self::Tf => "Transfer Function",
            Self::Sens => "Sensitivity Analysis",
            Self::Four => "Fourier Analysis",
            Self::Pz => "Pole-Zero Analysis",
            Self::Op => "Operating Point",
            Self::Dc => "DC transfer characteristic",
            Self::Ac => "AC Analysis",
            Self::Tran => "Transient Analysis",
        }
    }
}

/// Column-wise data.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Data {
    /// Real values: `columns[var][point]`.
    Real {
        /// One column per variable.
        columns: Vec<Vec<f64>>,
    },
    /// Complex values as `(re, im)`: `columns[var][point]`.
    Complex {
        /// One column per variable.
        columns: Vec<Vec<(f64, f64)>>,
    },
}

/// Result of one analysis.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisResult {
    /// Analysis kind.
    pub kind: AnalysisKind,
    /// Sweep variable (time, frequency, swept source), if any. Always real.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<Variable>,
    /// Sweep values (empty for `.op`).
    pub scale_values: Vec<f64>,
    /// Output variables (unknowns).
    pub variables: Vec<Variable>,
    /// Output data.
    pub data: Data,
}

impl AnalysisResult {
    /// Number of points.
    pub fn len(&self) -> usize {
        if self.scale.is_some() {
            return self.scale_values.len();
        }
        match &self.data {
            Data::Real { columns } => columns.first().map_or(0, Vec::len),
            Data::Complex { columns } => columns.first().map_or(0, Vec::len),
        }
    }

    /// Whether the result has no points.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Index of a variable by (case-insensitive) name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.variables
            .iter()
            .position(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// Real column of a variable.
    pub fn real(&self, name: &str) -> Option<&[f64]> {
        let i = self.index_of(name)?;
        match &self.data {
            Data::Real { columns } => columns.get(i).map(Vec::as_slice),
            Data::Complex { .. } => None,
        }
    }

    /// Complex column of a variable.
    pub fn complex(&self, name: &str) -> Option<&[(f64, f64)]> {
        let i = self.index_of(name)?;
        match &self.data {
            Data::Complex { columns } => columns.get(i).map(Vec::as_slice),
            Data::Real { .. } => None,
        }
    }
}

/// Result of one `.meas` card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measurement {
    /// Measurement name.
    pub name: String,
    /// Analysis it was evaluated on (`tran`, `ac`, `dc`).
    pub analysis: String,
    /// Value, if the measurement could be evaluated.
    pub value: Option<f64>,
    /// Unit of the value, e.g. `s`, `V`, `dB`, `V·s`.
    pub unit: String,
    /// Why the measurement failed, if it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Results of all analyses of one run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimResults {
    /// Netlist title.
    pub title: String,
    /// One entry per analysis, in netlist order.
    pub analyses: Vec<AnalysisResult>,
    /// `.meas` results in netlist order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<Measurement>,
}

/// Write results as an ngspice-compatible ASCII raw file (one plot per analysis).
pub fn write_raw_ascii(results: &SimResults, w: &mut impl Write) -> io::Result<()> {
    for a in &results.analyses {
        let complex = matches!(a.data, Data::Complex { .. });
        let mut vars: Vec<&Variable> = Vec::new();
        if let Some(s) = &a.scale {
            vars.push(s);
        }
        vars.extend(a.variables.iter());
        let npoints = a.len();
        writeln!(w, "Title: {}", results.title)?;
        writeln!(w, "Date: unknown")?;
        writeln!(w, "Plotname: {}", a.kind.plot_name())?;
        writeln!(w, "Flags: {}", if complex { "complex" } else { "real" })?;
        writeln!(w, "No. Variables: {}", vars.len())?;
        writeln!(w, "No. Points: {npoints}")?;
        writeln!(w, "Variables:")?;
        for (i, v) in vars.iter().enumerate() {
            writeln!(w, "\t{i}\t{}\t{}", v.name, v.quantity.raw_name())?;
        }
        writeln!(w, "Values:")?;
        for p in 0..npoints {
            let mut first = true;
            let mut emit = |w: &mut dyn Write, s: String| -> io::Result<()> {
                if first {
                    first = false;
                    writeln!(w, " {p}\t{s}")
                } else {
                    writeln!(w, "\t{s}")
                }
            };
            if a.scale.is_some() {
                let s = a.scale_values.get(p).copied().unwrap_or(0.0);
                if complex {
                    emit(w, format!("{s:.15e},{:.15e}", 0.0))?;
                } else {
                    emit(w, format!("{s:.15e}"))?;
                }
            }
            match &a.data {
                Data::Real { columns } => {
                    for c in columns {
                        emit(w, format!("{:.15e}", c[p]))?;
                    }
                }
                Data::Complex { columns } => {
                    for c in columns {
                        emit(w, format!("{:.15e},{:.15e}", c[p].0, c[p].1))?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_ascii_layout() {
        let r = SimResults {
            measures: vec![],
            title: "t".into(),
            analyses: vec![AnalysisResult {
                kind: AnalysisKind::Tran,
                scale: Some(Variable {
                    name: "time".into(),
                    quantity: Quantity::Time,
                }),
                scale_values: vec![0.0, 1e-9],
                variables: vec![Variable {
                    name: "v(out)".into(),
                    quantity: Quantity::Voltage,
                }],
                data: Data::Real {
                    columns: vec![vec![0.0, 0.5]],
                },
            }],
        };
        let mut buf = Vec::new();
        write_raw_ascii(&r, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("No. Variables: 2\nNo. Points: 2\n"));
        assert!(s.contains("\t1\tv(out)\tvoltage\n"));
        assert!(s.contains(" 1\t1.000000000000000e-9\n\t5.000000000000000e-1\n"));
        assert_eq!(r.analyses[0].real("V(OUT)"), Some(&[0.0, 0.5][..]));
    }
}
