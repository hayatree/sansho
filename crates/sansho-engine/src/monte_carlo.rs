//! Independent statistical instances of a single elaborated deck.
//! Provenance: original implementation. Sample variance uses Welford's recurrence.

use super::{Checked, RunConfig, RunError, RunOutput};
use rayon::prelude::*;
use sansho_diag::Diagnostic;
use sansho_netlist::{ParameterPlan, Sample, SourceLoader};
use sansho_output::{AnalysisResult, Measurement};
use serde::Serialize;

/// Source expansion, topology and ordering shared by all requested indices.
#[derive(Debug)]
pub struct MonteCarlo {
    parameters: ParameterPlan,
    pub(crate) base: Checked,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Run {
    pub seed: u64,
    pub index: u64,
    pub success: bool,
    pub measures: Vec<Measurement>,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waves: Option<Vec<AnalysisResult>>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Statistics {
    pub name: String,
    pub count: usize,
    pub mean: Option<f64>,
    pub std: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub p05: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Report {
    pub seed: u64,
    pub runs: Vec<Run>,
    pub statistics: Vec<Statistics>,
}

impl MonteCarlo {
    pub fn prepare(text: &str, loader: &dyn SourceLoader) -> Result<Self, Vec<Diagnostic>> {
        let parameters = sansho_netlist::prepare_parameters(text, loader)?;
        let mut base = super::check(parameters.nominal())?;
        base.circuit.prepare_batch(&parameters)?;
        Ok(Self { parameters, base })
    }

    /// Reproduce an index without advancing a generator or running earlier indices.
    pub fn run(&self, seed: u64, index: u64) -> Result<RunOutput, RunError> {
        self.run_bound(seed, index, |_| Ok(()))
    }

    /// Run an index after `bind` has adjusted its freshly bound circuit.
    pub(crate) fn run_bound(
        &self,
        seed: u64,
        index: u64,
        bind: impl FnOnce(&mut super::circuit::Circuit) -> Result<(), Vec<Diagnostic>>,
    ) -> Result<RunOutput, RunError> {
        let fail = |diagnostics| RunError {
            diagnostics,
            measurement_context: None,
            partial: None,
        };
        let nl = self
            .parameters
            .sample(Sample { seed, index })
            .map_err(fail)?;
        let (mut circuit, mut warnings) = self
            .base
            .circuit
            .bind_run(&nl, &self.base.options)
            .map_err(fail)?;
        bind(&mut circuit).map_err(fail)?;
        for w in &self.base.warnings {
            if !warnings.contains(w) {
                warnings.push(w.clone());
            }
        }
        super::run_checked(
            &nl,
            Checked {
                circuit,
                options: self.base.options.clone(),
                warnings,
            },
        )
        .map_err(|mut err| {
            if let Some(partial) = &mut err.partial {
                partial.measures = err.measurement_context.as_ref().map_or_else(
                    || super::measure(&nl.measures, partial),
                    |ctx| ctx.measure(&nl.measures),
                );
            }
            err
        })
    }

    pub fn run_with(
        &self,
        seed: u64,
        index: u64,
        config: &RunConfig,
    ) -> Result<RunOutput, RunError> {
        let threads = if self.base.circuit.parallel {
            config.threads
        } else {
            1
        };
        super::parallel::with_threads(threads, || self.run(seed, index))
    }

    pub fn batch(
        &self,
        seed: u64,
        n: usize,
        save_waves: bool,
        config: &RunConfig,
    ) -> Result<Report, Vec<Diagnostic>> {
        super::parallel::with_threads(config.threads.min(n), || {
            self.batch_in_current_pool(seed, n, save_waves)
        })
    }

    /// Execute on an embedding's existing Rayon pool.
    pub fn batch_in_current_pool(
        &self,
        seed: u64,
        n: usize,
        save_waves: bool,
    ) -> Result<Report, Vec<Diagnostic>> {
        if n == 0 {
            return Err(vec![Diagnostic::error(
                "E-MC-COUNT",
                "at least one run is required",
            )]);
        }
        // Bound live waveforms even when nested joins yield to other ready runs.
        let workers = rayon::current_num_threads();
        let window = if self.base.circuit.parallel || self.base.circuit.dim() >= 256 {
            workers.min(2)
        } else {
            workers
        };
        let mut runs = Vec::new();
        runs.try_reserve(n).map_err(|_| {
            vec![Diagnostic::error(
                "E-MC-COUNT",
                "cannot allocate the requested run table",
            )]
        })?;
        for start in (0..n).step_by(window) {
            let end = start.saturating_add(window).min(n);
            runs.extend(
                (start..end)
                    .into_par_iter()
                    .map(|i| self.row(seed, i as u64, save_waves))
                    .collect::<Vec<_>>(),
            );
        }
        Ok(self.report(seed, runs))
    }

    /// Summarize finished runs in index order.
    pub(crate) fn report(&self, seed: u64, runs: Vec<Run>) -> Report {
        let statistics = self
            .parameters
            .nominal()
            .measures
            .iter()
            .enumerate()
            .map(|(i, card)| {
                let values = runs
                    .iter()
                    .filter(|r| r.success)
                    .filter_map(|r| r.measures.get(i)?.value.filter(|v| v.is_finite()))
                    .collect();
                summarize(&card.name, values)
            })
            .collect();
        Report {
            seed,
            runs,
            statistics,
        }
    }
    fn row(&self, seed: u64, index: u64, save_waves: bool) -> Run {
        Self::row_from(seed, index, save_waves, self.run(seed, index))
    }

    pub(crate) fn row_from(
        seed: u64,
        index: u64,
        save_waves: bool,
        outcome: Result<RunOutput, RunError>,
    ) -> Run {
        let (result, diagnostics, success) = match outcome {
            Ok(out) => (Some(out.results), out.warnings, true),
            Err(err) => (err.partial, err.diagnostics, false),
        };
        let (measures, waves) = result.map_or_else(
            || (Vec::new(), None),
            |r| (r.measures, save_waves.then_some(r.analyses)),
        );
        Run {
            seed,
            index,
            success,
            measures,
            waves,
            diagnostics,
        }
    }
}

fn summarize(name: &str, mut x: Vec<f64>) -> Statistics {
    let count = x.len();
    let mut mean = 0.;
    let mut m2 = 0.;
    for (i, v) in x.iter().enumerate() {
        let d = v - mean;
        mean += d / (i + 1) as f64;
        m2 += d * (v - mean);
    }
    x.sort_by(f64::total_cmp);
    let quantile = |p: f64| {
        let h = count.checked_sub(1)? as f64 * p;
        let lo = x[h.floor() as usize];
        Some(lo + (x[h.ceil() as usize] - lo) * h.fract())
    };
    Statistics {
        name: name.into(),
        count,
        mean: (count > 0).then_some(mean),
        std: (count > 1).then(|| (m2 / (count - 1) as f64).sqrt()),
        min: x.first().copied(),
        max: x.last().copied(),
        p05: quantile(0.05),
        p50: quantile(0.5),
        p95: quantile(0.95),
    }
}
