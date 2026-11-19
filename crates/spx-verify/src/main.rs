//! `spx-verify`: run every deck in a directory with spx and a reference simulator and compare.
//!
//! ```text
//! spx-verify                                   # decks in verify/decks, ngspice via Docker
//! spx-verify --cached                          # reuse reference raw files in the output dir
//! spx-verify --reference "ngspice" deck.cir    # explicit reference command and decks
//! ```
//!
//! The reference command is invoked as `<cmd> -b <deck> -r <raw>` and must write an ASCII raw
//! file. Exit code 1 for a failed deck, 2 for a usage or setup error, and 0 otherwise.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod reference;

use reference::Problem;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use serde::Serialize;
use spx_verify::{
    DeckReport, Tolerances, compare, compare_measures, measure_deck, parse_measure_output,
};

#[derive(Debug, Parser)]
#[command(
    name = "spx-verify",
    about = "Compare spx against a reference simulator"
)]
struct Cli {
    /// Decks to verify (default: every `.cir` in `verify/decks`).
    decks: Vec<PathBuf>,
    /// Add every .cir in this directory (a positional directory works too).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Reference simulator command.
    #[arg(long, default_value = reference::WRAPPER)]
    reference: String,
    /// Directory for raw files.
    #[arg(long, default_value = "verify/out")]
    out: PathBuf,
    /// Reuse existing reference raw files instead of running the reference simulator.
    #[arg(long)]
    cached: bool,
    /// Print a JSON report.
    #[arg(long)]
    json: bool,
    /// Show every variable, not only the worst one per deck.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Serialize)]
struct Entry {
    deck: String,
    pass: bool,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    missing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_difference: Option<String>,
    #[serde(flatten)]
    report: DeckReport,
}

fn fail(msg: String) -> DeckReport {
    DeckReport {
        vars: Vec::new(),
        measures: Vec::new(),
        problems: vec![msg],
    }
}

fn known_difference(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let directive = line.trim_start().strip_prefix('*')?.trim_start();
        let reason = directive.strip_prefix("spx-verify: known-difference")?;
        (reason.starts_with(char::is_whitespace) && !reason.trim().is_empty())
            .then(|| reason.trim())
    })
}

fn verify(deck: &Path, cli: &Cli) -> Result<(DeckReport, Option<String>), Problem> {
    let deck = deck.canonicalize()?;
    let text = std::fs::read_to_string(&deck)?;
    let nl = reference::parse(&deck, &text)?;
    let prepared = reference::prepare(&deck, &text, &nl, cli)?;
    let stem = deck
        .file_stem()
        .map_or_else(|| "deck".into(), |s| s.to_string_lossy().into_owned());
    let ref_raw = cli.out.join(format!("{stem}.ref.raw"));
    let ours = match spx_engine::run(&nl) {
        Ok(o) => o.results,
        Err(e) => {
            return Err(Problem::Failed(format!(
                "simulation failed: {}",
                e.diagnostics
                    .first()
                    .map_or(String::new(), ToString::to_string)
            )));
        }
    };
    if let Ok(f) = std::fs::File::create(cli.out.join(format!("{stem}.spx.raw"))) {
        let mut w = std::io::BufWriter::new(f);
        // Best effort: the raw file is only kept for inspection.
        let _ = spx_output::write_raw_ascii(&ours, &mut w);
    }
    if cli.cached
        && text
            .lines()
            .any(|l| l.trim() == "* spx-verify: pending-reference")
        && matches!(ref_raw.try_exists(), Ok(false))
    {
        return Err(Problem::Missing(ref_raw.display().to_string()));
    }
    let reference = if cli.cached {
        read_reference(&ref_raw)?
    } else {
        let path = if nl.hdl.is_empty() {
            deck.clone()
        } else {
            let p = cli.out.join(format!("{stem}.ref.cir"));
            std::fs::write(&p, &prepared)?;
            p
        };
        // Only publish a cache entry after the command produces a readable plot.
        let pending = ref_raw.with_extension(format!("{}.raw", std::process::id()));
        std::fs::File::create(&pending)?;
        let fresh =
            run_reference(cli, &path, Some(&pending)).and_then(|_| read_reference(&pending));
        if fresh.is_err() {
            let _ = std::fs::remove_file(&pending);
        }
        let fresh = fresh?;
        std::fs::rename(pending, &ref_raw)?;
        fresh
    };
    let tol = Tolerances::from_deck(&text);
    let mut report = compare(&reference, &ours, &tol);
    if !ours.measures.is_empty() {
        let r = reference_measures(&prepared, &stem, &nl, cli)?;
        compare_measures(&r, &ours, &tol, &mut report);
    }
    Ok((report, known_difference(&text).map(str::to_owned)))
}

fn read_reference(path: &Path) -> Result<spx_output::SimResults, Problem> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Problem::Failed(format!("cannot read {}: {e}", path.display())))?;
    let results = spx_output::read_raw_ascii(&text)
        .map_err(|e| Problem::Failed(format!("{}: {e}", path.display())))?;
    if results.analyses.is_empty() {
        return Err(Problem::Failed(format!(
            "{} contains no supported reference plots",
            path.display()
        )));
    }
    Ok(results)
}

fn run_reference(
    cli: &Cli,
    path: &Path,
    raw: Option<&Path>,
) -> Result<std::process::Output, Problem> {
    let mut cmd = Command::new(&cli.reference);
    cmd.env("SPX_VERIFY_OUT", &cli.out).arg("-b").arg(path);
    if let Some(p) = raw {
        cmd.arg("-r").arg(p);
    }
    let output = cmd
        .output()
        .map_err(|e| Problem::Failed(format!("cannot run {}: {e}", cli.reference)))?;
    if !output.status.success() {
        return Err(Problem::Failed(format!(
            "reference simulator exited with {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output)
}

/// Run (or load cached) reference measurements.
fn reference_measures(
    text: &str,
    stem: &str,
    nl: &spx_netlist::Netlist,
    cli: &Cli,
) -> Result<Vec<(String, f64)>, Problem> {
    let log = cli.out.join(format!("{stem}.ref.meas"));
    if !cli.cached {
        let deck = measure_deck(text).ok_or_else(|| Problem::Failed("no .meas cards".into()))?;
        let path = cli.out.join(format!("{stem}.meas.cir"));
        std::fs::write(&path, deck)?;
        let output = run_reference(cli, &path, None)?;
        std::fs::write(&log, &output.stdout)?;
    }
    let out = std::fs::read_to_string(&log)
        .map_err(|e| Problem::Failed(format!("cannot read {}: {e}", log.display())))?;
    let names = nl
        .measures
        .iter()
        .map(|m| m.name.clone())
        .collect::<Vec<_>>();
    Ok(parse_measure_output(&out, &names))
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let mut inputs = cli.decks.clone();
    inputs.extend(cli.dir.clone());
    if inputs.is_empty() {
        inputs.push("verify/decks".into());
    }
    let mut decks = Vec::new();
    for path in inputs {
        if path.is_dir() {
            let listing = std::fs::read_dir(&path).and_then(|rd| {
                rd.map(|e| e.map(|e| e.path()))
                    .collect::<Result<Vec<_>, _>>()
            });
            match listing {
                Ok(mut paths) => {
                    paths.retain(|p| p.extension().is_some_and(|x| x == "cir"));
                    paths.sort();
                    decks.extend(paths);
                }
                Err(e) => {
                    eprintln!("cannot list {}: {e}", path.display());
                    return ExitCode::from(2);
                }
            }
        } else {
            decks.push(path);
        }
    }
    if let Err(e) = std::fs::create_dir_all(&cli.out) {
        eprintln!("error: cannot create {}: {e}", cli.out.display());
        return ExitCode::from(2);
    }
    match cli.out.canonicalize() {
        Ok(path) => cli.out = path,
        Err(e) => {
            eprintln!("cannot resolve output directory: {e}");
            return ExitCode::from(2);
        }
    }
    let mut entries = Vec::new();
    for d in &decks {
        let (report, missing, known_difference) = match verify(d, &cli) {
            Ok((r, reason)) => (r, None, reason),
            Err(Problem::Failed(e)) => (fail(e), None, None),
            Err(Problem::Missing(path)) => (
                DeckReport {
                    vars: vec![],
                    measures: vec![],
                    problems: vec![],
                },
                Some(path),
                None,
            ),
        };
        let pass = missing.is_none() && report.pass();
        entries.push(Entry {
            deck: d.display().to_string(),
            pass,
            status: if missing.is_some() {
                "SKIP"
            } else if known_difference.is_some() && report.problems.is_empty() {
                "KNOWN"
            } else if pass {
                "PASS"
            } else {
                "FAIL"
            },
            missing,
            known_difference,
            report,
        });
    }
    // Keep the JSON array and its entry schema compatible. Group annotated
    // comparisons at the end in both report formats, preserving deck order.
    entries.sort_by_key(|e| e.status == "KNOWN");
    let all = entries.iter().all(|e| e.status != "FAIL");
    if cli.json {
        match serde_json::to_string_pretty(&entries) {
            Ok(s) => println!("{s}"),
            Err(_) => return ExitCode::from(2),
        }
    } else {
        let known_start = entries.iter().position(|e| e.status == "KNOWN");
        for (index, e) in entries.iter().enumerate() {
            if Some(index) == known_start {
                println!("Known differences");
            }
            println!("{} {}", e.status, e.deck);
            if let Some(path) = &e.missing {
                println!("    missing input: {path}");
            }
            if let Some(reason) = &e.known_difference {
                println!("    known difference: {reason}");
            }
            for p in &e.report.problems {
                println!("    problem: {p}");
            }
            let mut vars: Vec<_> = e.report.vars.iter().collect();
            vars.sort_by(|a, b| b.score.total_cmp(&a.score));
            let shown = if cli.verbose {
                vars.len()
            } else {
                1.min(vars.len())
            };
            for m in &e.report.measures {
                if cli.verbose || m.score > 1.0 {
                    println!(
                        "    meas  {:<14} score {:>8.3}  ours {:>12} ref {:.6e}",
                        m.name,
                        m.score,
                        m.ours.map_or("failed".into(), |v| format!("{v:.6e}")),
                        m.reference
                    );
                }
            }
            for v in &vars[..shown] {
                println!(
                    "    {:<5} {:<14} score {:>8.3}  max|Δ| {:.3e} at {:.4e} ({} pts)",
                    format!("{:?}", v.analysis).to_lowercase(),
                    v.name,
                    v.score,
                    v.max_abs,
                    v.at,
                    v.points
                );
            }
        }
        let passed = entries.iter().filter(|e| e.status == "PASS").count();
        let skipped = entries.iter().filter(|e| e.missing.is_some()).count();
        let known = entries.iter().filter(|e| e.status == "KNOWN").count();
        println!(
            "{passed}/{} decks passed, {known} known, {skipped} skipped",
            entries.len() - skipped - known
        );
    }
    if all {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
