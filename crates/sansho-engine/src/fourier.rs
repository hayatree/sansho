//! Fourier postprocessing of the last recorded transient period.
//! Provenance: original real-signal DFT and linear interpolation from standard Fourier analysis.

use super::small_signal::{error, var};
use sansho_diag::Diagnostic;
use sansho_netlist::Probe;
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity};
use std::f64::consts::TAU;

/// Compute harmonics 0 through 9 of an existing real transient plot.
pub fn fourier(
    tran: &AnalysisResult,
    frequency: f64,
    outputs: &[Probe],
) -> Result<AnalysisResult, Diagnostic> {
    if tran.kind != AnalysisKind::Tran || !frequency.is_finite() || frequency <= 0. {
        return Err(error(
            "Fourier analysis requires a transient and positive frequency",
        ));
    }
    let time = &tran.scale_values;
    let Some((&end, &begin)) = time.last().zip(time.first()) else {
        return Err(error("empty transient"));
    };
    if time.windows(2).any(|p| p[1] <= p[0]) || time.iter().any(|t| !t.is_finite()) {
        return Err(error("invalid transient time grid"));
    }
    let period = 1. / frequency;
    let start = end - period;
    if time.len() < 2 || start < begin - 8. * f64::EPSILON * end.abs() {
        return Err(error("transient must cover a complete Fourier period"));
    }
    const GRID: usize = 200;
    const HARMONICS: usize = 10;
    let mut variables = vec![];
    let mut columns = vec![];
    for probe in outputs {
        let column = |s: String| {
            tran.real(&s)
                .filter(|c| c.len() == time.len())
                .ok_or_else(|| error(format!("unknown Fourier output {s}")))
        };
        let voltage = |n: &str| {
            if n == "0" {
                Ok(None)
            } else {
                column(format!("v({n})")).map(Some)
            }
        };
        let (a, b, unit) = match probe {
            Probe::Voltage(p, n) => (voltage(p)?, voltage(n)?, Quantity::Voltage),
            Probe::Current(_) => (Some(column(probe.to_string())?), None, Quantity::Current),
        };
        let sample = |i| a.map_or(0., |v| v[i]) - b.map_or(0., |v| v[i]);
        let mut grid = [0.; GRID];
        for (i, v) in grid.iter_mut().enumerate() {
            let t = start + period * (i as f64 / GRID as f64);
            let j = time.partition_point(|&x| x < t).clamp(1, time.len() - 1);
            let w = ((t - time[j - 1]) / (time[j] - time[j - 1])).clamp(0., 1.);
            *v = sample(j - 1) + (sample(j) - sample(j - 1)) * w;
        }
        let mut magnitude = vec![];
        let mut phase = vec![];
        for h in 0..HARMONICS {
            let (mut sin, mut cos) = (0., 0.);
            for (i, v) in grid.iter().enumerate() {
                let (s, c) = (TAU * (h * i) as f64 / GRID as f64).sin_cos();
                sin += v * s;
                cos += v * c;
            }
            magnitude.push(if h == 0 {
                cos / GRID as f64
            } else {
                2. * sin.hypot(cos) / GRID as f64
            });
            phase.push(if h == 0 {
                0.
            } else {
                cos.atan2(sin).to_degrees()
            });
        }
        variables.push(var(format!("magnitude({probe})"), unit));
        let fundamental = magnitude[1];
        let peak = grid.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        if fundamental > 64. * f64::EPSILON * peak {
            let thd = 100. * magnitude[2..].iter().fold(0.0f64, |a, &v| a.hypot(v)) / fundamental;
            variables.extend([
                var(format!("phase({probe})"), Quantity::Phase),
                var(format!("thd({probe})"), Quantity::Percent),
            ]);
            columns.extend([magnitude, phase, vec![thd; HARMONICS]]);
        } else {
            variables.push(var(format!("phase({probe})"), Quantity::Phase));
            columns.extend([magnitude, phase]);
        }
    }
    Ok(AnalysisResult {
        events: Default::default(),
        kind: AnalysisKind::Four,
        scale: Some(var("frequency", Quantity::Frequency)),
        scale_values: (0..HARMONICS).map(|h| frequency * h as f64).collect(),
        variables,
        data: Data::Real { columns },
    })
}
