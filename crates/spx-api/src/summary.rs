//! Summary-first result views: compact, unit-annotated and reproducible.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use serde::Serialize;
use spx_diag::Diagnostic;
use spx_engine::{Circuit, SimOptions};
use spx_netlist::{AnalysisCard, Netlist};
use spx_output::{AnalysisKind, AnalysisResult, Data, Measurement, SimResults};

use crate::hash;

/// Outcome of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// All analyses completed.
    Ok,
    /// An analysis failed; see the diagnostics (completed analyses are kept).
    Error,
}

/// A number with its unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Value {
    /// Value in SI units.
    pub value: f64,
    /// Unit symbol.
    pub unit: &'static str,
}

/// One operating-point value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpValue {
    /// Variable name (`v(out)`, `i(v1)`).
    pub name: String,
    /// Value in SI units.
    pub value: f64,
    /// Unit symbol.
    pub unit: &'static str,
}

/// Statistics of one signal over an analysis.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalStats {
    /// Variable name.
    pub name: String,
    /// Unit of `min`, `max`, `first`, `last` (`dB` for AC signals).
    pub unit: &'static str,
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// Value at the first point.
    pub first: f64,
    /// Value at the last point.
    pub last: f64,
}

/// Summary of one analysis.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisSummary {
    /// Index within the job (for `get_waveform`).
    pub index: usize,
    /// Analysis kind.
    pub kind: AnalysisKind,
    /// Number of stored points.
    pub points: usize,
    /// Sweep axis name, unit and range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sweep: Option<SweepAxis>,
    /// Largest device contributions to integrated output noise, in RMS volts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub noise_contributors: Vec<OpValue>,
    /// Operating-point values (`.op` only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<OpValue>,
    /// Per-signal statistics (sweeps only), at most `max_signals` entries.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalStats>,
    /// Total number of variables (more may exist than are listed).
    pub variables: usize,
    /// Whether `values` / `signals` were truncated.
    pub truncated: bool,
}

/// Sweep axis of an analysis.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SweepAxis {
    /// Name (`time`, `frequency`, `v-sweep`).
    pub name: String,
    /// Unit.
    pub unit: &'static str,
    /// First value.
    pub from: f64,
    /// Last value.
    pub to: f64,
}

/// Summary of a job with reproducibility metadata.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobSummary {
    /// Job identifier.
    pub job_id: String,
    /// Circuit identifier.
    pub circuit_id: String,
    /// Outcome.
    pub status: Status,
    /// Hash of the canonical deck that was simulated.
    pub input_hash: String,
    /// Bit-exact hash of all numeric results (same input ⇒ same hash, for any thread count).
    pub result_hash: String,
    /// Engine version.
    pub engine_version: &'static str,
    /// Effective simulation options.
    pub options: SimOptions,
    /// Wall-clock time in milliseconds (not part of any hash).
    pub elapsed_ms: f64,
    /// Analyses that completed.
    pub analyses: Vec<AnalysisSummary>,
    /// Measurements.
    pub measures: Vec<Measurement>,
    /// Errors first, then warnings.
    pub diagnostics: Vec<Diagnostic>,
}

/// Device count per type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceCount {
    /// Device type.
    pub kind: &'static str,
    /// Number of instances.
    pub count: usize,
}

/// Static description of a circuit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CircuitStats {
    /// Title line.
    pub title: String,
    /// Number of nodes (excluding ground and internal device nodes).
    pub nodes: usize,
    /// Number of unknowns (node voltages, internal nodes, branch currents).
    pub unknowns: usize,
    /// Structurally non-zero matrix entries.
    pub nnz: usize,
    /// Device instances per type.
    pub devices: Vec<DeviceCount>,
    /// Whether all devices are linear.
    pub linear: bool,
    /// Analysis cards, canonical form.
    pub analyses: Vec<String>,
    /// Measurement cards, canonical form.
    pub measures: Vec<String>,
    /// Names of all node voltages and branch currents.
    pub signals: Vec<String>,
}

/// Rough a-priori cost of a simulation. Times are order-of-magnitude estimates.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostEstimate {
    /// Circuit statistics.
    pub stats: CircuitStats,
    /// Estimated number of stored points over all analyses.
    pub points: u64,
    /// Estimated memory for the stored results.
    pub result_bytes: Value,
    /// Estimated run time (order of magnitude only).
    pub time: Value,
    /// Notes on how the estimate was made.
    pub notes: Vec<String>,
}

pub(crate) fn stats(nl: &Netlist, c: &Circuit) -> CircuitStats {
    CircuitStats {
        title: nl.title.clone(),
        nodes: c.num_nodes(),
        unknowns: c.dim(),
        nnz: c.nnz(),
        devices: c
            .device_counts()
            .into_iter()
            .map(|(kind, count)| DeviceCount { kind, count })
            .collect(),
        linear: c.is_linear(),
        analyses: nl
            .analyses
            .iter()
            .map(spx_netlist::write_analysis)
            .collect(),
        measures: nl.measures.iter().map(spx_netlist::write_measure).collect(),
        signals: c.unknowns().names().to_vec(),
    }
}

/// Estimated stored points and Newton iterations of one analysis.
fn analysis_work(a: &AnalysisCard) -> (u64, u64) {
    match a {
        AnalysisCard::Op { .. }
        | AnalysisCard::Tf { .. }
        | AnalysisCard::Sens { ac: None, .. }
        | AnalysisCard::Pz { .. } => (1, 20),
        AnalysisCard::Four { .. } => (10, 0),
        AnalysisCard::Dc {
            start, stop, step, ..
        } => {
            let n = ((stop - start) / step).abs().floor() as u64 + 1;
            (n, 4 * n)
        }
        AnalysisCard::Sens {
            ac:
                Some(spx_netlist::FrequencySweep {
                    sweep,
                    points,
                    fstart,
                    fstop,
                }),
            ..
        }
        | AnalysisCard::Noise {
            sweep,
            points,
            fstart,
            fstop,
            ..
        }
        | AnalysisCard::Ac {
            sweep,
            points,
            fstart,
            fstop,
            ..
        } => {
            let n = match sweep {
                spx_netlist::AcSweep::Lin => u64::from(*points),
                spx_netlist::AcSweep::Dec => {
                    ((fstop.log10() - fstart.log10()) * f64::from(*points)).round() as u64 + 1
                }
                spx_netlist::AcSweep::Oct => {
                    ((fstop.log2() - fstart.log2()) * f64::from(*points)).round() as u64 + 1
                }
            };
            (n, n + 20)
        }
        AnalysisCard::Tran { tstep, tstop, .. } => {
            // Adaptive stepping typically stores a few points per print step.
            let n = (tstop / tstep).max(1.0) as u64 * 2;
            (n, 3 * n + 20)
        }
    }
}

pub(crate) fn estimate(nl: &Netlist, c: &Circuit) -> CostEstimate {
    let st = stats(nl, c);
    let (mut points, mut iters) = (0u64, 0u64);
    for a in &nl.analyses {
        let (p, i) = analysis_work(a);
        points += p;
        iters += i;
    }
    let devices: usize = st.devices.iter().map(|d| d.count).sum();
    // Calibrated on a MOS1 ring oscillator: ≈ 0.1 µs per device and ≈ 0.05 µs per matrix
    // entry per Newton iteration on one core.
    let per_iter = 1e-7 * devices as f64 + 5e-8 * st.nnz as f64;
    let bytes = points as f64 * (st.unknowns as f64 + 1.0) * 8.0;
    CostEstimate {
        stats: st,
        points,
        result_bytes: Value {
            value: bytes,
            unit: "B",
        },
        time: Value {
            value: per_iter * iters as f64,
            unit: "s",
        },
        notes: vec![
            "transient point counts depend on circuit activity; the estimate assumes about two \
             stored points per print step"
                .into(),
            "time is an order-of-magnitude single-core estimate".into(),
        ],
    }
}

/// Fixed metadata of a job.
pub(crate) struct JobMeta {
    pub job_id: String,
    pub circuit_id: String,
    pub status: Status,
    pub input_hash: String,
    pub engine_version: &'static str,
    pub options: SimOptions,
    pub elapsed_ms: f64,
}

fn summarize(index: usize, a: &AnalysisResult, max_signals: usize) -> AnalysisSummary {
    let n = a.variables.len();
    let sweep = a.scale.as_ref().map(|v| SweepAxis {
        name: v.name.clone(),
        unit: v.quantity.unit(),
        from: a.scale_values.first().copied().unwrap_or(0.0),
        to: a.scale_values.last().copied().unwrap_or(0.0),
    });
    let mut s = AnalysisSummary {
        index,
        kind: a.kind,
        points: a.len(),
        sweep,
        noise_contributors: Vec::new(),
        values: Vec::new(),
        signals: Vec::new(),
        variables: n,
        truncated: false,
    };
    if a.kind == AnalysisKind::IntegratedNoise
        && let Data::Real { columns } = &a.data
    {
        for (v, col) in a.variables.iter().zip(columns) {
            if let Some(name) = v.name.strip_prefix("onoise_total.")
                && let Some(&value) = col.first()
            {
                s.noise_contributors.push(OpValue {
                    name: name.into(),
                    value,
                    unit: "V",
                });
            }
        }
        s.noise_contributors.sort_by(|a, b| {
            b.value
                .total_cmp(&a.value)
                .then_with(|| a.name.cmp(&b.name))
        });
        s.noise_contributors.truncate(max_signals.max(1));
    }
    let limit = if a.scale.is_none() {
        max_signals.max(1) * 4
    } else {
        max_signals.max(1)
    };
    s.truncated = n > limit;
    for (i, v) in a.variables.iter().enumerate().take(limit) {
        let unit = v.quantity.unit();
        match (&a.data, a.kind) {
            (Data::Real { columns }, _) if a.scale.is_none() => s.values.push(OpValue {
                name: v.name.clone(),
                value: columns[i].first().copied().unwrap_or(f64::NAN),
                unit,
            }),
            (Data::Real { columns }, _) => s.signals.push(stats_of(&v.name, unit, &columns[i])),
            (Data::Complex { columns }, AnalysisKind::Pz) => {
                if let Some(&(re, im)) = columns[i].first() {
                    s.values.push(OpValue {
                        name: format!("real({})", v.name),
                        value: re,
                        unit,
                    });
                    s.values.push(OpValue {
                        name: format!("imag({})", v.name),
                        value: im,
                        unit,
                    });
                }
            }
            (Data::Complex { columns }, AnalysisKind::Sens) => {
                let magnitude: Vec<_> = columns[i].iter().map(|(re, im)| re.hypot(*im)).collect();
                s.signals.push(stats_of(&v.name, unit, &magnitude));
            }
            (Data::Complex { columns }, _) => {
                let db: Vec<f64> = columns[i]
                    .iter()
                    .map(|&(re, im)| 20.0 * re.hypot(im).log10())
                    .collect();
                s.signals.push(stats_of(&v.name, "dB", &db));
            }
        }
    }
    s
}

fn stats_of(name: &str, unit: &'static str, c: &[f64]) -> SignalStats {
    let (min, max) = c
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    SignalStats {
        name: name.to_string(),
        unit,
        min,
        max,
        first: c.first().copied().unwrap_or(f64::NAN),
        last: c.last().copied().unwrap_or(f64::NAN),
    }
}

pub(crate) fn job(
    meta: JobMeta,
    results: Option<&SimResults>,
    diagnostics: Vec<Diagnostic>,
    max_signals: usize,
) -> JobSummary {
    let (analyses, measures, result_hash) = match results {
        Some(r) => (
            r.analyses
                .iter()
                .enumerate()
                .map(|(i, a)| summarize(i, a, max_signals))
                .collect(),
            r.measures.clone(),
            hash::results(r),
        ),
        None => (Vec::new(), Vec::new(), String::new()),
    };
    JobSummary {
        job_id: meta.job_id,
        circuit_id: meta.circuit_id,
        status: meta.status,
        input_hash: meta.input_hash,
        result_hash,
        engine_version: meta.engine_version,
        options: meta.options,
        elapsed_ms: meta.elapsed_ms,
        analyses,
        measures,
        diagnostics,
    }
}
