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
    Power,
    Capacitance,
    Charge,
    VoltageDensity,
    CurrentDensity,
    /// Short-circuit current noise cross-spectral density.
    CurrentNoisePower,
    Resistance,
    Admittance,
    Scalar,
    Decibels,
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
            Self::Power => "power",
            Self::Capacitance => "capacitance",
            Self::Charge => "charge",
            Self::VoltageDensity => "voltage-density",
            Self::CurrentDensity => "current-density",
            Self::CurrentNoisePower => "current-noise-power",
            Self::Resistance => "resistance",
            Self::Admittance => "admittance",
            Self::Scalar => "notype",
            Self::Decibels => "decibel",
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
            Self::Power => "W",
            Self::Capacitance => "F",
            Self::Charge => "C",
            Self::VoltageDensity => "V/√Hz",
            Self::CurrentDensity => "A/√Hz",
            Self::CurrentNoisePower => "A²/Hz",
            Self::Resistance => "Ω",
            Self::Admittance => "S",
            Self::Scalar => "",
            Self::Decibels => "dB",
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
    /// User-created vector plot.
    Vectors,
    Pss,
    PssFreq,
    Sp,
    Harmonic2,
    Harmonic3,
    Sum,
    Difference,
    Intermod3,

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
    pub fn is_distortion(self) -> bool {
        matches!(
            self,
            Self::Harmonic2 | Self::Harmonic3 | Self::Sum | Self::Difference | Self::Intermod3
        )
    }
    /// ngspice plot name.
    pub fn plot_name(self) -> &'static str {
        match self {
            Self::Vectors => "unknown",
            Self::Pss => "Time Domain Periodic Steady State Analysis",
            Self::PssFreq => "Frequency Domain Periodic Steady State Analysis",
            Self::Sp => "SP Analysis",
            Self::Harmonic2 => "DISTORTION - 2nd harmonic",
            Self::Harmonic3 => "DISTORTION - 3rd harmonic",
            Self::Sum => "DISTORTION - IM: f1+f2",
            Self::Difference => "DISTORTION - IM: f1-f2",
            Self::Intermod3 => "DISTORTION - IM: 2f1-f2",
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

/// Node name to successive (axis coordinate, digital symbol) changes.
pub type EventTraces = std::collections::BTreeMap<String, Vec<(f64, String)>>;

/// Result of one analysis.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisResult {
    /// Event-domain output retained independently of analog raw-file projection.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub events: EventTraces,
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
    /// Keep selected variables and their waveform columns in their existing order.
    pub fn retain_variables(&mut self, mut keep: impl FnMut(&Variable) -> bool) {
        let mask: Vec<_> = self.variables.iter().map(&mut keep).collect();
        let mut include = mask.iter().copied();
        self.variables.retain(|_| include.next().unwrap_or(false));
        let mut include = mask.into_iter();
        match &mut self.data {
            Data::Real { columns } => columns.retain(|_| include.next().unwrap_or(false)),
            Data::Complex { columns } => columns.retain(|_| include.next().unwrap_or(false)),
        }
    }

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
        if matches!(self.kind, AnalysisKind::Pss | AnalysisKind::PssFreq) {
            // In a 44.2 #branch capture the last matching column carries the current.
            return self
                .variables
                .iter()
                .rposition(|v| v.name.eq_ignore_ascii_case(name));
        }
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

/// Selected batch output columns in deck order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrintTable {
    pub analysis: AnalysisKind,
    pub headings: Vec<String>,
    pub columns: Vec<Vec<f64>>,
}

/// Results of all analyses of one run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimResults {
    /// Batch raw selection. Interactive writes and decoded files use their supplied layout.
    #[serde(skip)]
    pub batch_raw: Option<BatchRaw>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prints: Vec<PrintTable>,
    /// Netlist title.
    pub title: String,
    /// One entry per analysis, in netlist order.
    pub analyses: Vec<AnalysisResult>,
    /// `.meas` results in netlist order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<Measurement>,
}

/// Batch output policy from elaboration and the noise cards in netlist order.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchRaw {
    pub private_nodes: std::collections::BTreeSet<String>,
    pub noise_intervals: Vec<u32>,
    /// The 44.2 batch writer emits only the start of a two-point linear AC sweep.
    pub ac_start_only: Vec<bool>,
}

impl BatchRaw {
    fn visible(&self, kind: AnalysisKind, name: &str, detail: bool) -> bool {
        match kind {
            AnalysisKind::Noise => detail || matches!(name, "onoise_spectrum" | "inoise_spectrum"),
            AnalysisKind::IntegratedNoise => {
                detail || matches!(name, "onoise_total" | "inoise_total")
            }
            _ => !self.private_nodes.contains(name),
        }
    }
}

fn integrated_noise_name(name: &str) -> bool {
    matches!(name, "onoise_total" | "inoise_total")
        || name.starts_with("onoise_total_")
        || name.starts_with("inoise_total_")
}

/// ngspice 44.2 batch raw spelling of transfer-function, pole-zero and
/// S-parameter results: every value is written as a voltage vector named
/// `v(...)`, with the capitalisation of its printed report (R22g probes).
fn special_raw_name(kind: AnalysisKind, name: &str) -> Option<String> {
    if name.starts_with("v(") || name.starts_with("i(") {
        return None;
    }
    match kind {
        AnalysisKind::Pz | AnalysisKind::Sp => Some(name.to_owned()),
        AnalysisKind::Tf => Some(if name == "transfer_function" {
            "Transfer_function".to_owned()
        } else if let Some(source) = name.strip_suffix("#input_impedance") {
            format!("{source}#Input_impedance")
        } else if let Some(branch) = name.strip_suffix("#output_impedance") {
            format!("{branch}#Output_impedance")
        } else if let Some(node) = name.strip_prefix("output_impedance_at_v(") {
            format!("output_impedance_at_V({node}")
        } else {
            name.to_owned()
        }),
        _ => None,
    }
}

struct RawColumn<'a> {
    index: usize,
    name: std::borrow::Cow<'a, str>,
    zero: bool,
}

impl RawColumn<'_> {
    fn complex(&self, value: (f64, f64)) -> (f64, f64) {
        if self.zero { (0., 0.) } else { value }
    }
}

fn raw_column<'a>(
    batch: Option<&BatchRaw>,
    kind: AnalysisKind,
    index: usize,
    name: &'a str,
) -> RawColumn<'a> {
    let zero = batch.is_some()
        && kind == AnalysisKind::Ac
        && name.starts_with("@r")
        && name.ends_with("[i]");
    let name = if batch.is_some()
        && matches!(kind, AnalysisKind::Noise | AnalysisKind::IntegratedNoise)
        && ["onoise.", "onoise_total.", "inoise_total."]
            .iter()
            .any(|p| name.starts_with(p))
    {
        name.replacen('.', "_", 1).into()
    } else {
        name.into()
    };
    RawColumn { index, name, zero }
}

#[derive(Clone, Copy)]
struct RawPlot {
    index: usize,
    stride: usize,
    detail: bool,
}

impl SimResults {
    fn raw_layout(&self) -> Vec<RawPlot> {
        let mut intervals = self.batch_raw.iter().flat_map(|b| &b.noise_intervals);
        // A single-frequency noise card has no integrated plot, so the totals
        // take the interval of the spectrum that precedes them.
        let mut last_interval = None;
        let mut ac = self.batch_raw.iter().flat_map(|b| &b.ac_start_only);
        let mut layout: Vec<_> = self
            .analyses
            .iter()
            .enumerate()
            .map(|(index, a)| {
                let interval = match a.kind {
                    AnalysisKind::Noise => {
                        last_interval = intervals.next();
                        last_interval
                    }
                    AnalysisKind::IntegratedNoise => last_interval,
                    _ => None,
                }
                .copied()
                .unwrap_or(0);
                let stride = if a.kind == AnalysisKind::Noise {
                    interval.max(1) as usize
                } else if a.kind == AnalysisKind::Ac && ac.next() == Some(&true) {
                    2
                } else {
                    1
                };
                RawPlot {
                    index,
                    stride,
                    detail: interval > 0,
                }
            })
            .collect();
        if self.batch_raw.is_some() {
            // ngspice 44.2 batch raw files list plots in a fixed analysis-type
            // order, independent of card order (R22e/R22f black-box probes).
            // Plots of one family keep their card order, so a noise spectrum
            // stays next to its integrated totals and PSS time next to PSS
            // frequency. The Fourier report is not written: with a rawfile
            // ngspice ignores `.four` and keeps only the transient.
            let rank = |p: &RawPlot| -> i32 {
                match self.analyses[p.index].kind {
                    AnalysisKind::Ac => 0,
                    AnalysisKind::Dc => 1,
                    AnalysisKind::Op => 2,
                    AnalysisKind::Tran => 3,
                    AnalysisKind::Pz => 4,
                    AnalysisKind::Tf => 5,
                    k if k.is_distortion() => 6,
                    AnalysisKind::Noise | AnalysisKind::IntegratedNoise => 7,
                    AnalysisKind::Sens => 8,
                    AnalysisKind::Pss | AnalysisKind::PssFreq => 9,
                    AnalysisKind::Sp => 10,
                    _ => 11,
                }
            };
            layout.retain(|p| self.analyses[p.index].kind != AnalysisKind::Four);
            // Within one family ngspice writes the cards in reverse order, each
            // card's plots kept together in their own order (R22g probes).
            let part = |kind: AnalysisKind| match kind {
                AnalysisKind::IntegratedNoise | AnalysisKind::PssFreq | AnalysisKind::Harmonic3 => {
                    1
                }
                AnalysisKind::Sum => 2,
                AnalysisKind::Difference => 3,
                AnalysisKind::Intermod3 => 4,
                _ => 0,
            };
            let mut card = 0usize;
            let mut previous: Option<(i32, i32)> = None;
            let mut keyed: Vec<_> = layout
                .iter()
                .map(|p| {
                    let kind = self.analyses[p.index].kind;
                    let (family, sub) = (rank(p), part(kind));
                    if previous.is_none_or(|(f, s)| f != family || sub <= s) {
                        card += 1;
                    }
                    previous = Some((family, sub));
                    ((family, std::cmp::Reverse(card), sub), *p)
                })
                .collect();
            keyed.sort_by_key(|&(key, _)| key);
            layout = keyed.into_iter().map(|(_, p)| p).collect();
        }
        layout
    }

    /// Consume a run as batch raw plots, moving waveform storage without copying samples.
    pub fn into_raw_output(mut self) -> Self {
        let layout = self.raw_layout();
        let mut plots: Vec<_> = self.analyses.drain(..).map(Some).collect();
        for RawPlot {
            index,
            stride,
            detail,
        } in layout
        {
            if let Some(mut a) = plots[index].take() {
                if let Some(batch) = &self.batch_raw {
                    let kind = a.kind;
                    a.retain_variables(|v| batch.visible(kind, &v.name, detail));
                }
                for (i, v) in a.variables.iter_mut().enumerate() {
                    let col = raw_column(self.batch_raw.as_ref(), a.kind, i, &v.name);
                    if col.zero
                        && let Data::Complex { columns } = &mut a.data
                        && let Some(column) = columns.get_mut(i)
                    {
                        column.fill((0., 0.));
                    }
                    if let std::borrow::Cow::Owned(name) = col.name {
                        v.name = name;
                    }
                }
                if stride > 1 {
                    retain_stride(&mut a.scale_values, stride);
                    match &mut a.data {
                        Data::Real { columns } => {
                            columns.iter_mut().for_each(|c| retain_stride(c, stride))
                        }
                        Data::Complex { columns } => {
                            columns.iter_mut().for_each(|c| retain_stride(c, stride))
                        }
                    }
                }
                self.analyses.push(a);
            }
        }
        self.batch_raw = None;
        self
    }
}

fn retain_stride<T>(values: &mut Vec<T>, stride: usize) {
    for (dst, src) in (0..values.len()).step_by(stride).enumerate() {
        values.swap(dst, src);
    }
    values.truncate(values.len().div_ceil(stride));
}

/// Write results as an ngspice-compatible ASCII raw file (one plot per analysis).
pub fn write_raw_ascii(results: &SimResults, w: &mut impl Write) -> io::Result<()> {
    write_raw(results, w, false)
}

/// Write point-major native double precision records after each plot header.
pub fn write_raw_binary(results: &SimResults, w: &mut impl Write) -> io::Result<()> {
    write_raw(results, w, true)
}

fn write_raw(results: &SimResults, w: &mut impl Write, binary: bool) -> io::Result<()> {
    for RawPlot {
        index,
        stride,
        detail,
    } in results.raw_layout()
    {
        let a = &results.analyses[index];
        let complex = matches!(a.data, Data::Complex { .. });
        let batch = results.batch_raw.as_ref();
        let selected: Vec<_> = a
            .variables
            .iter()
            .enumerate()
            .filter(|(_, v)| batch.is_none_or(|b| b.visible(a.kind, &v.name, detail)))
            .map(|(i, v)| raw_column(batch, a.kind, i, &v.name))
            .collect();
        let vars = a.scale.iter().map(|v| (v.name.as_str(), v.quantity)).chain(
            selected
                .iter()
                .map(|c| (c.name.as_ref(), a.variables[c.index].quantity)),
        );
        let npoints = a.len();

        let valid = match &a.data {
            Data::Real { columns } => {
                columns.len() == a.variables.len() && columns.iter().all(|c| c.len() == npoints)
            }
            Data::Complex { columns } => {
                columns.len() == a.variables.len() && columns.iter().all(|c| c.len() == npoints)
            }
        };
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "raw plot has inconsistent column dimensions",
            ));
        }
        writeln!(w, "Title: {}", results.title)?;
        writeln!(w, "Date: unknown")?;
        writeln!(w, "Plotname: {}", a.kind.plot_name())?;
        writeln!(w, "Flags: {}", if complex { "complex" } else { "real" })?;
        writeln!(
            w,
            "No. Variables: {}",
            selected.len() + usize::from(a.scale.is_some())
        )?;
        writeln!(w, "No. Points: {}", npoints.div_ceil(stride))?;
        writeln!(w, "Variables:")?;
        for (i, (name, quantity)) in vars.enumerate() {
            if i == 0 && a.kind == AnalysisKind::Dc && name == "v-sweep" {
                writeln!(w, "\t{i}\tv(v-sweep)\tvoltage")?;
                continue;
            }
            if batch.is_some()
                && !(i == 0 && a.scale.is_some())
                && let Some(name) = special_raw_name(a.kind, name)
            {
                writeln!(w, "\t{i}\tv({name})\tvoltage")?;
                continue;
            }
            let wrapped = name.starts_with('@')
                || a.kind == AnalysisKind::Sens
                || (a.kind == AnalysisKind::IntegratedNoise && integrated_noise_name(name));
            match (wrapped, quantity) {
                (true, Quantity::Current) => writeln!(w, "\t{i}\ti({})\tcurrent", name)?,
                (true, Quantity::Voltage) => writeln!(w, "\t{i}\tv({})\tvoltage", name)?,
                _ => writeln!(w, "\t{i}\t{}\t{}", name, quantity.raw_name())?,
            }
        }
        if binary {
            writeln!(w, "Binary:")?;
            for p in (0..npoints).step_by(stride) {
                if a.scale.is_some() {
                    w.write_all(&a.scale_values[p].to_ne_bytes())?;
                    if complex {
                        w.write_all(&0f64.to_ne_bytes())?;
                    }
                }
                match &a.data {
                    Data::Real { columns } => {
                        for col in &selected {
                            let c = &columns[col.index];
                            w.write_all(&c[p].to_ne_bytes())?;
                        }
                    }
                    Data::Complex { columns } => {
                        for col in &selected {
                            let c = &columns[col.index];
                            let (re, im) = col.complex(c[p]);
                            w.write_all(&re.to_ne_bytes())?;
                            w.write_all(&im.to_ne_bytes())?;
                        }
                    }
                }
            }
            continue;
        }
        writeln!(w, "Values:")?;
        for (row, p) in (0..npoints).step_by(stride).enumerate() {
            let mut first = true;
            let mut emit = |w: &mut dyn Write, s: String| -> io::Result<()> {
                if first {
                    first = false;
                    writeln!(w, " {row}\t{s}")
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
                    for col in &selected {
                        let c = &columns[col.index];
                        emit(w, format!("{:.15e}", c[p]))?;
                    }
                }
                Data::Complex { columns } => {
                    for col in &selected {
                        let c = &columns[col.index];
                        let (re, im) = col.complex(c[p]);
                        emit(w, format!("{re:.15e},{im:.15e}"))?;
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
            batch_raw: Default::default(),
            prints: Vec::new(),
            measures: vec![],
            title: "t".into(),
            analyses: vec![AnalysisResult {
                events: Default::default(),
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
        let mut binary = Vec::new();
        let mut plots = r.clone();
        let mut ac = r.analyses[0].clone();
        ac.kind = AnalysisKind::Ac;
        ac.data = Data::Complex {
            columns: vec![vec![(1., -2.), (3., -4.)]],
        };
        plots.analyses.push(ac);
        write_raw_binary(&plots, &mut binary).unwrap();
        let marker = b"Binary:\n";
        let mut rest = binary.as_slice();
        for expected in [
            vec![0., 0., 1e-9, 0.5],
            vec![0., 0., 1., -2., 1e-9, 0., 3., -4.],
        ] {
            let offset = rest
                .windows(marker.len())
                .position(|v| v == marker)
                .unwrap()
                + marker.len();
            let bytes: Vec<_> = expected.into_iter().flat_map(f64::to_ne_bytes).collect();
            assert_eq!(&rest[offset..offset + bytes.len()], bytes);
            rest = &rest[offset + bytes.len()..];
        }
        assert!(rest.is_empty());
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("No. Variables: 2\nNo. Points: 2\n"));
        assert!(s.contains("\t1\tv(out)\tvoltage\n"));
        assert!(s.contains(" 1\t1.000000000000000e-9\n\t5.000000000000000e-1\n"));
        assert_eq!(r.analyses[0].real("V(OUT)"), Some(&[0.0, 0.5][..]));
    }
}
