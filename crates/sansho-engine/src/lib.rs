//! MNA assembly, nonlinear solver and circuit analyses.
//!
//! Entry point: [`run`] parses nothing itself; it takes a parsed [`Netlist`], compiles it and
//! runs every analysis card in order.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod analysis;
mod circuit;
mod fourier;
pub use fourier::fourier;
mod distortion;
mod lint;
mod measure;
pub mod monte_carlo;
mod options;
mod parallel;
mod probes;
mod rf;
mod small_signal;
mod solve;

pub use circuit::{Circuit, compile, hdl_files};
pub use options::{Method, SimOptions};
pub use parallel::default_threads;

/// Evaluate `.meas` cards against existing results (e.g. measurements added after a run).
pub fn measure(
    cards: &[sansho_netlist::MeasureCard],
    results: &SimResults,
) -> Vec<sansho_output::Measurement> {
    measure::evaluate(cards, results)
}

use rayon::prelude::*;
use sansho_diag::Diagnostic;
use sansho_netlist::{AnalysisCard, Netlist};
use sansho_output::SimResults;

/// Newton work across an entire DC sweep, including its initial operating point.
#[derive(Debug, Clone)]
pub struct DcStats {
    /// Swept source.
    pub source: String,
    /// Number of converged bias points.
    pub points: usize,
    /// All Newton iterations, including unsuccessful attempts.
    pub iterations: usize,
    /// Additional solves beyond one Newton solve per point.
    pub extra_solves: usize,
}

/// Transient solver work including the initial operating point.
#[derive(Debug, Clone)]
pub struct TranStats {
    /// Accepted advances in time, excluding time zero.
    pub accepted_steps: usize,
    /// Attempts rejected by Newton or LTE control.
    pub rejected_steps: usize,
    /// Total Newton iterations.
    pub iterations: usize,
    /// Total device assemblies, including converged-point charge evaluation.
    pub assemblies: usize,
}

enum Stats {
    Dc(DcStats),
    Tran(TranStats),
}

/// Private full traces for evaluating additional measurements after a projected run.
#[derive(Debug, Clone)]
pub struct MeasurementContext(Box<SimResults>);

impl MeasurementContext {
    /// Evaluate measurements using saved and unsaved signals.
    pub fn measure(
        &self,
        cards: &[sansho_netlist::MeasureCard],
    ) -> Vec<sansho_output::Measurement> {
        measure::evaluate(cards, &self.0)
    }
}

/// Successful run: results plus non-fatal diagnostics.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Full traces retained for post-run measurements when output was projected.
    pub measurement_context: Option<MeasurementContext>,
    /// CLI contributor reporting intervals, in noise card order.
    pub noise_summary_intervals: Vec<u32>,
    /// Statistics in transient card order.
    pub tran_stats: Vec<TranStats>,
    /// Statistics in DC card order.
    pub dc_stats: Vec<DcStats>,
    /// Results of every analysis, in netlist order.
    pub results: SimResults,
    /// Warnings collected during parsing, elaboration and analysis.
    pub warnings: Vec<Diagnostic>,
}

/// Failed run: diagnostics (at least one error) and results of the analyses that completed
/// before the failure.
#[derive(Debug, Clone)]
pub struct RunError {
    /// Measurement inputs for completed analyses with projected output.
    pub measurement_context: Option<MeasurementContext>,
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
    let _phase = sansho_diag::Phase::start("setup");
    let (options, mut warnings) = SimOptions::from_cards(&nl.options);
    warnings.extend(nl.warnings.iter().cloned());
    let (circuit, w) = compile(nl, &options).map_err(|mut d| {
        d.extend(warnings.iter().cloned());
        d
    })?;
    // Validate selections even for check-only, empty, and noise-only runs.
    probes::Probes::bind(&circuit, nl, options.savecurrents).map_err(|d| vec![d])?;
    warnings.extend(w);
    warnings.extend(lint::lint(nl, &circuit, &options));
    if nl.analyses.is_empty() && nl.controls.is_empty() {
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
) -> Result<(Vec<sansho_output::AnalysisResult>, Option<Stats>), Diagnostic> {
    let mut stats = None;
    // Retained VA interpreter variables belong to an analysis, never to concurrent analyses.
    let owned = if nl.analyses.len() > 1
        && !nl.hdl.is_empty()
        && !matches!(card, AnalysisCard::Dc { .. } | AnalysisCard::Four { .. })
    {
        Some(
            if ckt.ordering.is_some() {
                ckt.bind_run(nl, opts)
            } else {
                circuit::compile_cached(
                    nl,
                    opts,
                    &ckt.va_modules,
                    matches!(card, AnalysisCard::Op { .. }),
                )
            }
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
        AnalysisCard::Sp { grid, noise, .. } => {
            return rf::analyze(ckt, nl, opts, *grid, *noise).map(|p| (vec![p], None));
        }
        AnalysisCard::Disto { grid, ratio, .. } => {
            return distortion::analyze(ckt, nl, opts, *grid, *ratio).map(|p| (p, None));
        }
        AnalysisCard::Noise {
            output,
            source,
            sweep,
            points,
            fstart,
            fstop,
            ..
        } => {
            return small_signal::noise(
                ckt,
                nl,
                opts,
                output,
                source,
                analysis::ac_frequencies(*sweep, *points, *fstart, *fstop)?,
            )
            .map(|plots| (plots, None));
        }
        AnalysisCard::Tf { output, source, .. } => {
            return small_signal::tf(ckt, nl, opts, output, source).map(|a| (vec![a], None));
        }
        AnalysisCard::Sens {
            output,
            ac: Some(grid),
            ..
        } => return small_signal::ac_sens(ckt, nl, opts, output, *grid).map(|a| (vec![a], None)),
        AnalysisCard::Sens {
            output, ac: None, ..
        } => {
            return small_signal::sens(ckt, nl, opts, output).map(|a| (vec![a], None));
        }
        AnalysisCard::Pz {
            nodes,
            voltage,
            poles,
            zeros,
            ..
        } => {
            return small_signal::pz(ckt, opts, nodes, *voltage, *poles, *zeros)
                .map(|a| (vec![a], None));
        }
        AnalysisCard::Four { .. } => return Ok((vec![], None)),
        AnalysisCard::Op { .. } => analysis::op(ckt, opts, nl),
        AnalysisCard::Dc { inner, outer, .. } => {
            // A sweep modifies a source value, so it works on its own copy of the circuit.
            let mut own = if ckt.ordering.is_some() {
                ckt.bind_run(nl, opts)
            } else {
                circuit::compile_cached(nl, opts, &ckt.va_modules, true)
            }
            .map_err(|d| {
                d.into_iter()
                    .next()
                    .unwrap_or_else(|| Diagnostic::error("E-ELAB-INTERNAL", "recompilation failed"))
            })?
            .0;
            analysis::dc_sweep(&mut own, opts, nl, inner, outer.as_ref()).map(|(analysis, dc)| {
                stats = Some(Stats::Dc(dc));
                analysis
            })
        }
        AnalysisCard::Ac {
            sweep,
            points,
            fstart,
            fstop,
            ..
        } => analysis::ac(ckt, nl, opts, *sweep, *points, *fstart, *fstop),
        AnalysisCard::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            uic,
            ..
        } => analysis::tran(
            ckt,
            opts,
            sansho_devices::TranParams {
                tstep: *tstep,
                tstop: *tstop,
            },
            *tstart,
            *tmax,
            *uic,
            nl,
        )
        .map(|(a, t)| {
            stats = Some(Stats::Tran(t));
            a
        }),
    }?;
    add_aliases(ckt, &mut result);
    Ok((vec![result], stats))
}

fn add_aliases(ckt: &Circuit, result: &mut sansho_output::AnalysisResult) {
    for (name, index) in &ckt.aliases {
        let points = result.len().max(result.scale_values.len());
        result.variables.push(sansho_output::Variable {
            name: name.clone(),
            quantity: sansho_output::Quantity::Voltage,
        });
        match &mut result.data {
            sansho_output::Data::Real { columns } => {
                columns.push(index.map_or_else(|| vec![0.; points], |k| columns[k].clone()))
            }
            sansho_output::Data::Complex { columns } => {
                columns.push(index.map_or_else(|| vec![(0., 0.); points], |k| columns[k].clone()))
            }
        }
    }
}

/// Project public output in circuit order. The independent scale is always retained.
fn project_results(full: &mut SimResults, saves: &[String]) -> Option<MeasurementContext> {
    if saves.is_empty() || saves.iter().any(|s| s == "all") {
        return None;
    }
    let analyses = full
        .analyses
        .iter()
        .map(|a| {
            if !a.kind.is_distortion()
                && !matches!(
                    a.kind,
                    sansho_output::AnalysisKind::Op
                        | sansho_output::AnalysisKind::Tran
                        | sansho_output::AnalysisKind::Ac
                        | sansho_output::AnalysisKind::Dc
                )
            {
                return a.clone();
            }
            let indices: Vec<_> = a
                .variables
                .iter()
                .enumerate()
                .filter(|(_, v)| saves.iter().any(|s| s.eq_ignore_ascii_case(&v.name)))
                .map(|(i, _)| i)
                .collect();
            use sansho_output::Data;
            sansho_output::AnalysisResult {
                kind: a.kind,
                scale: a.scale.clone(),
                scale_values: a.scale_values.clone(),
                variables: indices.iter().map(|&i| a.variables[i].clone()).collect(),
                data: match &a.data {
                    Data::Real { columns } => Data::Real {
                        columns: indices.iter().map(|&i| columns[i].clone()).collect(),
                    },
                    Data::Complex { columns } => Data::Complex {
                        columns: indices.iter().map(|&i| columns[i].clone()).collect(),
                    },
                },
            }
        })
        .collect();
    let saved = SimResults {
        prints: full.prints.clone(),
        title: full.title.clone(),
        measures: full.measures.clone(),
        analyses,
    };
    Some(MeasurementContext(Box::new(std::mem::replace(full, saved))))
}

fn card_span(card: &AnalysisCard) -> sansho_diag::Span {
    match card {
        AnalysisCard::Sp { span, .. }
        | AnalysisCard::Disto { span, .. }
        | AnalysisCard::Four { span, .. }
        | AnalysisCard::Pz { span, .. }
        | AnalysisCard::Tf { span, .. }
        | AnalysisCard::Sens { span, .. }
        | AnalysisCard::Noise { span, .. }
        | AnalysisCard::Op { span }
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
    if cfg.threads <= 1 {
        return parallel::with_threads(1, || run_in_current_pool(nl));
    }
    if nl.hdl.is_empty() && nl.elements.len() < circuit::PARALLEL_MIN_NONLINEAR {
        let jobs = cfg.threads.min(nl.analyses.len().max(1));
        return parallel::with_threads(jobs, || run_in_current_pool(nl));
    }
    if nl.hdl.is_empty() && nl.analyses.len() == 1 {
        // Native elaboration is serial. Budget workers for useful device
        // evaluation rather than waking the entire host for each assembly.
        let checked = check_run(nl)?;
        let ckt = &checked.circuit;
        let work: usize = ckt
            .groups
            .iter()
            .map(|g| {
                let weight = if g.type_name() == "bsim4" { 4 } else { 1 };
                g.len().saturating_mul(weight)
            })
            .sum();
        let jobs = if ckt.parallel {
            work.div_ceil(4096).max(2)
        } else {
            1
        };
        return parallel::with_threads(cfg.threads.min(jobs), || run_checked(nl, checked));
    }
    #[cfg(feature = "jit")]
    if !nl.hdl.is_empty() && nl.analyses.len() == 1 {
        let bindings = nl
            .elements
            .iter()
            .filter(|e| matches!(e.kind, sansho_netlist::ElementKind::Va { .. }))
            .count();
        // Preparation queues at most one job per VA instance. Large circuits
        // can also use workers for other device groups during the analysis.
        let preparation_threads = cfg.threads.min(bindings.max(1));
        let checked = parallel::with_threads(preparation_threads, || check_run(nl))?;
        let ckt = &checked.circuit;
        // A single analysis has at most one ready job per device group or
        // gather chunk. Idle workers otherwise contend on every Newton assembly.
        let jobs = if checked.options.va_jit && sansho_va::jit_enabled() {
            if ckt.parallel {
                ckt.groups
                    .len()
                    .max(ckt.nnz().max(ckt.dim()).div_ceil(solve::GATHER_CHUNK))
            } else {
                1
            }
        } else {
            cfg.threads
        };
        return parallel::with_threads(cfg.threads.min(jobs), || run_checked(nl, checked));
    }
    parallel::with_threads(cfg.threads, || run_in_current_pool(nl))
}

/// Like [`run_with`], but uses the rayon thread pool the caller is running in (see
/// `rayon::ThreadPool::install`), so that an embedding application can share one pool.
pub fn run_in_current_pool(nl: &Netlist) -> Result<RunOutput, RunError> {
    run_checked(nl, check_run(nl)?)
}

fn check_run(nl: &Netlist) -> Result<Checked, RunError> {
    check(nl).map_err(|diagnostics| RunError {
        diagnostics,
        measurement_context: None,
        partial: None,
    })
}

fn run_checked(nl: &Netlist, checked: Checked) -> Result<RunOutput, RunError> {
    let Checked {
        circuit: mut ckt,
        options: opts,
        mut warnings,
    } = checked;
    let mut saves = nl.saves.clone();
    if !saves.is_empty() && opts.savecurrents {
        saves.extend(probes::currents(nl));
    }
    let mut outcomes: Vec<_> = match nl.analyses.as_slice() {
        // A lone sweep owns the already elaborated DC circuit. Concurrent
        // analysis cards still need private mutable devices for each sweep.
        [AnalysisCard::Dc { inner, outer, .. }] => {
            vec![
                analysis::dc_sweep(&mut ckt, &opts, nl, inner, outer.as_ref()).map(
                    |(mut result, stats)| {
                        add_aliases(&ckt, &mut result);
                        (vec![result], Some(Stats::Dc(stats)))
                    },
                ),
            ]
        }
        [card] => vec![run_analysis(&ckt, nl, &opts, card)],
        cards => cards
            .par_iter()
            .map(|card| run_analysis(&ckt, nl, &opts, card))
            .collect(),
    };
    for (i, card) in nl.analyses.iter().enumerate() {
        if let AnalysisCard::Four {
            frequency, outputs, ..
        } = card
        {
            let transient = outcomes
                .iter()
                .filter_map(|r| r.as_ref().ok())
                .flat_map(|(v, _)| v)
                .find(|a| a.kind == sansho_output::AnalysisKind::Tran);
            outcomes[i] = transient
                .ok_or_else(|| {
                    small_signal::error("Fourier analysis requires a completed transient")
                })
                .and_then(|tran| fourier(tran, *frequency, outputs))
                .map(|a| (vec![a], None));
        }
    }
    let mut results = SimResults {
        prints: Vec::new(),
        measures: Vec::new(),
        title: nl.title.clone(),
        analyses: Vec::new(),
    };
    let mut tran_stats = vec![];
    let mut dc_stats = vec![];
    for (card, r) in nl.analyses.iter().zip(outcomes) {
        match r {
            Ok((a, stats)) => {
                results.analyses.extend(a);
                match stats {
                    Some(Stats::Dc(s)) => dc_stats.push(s),
                    Some(Stats::Tran(s)) => tran_stats.push(s),
                    None => {}
                }
            }
            Err(d) => {
                let mut diagnostics = vec![d.with_span(card_span(card))];
                diagnostics.extend(warnings);
                return Err(RunError {
                    diagnostics,
                    measurement_context: project_results(&mut results, &saves),
                    partial: Some(results),
                });
            }
        }
    }
    measure::print_tables(nl, &mut results, &mut warnings);
    results.measures = measure::evaluate(&nl.measures, &results);
    for m in &results.measures {
        if let Some(e) = &m.error {
            warnings.push(Diagnostic::warning(
                "W-MEAS-FAILED",
                format!("measurement `{}` failed: {e}", m.name),
            ));
        }
    }
    let measurement_context = project_results(&mut results, &saves);
    Ok(RunOutput {
        noise_summary_intervals: nl
            .analyses
            .iter()
            .filter_map(|c| {
                if let AnalysisCard::Noise { summary, .. } = c {
                    Some(*summary)
                } else {
                    None
                }
            })
            .collect(),
        measurement_context,
        results,
        warnings,
        dc_stats,
        tran_stats,
    })
}
