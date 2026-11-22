//! Compare autonomous oscillators on a common phase grid.
//! Provenance: original discrete Fourier and periodic interpolation checks on
//! black-box ngspice 44.2 waveforms from Sansho-authored circuits.

use crate::{Cli, DeckReport, Problem, read_reference, reference};
use sansho_netlist::PssSpec;
use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity, SimResults};
use sansho_verify::MeasReport;
use std::{
    f64::consts::TAU,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Orbit<'a> {
    time: &'a AnalysisResult,
    waves: &'a [Vec<f64>],
    bins: &'a [Vec<f64>],
    frequency: f64,
    intervals: usize,
}

impl<'a> Orbit<'a> {
    fn read(results: &'a SimResults, spec: &PssSpec) -> Result<Self, Problem> {
        let invalid =
            || Problem::Failed("invalid autonomous PSS plots, vectors or sample grid".into());
        let [time, freq] = results.analyses.as_slice() else {
            return Err(invalid());
        };
        let (Data::Real { columns: waves }, Data::Real { columns: bins }) =
            (&time.data, &freq.data)
        else {
            return Err(invalid());
        };
        let n = spec.points.max(2 * spec.harmonics + 1);
        if time.kind != AnalysisKind::Pss
            || freq.kind != AnalysisKind::PssFreq
            || time.scale.as_ref().map(|v| v.quantity) != Some(Quantity::Time)
            || freq.scale.as_ref().map(|v| v.quantity) != Some(Quantity::Frequency)
            || time.len() != n + 1
            || freq.len() != spec.harmonics
            || time.variables != freq.variables
            || waves.len() != time.variables.len()
            || bins.len() != waves.len()
            || waves
                .iter()
                .any(|v| v.len() != n + 1 || v.iter().any(|x| !x.is_finite()))
            || bins.iter().any(|v| {
                v.len() != spec.harmonics
                    || v.iter()
                        .enumerate()
                        .any(|(h, x)| !x.is_finite() || (h > 0 && *x < 0.))
            })
        {
            return Err(invalid());
        }
        let t = &time.scale_values;
        let period = t[n] - t[0];
        let frequency = freq
            .scale_values
            .get(1)
            .copied()
            .unwrap_or_else(|| period.recip());
        // Repeated addition and decimal raw serialization round absolute times.
        let grid_tol = 32. * n as f64 * f64::EPSILON * t[n].abs();
        if !frequency.is_finite()
            || frequency <= 0.
            || t[0] <= 0.
            || t.windows(2).any(|w| w[1] <= w[0])
            || t.iter().enumerate().any(|(k, x)| {
                !x.is_finite() || ((x - t[0]) - k as f64 / n as f64 / frequency).abs() > grid_tol
            })
            || freq
                .scale_values
                .iter()
                .enumerate()
                .any(|(h, x)| !x.is_finite() || (x / frequency - h as f64).abs() > 1e-11)
        {
            return Err(invalid());
        }
        Ok(Self {
            time,
            waves,
            bins,
            frequency,
            intervals: n,
        })
    }

    fn phase(&self, signal: &str) -> Result<f64, Problem> {
        let wave = self
            .time
            .real(signal)
            .ok_or_else(|| Problem::Failed(format!("missing phase monitor {signal}")))?;
        let (mut re, mut im) = (0., 0.);
        for (k, v) in wave[..self.intervals].iter().enumerate() {
            let (s, c) = (-TAU * k as f64 / self.intervals as f64).sin_cos();
            re += v * c;
            im += v * s;
        }
        if re.hypot(im) <= f64::EPSILON * wave.iter().map(|v| v.abs()).sum::<f64>() {
            return Err(Problem::Failed(format!(
                "unresolved fundamental at {signal}"
            )));
        }
        Ok(im.atan2(re))
    }
}

fn compare(
    ours: &Orbit<'_>,
    oracle: &Orbit<'_>,
    signal: &str,
    coeff: f64,
) -> Result<DeckReport, Problem> {
    if ours.time.variables != oracle.time.variables {
        return Err(Problem::Failed(format!(
            "PSS vector mismatch: {:?} vs {:?}",
            ours.time.variables, oracle.time.variables
        )));
    }
    let mut measures = Vec::new();
    let mut metric = |name: String, x: f64, y: f64, budget: f64| {
        measures.push(MeasReport {
            name,
            ours: Some(x),
            reference: y,
            score: (x - y).abs() / budget,
        })
    };
    // A fractional period error corresponds to 2*pi times as much phase error.
    metric(
        "fundamental (Hz)".into(),
        ours.frequency,
        oracle.frequency,
        oracle.frequency * coeff / TAU,
    );
    let n = oracle.intervals;
    let shift = (oracle.phase(signal)? - ours.phase(signal)?) * n as f64 / TAU;
    for (i, var) in oracle.time.variables.iter().enumerate() {
        let wave = &oracle.waves[i];
        let ours_wave = &ours.waves[i];
        let peak = wave.iter().map(|v| v.abs()).fold(0., f64::max);
        let floor = if var.quantity == Quantity::Current {
            1e-12
        } else {
            1e-6
        };
        // Closure uncertainty leaks full-scale signal into even very small bins.
        let budget = peak * coeff + floor;
        for h in (0..oracle.bins[i].len()).filter(|h| *h == 0 || h % 2 == 1) {
            metric(
                format!("{} H{h}", var.name),
                ours.bins[i][h],
                oracle.bins[i][h],
                budget,
            );
        }
        let mut square = 0.;
        let mut curvature = 0.0_f64;
        for k in 0..n {
            let u = (k as f64 + shift).rem_euclid(n as f64);
            let j = u.floor() as usize % n;
            let aligned = ours_wave[j] + u.fract() * (ours_wave[(j + 1) % n] - ours_wave[j]);
            square += (aligned - wave[k]).powi(2);
            curvature =
                curvature.max((wave[(k + 1) % n] - 2. * wave[k] + wave[(k + n - 1) % n]).abs());
        }
        // Linear interpolation on one interval contributes at most max|y''|*dt^2/8.
        metric(
            format!("{} aligned RMS", var.name),
            (square / n as f64).sqrt(),
            0.,
            budget + curvature / 8.,
        );
    }
    Ok(DeckReport {
        vars: vec![],
        problems: vec![],
        measures,
    })
}

/// Staged oracle output. Any return before publication discards this run's file.
struct PendingFile(PathBuf);

impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn verify(
    cli: &Cli,
    deck: &Path,
    stem: &str,
    ours: &SimResults,
    spec: &PssSpec,
    signal: &str,
) -> Result<(DeckReport, Option<String>, bool), Problem> {
    let raw = cli.out.join(format!("{stem}.ref.raw"));
    let log = cli.out.join(format!("{stem}.ref.log"));
    if cli.cached {
        let reference = read_reference(&raw)?;
        let transcript = fs::read_to_string(&log)
            .map_err(|e| Problem::Failed(format!("cannot read {}: {e}", log.display())))?;
        return check(ours, &reference, &transcript, spec, signal);
    }

    let pid = std::process::id();
    let pending = PendingFile(raw.with_extension(format!("{pid}.raw")));
    fs::File::create(&pending.0)?;
    let out = Command::new(reference::WRAPPER)
        .env("SANSHO_NGSPICE_IMAGE", "sansho-ngspice-pss:latest")
        .env("SANSHO_VERIFY_OUT", &cli.out)
        .arg("-b")
        .arg(deck)
        .arg("-r")
        .arg(&pending.0)
        .output()?;
    let transcript = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        return Err(Problem::Failed(format!(
            "PSS oracle {}: {transcript}",
            out.status
        )));
    }
    let report = check(
        ours,
        &read_reference(&pending.0)?,
        &transcript,
        spec,
        signal,
    )?;
    let tmp_log = PendingFile(log.with_extension(format!("{pid}.log")));
    fs::write(&tmp_log.0, transcript)?;
    fs::rename(&tmp_log.0, log)?;
    fs::rename(&pending.0, raw)?;
    Ok(report)
}

fn check(
    ours: &SimResults,
    reference: &SimResults,
    transcript: &str,
    spec: &PssSpec,
    signal: &str,
) -> Result<(DeckReport, Option<String>, bool), Problem> {
    let oracle = Orbit::read(reference, spec)?;
    let ours = Orbit::read(ours, spec)?;
    let line = transcript
        .lines()
        .find(|l| {
            l.starts_with("Convergence reached.") || l.starts_with("Convergence not reached.")
        })
        .ok_or_else(|| Problem::Failed("missing PSS oracle convergence record".into()))?;
    let stated = line
        .split_whitespace()
        .rev()
        .nth(1)
        .and_then(|x| x.parse::<f64>().ok());
    if stated.is_none_or(|f| !f.is_finite() || (f / oracle.frequency - 1.).abs() > 1e-8) {
        return Err(Problem::Failed(
            "PSS oracle log and raw frequencies disagree".into(),
        ));
    }
    let final_clock = transcript.lines().rev().find_map(|line| {
        let (_, clock) = line.split_once(" and current time is ")?;
        clock.trim_end_matches('.').parse::<f64>().ok()
    });
    if final_clock
        .is_none_or(|t| !t.is_finite() || (t / oracle.time.scale_values[0] - 1.).abs() > 1e-8)
    {
        return Err(Problem::Failed(
            "PSS reference does not start at the final shooting clock".into(),
        ));
    }
    let report = compare(&ours, &oracle, signal, spec.steadycoeff)?;
    let known = line.starts_with("Convergence not reached.").then(|| {
        format!(
            "{line} Sansho {:.9} Hz, relative difference {:+.3e}.",
            ours.frequency,
            ours.frequency / oracle.frequency - 1.
        )
    });
    Ok((report, known, false))
}
