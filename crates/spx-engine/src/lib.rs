//! MNA assembly, nonlinear solver and circuit analyses.
//!
//! Entry point: [`run`] parses nothing itself; it takes a parsed [`Netlist`], compiles it and
//! runs every analysis card in order.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod analysis;
mod circuit;
mod lint;
mod measure;
mod options;
mod parallel;
mod solve;

pub use circuit::{Circuit, compile};
pub use options::{Method, SimOptions};
pub use parallel::default_threads;

/// Evaluate `.meas` cards against existing results (e.g. measurements added after a run).
pub fn measure(
    cards: &[spx_netlist::MeasureCard],
    results: &SimResults,
) -> Vec<spx_output::Measurement> {
    measure::evaluate(cards, results)
}

use rayon::prelude::*;
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
    /// Maximum number of worker threads. Results do not depend on this value.
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
    parallel::with_threads(cfg.threads, || {
        netlists.par_iter().map(run_in_current_pool).collect()
    })
}

/// A netlist that passed elaboration and lint.
#[derive(Debug)]
pub struct Checked {
    /// The compiled circuit.
    pub circuit: Circuit,
    /// Effective simulation options.
    pub options: SimOptions,
    /// Warnings from parsing, options, elaboration and lint.
    pub warnings: Vec<Diagnostic>,
}

/// Elaborate and lint a netlist without simulating it.
///
/// On failure, returns all diagnostics (errors first).
pub fn check(nl: &Netlist) -> Result<Checked, Vec<Diagnostic>> {
    let (options, mut warnings) = SimOptions::from_cards(&nl.options);
    warnings.extend(nl.warnings.iter().cloned());
    let (circuit, w) = compile(nl, &options).map_err(|mut d| {
        d.extend(warnings.iter().cloned());
        d
    })?;
    warnings.extend(w);
    warnings.extend(lint::lint(nl, &circuit, &options));
    if nl.analyses.is_empty() {
        warnings.push(Diagnostic::warning(
            "W-RUN-NOANALYSIS",
            "the netlist contains no analysis card (.op, .dc, .ac, .tran)",
        ));
    }
    Ok(Checked {
        circuit,
        options,
        warnings,
    })
}

fn run_analysis(
    ckt: &Circuit,
    nl: &Netlist,
    opts: &SimOptions,
    card: &AnalysisCard,
) -> Result<spx_output::AnalysisResult, Diagnostic> {
    // Retained VA interpreter variables belong to an analysis, never to concurrent analyses.
    let owned = if !nl.hdl.is_empty() && !matches!(card, AnalysisCard::Dc { .. }) {
        Some(
            circuit::compile_cached(nl, opts, &ckt.va_modules)
                .map_err(|ds| {
                    ds.into_iter().next().unwrap_or_else(|| {
                        Diagnostic::error("E-ELAB-INTERNAL", "VA analysis elaboration failed")
                    })
                })?
                .0,
        )
    } else {
        None
    };
    let ckt = owned.as_ref().unwrap_or(ckt);
    let mut result = match card {
        AnalysisCard::Op { .. } => analysis::op(ckt, opts),
        AnalysisCard::Dc {
            source,
            start,
            stop,
            step,
            ..
        } => {
            // A sweep modifies a source value, so it works on its own copy of the circuit.
            let mut own = circuit::compile_cached(nl, opts, &ckt.va_modules)
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
            uic,
            ..
        } => analysis::tran(ckt, opts, *tstep, *tstop, *tstart, *tmax, *uic),
    }?;
    for (name, index) in &ckt.aliases {
        let points = result.len().max(result.scale_values.len());
        result.variables.push(spx_output::Variable {
            name: name.clone(),
            quantity: spx_output::Quantity::Voltage,
        });
        match &mut result.data {
            spx_output::Data::Real { columns } => {
                columns.push(index.map_or_else(|| vec![0.; points], |k| columns[k].clone()))
            }
            spx_output::Data::Complex { columns } => {
                columns.push(index.map_or_else(|| vec![(0., 0.); points], |k| columns[k].clone()))
            }
        }
    }
    Ok(result)
}

fn card_span(card: &AnalysisCard) -> spx_diag::Span {
    match card {
        AnalysisCard::Op { span }
        | AnalysisCard::Dc { span, .. }
        | AnalysisCard::Ac { span, .. }
        | AnalysisCard::Tran { span, .. } => *span,
    }
}

/// Compile a netlist and run all of its analyses on a pool of `cfg.threads` threads.
///
/// Analyses run concurrently (L0) and large circuits evaluate devices and assemble the matrix
/// in parallel (L1/L2); the results do not depend on the thread count.
pub fn run_with(nl: &Netlist, cfg: &RunConfig) -> Result<RunOutput, RunError> {
    parallel::with_threads(cfg.threads, || run_in_current_pool(nl))
}

/// Like [`run_with`], but uses the rayon thread pool the caller is running in (see
/// `rayon::ThreadPool::install`), so that an embedding application can share one pool.
pub fn run_in_current_pool(nl: &Netlist) -> Result<RunOutput, RunError> {
    let Checked {
        circuit: ckt,
        options: opts,
        mut warnings,
    } = check(nl).map_err(|diagnostics| RunError {
        diagnostics,
        partial: None,
    })?;
    let outcomes: Vec<_> = nl
        .analyses
        .par_iter()
        .map(|card| run_analysis(&ckt, nl, &opts, card))
        .collect();
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
