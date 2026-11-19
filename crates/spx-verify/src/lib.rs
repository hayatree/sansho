//! Comparison of simulation results against reference results (e.g. from ngspice run as a
//! black-box program).
//!
//! Error metric per variable: `max |ours − ref| / (rtol · max |ref| + atol)`, i.e. errors are
//! measured relative to the full-scale value of the reference waveform. A value ≤ 1 passes.
//! Transient results are linearly interpolated onto the reference time points; AC results
//! compare complex values.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

use serde::Serialize;
use spx_output::{AnalysisKind, AnalysisResult, Data, Quantity, SimResults};

/// Tolerances for one deck.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Tolerances {
    /// Relative tolerance for OP, DC and AC (fraction of full scale).
    pub rtol: f64,
    /// Relative tolerance for transient voltages.
    pub rtol_tran: f64,
    /// Relative tolerance for transient currents. Capacitive and switching currents are
    /// spiky, so differences in timestep placement show up much more strongly than in
    /// voltages.
    pub rtol_tran_i: f64,
    /// Relative tolerance for `.meas` results.
    pub rtol_meas: f64,
    /// Absolute floor for voltages (V).
    pub atol_v: f64,
    /// Absolute floor for currents (A).
    pub atol_i: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            rtol: 2e-3,
            rtol_tran: 1e-2,
            rtol_tran_i: 3e-2,
            rtol_meas: 1e-2,
            atol_v: 1e-6,
            atol_i: 1e-9,
        }
    }
}

impl Tolerances {
    /// Apply overrides from netlist comment lines of the form
    /// `* verify: rtol=1e-3 rtol_tran=5e-3 rtol_tran_i=2e-2 atol_v=1e-6 atol_i=1e-12`.
    pub fn from_deck(text: &str) -> Self {
        let mut t = Self::default();
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix('*') else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix("verify:") else {
                continue;
            };
            for kv in rest.split_whitespace() {
                let Some((k, v)) = kv.split_once('=') else {
                    continue;
                };
                let Some(v) = spx_netlist::parse_number(v) else {
                    continue;
                };
                match k {
                    "rtol" => t.rtol = v,
                    "rtol_tran" => t.rtol_tran = v,
                    "rtol_tran_i" => t.rtol_tran_i = v,
                    "rtol_meas" => t.rtol_meas = v,
                    "atol_v" => t.atol_v = v,
                    "atol_i" => t.atol_i = v,
                    _ => {}
                }
            }
        }
        t
    }
}

/// Comparison result of one variable.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VarReport {
    /// Analysis kind.
    pub analysis: AnalysisKind,
    /// Variable name.
    pub name: String,
    /// Number of compared points.
    pub points: usize,
    /// Maximum absolute difference.
    pub max_abs: f64,
    /// Scale or sweep value at the maximum difference (NaN for `.op`).
    pub at: f64,
    /// Normalized error (≤ 1 passes).
    pub score: f64,
}

impl VarReport {
    /// Whether the variable is within tolerance.
    pub fn pass(&self) -> bool {
        self.score <= 1.0
    }
}

/// Comparison result of one `.meas` value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeasReport {
    /// Measurement name.
    pub name: String,
    /// Reference value.
    pub reference: f64,
    /// Our value (`None` if our measurement failed).
    pub ours: Option<f64>,
    /// Normalized error (≤ 1 passes).
    pub score: f64,
}

/// Comparison result of one deck.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeckReport {
    /// Variable reports.
    pub vars: Vec<VarReport>,
    /// Measurement reports.
    pub measures: Vec<MeasReport>,
    /// Problems that prevent comparison (missing analyses, length mismatch, ...).
    pub problems: Vec<String>,
}

impl DeckReport {
    /// Whether everything passed.
    pub fn pass(&self) -> bool {
        self.problems.is_empty()
            && self.vars.iter().all(VarReport::pass)
            && self.measures.iter().all(|m| m.score <= 1.0)
    }
}

/// Linear interpolation of `(xs, ys)` at `x` (`xs` ascending; clamps outside the range).
fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let j = xs.partition_point(|&v| v < x);
    if j == 0 {
        return ys.first().copied().unwrap_or(f64::NAN);
    }
    if j >= xs.len() {
        return ys.last().copied().unwrap_or(f64::NAN);
    }
    let (x0, x1) = (xs[j - 1], xs[j]);
    if x1 == x0 {
        return ys[j];
    }
    let w = (x - x0) / (x1 - x0);
    ys[j - 1] + w * (ys[j] - ys[j - 1])
}

/// A raw writer may wrap special vector names according to the output units.
fn vector_name(kind: AnalysisKind, name: &str) -> &str {
    if matches!(
        kind,
        AnalysisKind::Noise
            | AnalysisKind::IntegratedNoise
            | AnalysisKind::Tf
            | AnalysisKind::Sens
            | AnalysisKind::Pz
    ) && name.get(..2).is_some_and(|prefix| {
        prefix.eq_ignore_ascii_case("v(") || prefix.eq_ignore_ascii_case("i(")
    }) && let Some(inner) = name[2..].strip_suffix(')')
    {
        return inner;
    }
    name
}

struct Root<'a> {
    name: &'a str,
    re: f64,
    im: f64,
}

fn roots<'a>(plot: &'a AnalysisResult, family: &str) -> Result<Vec<Root<'a>>, String> {
    let Data::Complex { columns } = &plot.data else {
        return Err("Pz: expected complex root values".into());
    };
    let mut roots = Vec::new();
    for (i, variable) in plot.variables.iter().enumerate() {
        let name = vector_name(plot.kind, &variable.name);
        let Some((kind, index)) = name.split_once('(') else {
            return Err(format!("Pz: unexpected root name {name}"));
        };
        if !kind.eq_ignore_ascii_case("pole") && !kind.eq_ignore_ascii_case("zero") {
            return Err(format!("Pz: unexpected root name {name}"));
        }
        if !kind.eq_ignore_ascii_case(family) {
            continue;
        }
        if index
            .strip_suffix(')')
            .and_then(|n| n.parse::<usize>().ok())
            .is_none()
        {
            return Err(format!("Pz: invalid root index in {name}"));
        }
        let Some(&[(re, im)]) = columns.get(i).map(Vec::as_slice) else {
            return Err(format!("Pz: expected one complex value for {name}"));
        };
        if !re.is_finite() || !im.is_finite() {
            return Err(format!("Pz: non-finite {name}"));
        }
        roots.push(Root { name, re, im });
    }
    Ok(roots)
}

// Find a one-to-one assignment at a given worst normalized error. Reassigning
// an earlier root matters for clusters where two roots share a nearest neighbor.
fn assign_roots(costs: &[Vec<f64>], limit: f64) -> Option<Vec<usize>> {
    fn augment(
        i: usize,
        costs: &[Vec<f64>],
        limit: f64,
        seen: &mut [bool],
        owners: &mut [Option<usize>],
    ) -> bool {
        for (j, &cost) in costs[i].iter().enumerate() {
            if cost <= limit && !seen[j] {
                seen[j] = true;
                if owners[j].is_none_or(|prev| augment(prev, costs, limit, seen, owners)) {
                    owners[j] = Some(i);
                    return true;
                }
            }
        }
        false
    }
    let mut owners = vec![None; costs.len()];
    for i in 0..costs.len() {
        if !augment(i, costs, limit, &mut vec![false; costs.len()], &mut owners) {
            return None;
        }
    }
    owners.into_iter().collect()
}

fn compare_roots(
    reference: &AnalysisResult,
    ours: &AnalysisResult,
    tol: &Tolerances,
    out: &mut DeckReport,
) -> Result<(), String> {
    for family in ["pole", "zero"] {
        let r = roots(reference, family)?;
        let o = roots(ours, family)?;
        if r.len() != o.len() {
            return Err(format!(
                "Pz: {} reference {family}s vs {} computed",
                r.len(),
                o.len()
            ));
        }
        if r.is_empty() {
            continue;
        }
        let distance = |a: &Root, b: &Root| (a.re - b.re).hypot(a.im - b.im);
        let costs: Vec<Vec<_>> = r
            .iter()
            .map(|a| {
                let scale = tol.rtol * a.re.hypot(a.im) + tol.atol_v;
                o.iter().map(|b| distance(a, b) / scale).collect()
            })
            .collect();
        let mut limits: Vec<_> = costs.iter().flatten().copied().collect();
        limits.sort_by(f64::total_cmp);
        limits.dedup();
        let best = limits.partition_point(|&limit| assign_roots(&costs, limit).is_none());
        let owners = limits
            .get(best)
            .and_then(|&limit| assign_roots(&costs, limit))
            .ok_or_else(|| format!("Pz: cannot match {family}s"))?;
        for (j, i) in owners.into_iter().enumerate() {
            out.vars.push(VarReport {
                analysis: AnalysisKind::Pz,
                name: r[i].name.to_owned(),
                points: 1,
                max_abs: distance(&r[i], &o[j]),
                at: f64::NAN,
                score: costs[i][j],
            });
        }
    }
    Ok(())
}

fn compare_analysis(
    reference: &AnalysisResult,
    ours: &AnalysisResult,
    tol: &Tolerances,
    out: &mut DeckReport,
) {
    if reference.kind == AnalysisKind::Pz {
        if let Err(error) = compare_roots(reference, ours, tol, out) {
            out.problems.push(error);
        }
        return;
    }
    let start = out.vars.len();
    let tran = reference.kind == AnalysisKind::Tran;
    for (vi, var) in reference.variables.iter().enumerate() {
        let rtol = match (tran, var.quantity) {
            (true, Quantity::Current) => tol.rtol_tran_i,
            (true, _) => tol.rtol_tran,
            _ => tol.rtol,
        };
        let name = vector_name(reference.kind, &var.name);
        let Some(oi) = ours
            .variables
            .iter()
            .position(|v| vector_name(ours.kind, &v.name).eq_ignore_ascii_case(name))
        else {
            if reference.kind == AnalysisKind::Tf
                || (reference.kind == AnalysisKind::IntegratedNoise
                    && ["onoise_total", "inoise_total"]
                        .iter()
                        .any(|n| name.eq_ignore_ascii_case(n)))
            {
                out.problems
                    .push(format!("{:?}: missing {name}", reference.kind));
            }
            continue;
        };
        let atol = match var.quantity {
            Quantity::Current => tol.atol_i,
            _ => tol.atol_v,
        };
        let (diffs, scale): (Vec<f64>, f64) = match (&reference.data, &ours.data) {
            (Data::Real { columns: rc }, Data::Real { columns: oc }) => {
                let (r, o) = (&rc[vi], &oc[oi]);
                let full = r.iter().fold(0.0f64, |m, v| m.max(v.abs()));
                let d = if reference.kind == AnalysisKind::Tran {
                    reference
                        .scale_values
                        .iter()
                        .zip(r)
                        .map(|(&t, &rv)| (interp(&ours.scale_values, o, t) - rv).abs())
                        .collect()
                } else if r.len() == o.len() {
                    r.iter().zip(o).map(|(a, b)| (a - b).abs()).collect()
                } else {
                    out.problems.push(format!(
                        "{:?} {}: {} reference points vs {} points",
                        reference.kind,
                        var.name,
                        r.len(),
                        o.len()
                    ));
                    continue;
                };
                (d, full)
            }
            (Data::Complex { columns: rc }, Data::Complex { columns: oc }) => {
                let (r, o) = (&rc[vi], &oc[oi]);
                if r.len() != o.len() {
                    out.problems.push(format!(
                        "{:?} {}: {} reference points vs {} points",
                        reference.kind,
                        var.name,
                        r.len(),
                        o.len()
                    ));
                    continue;
                }
                let full = r.iter().fold(0.0f64, |m, v| m.max(v.0.hypot(v.1)));
                let d = r
                    .iter()
                    .zip(o)
                    .map(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1))
                    .collect();
                (d, full)
            }
            _ => {
                out.problems.push(format!(
                    "{:?} {}: data type mismatch",
                    reference.kind, var.name
                ));
                continue;
            }
        };
        if diffs.is_empty() || diffs.iter().any(|v| !v.is_finite()) {
            out.problems.push(format!(
                "{:?} {}: empty or non-finite comparison",
                reference.kind, var.name
            ));
            continue;
        }
        let (k, max_abs) = diffs
            .iter()
            .copied()
            .enumerate()
            .fold(
                (0, 0.0f64),
                |acc, (i, d)| if d > acc.1 { (i, d) } else { acc },
            );
        out.vars.push(VarReport {
            analysis: reference.kind,
            name: name.to_owned(),
            points: diffs.len(),
            max_abs,
            at: reference.scale_values.get(k).copied().unwrap_or(f64::NAN),
            score: max_abs / (rtol * scale + atol),
        });
    }
    if out.vars.len() == start {
        out.problems.push(format!(
            "{:?}: no common variables to compare",
            reference.kind
        ));
    }
}

/// Compare all analyses. Analyses are matched by kind in order of appearance.
pub fn compare(reference: &SimResults, ours: &SimResults, tol: &Tolerances) -> DeckReport {
    let mut out = DeckReport {
        vars: Vec::new(),
        measures: Vec::new(),
        problems: Vec::new(),
    };
    let mut used = vec![false; ours.analyses.len()];
    for r in &reference.analyses {
        let found = ours
            .analyses
            .iter()
            .enumerate()
            .find(|(i, o)| !used[*i] && o.kind == r.kind);
        match found {
            Some((i, o)) => {
                used[i] = true;
                compare_analysis(r, o, tol, &mut out);
            }
            None => out
                .problems
                .push(format!("missing {:?} analysis in our results", r.kind)),
        }
    }
    if reference.analyses.is_empty() {
        out.problems.push("no reference analyses".into());
    }
    for (i, analysis) in ours.analyses.iter().enumerate() {
        if !used[i] {
            out.problems.push(format!(
                "missing {:?} analysis in reference results",
                analysis.kind
            ));
        }
    }
    out
}

/// Compare measurements. `reference` holds `(name, value)` pairs (lower-case names).
pub fn compare_measures(
    reference: &[(String, f64)],
    ours: &SimResults,
    tol: &Tolerances,
    out: &mut DeckReport,
) {
    for m in &ours.measures {
        let Some(&(_, r)) = reference.iter().find(|(n, _)| *n == m.name) else {
            out.problems.push(format!(
                "measurement `{}` missing from the reference",
                m.name
            ));
            continue;
        };
        let score = match m.value {
            Some(v) => (v - r).abs() / (tol.rtol_meas * r.abs() + 1e-30),
            None => f64::INFINITY,
        };
        out.measures.push(MeasReport {
            name: m.name.clone(),
            reference: r,
            ours: m.value,
            score,
        });
    }
}

/// Extract `name = value` lines printed by the reference simulator for the given names.
pub fn parse_measure_output(text: &str, names: &[String]) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((k, rest)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        if !names.contains(&k) || out.iter().any(|(n, _)| *n == k) {
            continue;
        }
        if let Some(v) = rest
            .split_whitespace()
            .next()
            .and_then(|t| t.parse::<f64>().ok())
        {
            out.push((k, v));
        }
    }
    out
}

/// Rewrite a deck so that its `.meas` cards run as interactive `meas` commands in a
/// `.control` block (ngspice prints batch-mode measurements only in that form). Returns
/// `None` when the deck has no `.meas` cards.
pub fn measure_deck(text: &str) -> Option<String> {
    let mut body = Vec::new();
    let mut cmds = Vec::new();
    let mut control = None;
    let mut ran = false;
    let mut in_control = false;
    let mut quit = None;
    for (i, line) in text.lines().enumerate() {
        let mut fields = line.split_whitespace();
        let card = fields.next().unwrap_or("").to_ascii_lowercase();
        if i == 0 {
            body.push(line.to_string());
            continue;
        }
        if matches!(card.as_str(), ".meas" | ".measure") && !in_control {
            if let Some(kind) = fields.next() {
                cmds.push(format!("setplot {}1", kind.to_ascii_lowercase()));
                cmds.push(format!(
                    "meas {kind} {}",
                    fields.collect::<Vec<_>>().join(" ")
                ));
            }
            continue;
        }
        match card.as_str() {
            ".control" => in_control = true,
            ".endc" => {
                in_control = false;
                control = Some(body.len());
            }
            "run" if in_control => ran = true,
            "quit" if in_control => {
                quit.get_or_insert(body.len());
            }
            ".end" => continue,
            _ => {}
        }
        body.push(line.to_string());
    }
    if cmds.is_empty() {
        return None;
    }
    if !ran {
        cmds.insert(0, "run".into());
    }
    if quit.is_none() {
        cmds.push("quit".into());
    }
    if let Some(i) = quit.or(control) {
        body.splice(i..i, cmds);
    } else {
        body.push(".control".into());
        body.extend(cmds);
        body.push(".endc".into());
    }
    body.push(".end".into());
    Some(body.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use spx_output::Variable;

    fn tran(times: Vec<f64>, vals: Vec<f64>) -> SimResults {
        SimResults {
            measures: Vec::new(),
            title: String::new(),
            analyses: vec![AnalysisResult {
                kind: AnalysisKind::Tran,
                scale: Some(Variable {
                    name: "time".into(),
                    quantity: Quantity::Time,
                }),
                scale_values: times,
                variables: vec![Variable {
                    name: "v(a)".into(),
                    quantity: Quantity::Voltage,
                }],
                data: Data::Real {
                    columns: vec![vals],
                },
            }],
        }
    }

    #[test]
    fn interpolates_transient_onto_reference_grid() {
        let r = tran(vec![0.0, 0.5, 1.0], vec![0.0, 0.5, 1.0]);
        let o = tran(vec![0.0, 1.0], vec![0.0, 1.0]);
        let rep = compare(&r, &o, &Tolerances::default());
        assert!(rep.pass());
        assert!(rep.vars[0].max_abs < 1e-15);
        let o = tran(vec![0.0, 1.0], vec![0.0, 1.1]);
        let rep = compare(&r, &o, &Tolerances::default());
        assert!(!rep.pass());
        assert_eq!(rep.vars[0].at, 1.0);
    }

    #[test]
    fn matches_complex_root_multisets_per_family() {
        let plot = |roots: &[(&str, f64, f64)]| SimResults {
            title: String::new(),
            measures: vec![],
            analyses: vec![AnalysisResult {
                kind: AnalysisKind::Pz,
                scale: None,
                scale_values: vec![],
                variables: roots
                    .iter()
                    .map(|&(name, _, _)| Variable {
                        name: name.into(),
                        quantity: Quantity::PoleZero,
                    })
                    .collect(),
                data: Data::Complex {
                    columns: roots.iter().map(|&(_, re, im)| vec![(re, im)]).collect(),
                },
            }],
        };
        let reference = plot(&[
            ("V(Pole(1))", -2., 1.),
            ("v(pole(2))", -2., -1.),
            ("zero(1)", -2., 1.),
        ]);
        let mut values = [
            ("pole(1)", -2., -1.),
            ("pole(2)", -2., 1.),
            ("V(ZERO(1))", -2., 1.),
        ];
        let tol = Tolerances::default();
        assert!(compare(&reference, &plot(&values), &tol).pass());
        assert!(!compare(&reference, &plot(&values[..2]), &tol).pass());
        values[0].2 = 1.;
        assert!(!compare(&reference, &plot(&values), &tol).pass());
        values[2].2 = -1.;
        assert!(!compare(&reference, &plot(&values), &tol).pass());
    }

    #[test]
    fn measure_output_and_deck_rewrite() {
        let names = vec!["period".to_string(), "x".to_string()];
        let got = parse_measure_output(
            "period              =  1.560173e-09 targ=  2.9e-09\nfoo = 1\nx = -2.5\n",
            &names,
        );
        assert_eq!(
            got,
            vec![("period".into(), 1.560173e-9), ("x".into(), -2.5)]
        );
        let d = measure_deck("t\nR1 a 0 1\n.tran 1n 1u\n.meas tran p MAX v(a)\n.end\n").unwrap();
        assert!(
            d.contains(".control\nrun\nsetplot tran1\nmeas tran p MAX v(a)\nquit\n.endc\n.end\n")
        );
    }

    #[test]
    fn tolerance_overrides() {
        let t = Tolerances::from_deck("title\n* verify: rtol=1e-4 atol_i=1p\nR1 a 0 1\n");
        assert_eq!(t.rtol, 1e-4);
        assert!((t.atol_i - 1e-12).abs() < 1e-24);
        assert_eq!(t.rtol_tran, Tolerances::default().rtol_tran);
    }
}
