//! MNA assembly, nonlinear solver and circuit analyses.
//!
//! Entry point: [`run`] parses nothing itself; it takes a parsed [`Netlist`], compiles it and
//! runs every analysis card in order.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod analysis;
mod circuit;
mod measure;
mod options;
mod parallel;
mod solve;

pub use circuit::{Circuit, compile};
pub use options::{Method, SimOptions};
pub use parallel::default_threads;

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

/// Execution settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunConfig {
    /// Maximum number of worker threads (independent analyses run concurrently).
    pub threads: usize,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            threads: parallel::default_threads(),
        }
    }
}

/// Compile a netlist and run all of its analyses with the default configuration.
pub fn run(nl: &Netlist) -> Result<RunOutput, RunError> {
    run_with(nl, &RunConfig::default())
}

/// Run several independent netlists, in parallel. Results are returned in input order and
/// are identical to running each netlist alone.
pub fn run_batch(netlists: &[Netlist], cfg: &RunConfig) -> Vec<Result<RunOutput, RunError>> {
    let inner = RunConfig { threads: 1 };
    parallel::map(netlists.len(), cfg.threads, |i| {
        run_with(&netlists[i], &inner)
    })
}

fn run_analysis(
    ckt: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    card: &AnalysisCard,
) -> Result<spx_output::AnalysisResult, Diagnostic> {
    match card {
        AnalysisCard::Op { .. } => analysis::op(ckt, opts),
        AnalysisCard::Dc {
            source,
            start,
            stop,
            step,
            ..
        } => {
            // A sweep modifies a source value, so it works on its own copy of the circuit.
            let mut own = compile(nl, opts)
                .map_err(|d| {
                    d.into_iter().next().unwrap_or_else(|| {
                        Diagnostic::error("E-ELAB-INTERNAL", "recompilation failed")
                    })
                })?
                .0;
            analysis::dc_sweep(&mut own, opts, source, *start, *stop, *step)
        }
        AnalysisCard::Ac {
            sweep,
            points,
            fstart,
            fstop,
            ..
        } => analysis::ac(ckt, opts, *sweep, *points, *fstart, *fstop),
        AnalysisCard::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            ..
        } => analysis::tran(ckt, opts, *tstep, *tstop, *tstart, *tmax),
    }
}

fn card_span(card: &AnalysisCard) -> spx_diag::Span {
    match card {
        AnalysisCard::Op { span }
        | AnalysisCard::Dc { span, .. }
        | AnalysisCard::Ac { span, .. }
        | AnalysisCard::Tran { span, .. } => *span,
    }
}

/// Compile a netlist and run all of its analyses.
///
/// Analyses are independent of each other and run concurrently when `cfg.threads > 1`; the
/// results do not depend on the thread count.
pub fn run_with(nl: &Netlist, cfg: &RunConfig) -> Result<RunOutput, RunError> {
    let (opts, mut warnings) = SimOptions::from_cards(&nl.options);
    warnings.extend(nl.warnings.iter().cloned());
    let (ckt, w) = compile(nl, &opts).map_err(|mut d| {
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
    let outcomes = parallel::map(nl.analyses.len(), cfg.threads, |i| {
        run_analysis(&ckt, nl, &opts, &nl.analyses[i])
    });
    let mut results = SimResults {
        measures: Vec::new(),
        title: nl.title.clone(),
        analyses: Vec::new(),
    };
    for (card, r) in nl.analyses.iter().zip(outcomes) {
        match r {
            Ok(a) => results.analyses.push(a),
            Err(d) => {
                let mut diagnostics = vec![d.with_span(card_span(card))];
                diagnostics.extend(warnings);
                return Err(RunError {
                    diagnostics,
                    partial: Some(results),
                });
            }
        }
    }
    results.measures = measure::evaluate(&nl.measures, &results);
    for m in &results.measures {
        if let Some(e) = &m.error {
            warnings.push(Diagnostic::warning(
                "W-MEAS-FAILED",
                format!("measurement `{}` failed: {e}", m.name),
            ));
        }
    }
    Ok(RunOutput { results, warnings })
}
