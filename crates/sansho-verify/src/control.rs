//! Compare the actual files and ordered measurements emitted by batch control decks.
//! Provenance: original black-box comparison using documented ngspice batch and raw output.

use super::{Cli, DeckReport, Problem, Tolerances, compare};
use sansho_api::control::{ControlOptions, execute_with};
use sansho_netlist::Netlist;
use sansho_output::SimResults;
use sansho_verify::MeasReport;
use std::{collections::BTreeSet, fs, path::Path, process::Command};

/// Normalize a native-double binary payload to the existing ASCII raw reader's input.
/// Counts are checked against the remaining byte length before visiting any records.
fn raw(mut bytes: &[u8]) -> Result<SimResults, Problem> {
    use std::fmt::Write;
    let mut ascii = String::new();
    let mut vars = 0usize;
    let mut points = 0usize;
    let mut complex = false;
    while !bytes.is_empty() {
        let n = bytes
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(bytes.len());
        let line = std::str::from_utf8(&bytes[..n])
            .map_err(|_| Problem::Failed("invalid raw header".into()))?;
        bytes = &bytes[(n + 1).min(bytes.len())..];
        if let Some((key, value)) = line.split_once(':') {
            match key.trim().to_ascii_lowercase().as_str() {
                "no. variables" => {
                    vars = value
                        .trim()
                        .parse()
                        .map_err(|_| Problem::Failed("invalid raw variable count".into()))?
                }
                "no. points" => {
                    points = value
                        .trim()
                        .parse()
                        .map_err(|_| Problem::Failed("invalid raw point count".into()))?
                }
                "flags" => complex = value.to_ascii_lowercase().contains("complex"),
                "binary" => {
                    let width = if complex { 16 } else { 8 };
                    let size = vars
                        .checked_mul(points)
                        .and_then(|n| n.checked_mul(width))
                        .filter(|&n| n <= bytes.len() && n > 0)
                        .ok_or_else(|| Problem::Failed("invalid binary raw dimensions".into()))?;
                    let (data, rest) = bytes.split_at(size);
                    bytes = rest;
                    ascii.push_str("Values:\n");
                    let mut values = data
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .copied()
                        .map(f64::from_ne_bytes);
                    for i in 0..points {
                        let _ = write!(ascii, "{i}");
                        for _ in 0..vars {
                            let _ = write!(
                                ascii,
                                " {}",
                                values
                                    .next()
                                    .ok_or_else(|| Problem::Failed("short binary raw".into()))?
                            );
                            if complex {
                                let _ = write!(
                                    ascii,
                                    ",{}",
                                    values.next().ok_or_else(|| Problem::Failed(
                                        "short complex raw".into()
                                    ))?
                                );
                            }
                        }
                        ascii.push('\n');
                    }
                    continue;
                }
                _ => {}
            }
        }
        ascii.push_str(line);
        ascii.push('\n');
    }
    sansho_output::read_raw_ascii(&ascii).map_err(|e| Problem::Failed(e.to_string()))
}

fn compare_text(
    name: &str,
    reference: &str,
    ours: &str,
    tol: &Tolerances,
    report: &mut DeckReport,
) {
    let rows = |s: &str| s.lines().filter(|l| !l.trim().is_empty()).count();
    if rows(reference) != rows(ours) || rows(ours) == 0 {
        report
            .problems
            .push(format!("{name}: different or empty row counts"));
        return;
    }
    let mut worst = 0.0f64;
    for (i, (r, o)) in reference
        .lines()
        .filter(|l| !l.trim().is_empty())
        .zip(ours.lines().filter(|l| !l.trim().is_empty()))
        .enumerate()
    {
        let r: Vec<_> = r.split_whitespace().collect();
        let o: Vec<_> = o.split_whitespace().collect();
        if r.len() != o.len() {
            report
                .problems
                .push(format!("{name} row {}: column count differs", i + 1));
            return;
        }
        for (x, y) in r.into_iter().zip(o) {
            match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(a), Ok(b)) if a.is_finite() && b.is_finite() => {
                    worst = worst.max((a - b).abs() / (tol.atol_v + tol.rtol * a.abs()))
                }
                (Err(_), Err(_)) if x == y => {}
                _ => {
                    report.problems.push(format!(
                        "{name} row {}: invalid or mismatched fields {x}, {y}",
                        i + 1
                    ));
                    return;
                }
            }
        }
    }
    if worst > 1. {
        report
            .problems
            .push(format!("{name}: normalized error {worst}"))
    }
}
/// A measurement may appear repeatedly inside a loop. Preserve every occurrence.
fn compare_log(
    log: &str,
    ours: &sansho_api::control::Transcript,
    tol: &Tolerances,
    report: &mut DeckReport,
) {
    let names: BTreeSet<_> = ours.measurements.iter().map(|m| m.name.as_str()).collect();
    let record = |line: &str| {
        let (k, rest) = line.split_once('=')?;
        let k = k.trim().to_ascii_lowercase();
        if !names.contains(k.as_str()) {
            return None;
        }
        Some((k, rest.split_whitespace().next()?.parse::<f64>().ok()?))
    };
    // A script can also print a measurement explicitly. Match those records in order,
    // so duplicate names cannot accidentally compare every loop iteration to the first.
    let expected: Vec<_> = ours.lines.iter().filter_map(|l| record(l)).collect();
    let actual: Vec<_> = log.lines().filter_map(record).collect();
    if expected.len() != actual.len() {
        report.problems.push(format!(
            "{} reference measurement records for {} printed values",
            actual.len(),
            expected.len()
        ));
        return;
    }
    for ((name, r), (k, x)) in actual.into_iter().zip(expected) {
        if name != k || !r.is_finite() || !x.is_finite() {
            report
                .problems
                .push(format!("unexpected measurement record {name}"));
            continue;
        }
        report.measures.push(MeasReport {
            name: k,
            reference: r,
            ours: Some(x),
            score: (x - r).abs() / (tol.rtol_meas * r.abs() + 1e-30),
        });
    }
}

pub(super) fn verify(
    deck: &Path,
    text: &str,
    prepared: &str,
    nl: &Netlist,
    cli: &Cli,
    tol: &Tolerances,
) -> Result<(DeckReport, bool), Problem> {
    let stem = deck.file_stem().unwrap_or_default().to_string_lossy();
    let ours = cli.out.join(format!("{stem}.sansho.control"));
    fs::create_dir_all(&ours)?;
    let out = execute_with(
        nl,
        &ControlOptions {
            output_dir: Some(ours.clone()),
            ..Default::default()
        },
        1,
    );
    if let Some(e) = out.diagnostics.iter().find(|e| e.is_error()) {
        return Err(Problem::Failed(e.to_string()));
    }
    let reference_dir = cli.out.join(format!("{stem}.ref.control"));
    // Keep the reference stdout separate from deck-chosen filenames.
    let log = cli.out.join(format!("{stem}.ref.control.log"));
    let mut analytic = None;
    if !cli.cached {
        // Start in an empty directory so missing files cannot pass via stale outputs.
        let fresh = cli
            .out
            .join(format!("{stem}.control-stage-{}", std::process::id()));
        fs::create_dir(&fresh)?;
        let result = (|| {
            let path = cli.out.join(format!("{stem}.ref.control.cir"));
            fs::write(&path, prepared)?;
            let exe = Path::new(&cli.reference);
            let exe = if exe.components().count() > 1 {
                exe.canonicalize()?
            } else {
                exe.to_owned()
            };
            let result = Command::new(exe)
                .current_dir(&fresh)
                .env("SANSHO_VERIFY_OUT", &cli.out)
                .arg("-b")
                .arg(&path)
                .output()?;
            if !result.status.success() {
                return Err(Problem::Failed(format!(
                    "reference failed: {}{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                )));
            }
            for name in &out.transcript.written_files {
                if !fresh.join(name).is_file() {
                    return Err(Problem::Failed(format!("reference did not write {name}")));
                }
            }
            fs::create_dir_all(&reference_dir)?;
            for name in out.transcript.written_files.iter().collect::<BTreeSet<_>>() {
                let path = reference_dir.join(name);
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::copy(fresh.join(name), path)?;
            }
            fs::write(&log, result.stdout)?;
            Ok(())
        })();
        fs::remove_dir_all(fresh)?;
        result?;
    } else if !log.try_exists()? && !reference_dir.try_exists()? {
        analytic = Some(
            crate::control_analytic::Analytic::for_deck(text).ok_or_else(|| {
                Problem::Failed(format!("missing control reference {}", log.display()))
            })?,
        );
    }
    let mut report = DeckReport {
        vars: vec![],
        measures: vec![],
        problems: vec![],
    };
    let reference_log = match &analytic {
        Some(a) => a.log.clone(),
        None => fs::read_to_string(log)?,
    };
    compare_log(&reference_log, &out.transcript, tol, &mut report);
    if let Some(a) = &analytic
        && a.files.keys().collect::<BTreeSet<_>>() != out.transcript.written_files.iter().collect()
    {
        report
            .problems
            .push("expected and written file sets differ".into())
    }
    if out.transcript.written_files.is_empty() && out.results.measures.is_empty() {
        report
            .problems
            .push("control deck produced no files or measurements to compare".into())
    }
    for name in out.transcript.written_files.iter().collect::<BTreeSet<_>>() {
        let r = match &analytic {
            Some(a) => a
                .files
                .get(name)
                .cloned()
                .ok_or_else(|| Problem::Failed(format!("unexpected output {name}")))?,
            None => fs::read(reference_dir.join(name))?,
        };
        let o = fs::read(ours.join(name))?;
        if o.starts_with(b"Title:") {
            let r = raw(&r)?;
            let o = raw(&o)?;
            // ngspice may wrap a user-defined voltage vector in v(...).
            let mut r = r;
            for (a, b) in r.analyses.iter_mut().zip(&o.analyses) {
                if a.variables.len() != b.variables.len() {
                    report
                        .problems
                        .push(format!("{name}: raw column count differs"));
                }
                for v in &mut a.variables {
                    if b.index_of(&v.name).is_none()
                        && let Some(n) = v.name.strip_prefix("v(").and_then(|s| s.strip_suffix(')'))
                        && b.index_of(n).is_some()
                    {
                        v.name = n.into()
                    }
                    if b.index_of(&v.name).is_none() {
                        report.problems.push(format!("{name}: missing {}", v.name));
                    }
                }
                if a.scale_values.len() != b.scale_values.len()
                    || a.scale_values
                        .iter()
                        .zip(&b.scale_values)
                        .any(|(r, o)| (r - o).abs() > 1e-12 * r.abs().max(1e-15))
                {
                    report.problems.push(format!("{name}: raw scales differ"));
                }
            }
            let cmp = compare(&r, &o, tol);
            report.vars.extend(cmp.vars);
            report.problems.extend(cmp.problems);
        } else {
            let utf8 = |b| {
                std::str::from_utf8(b).map_err(|_| Problem::Failed(format!("{name}: invalid text")))
            };
            compare_text(name, utf8(&r)?, utf8(&o)?, tol, &mut report)
        }
    }
    Ok((report, analytic.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(clippy::unwrap_used)]
    fn native_binary_records_across_plot_boundaries() {
        use sansho_output::{AnalysisKind, AnalysisResult, Data, Quantity, Variable};
        let mut a = AnalysisResult {
            events: Default::default(),
            kind: AnalysisKind::Ac,
            scale: Some(Variable {
                name: "frequency".into(),
                quantity: Quantity::Frequency,
            }),
            scale_values: vec![1., 2.],
            variables: vec![Variable {
                name: "v(a)".into(),
                quantity: Quantity::Voltage,
            }],
            data: Data::Complex {
                columns: vec![vec![(1., -1.), (2., -2.)]],
            },
        };
        let mut r = SimResults {
            title: "raw records".into(),
            analyses: vec![a.clone()],
            prints: vec![],
            measures: vec![],
        };
        a.kind = AnalysisKind::Tran;
        a.data = Data::Real {
            columns: vec![vec![0., 2.]],
        };
        r.analyses.push(a);
        let mut bytes = vec![];
        sansho_output::write_raw_binary(&r, &mut bytes).unwrap();
        assert_eq!(raw(&bytes).unwrap(), r);
        bytes.pop();
        assert!(raw(&bytes).is_err());
    }
    #[test]
    fn text_mismatch_and_repeated_measurement_order() {
        let mut report = DeckReport {
            vars: vec![],
            measures: vec![],
            problems: vec![],
        };
        let tol = Tolerances::default();
        compare_text(
            "out",
            "frequency signal\n1.0 2.0\n",
            "frequency signal\n1.0000 2.00001\n",
            &tol,
            &mut report,
        );
        assert!(report.pass());
        compare_text("out", "1 2\n", "1 200\n", &tol, &mut report);
        assert!(!report.pass());
        report.problems.clear();
        let results = sansho_api::control::Transcript {
            lines: vec!["r = 1".into(), "r = 2".into()],
            measurements: [1., 2.]
                .map(|v| sansho_output::Measurement {
                    name: "r".into(),
                    analysis: "dc".into(),
                    value: Some(v),
                    unit: "V".into(),
                    error: None,
                })
                .to_vec(),
            ..Default::default()
        };
        compare_log("r = 1\nr = 2", &results, &tol, &mut report);
        assert!(report.pass());
        compare_log("r = 2\nr = 1", &results, &tol, &mut report);
        assert!(!report.pass());
    }
}
