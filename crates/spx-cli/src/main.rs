//! `spx` command-line interface.
//!
//! ```text
//! spx run deck.cir                # human-readable summary
//! spx run deck.cir -r out.raw     # also write an ngspice-compatible ASCII raw file
//! spx run deck.cir --json         # machine-readable JSON on stdout
//! spx batch a.cir b.cir -j 8      # independent netlists in parallel, one JSON line each
//! ```
//!
//! Exit codes: 0 success, 1 simulation or netlist error, 2 I/O or usage error.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;
use spx_diag::Diagnostic;
use spx_output::{AnalysisKind, Data, SimResults};

#[derive(Debug, Parser)]
#[command(name = "spx", version, about = "Fast, agent-native SPICE simulator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Parse and simulate a netlist.
    Run {
        /// Master seed for single-run Monte Carlo replay.
        #[arg(long, requires = "mc_run")]
        mc_seed: Option<u64>,
        /// Zero-based statistical run index.
        #[arg(long, requires = "mc_seed")]
        mc_run: Option<u64>,
        /// Netlist file.
        netlist: PathBuf,
        /// Write an ASCII raw file.
        #[arg(short, long)]
        raw: Option<PathBuf>,
        /// Print results and diagnostics as JSON on stdout.
        #[arg(long)]
        json: bool,
        /// Maximum number of threads (default: all cores).
        #[arg(short, long)]
        jobs: Option<usize>,
    },
    /// Run a fixed-topology Monte Carlo batch.
    Mc {
        netlist: PathBuf,
        #[arg(short = 'n', long)]
        runs: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(short, long)]
        jobs: Option<usize>,
        #[arg(long)]
        json: bool,
        /// Include the deck's saved waveforms in JSON.
        #[arg(long, requires = "json")]
        save_waves: bool,
    },
    /// Simulate several independent netlists in parallel and print one JSON line per netlist.
    Batch {
        /// Netlist files.
        #[arg(required = true)]
        netlists: Vec<PathBuf>,
        /// Maximum number of threads (default: all cores).
        #[arg(short, long)]
        jobs: Option<usize>,
    },
    /// Parse and elaborate a netlist without simulating it.
    Check {
        /// Netlist file.
        netlist: PathBuf,
        /// Print diagnostics as JSON on stdout.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum JsonOut<'a> {
    Ok {
        diagnostics: &'a [Diagnostic],
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<&'a SimResults>,
    },
    Error {
        diagnostics: &'a [Diagnostic],
        #[serde(skip_serializing_if = "Option::is_none")]
        partial: Option<&'a SimResults>,
    },
}

fn print_json(out: &impl Serialize) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    let ok = serde_json::to_writer(&mut stdout, out).is_ok() && writeln!(stdout).is_ok();
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

fn print_diags(diags: &[Diagnostic]) {
    for d in diags {
        eprintln!("{d}");
    }
}

fn summary(results: &SimResults) {
    for a in &results.analyses {
        match (a.kind, &a.data) {
            (AnalysisKind::Op, Data::Real { columns }) => {
                println!("Operating point:");
                for (v, c) in a.variables.iter().zip(columns) {
                    let val = c.first().copied().unwrap_or(f64::NAN);
                    println!("  {:<20} {:>16.6e} {}", v.name, val, v.quantity.unit());
                }
            }
            _ => println!(
                "{}: {} points, {} variables",
                a.kind.plot_name(),
                a.len(),
                a.variables.len()
            ),
        }
    }
    if !results.measures.is_empty() {
        println!("Measurements:");
        for m in &results.measures {
            match m.value {
                Some(v) => println!("  {:<20} {:>16.6e}", m.name, v),
                None => println!("  {:<20} {:>16}", m.name, "failed"),
            }
        }
    }
}

fn write_raw(path: &PathBuf, results: &SimResults) -> Result<(), ExitCode> {
    let file = std::fs::File::create(path).map_err(|e| {
        eprintln!("error: cannot create {}: {e}", path.display());
        ExitCode::from(2)
    })?;
    let mut w = std::io::BufWriter::new(file);
    spx_output::write_raw_ascii(results, &mut w)
        .and_then(|()| w.flush())
        .map_err(|e| {
            eprintln!("error: cannot write {}: {e}", path.display());
            ExitCode::from(2)
        })
}

/// Resolves `.include` / `.lib` relative to the netlist's directory.
fn loader(path: &std::path::Path) -> spx_netlist::FsLoader {
    spx_netlist::FsLoader {
        base: path
            .parent()
            .map_or_else(PathBuf::new, std::path::Path::to_path_buf),
    }
}

fn read(path: &PathBuf) -> Result<String, ExitCode> {
    let _phase = spx_diag::Phase::start("read");
    std::fs::read_to_string(path).map_err(|e| {
        eprintln!("error: cannot read {}: {e}", path.display());
        ExitCode::from(2)
    })
}

fn config(jobs: Option<usize>) -> spx_engine::RunConfig {
    spx_engine::RunConfig {
        threads: jobs.unwrap_or_else(spx_engine::default_threads).max(1),
    }
}

#[derive(Serialize)]
struct BatchLine<'a> {
    netlist: String,
    #[serde(flatten)]
    out: JsonOut<'a>,
}

fn batch(netlists: &[PathBuf], jobs: Option<usize>) -> Result<ExitCode, ExitCode> {
    let mut parsed = Vec::with_capacity(netlists.len());
    for p in netlists {
        let text = read(p)?;
        parsed.push(spx_netlist::parse_with(&text, &loader(p)));
    }
    let good: Vec<spx_netlist::Netlist> = parsed
        .iter()
        .filter_map(|r| r.as_ref().ok().cloned())
        .collect();
    let mut outputs = spx_engine::run_batch(&good, &config(jobs)).into_iter();
    let mut ok = true;
    let mut stdout = std::io::stdout().lock();
    for (path, r) in netlists.iter().zip(&parsed) {
        let run = match r {
            Ok(_) => outputs.next(),
            Err(_) => None,
        };
        let out = match (r, &run) {
            (Err(d), _) => JsonOut::Error {
                diagnostics: d,
                partial: None,
            },
            (Ok(_), Some(Ok(o))) => JsonOut::Ok {
                diagnostics: &o.warnings,
                results: Some(&o.results),
            },
            (Ok(_), Some(Err(e))) => JsonOut::Error {
                diagnostics: &e.diagnostics,
                partial: e.partial.as_ref(),
            },
            (Ok(_), None) => return Err(ExitCode::from(2)),
        };
        ok &= matches!(out, JsonOut::Ok { .. });
        let line = BatchLine {
            netlist: path.display().to_string(),
            out,
        };
        if serde_json::to_writer(&mut stdout, &line).is_err() || writeln!(stdout).is_err() {
            return Err(ExitCode::from(2));
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn mc(
    netlist: &PathBuf,
    n: usize,
    seed: u64,
    jobs: Option<usize>,
    json: bool,
    save_waves: bool,
) -> Result<ExitCode, ExitCode> {
    let text = read(netlist)?;
    let report = spx_engine::monte_carlo::MonteCarlo::prepare(&text, &loader(netlist))
        .and_then(|p| p.batch(seed, n, save_waves, &config(jobs)));
    match report {
        Ok(report) => {
            let failed = report.runs.iter().filter(|r| !r.success).count();
            let code = if json {
                print_json(&report)
            } else {
                println!(
                    "{} runs, {} failed, seed {}",
                    report.runs.len(),
                    failed,
                    seed
                );
                for r in &report.runs {
                    if !r.success {
                        eprintln!("Run {}", r.index);
                        print_diags(&r.diagnostics);
                    }
                    for m in &r.measures {
                        if let Some(v) = m.value {
                            println!("{} {} {v:e} {}", r.index, m.name, m.unit)
                        } else {
                            eprintln!(
                                "run {} {} failed: {}",
                                r.index,
                                m.name,
                                m.error.as_deref().unwrap_or("no value")
                            );
                        }
                    }
                }
                let display =
                    |v: Option<f64>| v.map_or_else(|| "n/a".into(), |x| format!("{x:.7e}"));
                for s in &report.statistics {
                    println!(
                        "{} (n={}): mean {}, std {}, range {}..{}, p05/p50/p95 {}/{}/{}",
                        s.name,
                        s.count,
                        display(s.mean),
                        display(s.std),
                        display(s.min),
                        display(s.max),
                        display(s.p05),
                        display(s.p50),
                        display(s.p95)
                    );
                }
                ExitCode::SUCCESS
            };
            Ok(if failed > 0 { code.max_with(1) } else { code })
        }
        Err(d) => Ok(input_error(&d, json)),
    }
}

fn input_error(d: &[Diagnostic], json: bool) -> ExitCode {
    if json {
        print_json(&JsonOut::Error {
            diagnostics: d,
            partial: None,
        })
        .max_with(1)
    } else {
        print_diags(d);
        ExitCode::from(1)
    }
}

fn run(
    netlist: &PathBuf,
    raw: Option<&PathBuf>,
    json: bool,
    jobs: Option<usize>,
    replay: Option<(u64, u64)>,
) -> Result<ExitCode, ExitCode> {
    let text = read(netlist)?;
    let cfg = config(jobs);
    let result = if let Some((seed, index)) = replay {
        match spx_engine::monte_carlo::MonteCarlo::prepare(&text, &loader(netlist)) {
            Ok(plan) => plan.run_with(seed, index, &cfg),
            Err(d) => return Ok(input_error(&d, json)),
        }
    } else {
        match spx_netlist::parse_with(&text, &loader(netlist)) {
            Ok(nl) => spx_engine::run_with(&nl, &cfg),
            Err(d) => return Ok(input_error(&d, json)),
        }
    };
    let print_result = |out: JsonOut<'_>| {
        #[derive(Serialize)]
        struct Replay<'a> {
            seed: u64,
            index: u64,
            #[serde(flatten)]
            out: JsonOut<'a>,
        }
        match replay {
            Some((seed, index)) => print_json(&Replay { seed, index, out }),
            None => print_json(&out),
        }
    };
    match result {
        Ok(out) => {
            let _phase = spx_diag::Phase::start("output");
            if let Some(p) = raw {
                write_raw(p, &out.results)?;
            }
            if json {
                return Ok(print_result(JsonOut::Ok {
                    diagnostics: &out.warnings,
                    results: Some(&out.results),
                }));
            }
            print_diags(&out.warnings);
            summary(&out.results);
            for t in &out.tran_stats {
                println!(
                    "Transient: {} accepted steps, {} rejected, {} Newton iterations, {} assemblies",
                    t.accepted_steps, t.rejected_steps, t.iterations, t.assemblies
                );
            }
            for dc in &out.dc_stats {
                println!(
                    "DC {}: {:.2} Newton iterations/point ({} iterations, {} extra solves)",
                    dc.source,
                    dc.iterations as f64 / dc.points as f64,
                    dc.iterations,
                    dc.extra_solves
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            if let (Some(p), Some(partial)) = (raw, &err.partial)
                && !partial.analyses.is_empty()
            {
                write_raw(p, partial)?;
            }
            if json {
                return Ok(print_result(JsonOut::Error {
                    diagnostics: &err.diagnostics,
                    partial: err.partial.as_ref(),
                })
                .max_with(1));
            }
            print_diags(&err.diagnostics);
            Ok(ExitCode::from(1))
        }
    }
}

fn check(netlist: &PathBuf, json: bool) -> Result<ExitCode, ExitCode> {
    let text = read(netlist)?;
    let result = spx_netlist::parse_with(&text, &loader(netlist))
        .and_then(|nl| spx_engine::check(&nl).map(|c| c.warnings));
    let (diags, ok) = match result {
        Ok(w) => (w, true),
        Err(d) => (d, false),
    };
    if json {
        let out = if ok {
            JsonOut::Ok {
                diagnostics: &diags,
                results: None,
            }
        } else {
            JsonOut::Error {
                diagnostics: &diags,
                partial: None,
            }
        };
        let code = print_json(&out);
        return Ok(if ok { code } else { code.max_with(1) });
    }
    print_diags(&diags);
    if ok {
        println!("ok");
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

trait MaxWith {
    fn max_with(self, code: u8) -> ExitCode;
}

impl MaxWith for ExitCode {
    /// Keep an I/O failure (2) but otherwise report `code`.
    fn max_with(self, code: u8) -> ExitCode {
        if self == ExitCode::from(2) {
            self
        } else {
            ExitCode::from(code)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match &cli.command {
        Command::Run {
            netlist,
            raw,
            json,
            jobs,
            mc_seed,
            mc_run,
        } => run(netlist, raw.as_ref(), *json, *jobs, mc_seed.zip(*mc_run)),
        Command::Mc {
            netlist,
            runs,
            seed,
            jobs,
            json,
            save_waves,
        } => mc(netlist, *runs, *seed, *jobs, *json, *save_waves),
        Command::Batch { netlists, jobs } => batch(netlists, *jobs),
        Command::Check { netlist, json } => check(netlist, *json),
    };
    r.unwrap_or_else(|code| code)
}
