//! Batch print tables and confined, atomic publication of control output files.
//! Provenance: original work using the ngspice user manual's write and wrdata formats.

use super::{
    ControlOptions, Error, Print, Result, State, error,
    expr::{self, Vector},
    split,
};
use sansho_output::SimResults;
use std::{
    fs,
    io::{self, Write},
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};

fn denied(s: impl Into<String>) -> Error {
    Error::error("E-CONTROL-OUTPUT", s)
}
fn io_error(e: io::Error) -> Error {
    denied(e.to_string())
}
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Use an exclusively created sibling and rename it over the destination.
/// Neither an existing symlink nor a hard link is ever opened for writing.
fn publish(
    opts: &ControlOptions,
    name: &str,
    write: impl FnOnce(&mut io::BufWriter<fs::File>) -> io::Result<()>,
) -> Result<()> {
    let root = opts
        .output_dir
        .as_ref()
        .ok_or_else(|| denied("control file output is disabled"))?;
    let name = Path::new(name);
    if !opts.allow_external_paths
        && (name.is_absolute()
            || name
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)))
    {
        return Err(denied("output path must be relative and cannot contain .."));
    }
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let root = root.canonicalize().map_err(io_error)?;
    let path = root.join(name);
    let leaf = path
        .file_name()
        .ok_or_else(|| denied("missing output filename"))?;
    let parent = path
        .parent()
        .ok_or_else(|| denied("missing output directory"))?
        .canonicalize()
        .map_err(io_error)?;
    if !opts.allow_external_paths && !parent.starts_with(&root) {
        return Err(denied("output directory escapes the permitted root"));
    }
    // On Linux retain the opened directory across create/rename to prevent parent symlink swaps.
    #[cfg(target_os = "linux")]
    let (_handle, parent) = {
        use std::os::fd::AsRawFd;
        let dir = fs::File::open(&parent).map_err(io_error)?;
        let pinned = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        if !opts.allow_external_paths
            && !pinned.canonicalize().map_err(io_error)?.starts_with(&root)
        {
            return Err(denied("output directory moved outside the permitted root"));
        }
        (dir, pinned)
    };
    #[cfg(not(target_os = "linux"))]
    if !opts.allow_external_paths && parent != root {
        return Err(denied(
            "confined output requires a filename directly under the output root on this platform",
        ));
    }
    let dest = parent.join(leaf);
    if dest.symlink_metadata().is_ok_and(|m| !m.is_file()) {
        return Err(denied("output destination is not a regular file"));
    }
    let (tmp, file) = loop {
        let p = parent.join(format!(
            ".sansho-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        if p == dest {
            continue;
        }
        match fs::File::create_new(&p) {
            Ok(f) => break (p, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_error(e)),
        }
    };
    let mut buf = io::BufWriter::new(file);
    let written = write(&mut buf).and_then(|()| buf.flush());
    drop(buf);
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, &dest)) {
        let _ = fs::remove_file(tmp);
        return Err(io_error(e));
    }
    Ok(())
}
/// Scientific fields in interpreter output (manual §§13.5.59, 13.5.106).
/// Rust rounds the mantissa, including exponent carry and signed zero.
pub(super) fn batch_number(value: f64, decimals: usize) -> String {
    let field = format!("{value:.decimals$e}");
    if let Some((mantissa, exponent)) = field.split_once('e')
        && let Ok(power) = exponent.parse::<i32>()
    {
        return format!("{mantissa}e{power:+03}");
    }
    field.to_ascii_lowercase()
}

fn print_field(value: f64, decimals: usize) -> String {
    batch_number(
        value,
        decimals.saturating_sub(usize::from(value.is_sign_negative())),
    )
}

fn print_value(z: sansho_sparse::c64, complex: bool, decimals: usize, separator: &str) -> String {
    let real = print_field(z.re, decimals);
    if complex {
        format!("{real},{separator}{}", print_field(z.im, decimals))
    } else {
        real
    }
}

/// Nutmeg vector substitution uses six significant digits in general format.
pub(super) fn substitution(value: f64) -> String {
    let scientific = batch_number(value, 5);
    let Some((mantissa, exp)) = scientific.split_once('e') else {
        return scientific;
    };
    let Ok(power) = exp.parse::<i32>() else {
        return scientific;
    };
    if !(-4..6).contains(&power) {
        return format!(
            "{}e{exp}",
            mantissa.trim_end_matches('0').trim_end_matches('.')
        );
    }
    let places = (5 - power) as usize;
    let fixed = format!("{value:.places$}");
    if places == 0 {
        fixed
    } else {
        fixed.trim_end_matches('0').trim_end_matches('.').into()
    }
}

fn print_line(name: &str, v: &Vector, decimals: usize, lines: &mut Vec<String>) {
    if v.values.len() == 1 {
        lines.push(format!(
            "{name} = {}",
            print_value(v.values[0], v.complex, decimals, "")
        ));
        return;
    }
    let mut line = format!("{name} = (  ");
    let mut column = line.len();
    for (i, &z) in v.values.iter().enumerate() {
        if i != 0 {
            if column >= 72 {
                lines.push(std::mem::take(&mut line));
                column = 0;
            }
            line.push('\t');
            column = (column / 8 + 1) * 8;
        }
        let field = print_value(z, v.complex, decimals, "");
        column += field.len();
        line.push_str(&field);
    }
    line.push_str("\t)");
    lines.push(line);
}

impl State<'_> {
    pub(super) fn decimals(&self, default: usize) -> usize {
        self.vars
            .get("numdgt")
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| (1..=17).contains(n))
            .unwrap_or(default)
    }
    fn selected(&mut self, args: &str) -> Result<Vec<(String, Vector)>> {
        if args.is_empty() || args.eq_ignore_ascii_case("all") {
            let p = &self.plots[self.current];
            return Ok(p
                .names()
                .into_iter()
                .filter_map(|n| p.get(&n).map(|v| (n, v)))
                .collect());
        }
        expr::expressions(self, args)
    }
    pub(super) fn output(&mut self, cmd: &str, args: &str) -> Result<()> {
        let (dest, exprs) = if cmd == "print" {
            // Redirection follows the expression list. Parenthesize relational expressions.
            let mut depth = 0usize;
            let redirect = args.char_indices().find_map(|(i, c)| {
                match c {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth = depth.saturating_sub(1),
                    '>' if depth == 0
                        && !args[i + 1..].starts_with('=')
                        && !args[..i].ends_with('<') =>
                    {
                        return Some(i);
                    }
                    _ => {}
                }
                None
            });
            if let Some(i) = redirect {
                let (expr, file) = (&args[..i], &args[i + 1..]);
                (Some(file.trim()), expr.trim())
            } else {
                (None, args)
            }
        } else {
            let (name, rest) = split(args);
            let name = match (cmd, name) {
                ("write", "") => "rawspice.raw",
                (_, "") => return Err(error("missing output filename")),
                (_, n) => n,
            };
            (Some(name), rest)
        };
        if dest.is_some() && self.options.output_dir.is_none() {
            return Err(denied(format!("{cmd}: file output is disabled")));
        }
        if matches!(cmd, "write" | "wrdata") && self.vars.contains_key("appendwrite") {
            return Err(error("appendwrite is unsupported"));
        }
        let (style, exprs) = match split(exprs) {
            ("col", s) | ("column", s) if cmd == "print" => ("col", s),
            ("line", s) if cmd == "print" => ("line", s),
            _ => ("", exprs),
        };
        let selected = self.selected(exprs)?;
        if selected.is_empty() {
            return Err(error("no vectors selected"));
        }
        let dest = dest.map(|s| s.trim_matches('"'));
        let n = selected
            .iter()
            .map(|(_, v)| v.values.len())
            .max()
            .unwrap_or(0);
        if n == 0 {
            return Err(error("empty vector"));
        }
        let decimals = self.decimals(if cmd == "wrdata" { 8 } else { 6 });
        let fmt = |v| batch_number(v, decimals);
        let p = &self.plots[self.current];
        match cmd {
            "print" => {
                let mut lines = vec![];
                if style == "line" || n == 1 && style != "col" {
                    for (name, v) in &selected {
                        print_line(name, v, decimals, &mut lines);
                    }
                } else {
                    let scale = p
                        .analysis
                        .as_ref()
                        .and_then(|a| a.scale.as_ref().map(|s| (s, &a.scale_values)))
                        .filter(|(s, x)| {
                            x.len() == n && !selected.iter().any(|(k, _)| *k == s.name)
                        });
                    // Indexed listings have an eight-column index and
                    // sixteen display columns per real component.
                    let mut rest = selected.as_slice();
                    while !rest.is_empty() {
                        let mut width = 8 + usize::from(scale.is_some()) * 16;
                        let count = rest
                            .iter()
                            .take_while(|(_, v)| {
                                width += if v.complex { 32 } else { 16 };
                                width <= 80
                            })
                            .count();
                        let (page, remaining) = rest.split_at(count);
                        rest = remaining;
                        let mut header = format!("{:<8}", "Index");
                        if let Some((s, _)) = scale {
                            header.push_str(&format!("{:<16}", s.name));
                        }
                        for (name, v) in page {
                            let width = if v.complex { 32 } else { 16 };
                            header.push_str(&format!("{name:<width$}"));
                        }
                        lines.push(header);
                        for i in 0..n {
                            let mut row = format!("{i}\t");
                            if let Some((_, xs)) = scale {
                                row.push_str(&print_field(xs[i], decimals));
                                row.push('\t');
                            }
                            for (_, v) in page {
                                if let Some(&z) = v.values.get(i) {
                                    row.push_str(&print_value(z, v.complex, decimals, "\t"));
                                }
                                row.push('\t');
                            }
                            lines.push(row);
                        }
                    }
                }
                if let Some(name) = dest {
                    publish(self.options, name, |f| {
                        for line in &lines {
                            writeln!(f, "{line}")?
                        }
                        Ok(())
                    })?
                } else {
                    self.transcript.lines.extend(lines)
                }
                self.transcript.prints.push(Print {
                    plot: p.name.clone(),
                    expressions: selected.iter().map(|(k, _)| k.clone()).collect(),
                    columns: selected
                        .iter()
                        .map(|(_, v)| v.values.iter().map(|z| (z.re, z.im)).collect())
                        .collect(),
                });
            }
            "write" => {
                let r = SimResults {
                    title: self.deck.title.clone(),
                    analyses: vec![p.result(&selected)?],
                    prints: vec![],
                    measures: vec![],
                };
                let ascii = match self.vars.get("filetype").map(String::as_str) {
                    None | Some("binary") => false,
                    Some("ascii") => true,
                    _ => return Err(error("filetype must be ascii or binary")),
                };
                publish(self.options, dest.unwrap_or(""), |f| {
                    if ascii {
                        sansho_output::write_raw_ascii(&r, f)
                    } else {
                        sansho_output::write_raw_binary(&r, f)
                    }
                })?;
            }
            _ => {
                let single = self.vars.contains_key("wr_singlescale");
                let header = self.vars.contains_key("wr_vecnames");
                let a = p.analysis.as_ref();
                if selected.iter().any(|(_, v)| v.values.len() != n)
                    || a.is_some_and(|a| a.scale.is_some() && a.scale_values.len() != n)
                {
                    return Err(error("wrdata requires matching vector and scale lengths"));
                }
                let scale_name = a
                    .and_then(|a| a.scale.as_ref().or(a.variables.first()))
                    .map_or("index", |s| s.name.as_str());
                let scale = p
                    .get(scale_name)
                    .unwrap_or_else(|| Vector::real((0..n).map(|i| i as f64)));
                if scale.values.len() != n {
                    return Err(error("wrdata scale length mismatch"));
                }
                publish(self.options, dest.unwrap_or(""), |f| {
                    if header {
                        for (i, (name, v)) in selected.iter().enumerate() {
                            if !single || i == 0 {
                                write!(f, " {scale_name:<width$}", width = decimals + 7)?
                            }
                            write!(f, " {name:<width$}", width = decimals + 7)?;
                            if v.complex {
                                write!(f, " {name:<width$}", width = decimals + 7)?
                            }
                        }
                        writeln!(f)?;
                    }
                    for i in 0..n {
                        for (col, (_, v)) in selected.iter().enumerate() {
                            if !single || col == 0 {
                                write!(
                                    f,
                                    "{:>width$} ",
                                    fmt(v
                                        .scale
                                        .as_deref()
                                        .and_then(|xs| xs.get(i))
                                        .copied()
                                        .unwrap_or(scale.values[i].re)),
                                    width = decimals + 7
                                )?
                            }
                            write!(f, "{:>width$} ", fmt(v.values[i].re), width = decimals + 7)?;
                            if v.complex {
                                write!(f, "{:>width$} ", fmt(v.values[i].im), width = decimals + 7)?
                            }
                        }
                        writeln!(f)?;
                    }
                    Ok(())
                })?;
            }
        }
        if let Some(name) = dest {
            self.transcript.written_files.push(name.into())
        }
        Ok(())
    }
}

#[cfg(test)]
mod format_tests {
    use super::batch_number;
    #[test]
    fn scientific_rounding_sign_and_exponent() {
        for (v, places, field) in [
            (-0.0, 2, "-0.00e+00"),
            (9.9999, 2, "1.00e+01"),
            (-2.35e-105, 2, "-2.35e-105"),
            (f64::INFINITY, 6, "inf"),
        ] {
            assert_eq!(batch_number(v, places), field);
        }
    }
}
