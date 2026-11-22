//! Session / job API: the agent-facing library surface of Sansho (architecture §7).
//!
//! A [`Session`] holds loaded circuits and finished jobs. Results come back **summary first**
//! (operating-point values, signal statistics, measurements, diagnostics); full waveforms are
//! fetched on request with a point budget. Every job carries reproducibility metadata: an input
//! hash of the canonical deck, a bit-exact result hash, the engine version and the effective
//! options. Results do not depend on the number of threads.
//!
//! ```
//! use sansho_api::{Session, SessionOptions};
//! let mut s = Session::new(SessionOptions::default());
//! let c = s.load("divider\nV1 in 0 2\nR1 in out 1k\nR2 out 0 1k\n.op\n").unwrap();
//! let job = s.simulate(&c.circuit_id, None).unwrap();
//! assert_eq!(job.status, sansho_api::Status::Ok);
//! ```
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod alter;
pub mod control;
mod hash;
mod summary;
mod waveform;

use std::collections::BTreeMap;
use std::time::Instant;

use rayon::prelude::*;
use sansho_diag::{CodeInfo, Confidence, Diagnostic};
use sansho_netlist::Netlist;
use sansho_output::{Measurement, SimResults};
use serde::Serialize;

pub use alter::{Applied, Change};
pub use sansho_engine::monte_carlo::{MonteCarlo, Report as MonteCarloReport};
pub use summary::{
    AnalysisSummary, CircuitStats, CostEstimate, DeviceCount, JobSummary, OpValue, SignalStats,
    Status, Value,
};
pub use waveform::{Reduce, Waveform};

/// Engine version reported in every result.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Session settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionOptions {
    /// Worker threads for simulations and sweeps (results do not depend on it).
    pub threads: usize,
    /// Maximum number of jobs kept; the oldest are dropped first.
    pub max_jobs: usize,
    /// Maximum number of signals listed per analysis in a summary.
    pub max_signals: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            threads: sansho_engine::default_threads(),
            max_jobs: 256,
            max_signals: 64,
        }
    }
}

/// A failed API request: structured diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiError {
    /// Diagnostics, errors first.
    pub diagnostics: Vec<Diagnostic>,
}

impl ApiError {
    fn one(d: Diagnostic) -> Self {
        Self {
            diagnostics: vec![d],
        }
    }

    fn input(msg: impl Into<String>) -> Self {
        Self::one(Diagnostic::error("E-API-INPUT", msg))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// Result of [`Session::validate`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Validation {
    /// Whether the netlist can be simulated.
    pub ok: bool,
    /// Circuit statistics (when elaboration succeeded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<CircuitStats>,
    /// All diagnostics: errors, then warnings (including lint).
    pub diagnostics: Vec<Diagnostic>,
}

/// Result of [`Session::load`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadReport {
    /// Identifier for later requests.
    pub circuit_id: String,
    /// Hash of the canonical deck.
    pub input_hash: String,
    /// Circuit statistics.
    pub stats: CircuitStats,
    /// Warnings (including lint).
    pub diagnostics: Vec<Diagnostic>,
}

/// Result of [`Session::alter`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlterReport {
    /// Circuit identifier.
    pub circuit_id: String,
    /// Changes in the order applied.
    pub applied: Vec<Applied>,
    /// Hash of the modified deck.
    pub input_hash: String,
}

/// One point of a parameter sweep.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SweepRow {
    /// Parameter value.
    pub value: f64,
    /// Job holding the full results.
    pub job_id: String,
    /// Outcome.
    pub status: Status,
    /// Measurements at this point.
    pub measures: Vec<Measurement>,
    /// Errors at this point, if it failed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Diagnostic>,
}

/// Result of [`Session::sweep`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SweepReport {
    /// Circuit identifier (unchanged by the sweep).
    pub circuit_id: String,
    /// Swept target.
    pub target: String,
    /// Swept parameter.
    pub param: String,
    /// One row per value, in input order.
    pub rows: Vec<SweepRow>,
}

/// Explanation of the diagnostics of a job, or of a code.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Explanation {
    /// Diagnostics being explained.
    pub diagnostics: Vec<Diagnostic>,
    /// Documentation of every code that occurs, in order of first occurrence.
    pub codes: Vec<CodeInfo>,
}

#[derive(Debug)]
struct Loaded {
    altered: bool,
    netlist: Netlist,
}

#[derive(Debug)]
struct Job {
    measurement_context: Option<sansho_engine::MeasurementContext>,
    circuit_id: String,
    results: Option<SimResults>,
    summary: JobSummary,
}

/// A simulation session.
#[derive(Debug)]
pub struct Session {
    opts: SessionOptions,
    pool: Option<rayon::ThreadPool>,
    circuits: BTreeMap<u64, Loaded>,
    jobs: BTreeMap<u64, Job>,
    next_circuit: u64,
    next_job: u64,
}

fn parse_id(id: &str, prefix: char) -> Option<u64> {
    id.trim().strip_prefix(prefix)?.parse().ok()
}

fn unknown_id(kind: &str, id: &str) -> ApiError {
    ApiError::one(
        Diagnostic::error("E-API-UNKNOWN-ID", format!("unknown {kind} `{id}`")).with_suggestion(
            format!("use a {kind} id returned earlier in this session"),
            Confidence::High,
        ),
    )
}

/// Parse loose card lines (`tran 1n 1u`, `.meas tran ...`) with the netlist parser.
fn parse_cards(lines: &[String], dot: &str) -> Result<Netlist, ApiError> {
    let mut text = String::from("* cards\n");
    for l in lines {
        let l = l.trim();
        if !l.starts_with('.') {
            text.push_str(dot);
        }
        text.push_str(l);
        text.push('\n');
    }
    sansho_netlist::parse(&text).map_err(|diagnostics| ApiError { diagnostics })
}

impl Session {
    /// Run a statistical deck from text or a file with relative includes.
    pub fn monte_carlo(
        &self,
        text: &str,
        base: Option<&std::path::Path>,
        seed: u64,
        n: usize,
        save_waves: bool,
    ) -> Result<MonteCarloReport, ApiError> {
        let plan = match base {
            Some(base) => MonteCarlo::prepare(
                text,
                &sansho_netlist::FsLoader {
                    base: base.to_owned(),
                },
            ),
            None => MonteCarlo::prepare(text, &sansho_netlist::NoFiles),
        }
        .map_err(|diagnostics| ApiError { diagnostics })?;
        self.install(|| plan.batch_in_current_pool(seed, n, save_waves))
            .map_err(|diagnostics| ApiError { diagnostics })
    }
    /// Create a session.
    pub fn new(opts: SessionOptions) -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(opts.threads.max(1))
            .build()
            .ok();
        Self {
            opts,
            pool,
            circuits: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_circuit: 1,
            next_job: 1,
        }
    }

    fn install<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        match &self.pool {
            Some(p) => p.install(f),
            None => f(),
        }
    }

    fn circuit(&self, id: &str) -> Result<&Loaded, ApiError> {
        parse_id(id, 'c')
            .and_then(|k| self.circuits.get(&k))
            .ok_or_else(|| unknown_id("circuit", id))
    }

    fn job(&self, id: &str) -> Result<&Job, ApiError> {
        parse_id(id, 'j')
            .and_then(|k| self.jobs.get(&k))
            .ok_or_else(|| unknown_id("job", id))
    }

    /// Parse, elaborate and lint a netlist without storing it.
    pub fn validate(&self, text: &str) -> Validation {
        self.validate_parsed(sansho_netlist::parse(text))
    }

    /// Like [`Session::validate`] for a file (`.include` / `.lib` resolve relative to it).
    pub fn validate_file(&self, path: &std::path::Path) -> Validation {
        self.validate_parsed(sansho_netlist::parse_file(path))
    }

    fn validate_parsed(&self, parsed: Result<Netlist, Vec<Diagnostic>>) -> Validation {
        match parsed {
            Err(diagnostics) => Validation {
                ok: false,
                stats: None,
                diagnostics: sorted(diagnostics),
            },
            Ok(nl) => match sansho_engine::check(&nl) {
                Ok(c) => Validation {
                    ok: true,
                    stats: Some(summary::stats(&nl, &c.circuit)),
                    diagnostics: c.warnings,
                },
                Err(d) => Validation {
                    ok: false,
                    stats: None,
                    diagnostics: sorted(d),
                },
            },
        }
    }

    /// Load a netlist into the session (file includes are not available; see
    /// [`Session::load_file`]).
    pub fn load(&mut self, text: &str) -> Result<LoadReport, ApiError> {
        self.load_parsed(sansho_netlist::parse(text))
    }

    /// Load a netlist file; `.include` / `.lib` resolve relative to it.
    pub fn load_file(&mut self, path: &std::path::Path) -> Result<LoadReport, ApiError> {
        self.load_parsed(sansho_netlist::parse_file(path))
    }

    fn load_parsed(
        &mut self,
        parsed: Result<Netlist, Vec<Diagnostic>>,
    ) -> Result<LoadReport, ApiError> {
        let nl = parsed.map_err(|d| ApiError {
            diagnostics: sorted(d),
        })?;
        let checked = sansho_engine::check(&nl).map_err(|d| ApiError {
            diagnostics: sorted(d),
        })?;
        let stats = summary::stats(&nl, &checked.circuit);
        let id = self.next_circuit;
        self.next_circuit += 1;
        let input_hash = hash::netlist(&nl);
        self.circuits.insert(
            id,
            Loaded {
                netlist: nl,
                altered: false,
            },
        );
        Ok(LoadReport {
            circuit_id: format!("c{id}"),
            input_hash,
            stats,
            diagnostics: checked.warnings,
        })
    }

    /// Canonical SPICE deck of a loaded circuit (with all alterations applied), accepted by
    /// both Sansho and ngspice.
    pub fn export(&self, circuit_id: &str) -> Result<String, ApiError> {
        let loaded = self.circuit(circuit_id)?;
        if !loaded.altered
            && loaded.netlist.sources.len() == 1
            && let Some(source) = &loaded.netlist.parameter_source
        {
            return Ok(source.clone());
        }
        Ok(sansho_netlist::write(&loaded.netlist))
    }

    /// Rough cost estimate of simulating a loaded circuit.
    pub fn estimate(&self, circuit_id: &str) -> Result<CostEstimate, ApiError> {
        let nl = &self.circuit(circuit_id)?.netlist;
        let c = sansho_engine::check(nl).map_err(|d| ApiError { diagnostics: d })?;
        Ok(summary::estimate(nl, &c.circuit))
    }

    /// Run the netlist's analyses, or `analyses` instead when given (card lines such as
    /// `tran 1n 1u` or `.ac dec 10 1 1g`). Simulation failures are reported in the summary
    /// (`status: error`); only invalid requests return `Err`.
    pub fn simulate(
        &mut self,
        circuit_id: &str,
        analyses: Option<&[String]>,
    ) -> Result<JobSummary, ApiError> {
        self.simulate_with_control(circuit_id, analyses, &control::ControlOptions::default())
    }

    /// Execute control blocks with an explicit output capability. An explicit analysis
    /// override bypasses the script. MCP always uses the file-disabled default.
    pub fn simulate_with_control(
        &mut self,
        circuit_id: &str,
        analyses: Option<&[String]>,
        options: &control::ControlOptions,
    ) -> Result<JobSummary, ApiError> {
        let mut nl = self.circuit(circuit_id)?.netlist.clone();
        if analyses.is_none() && !nl.controls.is_empty() {
            let start = Instant::now();
            let output = self.install(|| control::execute(&nl, options));
            let out = if output.diagnostics.iter().any(Diagnostic::is_error) {
                Err(sansho_engine::RunError {
                    diagnostics: output.diagnostics,
                    partial: Some(output.results),
                    measurement_context: None,
                })
            } else {
                Ok(sansho_engine::RunOutput {
                    results: output.results,
                    warnings: output.diagnostics,
                    measurement_context: None,
                    noise_summary_intervals: vec![],
                    tran_stats: vec![],
                    dc_stats: vec![],
                })
            };
            return Ok(self.store(
                circuit_id,
                &nl,
                out,
                start.elapsed().as_secs_f64() * 1000.,
                Some(output.transcript),
            ));
        }
        if let Some(lines) = analyses {
            let cards = parse_cards(lines, ".")?;
            if cards.analyses.is_empty() {
                return Err(ApiError::input("no analysis card in `analyses`"));
            }
            nl.analyses = cards.analyses;
        }
        let (out, elapsed) = self.install(|| {
            let t0 = Instant::now();
            (sansho_engine::run_in_current_pool(&nl), t0.elapsed())
        });
        Ok(self.store(circuit_id, &nl, out, elapsed.as_secs_f64() * 1e3, None))
    }

    fn store(
        &mut self,
        circuit_id: &str,
        nl: &Netlist,
        out: Result<sansho_engine::RunOutput, sansho_engine::RunError>,
        elapsed_ms: f64,
        control: Option<control::Transcript>,
    ) -> JobSummary {
        let id = self.next_job;
        self.next_job += 1;
        let job_id = format!("j{id}");
        let input_hash = hash::netlist(nl);
        let (opts, _) = sansho_engine::SimOptions::from_cards(&nl.options);
        let (results, measurement_context, diagnostics, status) = match out {
            Ok(o) => (
                Some(o.results),
                o.measurement_context,
                o.warnings,
                Status::Ok,
            ),
            Err(e) => (
                e.partial,
                e.measurement_context,
                sorted(e.diagnostics),
                Status::Error,
            ),
        };
        let mut summary = summary::job(
            summary::JobMeta {
                job_id: job_id.clone(),
                circuit_id: circuit_id.to_string(),
                status,
                input_hash,
                engine_version: ENGINE_VERSION,
                options: opts,
                elapsed_ms,
            },
            results.as_ref(),
            diagnostics,
            self.opts.max_signals,
        );
        if let Some(t) = control {
            summary.result_hash = hash::control(&summary.result_hash, &t);
            summary.control = Some(t);
        }
        self.jobs.insert(
            id,
            Job {
                measurement_context,
                circuit_id: circuit_id.to_string(),
                results,
                summary: summary.clone(),
            },
        );
        while self.jobs.len() > self.opts.max_jobs.max(1) {
            self.jobs.pop_first();
        }
        summary
    }

    /// Summary of an existing job.
    pub fn job_summary(&self, job_id: &str) -> Result<JobSummary, ApiError> {
        Ok(self.job(job_id)?.summary.clone())
    }

    /// Fetch a signal of a job. `analysis` is the index of the analysis in the job (default:
    /// the last one that has the signal). At most `max_points` points are returned (LTTB
    /// downsampling).
    #[allow(clippy::too_many_arguments)]
    pub fn waveform(
        &self,
        job_id: &str,
        signal: &str,
        analysis: Option<usize>,
        reduce: Reduce,
        from: Option<f64>,
        to: Option<f64>,
        max_points: usize,
    ) -> Result<Waveform, ApiError> {
        let job = self.job(job_id)?;
        let Some(r) = &job.results else {
            return Err(ApiError::input(format!("job `{job_id}` has no results")));
        };
        let candidates: Vec<usize> = match analysis {
            Some(i) if i < r.analyses.len() => vec![i],
            Some(i) => {
                return Err(ApiError::input(format!(
                    "job `{job_id}` has {} analyses; index {i} is out of range",
                    r.analyses.len()
                )));
            }
            None => (0..r.analyses.len()).rev().collect(),
        };
        for i in candidates {
            if let Some(w) = waveform::extract(&r.analyses[i], signal, reduce, from, to, max_points)
            {
                return Ok(w);
            }
        }
        let names: Vec<&str> = r
            .analyses
            .first()
            .map(|a| a.variables.iter().map(|v| v.name.as_str()).collect())
            .unwrap_or_default();
        Err(ApiError::one(
            Diagnostic::error(
                "E-API-UNKNOWN-SIGNAL",
                format!("job `{job_id}` has no signal `{signal}`"),
            )
            .with_note(format!("available: {}", names.join(", "))),
        ))
    }

    /// Evaluate `.meas`-style measurements (`tran tr trig v(out) val=0.5 rise=1 targ ...`)
    /// on the results of a job.
    pub fn measure(&self, job_id: &str, cards: &[String]) -> Result<Vec<Measurement>, ApiError> {
        let job = self.job(job_id)?;
        let Some(r) = &job.results else {
            return Err(ApiError::input(format!("job `{job_id}` has no results")));
        };
        let nl = parse_cards(cards, ".meas ")?;
        if nl.measures.is_empty() {
            return Err(ApiError::input("no measurement given"));
        }
        Ok(job.measurement_context.as_ref().map_or_else(
            || sansho_engine::measure(&nl.measures, r),
            |context| context.measure(&nl.measures),
        ))
    }

    /// Apply parameter changes to a loaded circuit. All changes are validated first; on
    /// error nothing is changed.
    pub fn alter(&mut self, circuit_id: &str, changes: &[Change]) -> Result<AlterReport, ApiError> {
        let mut nl = self.circuit(circuit_id)?.netlist.clone();
        let mut applied = Vec::with_capacity(changes.len());
        for c in changes {
            applied.push(alter::apply(&mut nl, c).map_err(ApiError::one)?);
        }
        sansho_engine::check(&nl).map_err(|d| ApiError {
            diagnostics: sorted(d),
        })?;
        let input_hash = hash::netlist(&nl);
        if let Some(k) = parse_id(circuit_id, 'c')
            && let Some(l) = self.circuits.get_mut(&k)
        {
            l.netlist = nl;
            l.altered |= !changes.is_empty();
        }
        Ok(AlterReport {
            circuit_id: circuit_id.trim().to_string(),
            applied,
            input_hash,
        })
    }

    /// Run the circuit once per value of `target.param`, in parallel (the stored circuit is
    /// not modified). `measures` adds measurements to every run.
    pub fn sweep(
        &mut self,
        circuit_id: &str,
        target: &str,
        param: &str,
        values: &[f64],
        measures: &[String],
    ) -> Result<SweepReport, ApiError> {
        let base = self.circuit(circuit_id)?.netlist.clone();
        let extra = if measures.is_empty() {
            Vec::new()
        } else {
            parse_cards(measures, ".meas ")?.measures
        };
        let mut decks = Vec::with_capacity(values.len());
        for &v in values {
            let mut nl = base.clone();
            alter::apply(
                &mut nl,
                &Change {
                    target: target.into(),
                    param: param.into(),
                    value: v,
                },
            )
            .map_err(ApiError::one)?;
            nl.measures.extend(extra.iter().cloned());
            decks.push(nl);
        }
        let outs: Vec<_> = self.install(|| {
            decks
                .par_iter()
                .map(|nl| {
                    let t0 = Instant::now();
                    (sansho_engine::run_in_current_pool(nl), t0.elapsed())
                })
                .collect()
        });
        let mut rows = Vec::with_capacity(values.len());
        for ((nl, (out, el)), &v) in decks.iter().zip(outs).zip(values) {
            let s = self.store(circuit_id, nl, out, el.as_secs_f64() * 1e3, None);
            rows.push(SweepRow {
                value: v,
                job_id: s.job_id,
                status: s.status,
                measures: s.measures,
                errors: s
                    .diagnostics
                    .into_iter()
                    .filter(Diagnostic::is_error)
                    .collect(),
            });
        }
        Ok(SweepReport {
            circuit_id: circuit_id.trim().to_string(),
            target: target.trim().to_ascii_lowercase(),
            param: param.trim().to_ascii_lowercase(),
            rows,
        })
    }

    /// Explain the diagnostics of a job (`j3`) or a single code (`E-CONV-OP`).
    pub fn explain(&self, job_or_code: &str) -> Result<Explanation, ApiError> {
        if let Some(info) = sansho_diag::lookup(job_or_code.trim()) {
            return Ok(Explanation {
                diagnostics: Vec::new(),
                codes: vec![*info],
            });
        }
        let job = self.job(job_or_code)?;
        let diagnostics = job.summary.diagnostics.clone();
        let mut codes: Vec<CodeInfo> = Vec::new();
        for d in &diagnostics {
            if let Some(i) = d.info()
                && !codes.iter().any(|c| c.code == i.code)
            {
                codes.push(*i);
            }
        }
        Ok(Explanation { diagnostics, codes })
    }

    /// Circuit a job was run on.
    pub fn job_circuit(&self, job_id: &str) -> Result<String, ApiError> {
        Ok(self.job(job_id)?.circuit_id.clone())
    }

    /// Full results of a job (for embedding applications).
    pub fn results(&self, job_id: &str) -> Result<Option<&SimResults>, ApiError> {
        Ok(self.job(job_id)?.results.as_ref())
    }
}

/// Errors first, otherwise in the original order.
fn sorted(mut d: Vec<Diagnostic>) -> Vec<Diagnostic> {
    d.sort_by_key(|x| !x.is_error());
    d
}
