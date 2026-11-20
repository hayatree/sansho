//! Prepare ngspice inputs and cache black-box Verilog-A compilations.
//! Provenance: original harness using the documented ngspice and OpenVAF command lines.

use crate::Cli;
use sansho_netlist::{FsLoader, Netlist, SourceLoader};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const WRAPPER: &str = "tools/ngspice/ngspice.sh";

/// Distinguish missing circuit inputs from tool failures and missing cached results.
#[derive(Debug)]
pub(crate) enum Problem {
    Missing(String),
    Failed(String),
}

impl From<std::io::Error> for Problem {
    fn from(e: std::io::Error) -> Self {
        Self::Failed(e.to_string())
    }
}

struct Inputs {
    fs: FsLoader,
    missing: std::cell::RefCell<Option<PathBuf>>,
}

impl SourceLoader for Inputs {
    fn waveform(&self, path: &Path, from: Option<&str>) -> Result<String, String> {
        self.fs.waveform(path, from)
    }
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
        self.fs.load(path, from).inspect_err(|_| {
            let path = self.fs.resolve(path, from);
            if path.try_exists().is_ok_and(|present| !present) {
                self.missing.replace(Some(path));
            }
        })
    }
}

pub(crate) fn parse(deck: &Path, text: &str) -> Result<Netlist, Problem> {
    let loader = Inputs {
        fs: FsLoader {
            base: deck.parent().unwrap_or(Path::new(".")).into(),
        },
        missing: Default::default(),
    };
    sansho_netlist::parse_with(text, &loader).map_err(|ds| match loader.missing.into_inner() {
        Some(path) => Problem::Missing(path.display().to_string()),
        None => Problem::Failed(
            ds.into_iter()
                .find(|d| d.is_error())
                .map_or_else(|| "parse failed".into(), |d| d.to_string()),
        ),
    })
}

fn compile(source: &Path, files: &[PathBuf], cli: &Cli) -> Result<PathBuf, Problem> {
    let mut hash = DefaultHasher::new();
    source.hash(&mut hash);
    std::env::var_os("SANSHO_NGSPICE_IMAGE").hash(&mut hash);
    for path in files {
        path.hash(&mut hash);
        fs::read(path)?.hash(&mut hash);
    }
    let artifact = cli.out.join(format!("model-{:016x}.osdi", hash.finish()));
    if artifact.is_file() {
        return Ok(artifact);
    }
    let tmp = artifact.with_extension(format!("{}.osdi", std::process::id()));
    let output = Command::new(fs::canonicalize(WRAPPER)?)
        .current_dir(source.parent().unwrap_or(Path::new(".")))
        .env("SANSHO_VERIFY_OUT", &cli.out)
        .arg("--compile-va")
        .arg(source)
        .arg("-o")
        .arg(&tmp)
        .output()?;
    if !output.status.success() {
        let _ = fs::remove_file(tmp);
        return Err(Problem::Failed(format!(
            "compiling {} ({}): {}{}",
            source.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    fs::rename(tmp, &artifact)?;
    Ok(artifact)
}

/// Resolve top-level relative paths before relocating the deck. Included SPICE
/// files retain their own path base. The first physical line of a deck is its title.
fn rewrite(text: &str, base: &Path, staged: &BTreeMap<PathBuf, PathBuf>, title: bool) -> String {
    let mut result = String::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let (card, rest) =
            trimmed.split_at(trimmed.find(char::is_whitespace).unwrap_or(trimmed.len()));
        if i != 0 || !title {
            let rest = rest.trim_start();
            let lower = card.to_ascii_lowercase();
            if lower == ".hdl" {
                continue;
            }
            if matches!(lower.as_str(), ".lib" | ".include" | ".inc") {
                let (path, tail) = if let Some(quote @ ('"' | '\'')) = rest.chars().next() {
                    rest[1..].split_once(quote).unwrap_or((rest, ""))
                } else {
                    rest.split_at(rest.find(char::is_whitespace).unwrap_or(rest.len()))
                };
                // A .lib section definition has no file argument.
                if lower != ".lib"
                    || tail
                        .trim_start()
                        .chars()
                        .next()
                        .is_some_and(|c| !matches!(c, ';' | '$' | '*' | '/'))
                {
                    let original = base.join(path);
                    let absolute = original.canonicalize().unwrap_or(original);
                    let target = staged.get(&absolute).unwrap_or(&absolute);
                    result.push_str(&format!("{card} \"{}\"{tail}\n", target.display()));
                    continue;
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Insert pre_osdi commands at the start of an existing control block. With no
/// control block ngspice's batch -r invocation still performs the analyses.
fn preload(text: &str, osdi: &BTreeSet<PathBuf>) -> String {
    if osdi.is_empty() {
        return text.into();
    }
    let commands: String = osdi
        .iter()
        .map(|p| format!("pre_osdi {}\n", p.display())) // ngspice keeps quotes literally
        .collect();
    let mut deck = String::new();
    let mut inserted = false;
    let block = |s: &mut String| {
        s.push_str(".control\n");
        s.push_str(&commands);
        s.push_str(".endc\n");
    };
    for (i, line) in text.lines().enumerate() {
        let card = line.split_whitespace().next().unwrap_or("");
        if i > 0 && !inserted && card.eq_ignore_ascii_case(".end") {
            block(&mut deck);
            inserted = true;
        }
        deck.push_str(line);
        deck.push('\n');
        if i > 0 && !inserted && card.eq_ignore_ascii_case(".control") {
            deck.push_str(&commands);
            inserted = true;
        }
    }
    if !inserted {
        block(&mut deck);
    }
    deck
}

pub(crate) fn prepare(deck: &Path, text: &str, nl: &Netlist, cli: &Cli) -> Result<String, Problem> {
    let mut artifacts = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for h in &nl.hdl {
        let path = fs::canonicalize(&h.path)?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let inputs = sansho_engine::hdl_files(h).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Problem::Missing(e.to_string())
            } else {
                Problem::Failed(e.to_string())
            }
        })?;
        if !cli.cached {
            artifacts.insert(compile(&path, &inputs, cli)?);
        }
    }
    if cli.cached {
        return Ok(String::new());
    }
    let base = deck.parent().unwrap_or(Path::new("."));
    // Usually only the entry deck needs rewriting. If a library contains HDL
    // cards, mirror the active include graph so no input file is modified.
    let mut staged = BTreeMap::new();
    if nl.hdl.iter().any(|h| h.span.file != 0) {
        let stem = deck.file_stem().unwrap_or_default().to_string_lossy();
        let loader = FsLoader { base: base.into() };
        for path in nl.sources.iter().skip(1) {
            let path = fs::canonicalize(path)?;
            let index = staged.len();
            staged
                .entry(path)
                .or_insert_with(|| cli.out.join(format!("{stem}.include-{index}.lib")));
        }
        for (path, copy) in &staged {
            let (_, body) = loader
                .load(&path.to_string_lossy(), None)
                .map_err(Problem::Failed)?;
            fs::write(
                copy,
                rewrite(&body, path.parent().unwrap_or(base), &staged, false),
            )?;
        }
    }
    Ok(preload(&rewrite(text, base, &staged, true), &artifacts))
}
