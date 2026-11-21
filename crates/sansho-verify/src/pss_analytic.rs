//! Independent phasor, final-period transient and limit-cycle checks for the R19 decks.
//! Provenance: Kirchhoff's laws, sinusoidal steady state and the Van der Pol equation.

use crate::{DeckReport, Problem};
use sansho_output::{AnalysisKind, SimResults};
use sansho_verify::MeasReport;
use std::f64::consts::TAU;

pub(super) fn check(case: &str, results: &SimResults) -> Result<DeckReport, Problem> {
    let plot = |kind| {
        results
            .analyses
            .iter()
            .find(|p| p.kind == kind)
            .ok_or_else(|| Problem::Failed(format!("missing {kind:?} plot")))
    };
    let time = plot(AnalysisKind::Pss)?;
    let freq = plot(AnalysisKind::PssFreq)?;
    let t = &time.scale_values;
    let v = time
        .real("v(out)")
        .ok_or_else(|| Problem::Failed("missing time-domain v(out)".into()))?;
    let z = freq
        .complex("v(out)")
        .ok_or_else(|| Problem::Failed("missing harmonic v(out)".into()))?;
    let malformed = || Problem::Failed("invalid PSS grid, dimensions or nonfinite results".into());
    let &(re, im) = z.first().ok_or_else(malformed)?;
    let &f = freq.scale_values.first().ok_or_else(malformed)?;
    let (points, harmonics) = match case {
        "rc" | "rlc" => (128, 5),
        "rectifier" => (256, 12),
        "lc" | "ring" => (256, 10),
        "vdp" => (256, 8),
        _ => {
            return Err(Problem::Failed(format!(
                "unknown PSS analytic circuit {case}"
            )));
        }
    };
    if t.len() != points
        || v.len() != points
        || z.len() != harmonics
        || freq.len() != harmonics
        || !f.is_finite()
        || f <= 0.
        || t.iter()
            .enumerate()
            .any(|(k, t)| !t.is_finite() || (t * f - k as f64 / points as f64).abs() > 1e-12)
        || freq
            .scale_values
            .iter()
            .enumerate()
            .any(|(k, s)| !s.is_finite() || (s / f - (k + 1) as f64).abs() > 1e-12)
        || v.iter().any(|s| !s.is_finite())
        || z.iter().any(|(r, i)| !r.is_finite() || !i.is_finite())
    {
        return Err(malformed());
    }
    let mut measures = vec![];
    let mut metric = |name: &str, actual: f64, reference: f64, tolerance: f64| {
        measures.push(MeasReport {
            name: name.into(),
            ours: Some(actual),
            reference,
            score: (actual - reference).abs() / tolerance,
        });
    };
    let amplitude = |v: &[f64]| {
        (v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - v.iter().copied().fold(f64::INFINITY, f64::min))
            / 2.
    };
    match case {
        "rc" | "rlc" => {
            // Vout = -j / (a + jb), using a cosine reference for the sine source.
            let w = TAU * 1000.;
            let (a, b) = if case == "rc" {
                (1., w * 1000. * 220e-9)
            } else {
                (1. - w * w * 0.01 * 2e-6, w * 30. * 2e-6)
            };
            let reference = (-b / (a * a + b * b), -a / (a * a + b * b));
            metric("frequency (Hz)", f, 1000., 1e-9);
            metric(
                "complex phasor error (V)",
                (re - reference.0).hypot(im - reference.1),
                0.,
                1e-4,
            );
            let worst = v
                .iter()
                .zip(t)
                .map(|(&s, &t)| {
                    let (sin, cos) = (w * t).sin_cos();
                    (s - reference.0 * cos + reference.1 * sin).abs()
                })
                .fold(0., f64::max);
            metric("maximum waveform error (V)", worst, 0., 1e-4);
            metric(
                "largest spurious harmonic (V)",
                z[1..].iter().map(|(r, i)| r.hypot(*i)).fold(0., f64::max),
                0.,
                1e-9,
            );
        }
        "vdp" => metric("limit-cycle amplitude (V)", amplitude(v), 2., 0.02),
        _ => {
            let tran = plot(AnalysisKind::Tran)?;
            let times = &tran.scale_values;
            let wave = tran
                .real("v(out)")
                .filter(|v| v.len() == times.len() && v.len() >= 3)
                .ok_or_else(|| Problem::Failed("missing comparison transient".into()))?;
            if times.iter().chain(wave).any(|v| !v.is_finite())
                || times.windows(2).any(|t| t[1] <= t[0])
            {
                return Err(Problem::Failed("invalid long-transient waveform".into()));
            }
            if case == "rectifier" {
                let end = times[times.len() - 1];
                if times[0] > end - 1e-3 {
                    return Err(Problem::Failed(
                        "reference transient does not cover the final period".into(),
                    ));
                }
                let mut worst = 0.0_f64;
                for (&local, &s) in t.iter().zip(v) {
                    let at = end - 1e-3 + local;
                    let j = times.partition_point(|&t| t < at).clamp(1, times.len() - 1);
                    let fraction = (at - times[j - 1]) / (times[j] - times[j - 1]);
                    let estimate = wave[j - 1] + fraction * (wave[j] - wave[j - 1]);
                    worst = worst.max((s - estimate).abs());
                }
                metric("final-period maximum difference (V)", worst, 0., 1e-3);
            } else {
                let level = if case == "ring" { 1. } else { 0. };
                let mut crossings = wave
                    .windows(2)
                    .enumerate()
                    .rev()
                    .filter(|(_, w)| w[0] < level && w[1] >= level)
                    .map(|(i, w)| {
                        times[i] + (level - w[0]) * (times[i + 1] - times[i]) / (w[1] - w[0])
                    });
                let missing =
                    || Problem::Failed("fewer than two rising transient crossings".into());
                let last = crossings.next().ok_or_else(missing)?;
                let previous = crossings.next().ok_or_else(missing)?;
                let reference = 1. / (last - previous);
                metric("frequency (Hz)", f, reference, reference * 5e-4);
                let from = times.partition_point(|&t| t < previous);
                let to = times.partition_point(|&t| t < last);
                metric(
                    "half peak-to-peak amplitude (V)",
                    amplitude(v),
                    amplitude(&wave[from..to]),
                    1e-3,
                );
            }
        }
    }
    Ok(DeckReport {
        vars: vec![],
        problems: vec![],
        measures,
    })
}
