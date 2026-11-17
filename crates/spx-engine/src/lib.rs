//! MNA assembly, nonlinear solver and circuit analyses.
//!
//! Entry point: [`run`] parses nothing itself; it takes a parsed [`Netlist`], compiles it and
//! runs every analysis card in order.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod analysis;
mod circuit;
mod options;
mod solve;

pub use circuit::{Circuit, compile};
pub use options::SimOptions;

use spx_diag::Diagnostic;
use spx_netlist::{AnalysisCard, Netlist};
use spx_output::SimResults;

/// Successful run: results plus non-fatal diagnostics.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Results of every analysis, in netlist order.
    pub results: SimResults,
    /// Warnings collected during parsing, elaboration and analysis.
    pub warnings: Vec<Diagnostic>,
}

/// Failed run: diagnostics (at least one error) and results of the analyses that completed
/// before the failure.
#[derive(Debug, Clone)]
pub struct RunError {
    /// All diagnostics, errors first.
    pub diagnostics: Vec<Diagnostic>,
    /// Results completed before the failure.
    pub partial: Option<SimResults>,
}

/// Compile a netlist and run all of its analyses.
pub fn run(nl: &Netlist) -> Result<RunOutput, RunError> {
    let (opts, mut warnings) = SimOptions::from_cards(&nl.options);
    warnings.extend(nl.warnings.iter().cloned());
    let (mut ckt, w) = compile(nl, &opts).map_err(|mut d| {
        d.extend(warnings.iter().cloned());
        RunError {
            diagnostics: d,
            partial: None,
        }
    })?;
    warnings.extend(w);
    if nl.analyses.is_empty() {
        warnings.push(Diagnostic::warning(
            "W-RUN-NOANALYSIS",
            "the netlist contains no analysis card (.op, .dc, .ac, .tran)",
        ));
    }
    let mut results = SimResults {
        title: nl.title.clone(),
        analyses: Vec::new(),
    };
    for card in &nl.analyses {
        let (r, span) = match card {
            AnalysisCard::Op { span } => (analysis::op(&ckt, &opts), *span),
            AnalysisCard::Dc {
                span,
                source,
                start,
                stop,
                step,
            } => (
                analysis::dc_sweep(&mut ckt, &opts, source, *start, *stop, *step),
                *span,
            ),
            AnalysisCard::Ac {
                span,
                sweep,
                points,
                fstart,
                fstop,
            } => (
                analysis::ac(&ckt, &opts, *sweep, *points, *fstart, *fstop),
                *span,
            ),
            AnalysisCard::Tran {
                span,
                tstep,
                tstop,
                tstart,
                tmax,
                ..
            } => (
                analysis::tran(&ckt, &opts, *tstep, *tstop, *tstart, *tmax),
                *span,
            ),
        };
        match r {
            Ok(a) => results.analyses.push(a),
            Err(d) => {
                let mut diagnostics = vec![d.with_span(span)];
                diagnostics.extend(warnings);
                return Err(RunError {
                    diagnostics,
                    partial: Some(results),
                });
            }
        }
    }
    Ok(RunOutput { results, warnings })
}
