//! Pre-run lint: checks that catch likely mistakes before any simulation work.
//!
//! Topology problems (dangling nodes, missing DC paths, voltage loops) are found during
//! elaboration and unit mistakes during parsing; this module adds checks that need both the
//! netlist and the compiled circuit.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

use std::collections::HashSet;

use spx_diag::{Confidence, Diagnostic};
use spx_netlist::{AnalysisCard, ElementKind, GROUND, Netlist, Signal};

use crate::circuit::Circuit;
use crate::options::SimOptions;

/// Run every lint check. Returns warnings only.
pub(crate) fn lint(nl: &Netlist, ckt: &Circuit, opts: &SimOptions) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    unused_models(nl, &mut out);
    default_geometry(nl, opts, &mut out);
    meas_signals(nl, ckt, &mut out);
    tran_steps(nl, &mut out);
    out
}

fn unused_models(nl: &Netlist, out: &mut Vec<Diagnostic>) {
    let used: HashSet<&str> = nl.elements.iter().filter_map(|e| e.kind.model()).collect();
    for m in &nl.models {
        if !used.contains(m.name.as_str()) {
            out.push(
                Diagnostic::warning(
                    "W-LINT-UNUSED-MODEL",
                    format!("model `{}` is not used by any device", m.name),
                )
                .with_span(m.span),
            );
        }
    }
}

fn default_geometry(nl: &Netlist, opts: &SimOptions, out: &mut Vec<Diagnostic>) {
    for e in &nl.elements {
        let ElementKind::Mosfet { params, .. } = &e.kind else {
            continue;
        };
        let has = |k: &str| params.iter().any(|(n, _)| n == k);
        let mut missing = Vec::new();
        if !has("l") && opts.defl.is_none() {
            missing.push("l");
        }
        if !has("w") && opts.defw.is_none() {
            missing.push("w");
        }
        if !missing.is_empty() {
            out.push(
                Diagnostic::warning(
                    "W-LINT-DEFAULT-GEOM",
                    format!(
                        "`{}` has no {}: the default of 100 µm is used",
                        e.name,
                        missing.join("/")
                    ),
                )
                .with_span(e.span)
                .with_suggestion(
                    format!("add {} to the instance", missing.join("= ") + "="),
                    Confidence::Medium,
                ),
            );
        }
    }
}

fn meas_signals(nl: &Netlist, ckt: &Circuit, out: &mut Vec<Diagnostic>) {
    let names: HashSet<&str> = ckt
        .unknowns
        .names()
        .iter()
        .map(String::as_str)
        .chain(ckt.aliases.iter().map(|(name, _)| name.as_str()))
        .collect();
    let node_ok = |n: &str| n == GROUND || names.contains(format!("v({n})").as_str());
    let missing = |s: &Signal| -> Option<String> {
        if s.current {
            let n = format!("i({})", s.pos);
            (!names.contains(n.as_str())).then_some(n)
        } else if !node_ok(&s.pos) {
            Some(format!("v({})", s.pos))
        } else {
            s.neg
                .as_deref()
                .filter(|n| !node_ok(n))
                .map(|n| format!("v({n})"))
        }
    };
    for m in &nl.measures {
        for s in m.kind.signals() {
            if let Some(name) = missing(s) {
                let mut d = Diagnostic::warning(
                    "W-LINT-MEAS-SIGNAL",
                    format!(
                        "measurement `{}` reads `{name}`, which does not exist",
                        m.name
                    ),
                )
                .with_span(m.span);
                if let Some(close) = closest(&name, ckt.unknowns.names()) {
                    d = d.with_suggestion(format!("did you mean `{close}`?"), Confidence::Medium);
                }
                out.push(d);
            }
        }
    }
}

/// The most similar existing name (edit distance ≤ 2), if any.
fn closest<'a>(name: &str, candidates: &'a [String]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|c| (edit_distance(name, c), c))
        .filter(|&(d, _)| d <= 2)
        .min_by_key(|&(d, _)| d)
        .map(|(_, c)| c.as_str())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn tran_steps(nl: &Netlist, out: &mut Vec<Diagnostic>) {
    for a in &nl.analyses {
        let AnalysisCard::Tran {
            tstep, tstop, span, ..
        } = a
        else {
            continue;
        };
        let msg = if tstep > tstop {
            Some(format!(
                "tstep = {tstep:e} s is larger than tstop = {tstop:e} s"
            ))
        } else if *tstop / *tstep > 1e7 {
            Some(format!(
                "tstop / tstep = {:.1e} output points is very large",
                tstop / tstep
            ))
        } else {
            None
        };
        if let Some(m) = msg {
            out.push(
                Diagnostic::warning("W-LINT-TRAN-STEP", m)
                    .with_span(*span)
                    .with_suggestion(
                        format!("use tstep ≈ {:e}", tstop / 1000.0),
                        Confidence::Medium,
                    ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::edit_distance;

    #[test]
    fn edit_distances() {
        assert_eq!(edit_distance("v(out)", "v(out)"), 0);
        assert_eq!(edit_distance("v(otu)", "v(out)"), 2);
        assert_eq!(edit_distance("v(ou)", "v(out)"), 1);
    }
}
