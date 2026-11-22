//! `sansho-verify`: run every deck in a directory with Sansho and a reference simulator and compare.
//!
//! ```text
//! sansho-verify                                   # decks in verify/decks, ngspice via Docker
//! sansho-verify --cached                          # reuse reference raw files in the output dir
//! sansho-verify --reference "ngspice" deck.cir    # explicit reference command and decks
//! ```
//!
//! The reference command is invoked as `<cmd> -b <deck> -r <raw>` and must write an ASCII raw
//! file. Exit code 1 for a failed deck, 2 for a usage or setup error, and 0 otherwise.
//!
//! Provenance: original work. See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod control;
mod control_analytic;
#[cfg(test)]
mod digital_analytic;
mod pss_analytic;
mod pss_ng;
mod reference;

use reference::Problem;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use sansho_verify::{
    DeckReport, Tolerances, compare, compare_measures, measure_deck, parse_measure_output,
};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "sansho-verify",
    about = "Compare Sansho against a reference simulator"
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

fn directive<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let directive = line.trim_start().strip_prefix('*')?.trim_start();
        let reason = directive.strip_prefix(key)?;
        (reason.starts_with(char::is_whitespace) && !reason.trim().is_empty())
            .then(|| reason.trim())
    })
}

fn known_difference(text: &str) -> Option<&str> {
    directive(text, "sansho-verify: known-difference")
}

// Require both a real batch failure and the designated full diagnostic line.
fn known_rejection(text: &str, log: &str) -> Option<Problem> {
    let expected = directive(text, "sansho-verify: reference-error")?;
    let reason = known_difference(text)?;
    log.strip_prefix("reference simulator exited with exit status: 1:")?
        .lines()
        .any(|line| line.trim() == expected)
        .then(|| Problem::KnownRejection(format!("{reason} [reference diagnostic: {expected}]")))
}

/// Match the exact ordered inventory of absent sensitivity columns on an annotated deck.
fn sensitivity_gap(
    reference: &mut sansho_output::SimResults,
    ours: &sansho_output::SimResults,
    expected: &str,
) -> Result<String, Problem> {
    use sansho_output::AnalysisKind::Sens;
    let ([r], [o]) = (reference.analyses.as_mut_slice(), ours.analyses.as_slice()) else {
        return Err(Problem::Failed(
            "missing-sens requires one sensitivity plot".into(),
        ));
    };
    if r.kind != Sens || o.kind != Sens {
        return Err(Problem::Failed(
            "missing-sens requires sensitivity plots".into(),
        ));
    }
    let absent: Vec<_> = r
        .variables
        .iter()
        .filter(|v| o.index_of(&v.name).is_none())
        .collect();
    let raw_names: Vec<_> = absent
        .iter()
        .map(|v| {
            let prefix = if v.quantity == sansho_output::Quantity::Current {
                "i"
            } else {
                "v"
            };
            format!("{prefix}({})", v.name)
        })
        .collect();
    if !raw_names
        .iter()
        .map(String::as_str)
        .eq(expected.split_whitespace())
    {
        return Err(Problem::Failed(format!(
            "sensitivity coverage changed: expected missing {expected}, found {}",
            raw_names.join(" ")
        )));
    }
    for v in absent {
        let name = &v.name;
        let zero = r.real(name).is_some_and(|col| col.iter().all(|x| *x == 0.))
            || r.complex(name)
                .is_some_and(|col| col.iter().all(|x| *x == (0., 0.)));
        if !zero {
            return Err(Problem::Failed(format!(
                "missing sensitivity {name} has nonzero reference values"
            )));
        }
    }
    r.retain_variables(|v| o.index_of(&v.name).is_some());
    Ok(format!("Missing sensitivity vectors: {expected}"))
}

fn verify(deck: &Path, cli: &Cli) -> Result<(DeckReport, Option<String>, bool), Problem> {
    let deck = deck.canonicalize()?;
    let text = std::fs::read_to_string(&deck)?;
    let nl = reference::parse(&deck, &text)?;
    let prepared = reference::prepare(&deck, &text, &nl, cli)?;
    if !nl.controls.is_empty() {
        let (report, analytic) = control::verify(
            &deck,
            &text,
            &prepared,
            &nl,
            cli,
            &Tolerances::from_deck(&text),
        )?;
        return Ok((report, known_difference(&text).map(str::to_owned), analytic));
    }
    let stem = deck
        .file_stem()
        .map_or_else(|| "deck".into(), |s| s.to_string_lossy().into_owned());
    let ref_raw = cli.out.join(format!("{stem}.ref.raw"));
    let ours = match sansho_engine::run(&nl) {
        Ok(o) => {
            if directive(&text, "sansho-verify: pss-ng").is_some()
                && let Some(warning) = o.warnings.iter().find(|w| w.code == "W-CONV-PSS")
            {
                return Err(Problem::Failed(warning.to_string()));
            }
            o.results.into_raw_output()
        }
        Err(e) => {
            return Err(Problem::Failed(format!(
                "simulation failed: {}",
                e.diagnostics
                    .first()
                    .map_or(String::new(), ToString::to_string)
            )));
        }
    };
    if let Ok(f) = std::fs::File::create(cli.out.join(format!("{stem}.sansho.raw"))) {
        let mut w = std::io::BufWriter::new(f);
        // Best effort: the raw file is only kept for inspection.
        let _ = sansho_output::write_raw_ascii(&ours, &mut w);
    }
    if let Some(case) = directive(&text, "sansho-verify: pss-analytic") {
        return Ok((pss_analytic::check(case, &ours)?, None, true));
    }
    if cli.cached
        && text
            .lines()
            .any(|l| l.trim() == "* sansho-verify: pending-reference")
        && matches!(ref_raw.try_exists(), Ok(false))
    {
        return Err(Problem::Missing(ref_raw.display().to_string()));
    }
    if let Some(signal) = directive(&text, "sansho-verify: pss-ng") {
        let [sansho_netlist::AnalysisCard::Pss { spec, .. }] = nl.analyses.as_slice() else {
            return Err(Problem::Failed(
                "PSS oracle decks require a single .pss analysis".into(),
            ));
        };
        return pss_ng::verify(cli, &deck, &stem, &ours, spec, signal);
    }
    let error_log = cli.out.join(format!("{stem}.ref.error"));
    if cli.cached
        && directive(&text, "sansho-verify: reference-error").is_some()
        && !ref_raw.try_exists()?
    {
        let log = std::fs::read_to_string(&error_log)
            .map_err(|e| Problem::Failed(format!("cannot read {}: {e}", error_log.display())))?;
        return Err(known_rejection(&text, &log).unwrap_or(Problem::Failed(log)));
    }
    let mut reference = if cli.cached {
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
        if let Err(Problem::Failed(log)) = &fresh
            && let Some(known) = known_rejection(&text, log)
        {
            let tmp = error_log.with_extension(format!("{}.error", std::process::id()));
            std::fs::write(&tmp, log)?;
            std::fs::rename(tmp, &error_log)?;
            // A previous valid raw, if present, remains available for comparison in cached mode.
            return Err(known);
        }
        let fresh = fresh?;
        std::fs::rename(pending, &ref_raw)?;
        fresh
    };
    // Batch raw files contain the transient used by ngspice's Fourier report.
    // Compare amplitudes and distortion derived from that independent trajectory.
    let derive_fourier = !reference
        .analyses
        .iter()
        .any(|a| a.kind == sansho_output::AnalysisKind::Four);
    if derive_fourier {
        for card in &nl.analyses {
            if let sansho_netlist::AnalysisCard::Four {
                frequency, outputs, ..
            } = card
                && let Some(tran) = reference
                    .analyses
                    .iter()
                    .find(|a| a.kind == sansho_output::AnalysisKind::Tran)
            {
                let harmonics = sansho_engine::fourier(tran, *frequency, outputs)
                    .map_err(|d| Problem::Failed(d.to_string()))?;
                reference.analyses.push(harmonics);
            }
        }
    }
    let tol = Tolerances::from_deck(&text);
    let gap = directive(&text, "sansho-verify: missing-sens")
        .map(|list| sensitivity_gap(&mut reference, &ours, list))
        .transpose()?;
    let mut report = compare(&reference, &ours, &tol);
    if reference
        .analyses
        .iter()
        .map(|a| a.kind)
        .ne(ours.analyses.iter().map(|a| a.kind))
    {
        report.problems.push("batch plot order differs".into());
    }
    if derive_fourier {
        // The complete derived header was compared. Phase at a
        // symmetry-forbidden harmonic has no independent numerical reference.
        report.vars.retain(|v| {
            v.analysis != sansho_output::AnalysisKind::Four || !v.name.starts_with("phase(")
        });
    }
    if !ours.measures.is_empty() {
        let r = reference_measures(&prepared, &stem, &nl, cli)?;
        compare_measures(&r, &ours, &tol, &mut report);
    }
    // A successful oracle run supersedes an annotation for a specific rejection.
    let reason = if directive(&text, "sansho-verify: reference-error").is_some() {
        None
    } else {
        known_difference(&text).map(str::to_owned)
    };
    if let Some(gap) = gap {
        let known_value = directive(&text, "sansho-verify: known-sens-value");
        if let Some(v) = report
            .vars
            .iter()
            .find(|v| !v.pass() && Some(v.name.as_str()) != known_value)
        {
            report
                .problems
                .push(format!("unexpected sensitivity error in {}", v.name));
        }
        if report.measures.iter().any(|m| m.score > 1.) {
            report
                .problems
                .push("unexpected measurement difference".into());
        }
        return Ok((
            report,
            Some(reason.map_or_else(|| gap.clone(), |r| format!("{r}. {gap}"))),
            false,
        ));
    }
    Ok((report, reason, false))
}

fn read_reference(path: &Path) -> Result<sansho_output::SimResults, Problem> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Problem::Failed(format!("cannot read {}: {e}", path.display())))?;
    let results = sansho_output::read_raw_ascii(&text)
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
    cmd.env("SANSHO_VERIFY_OUT", &cli.out).arg("-b").arg(path);
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
    nl: &sansho_netlist::Netlist,
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
        let (report, missing, known_difference, analytic) = match verify(d, &cli) {
            Ok((r, reason, analytic)) => (r, None, reason, analytic),
            Err(Problem::Failed(e)) => (fail(e), None, None, false),
            Err(Problem::KnownRejection(reason)) => {
                entries.push(Entry {
                    deck: d.display().to_string(),
                    status: "KNOWN",
                    pass: false,
                    known_difference: Some(reason),
                    missing: None,
                    report: DeckReport {
                        vars: vec![],
                        measures: vec![],
                        problems: vec![],
                    },
                });
                continue;
            }
            Err(Problem::Missing(path)) => (
                DeckReport {
                    vars: vec![],
                    measures: vec![],
                    problems: vec![],
                },
                Some(path),
                None,
                false,
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
            } else if pass && analytic {
                "ANALYTIC"
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
        let analytic = entries.iter().filter(|e| e.status == "ANALYTIC").count();
        if analytic > 0 {
            println!("{analytic} decks passed independent analytic checks");
        }
        println!(
            "{passed}/{} decks passed, {known} known, {skipped} skipped",
            entries.len() - skipped - known - analytic
        );
    }
    if all {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sansho_output::read_raw_ascii;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn missing_sensitivity_annotation_only_accepts_the_listed_zeros() {
        let raw = "Title: sensitivity\nPlotname: Sensitivity Analysis\nFlags: real\nNo. Variables: 3\nNo. Points: 1\nVariables:\n0 v(r:short) voltage\n1 v(r) voltage\n2 v(vin) voltage\nValues:\n0 0\n1\n2\n";
        let reference = read_raw_ascii(raw).unwrap();
        let mut ours = reference.clone();
        ours.analyses[0].retain_variables(|v| v.name != "r:short");
        let mut shared = reference.clone();
        assert!(sensitivity_gap(&mut shared, &ours, "v(r:short)").is_ok());
        assert!(compare(&shared, &ours, &Tolerances::default()).pass());
        let mut nonzero = read_raw_ascii(&raw.replace("0 0\n1", "0 0.01\n1")).unwrap();
        assert!(sensitivity_gap(&mut nonzero, &ours, "v(r:short)").is_err());
        ours.analyses[0].variables.swap(0, 1);
        assert!(!compare(&shared, &ours, &Tolerances::default()).pass());
        ours.analyses[0].retain_variables(|v| v.name != "r");
        assert!(sensitivity_gap(&mut reference.clone(), &ours, "v(r:short)").is_err());
    }
}
