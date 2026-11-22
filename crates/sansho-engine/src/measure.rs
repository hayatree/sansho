//! Evaluation of `.meas` cards on analysis results.
//!
//! Crossings and interval boundaries are located by linear interpolation between stored
//! points; integrals use the trapezoidal rule. For AC results a plain `v(..)` / `i(..)` signal
//! is measured by its magnitude, and phases (`vp`, `ip`) are in radians as in ngspice.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use sansho_netlist::{
    CrossDir, GROUND, MeasAnalysis, MeasEvent, MeasKind, MeasStat, MeasureCard, Signal, SignalFn,
};
use sansho_output::{AnalysisKind, AnalysisResult, Data, Measurement, SimResults};

/// Build requested batch tables before application of .save.
pub(crate) fn print_tables(
    nl: &sansho_netlist::Netlist,
    results: &mut SimResults,
    warnings: &mut Vec<sansho_diag::Diagnostic>,
) {
    use sansho_diag::Diagnostic;
    for card in &nl.prints {
        let kind = match card.analysis.as_str() {
            "op" => AnalysisKind::Op,
            "dc" => AnalysisKind::Dc,
            "ac" => AnalysisKind::Ac,
            "tran" => AnalysisKind::Tran,
            "noise" => AnalysisKind::Noise,
            _ => {
                warnings.push(
                    Diagnostic::warning(
                        "W-PRINT",
                        format!("unknown print analysis {}", card.analysis),
                    )
                    .with_span(card.span),
                );
                continue;
            }
        };
        let mut matched = false;
        for a in results.analyses.iter().filter(|a| a.kind == kind) {
            matched = true;
            let mut headings = Vec::new();
            let mut columns = Vec::new();
            if let Some(scale) = &a.scale {
                headings.push(scale.name.clone());
                columns.push(a.scale_values.clone());
            }
            for s in &card.signals {
                if a.kind == AnalysisKind::Ac && s.func == SignalFn::Plain {
                    for (fun, component) in [(SignalFn::Re, "real"), (SignalFn::Im, "imag")] {
                        let signal = Signal {
                            func: fun,
                            ..s.clone()
                        };
                        match samples(&signal, a) {
                            Ok(values) => {
                                headings.push(format!("{} ({component})", s.expression()));
                                columns.push(values);
                            }
                            Err(e) => {
                                warnings
                                    .push(Diagnostic::warning("W-PRINT", e).with_span(card.span));
                                break;
                            }
                        }
                    }
                    continue;
                }
                match samples(s, a) {
                    Ok(values) => {
                        headings.push(s.expression());
                        columns.push(values);
                    }
                    Err(e) => warnings.push(Diagnostic::warning("W-PRINT", e).with_span(card.span)),
                }
            }
            results.prints.push(sansho_output::PrintTable {
                analysis: kind,
                headings,
                columns,
            });
        }
        if !matched {
            warnings.push(
                Diagnostic::warning(
                    "W-PRINT",
                    format!("no {} analysis for .print", card.analysis),
                )
                .with_span(card.span),
            );
        }
    }
}

/// Evaluate all measurements against the results.
pub(crate) fn evaluate(cards: &[MeasureCard], results: &SimResults) -> Vec<Measurement> {
    let mut prior = results
        .measures
        .iter()
        .filter_map(|m| Some((m.name.clone(), m.value?)))
        .collect();
    cards
        .iter()
        .map(|c| {
            let (label, kind) = match c.analysis {
                MeasAnalysis::Tran => ("tran", AnalysisKind::Tran),
                MeasAnalysis::Noise => ("noise", AnalysisKind::Noise),
                MeasAnalysis::Ac => ("ac", AnalysisKind::Ac),
                MeasAnalysis::Dc => ("dc", AnalysisKind::Dc),
            };
            let r = results
                .analyses
                .iter()
                .find(|a| a.kind == kind)
                .ok_or_else(|| format!("no {label} analysis was run"))
                .and_then(|a| measure(&c.kind, a, &prior))
                .and_then(|v| {
                    if v.is_finite() {
                        Ok(v)
                    } else {
                        Err("non-finite measurement result".into())
                    }
                });
            prior.remove(&c.name);
            if let Ok(v) = &r {
                prior.insert(c.name.clone(), *v);
            }
            let sweep_unit = match c.analysis {
                MeasAnalysis::Tran => "s",
                MeasAnalysis::Ac | MeasAnalysis::Noise => "Hz",
                MeasAnalysis::Dc => results
                    .analyses
                    .iter()
                    .find(|a| a.kind == kind)
                    .and_then(|a| a.scale.as_ref())
                    .map_or("V", |v| v.quantity.unit()),
            };
            Measurement {
                name: c.name.clone(),
                analysis: label.into(),
                value: r.as_ref().ok().copied(),
                unit: unit(
                    &c.kind,
                    sweep_unit,
                    results.analyses.iter().find(|a| a.kind == kind),
                ),
                error: r.err(),
            }
        })
        .collect()
}

/// Unit of a measurement result.
fn unit(kind: &MeasKind, sweep: &str, analysis: Option<&AnalysisResult>) -> String {
    let sig = |s: &Signal| -> &'static str {
        match s.func {
            SignalFn::Db => "dB",
            SignalFn::Phase => "rad",
            _ if s.is_vector() => analysis
                .and_then(|a| a.variables.iter().find(|v| v.name == s.pos))
                .map_or("", |v| v.quantity.unit()),
            _ if s.current => "A",
            _ => "V",
        }
    };
    match kind {
        MeasKind::TrigTarg { .. } | MeasKind::When { .. } => sweep.into(),
        MeasKind::Find { signal, .. } => sig(signal).into(),
        MeasKind::Deriv { signal, .. } => format!("{}/{sweep}", sig(signal)),
        MeasKind::Param { .. } => String::new(),
        MeasKind::Stat {
            stat: MeasStat::MinAt | MeasStat::MaxAt,
            ..
        } => sweep.into(),
        MeasKind::Stat {
            stat: MeasStat::Integ,
            signal,
            ..
        } => format!("{}·{sweep}", sig(signal)),
        MeasKind::Stat { signal, .. } => sig(signal).into(),
    }
}

fn signal_name(s: &Signal) -> String {
    if s.is_vector() {
        return s.pos.clone();
    }
    let f = if s.current { "i" } else { "v" };
    match &s.neg {
        Some(n) => format!("{f}({},{n})", s.pos),
        None => format!("{f}({})", s.pos),
    }
}

/// Real-valued samples of a signal.
fn samples(s: &Signal, a: &AnalysisResult) -> Result<Vec<f64>, String> {
    let lookup = |node: &str| -> Result<Option<usize>, String> {
        if !s.current && node == GROUND {
            return Ok(None);
        }
        let name = if s.is_vector() {
            node.to_owned()
        } else if s.current {
            format!("i({node})")
        } else {
            format!("v({node})")
        };
        a.index_of(&name)
            .map(Some)
            .ok_or_else(|| format!("signal `{}` not found in the results", signal_name(s)))
    };
    let p = lookup(&s.pos)?;
    let n = match &s.neg {
        Some(n) => lookup(n)?,
        None => None,
    };
    let len = a.len();
    let reduce = |re: f64, im: f64, complex: bool| -> f64 {
        match s.func {
            SignalFn::Plain if complex => re.hypot(im),
            SignalFn::Plain | SignalFn::Re => re,
            SignalFn::Mag => re.hypot(im),
            SignalFn::Db => 20.0 * re.hypot(im).log10(),
            SignalFn::Phase => im.atan2(re),
            SignalFn::Im => im,
        }
    };
    Ok(match &a.data {
        Data::Real { columns } => (0..len)
            .map(|k| {
                let v = p.map_or(0.0, |i| columns[i][k]) - n.map_or(0.0, |i| columns[i][k]);
                reduce(v, 0.0, false)
            })
            .collect(),
        Data::Complex { columns } => (0..len)
            .map(|k| {
                let a = p.map_or((0.0, 0.0), |i| columns[i][k]);
                let b = n.map_or((0.0, 0.0), |i| columns[i][k]);
                reduce(a.0 - b.0, a.1 - b.1, true)
            })
            .collect(),
    })
}

/// Index of the sampled segment ending at or containing x, in traversal order.
fn segment(xs: &[f64], x: f64) -> Result<usize, String> {
    let (Some(&first), Some(&last)) = (xs.first(), xs.last()) else {
        return Err("no data points".into());
    };
    if x < first.min(last) || x > first.max(last) {
        return Err(format!(
            "{x:e} is outside the simulated range [{first:e}, {last:e}]"
        ));
    }
    if xs.len() == 1 {
        return Ok(0);
    }
    Ok(xs
        .partition_point(|&v| if first <= last { v < x } else { v > x })
        .clamp(1, xs.len() - 1))
}

fn at(xs: &[f64], ys: &[f64], x: f64) -> Result<f64, String> {
    let j = segment(xs, x)?;
    if j == 0 {
        return Ok(ys[0]);
    }
    let weight = (x - xs[j - 1]) / (xs[j] - xs[j - 1]);
    Ok(ys[j - 1] + weight * (ys[j] - ys[j - 1]))
}

fn event_x(e: &MeasEvent, a: &AnalysisResult) -> Result<f64, String> {
    match e {
        MeasEvent::At { at } => Ok(*at),
        MeasEvent::Cross {
            signal,
            value,
            reference,
            dir,
            n,
            td,
        } => {
            let mut ys = samples(signal, a)?;
            if let Some(reference) = reference {
                for (y, r) in ys.iter_mut().zip(samples(reference, a)?) {
                    *y -= r;
                }
            }
            let xs = &a.scale_values;
            let mut hits = Vec::new();
            for k in 1..ys.len().min(xs.len()) {
                let (y0, y1) = (ys[k - 1] - value, ys[k] - value);
                let rise = y0 < 0.0 && y1 >= 0.0;
                let fall = y0 > 0.0 && y1 <= 0.0;
                let ok = match dir {
                    CrossDir::Rise => rise,
                    CrossDir::Fall => fall,
                    CrossDir::Cross => rise || fall,
                };
                if !ok {
                    continue;
                }
                let x = if y1 == y0 {
                    xs[k]
                } else {
                    xs[k - 1] + (xs[k] - xs[k - 1]) * (-y0) / (y1 - y0)
                };
                if td.is_none_or(|td| x >= td) {
                    hits.push(x);
                }
            }
            let pick = match n {
                Some(0) => None,
                Some(n) => hits.get(*n as usize - 1).copied(),
                None => hits.last().copied(),
            };
            pick.ok_or_else(|| {
                let what = match dir {
                    CrossDir::Rise => "rising crossing",
                    CrossDir::Fall => "falling crossing",
                    CrossDir::Cross => "crossing",
                };
                let which = n.map_or("last".to_string(), |n| format!("#{n}"));
                format!(
                    "{which} {what} of {} = {value:e} not found ({} found)",
                    signal_name(signal),
                    hits.len()
                )
            })
        }
    }
}

fn measure(
    kind: &MeasKind,
    a: &AnalysisResult,
    prior: &std::collections::HashMap<String, f64>,
) -> Result<f64, String> {
    let xs = &a.scale_values;
    if a.kind == AnalysisKind::Dc
        && !matches!(kind, MeasKind::Param { .. })
        && !xs.windows(2).all(|w| w[1] > w[0])
        && !xs.windows(2).all(|w| w[1] < w[0])
    {
        return Err("measurement requires a single DC sweep curve".into());
    }
    match kind {
        MeasKind::Param { expression } => sansho_netlist::eval_expr(
            &sansho_netlist::parse_expr(expression)?,
            &|n| {
                prior
                    .get(n)
                    .copied()
                    .ok_or_else(|| format!("no preceding successful measurement named {n}"))
            },
            &Default::default(),
        ),
        MeasKind::Deriv { signal, event } => {
            let x = event_x(event, a)?;
            let ys = samples(signal, a)?;
            let k = segment(xs, x)?;
            if k == 0 {
                return Err("DERIV needs two distinct sweep points".into());
            }
            Ok((ys[k] - ys[k - 1]) / (xs[k] - xs[k - 1]))
        }
        MeasKind::TrigTarg { trig, targ } => Ok(event_x(targ, a)? - event_x(trig, a)?),
        MeasKind::When { event } => event_x(event, a),
        MeasKind::Find { signal, event } => {
            let x = event_x(event, a)?;
            at(xs, &samples(signal, a)?, x)
        }
        MeasKind::Stat {
            stat,
            signal,
            from,
            to,
        } => {
            let ys = samples(signal, a)?;
            let (Some(&first), Some(&last)) = (xs.first(), xs.last()) else {
                return Err("no data points".into());
            };
            let lo = from.unwrap_or(first.min(last)).max(first.min(last));
            let hi = to.unwrap_or(first.max(last)).min(first.max(last));
            if hi < lo {
                return Err(format!("empty interval [{lo:e}, {hi:e}]"));
            }
            let (start, stop) = if first <= last { (lo, hi) } else { (hi, lo) };
            let mut px = vec![start];
            let mut py = vec![at(xs, &ys, start)?];
            for (&x, &y) in xs.iter().zip(&ys) {
                if x > lo && x < hi {
                    px.push(x);
                    py.push(y);
                }
            }
            if hi > lo {
                px.push(stop);
                py.push(at(xs, &ys, stop)?);
            }
            let integ = |f: &dyn Fn(f64) -> f64| -> f64 {
                px.windows(2)
                    .zip(py.windows(2))
                    .map(|(x, y)| 0.5 * (f(y[0]) + f(y[1])) * (x[1] - x[0]))
                    .sum()
            };
            let max = py.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let min = py.iter().copied().fold(f64::INFINITY, f64::min);
            let span = stop - start;
            Ok(match stat {
                MeasStat::MaxAt | MeasStat::MinAt => {
                    let extreme = if *stat == MeasStat::MaxAt { max } else { min };
                    px[py
                        .iter()
                        .position(|&v| v == extreme)
                        .ok_or("no finite extremum")?]
                }
                MeasStat::Max => max,
                MeasStat::Min => min,
                MeasStat::Pp => max - min,
                MeasStat::Integ => integ(&|y| y),
                MeasStat::Avg | MeasStat::Rms if span == 0.0 => {
                    return Err("AVG/RMS need an interval of non-zero length".into());
                }
                MeasStat::Avg => integ(&|y| y) / span,
                MeasStat::Rms => (integ(&|y| y * y) / span).sqrt(),
            })
        }
    }
}
