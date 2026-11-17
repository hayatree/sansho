//! `spx-verify`: run every deck in a directory with spx and a reference simulator and compare.
//!
//! ```text
//! spx-verify                                   # decks in verify/decks, ngspice via Docker
//! spx-verify --cached                          # reuse reference raw files in the output dir
//! spx-verify --reference "ngspice" deck.cir    # explicit reference command and decks
//! ```
//!
//! The reference command is invoked as `<cmd> -b <deck> -r <raw>` and must write an ASCII raw
//! file. Exit code 0 when every deck passes, 1 otherwise, 2 on usage or I/O errors.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

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
    /// Reference simulator command.
    #[arg(long, default_value = "tools/ngspice/ngspice.sh")]
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

fn verify(deck: &Path, cli: &Cli) -> DeckReport {
    let text = match std::fs::read_to_string(deck) {
        Ok(t) => t,
        Err(e) => return fail(format!("cannot read deck: {e}")),
    };
    let stem = deck
        .file_stem()
        .map_or_else(|| "deck".into(), |s| s.to_string_lossy().into_owned());
    let ref_raw = cli.out.join(format!("{stem}.ref.raw"));
    if !cli.cached {
        let status = Command::new(&cli.reference)
            .arg("-b")
            .arg(deck)
            .arg("-r")
            .arg(&ref_raw)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => return fail(format!("reference simulator exited with {s}")),
            Err(e) => return fail(format!("cannot run `{}`: {e}", cli.reference)),
        }
    }
    let reference = match std::fs::read_to_string(&ref_raw)
        .map_err(|e| e.to_string())
        .and_then(|t| spx_output::read_raw_ascii(&t).map_err(|e| e.to_string()))
    {
        Ok(r) => r,
        Err(e) => return fail(format!("cannot read reference results: {e}")),
    };
    let nl = match spx_netlist::parse_file(deck) {
        Ok(nl) => nl,
        Err(d) => return fail(format!("parse failed: {}", d[0])),
    };
    let ours = match spx_engine::run(&nl) {
        Ok(o) => o.results,
        Err(e) => {
            return fail(format!(
                "simulation failed: {}",
                e.diagnostics
                    .first()
                    .map_or(String::new(), ToString::to_string)
            ));
        }
    };
    if let Ok(f) = std::fs::File::create(cli.out.join(format!("{stem}.spx.raw"))) {
        let mut w = std::io::BufWriter::new(f);
        // Best effort: the raw file is only kept for inspection.
        let _ = spx_output::write_raw_ascii(&ours, &mut w);
    }
    let tol = Tolerances::from_deck(&text);
    let mut report = compare(&reference, &ours, &tol);
    if !ours.measures.is_empty() {
        match reference_measures(&text, &stem, cli) {
            Ok(r) => compare_measures(&r, &ours, &tol, &mut report),
            Err(e) => report.problems.push(e),
        }
    }
    report
}

/// Run (or load cached) reference measurements.
fn reference_measures(text: &str, stem: &str, cli: &Cli) -> Result<Vec<(String, f64)>, String> {
    let log = cli.out.join(format!("{stem}.ref.meas"));
    if !cli.cached {
        let deck = measure_deck(text).ok_or("no .meas cards")?;
        let path = cli.out.join(format!("{stem}.meas.cir"));
        std::fs::write(&path, deck).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        let output = Command::new(&cli.reference)
            .arg("-b")
            .arg(&path)
            .output()
            .map_err(|e| format!("cannot run `{}`: {e}", cli.reference))?;
        std::fs::write(&log, &output.stdout)
            .map_err(|e| format!("cannot write {}: {e}", log.display()))?;
    }
    let out =
        std::fs::read_to_string(&log).map_err(|e| format!("cannot read {}: {e}", log.display()))?;
    let names: Vec<String> = spx_netlist::parse(text)
        .map(|nl| nl.measures.iter().map(|m| m.name.clone()).collect())
        .unwrap_or_default();
    Ok(parse_measure_output(&out, &names))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut decks = cli.decks.clone();
    if decks.is_empty() {
        match std::fs::read_dir("verify/decks") {
            Ok(rd) => {
                decks = rd
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "cir"))
                    .collect();
                decks.sort();
            }
            Err(e) => {
                eprintln!("error: cannot list verify/decks: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if let Err(e) = std::fs::create_dir_all(&cli.out) {
        eprintln!("error: cannot create {}: {e}", cli.out.display());
        return ExitCode::from(2);
    }
    let mut entries = Vec::new();
    for d in &decks {
        let report = verify(d, &cli);
        entries.push(Entry {
            deck: d.display().to_string(),
            pass: report.pass(),
            report,
        });
    }
    let all = entries.iter().all(|e| e.pass);
    if cli.json {
        match serde_json::to_string_pretty(&entries) {
            Ok(s) => println!("{s}"),
            Err(_) => return ExitCode::from(2),
        }
    } else {
        for e in &entries {
            println!("{} {}", if e.pass { "PASS" } else { "FAIL" }, e.deck);
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
        let passed = entries.iter().filter(|e| e.pass).count();
        println!("{passed}/{} decks passed", entries.len());
    }
    if all {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
