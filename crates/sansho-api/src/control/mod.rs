//! Deterministic batch control evaluator. Each invocation owns its circuit, plots and variables.
//! Provenance: original implementation for the ngspice user manual's batch interpreter.

mod expr;
mod files;
mod plot;

use expr::{Vector, eval, integer};
use plot::Plot;
use sansho_diag::Diagnostic as Error;
use sansho_netlist::{
    AnalysisCard, ElementKind, Netlist,
    control::{Block, Statement},
};
use sansho_output::{AnalysisResult, Measurement, PrintTable, SimResults};
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

type Result<T> = std::result::Result<T, Error>;
fn error(s: impl Into<String>) -> Error {
    Error::error("E-CONTROL", s)
}
fn split(s: &str) -> (&str, &str) {
    let s = s.trim();
    s.split_once(char::is_whitespace)
        .map_or((s, ""), |(a, b)| (a, b.trim()))
}
fn first(ds: Vec<Error>) -> Error {
    ds.into_iter()
        .find(Error::is_error)
        .unwrap_or_else(|| error("invalid simulation request"))
}

#[derive(Debug, Clone)]
pub struct ControlOptions {
    /// None disables all script file writes, including redirected print.
    pub output_dir: Option<PathBuf>,
    pub allow_external_paths: bool,
    pub iteration_limit: usize,
}
impl Default for ControlOptions {
    fn default() -> Self {
        Self {
            output_dir: None,
            allow_external_paths: false,
            iteration_limit: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Print {
    pub plot: String,
    pub expressions: Vec<String>,
    /// Columns of (real, imaginary) pairs, in expression order.
    pub columns: Vec<Vec<(f64, f64)>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Transcript {
    /// Ordered stdout records for echo, print, measurements and plot listings.
    pub lines: Vec<String>,
    pub prints: Vec<Print>,
    pub measurements: Vec<Measurement>,
    /// Surviving plot names in creation order, parallel to results.analyses.
    pub plots: Vec<String>,
    pub current_plot: String,
    pub written_files: Vec<String>,
}

#[derive(Debug)]
pub struct ControlOutput {
    pub transcript: Transcript,
    pub results: SimResults,
    pub diagnostics: Vec<Error>,
}

#[derive(PartialEq)]
enum Flow {
    Next,
    Break,
    Continue,
    Quit,
}

struct State<'a> {
    initial: &'a Netlist,
    deck: Netlist,
    params: BTreeMap<String, f64>,
    probes: Vec<String>,
    vars: BTreeMap<String, String>,
    plots: Vec<Plot>,
    counts: BTreeMap<&'static str, usize>,
    current: usize,
    options: &'a ControlOptions,
    remaining: usize,
    diagnostics: Vec<Error>,
    transcript: Transcript,
    seed: Option<u64>,
    card_prints: Vec<PrintTable>,
}

/// Collect literal device references before the first solve. Expressions introduced by
/// substitution can explicitly request sampling with the save command.
fn probes(program: &[Vec<Statement>]) -> Vec<String> {
    fn visit(body: &[Statement], out: &mut std::collections::BTreeSet<String>) {
        for stmt in body {
            for (start, _) in stmt.arguments.match_indices('@') {
                let tail = &stmt.arguments[start + 1..];
                let end = tail
                    .find(|c: char| !c.is_ascii_alphanumeric() && !"._:".contains(c))
                    .unwrap_or(tail.len());
                if let Some(rest) = tail[end..].strip_prefix('[') {
                    let len = rest
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(rest.len());
                    if end > 0 && len > 0 && rest[len..].starts_with(']') {
                        out.insert(format!(
                            "@{}[{}]",
                            tail[..end].to_ascii_lowercase(),
                            rest[..len].to_ascii_lowercase()
                        ));
                    }
                }
            }
            match &stmt.body {
                Block::Loop(body) => visit(body, out),
                Block::If { yes, no } => {
                    visit(yes, out);
                    visit(no, out)
                }
                _ => {}
            }
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for body in program {
        visit(body, &mut names)
    }
    names.into_iter().collect()
}

/// Execute with a caller-selected number of simulation workers.
pub fn execute_with(deck: &Netlist, options: &ControlOptions, threads: usize) -> ControlOutput {
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
    {
        Ok(pool) => pool.install(|| execute(deck, options)),
        Err(_) => execute(deck, options),
    }
}

/// Execute a parsed program in the calling Rayon pool. Errors retain output produced so far.
pub fn execute(deck: &Netlist, options: &ControlOptions) -> ControlOutput {
    let mut state = State {
        initial: deck,
        deck: deck.clone(),
        params: BTreeMap::new(),
        probes: probes(&deck.controls),
        vars: BTreeMap::new(),
        plots: vec![Plot {
            name: "const".into(),
            analysis: None,
            grid: None,
            locals: deck
                .csparams
                .iter()
                .map(|(k, &x)| (k.clone(), Vector::scalar(x)))
                .collect(),
        }],
        counts: BTreeMap::new(),
        current: 0,
        options,
        remaining: options.iteration_limit,
        diagnostics: deck.warnings.clone(),
        transcript: Transcript::default(),
        seed: None,
        card_prints: vec![],
    };
    for block in &deck.controls {
        match state.block(block, 0) {
            Ok(Flow::Next) => {}
            Ok(Flow::Quit) => break,
            Ok(_) => {
                state
                    .diagnostics
                    .push(error("break or continue outside loop"));
                break;
            }
            Err(d) => {
                state.diagnostics.push(d);
                break;
            }
        }
    }
    state.transcript.current_plot = state.plots[state.current].name.clone();
    let mut analyses = vec![];
    for p in state.plots.iter().skip(1) {
        match p.snapshot() {
            Ok(a) => {
                analyses.push(a);
                state.transcript.plots.push(p.name.clone())
            }
            Err(d) => state.diagnostics.push(d),
        }
    }
    let results = SimResults {
        title: deck.title.clone(),
        analyses,
        measures: state.transcript.measurements.clone(),
        prints: state.card_prints,
    };
    ControlOutput {
        results,
        transcript: state.transcript,
        diagnostics: state.diagnostics,
    }
}

impl State<'_> {
    fn current_scale(&self) -> Result<Arc<[f64]>> {
        self.plots[self.current]
            .scale()
            .ok_or_else(|| error("current plot has no scale"))
    }
    fn resolve<'a>(&self, name: &'a str) -> (usize, &'a str) {
        if let Some((prefix, tail)) = name.split_once('.')
            && let Some(i) = self.plots.iter().position(|p| p.name == prefix)
        {
            (i, tail)
        } else {
            (self.current, name)
        }
    }
    fn lookup(&self, name: &str) -> Result<Vector> {
        if let Some(x) = sansho_netlist::parse_number(name) {
            return Ok(Vector::scalar(x));
        }
        let (i, n) = self.resolve(name);
        let p = &self.plots[i];
        if let Some(v) = p
            .get(n)
            .or_else(|| (n == name).then(|| self.plots[0].get(n)).flatten())
        {
            return Ok(v);
        }
        if let Some(nodes) = n.strip_prefix("v(").and_then(|s| s.strip_suffix(')'))
            && let Some((a, b)) = nodes.split_once(',')
        {
            return expr::binary(
                "-",
                self.lookup(&format!("{}.v({})", p.name, a.trim()))?,
                self.lookup(&format!("{}.v({})", p.name, b.trim()))?,
            );
        }
        if let Some((dev, param)) = n
            .strip_prefix('@')
            .and_then(|v| v.strip_suffix(']'))
            .and_then(|v| v.split_once('['))
            && let Some(value) = self.parameter(dev, param)
        {
            return Ok(Vector::scalar(value));
        }
        if let Some(v) = p.get(&format!("v({n})")) {
            return Ok(v);
        }
        let x = match n {
            "pi" => std::f64::consts::PI,
            "e" => std::f64::consts::E,
            "i" => {
                return Ok(Vector {
                    values: vec![sansho_sparse::c64::new(0., 1.)],
                    complex: true,
                    scale: None,
                });
            }
            "sqrt2" => std::f64::consts::SQRT_2,
            "pi2" => std::f64::consts::TAU,
            "boltz" => 1.380649e-23,
            "planck" => 6.62607015e-34,
            "echarge" => 1.602176634e-19,
            "kelvin" => -273.15,
            "c" => 299792458.,
            "yes" | "true" => 1.,
            "no" | "false" | "v(0)" => 0.,
            _ => return Err(error(format!("unknown vector {name}"))),
        };
        Ok(Vector::scalar(x))
    }
    fn parameter(&self, dev: &str, param: &str) -> Option<f64> {
        let get = |params: &[(String, f64)]| params.iter().find(|(k, _)| k == param).map(|p| p.1);
        if let Some(m) = self.deck.models.iter().find(|m| m.name == dev) {
            return get(&m.params);
        }
        let e = self.deck.elements.iter().find(|e| e.name == dev)?;
        match &e.kind {
            ElementKind::Resistor { value, .. }
                if matches!(param, "r" | "resistance" | "value") =>
            {
                Some(*value)
            }
            ElementKind::Capacitor { value, .. }
                if matches!(param, "c" | "capacitance" | "value") =>
            {
                Some(*value)
            }
            ElementKind::Inductor { value, .. }
                if matches!(param, "l" | "inductance" | "value") =>
            {
                Some(*value)
            }
            ElementKind::CurrentSource { source, .. }
            | ElementKind::VoltageSource { source, .. }
                if param == "dc" =>
            {
                Some(source.dc.unwrap_or(0.))
            }
            ElementKind::Mosfet { params, .. }
            | ElementKind::Diode { params, .. }
            | ElementKind::Bjt { params, .. }
            | ElementKind::Jfet { params, .. }
            | ElementKind::Resistor { params, .. }
            | ElementKind::Capacitor { params, .. }
            | ElementKind::Inductor { params, .. } => get(params),
            _ => None,
        }
    }
    fn expand(&self, s: &str) -> Result<String> {
        let mut rest = s;
        let mut out = String::new();
        while let Some((before, tail)) = rest.split_once('$') {
            out.push_str(before);
            let (vector, tail) = tail.strip_prefix('&').map_or((false, tail), |s| (true, s));
            let (length, tail) = tail.strip_prefix('#').map_or((false, tail), |t| (true, t));
            let (name, mut after) = if let Some(t) = tail.strip_prefix('{') {
                t.split_once('}')
                    .ok_or_else(|| error("unterminated variable"))?
            } else {
                let n = tail
                    .find(|c: char| {
                        !c.is_ascii_alphanumeric() && !"_.".contains(c) && !(vector && c == '@')
                    })
                    .unwrap_or(tail.len());
                tail.split_at(if vector { expr::probe_end(tail, n)? } else { n })
            };
            let name = name.to_ascii_lowercase();
            let value = if vector {
                let v = self.lookup(&name)?;
                v.values
                    .iter()
                    .map(|z| {
                        if v.complex {
                            format!("{},{}", z.re, z.im)
                        } else {
                            z.re.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else if name == "curplot" {
                self.plots[self.current].name.clone()
            } else if name == "plots" {
                self.plots
                    .iter()
                    .rev()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                self.vars
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| error(format!("undefined variable ${name}")))?
            };
            if length {
                out.push_str(&value.split_whitespace().count().to_string())
            } else if let Some(tail) = after.strip_prefix('[') {
                let (index, next) = tail
                    .split_once(']')
                    .ok_or_else(|| error("unterminated list index"))?;
                let i = index
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .ok_or_else(|| error("list indices start at one"))?;
                out.push_str(
                    value
                        .split_whitespace()
                        .nth(i)
                        .ok_or_else(|| error("list index out of bounds"))?,
                );
                after = next
            } else {
                out.push_str(&value)
            }
            rest = after;
            if out.len() > 1_048_576 {
                return Err(error("variable expansion too long"));
            }
        }
        out.push_str(rest);
        Ok(out)
    }
    fn iteration(&mut self, cmd: &str) -> Result<()> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            Error::error("E-CONTROL-LIMIT", format!("{cmd} exceeded iteration limit"))
        })?;
        Ok(())
    }
    fn block(&mut self, body: &[Statement], depth: usize) -> Result<Flow> {
        let mut pc = 0;
        while let Some(s) = body.get(pc) {
            pc += 1;
            if s.command == "goto" {
                let label = self.expand(&s.arguments).map_err(|e| e.with_span(s.span))?;
                pc = body
                    .iter()
                    .position(|s| s.command == "label" && s.arguments.eq_ignore_ascii_case(&label))
                    .ok_or_else(|| {
                        error(format!("goto: unknown label {label} in this block"))
                            .with_span(s.span)
                    })?
                    + 1;
                self.iteration("goto").map_err(|e| e.with_span(s.span))?;
                continue;
            }
            let flow = self.statement(s, depth).map_err(|mut e| {
                if e.span.is_none() {
                    e.span = Some(s.span);
                    e.message = format!("{}: {}", s.command, e.message)
                }
                e
            })?;
            if flow != Flow::Next {
                return Ok(flow);
            }
        }
        Ok(Flow::Next)
    }
    fn condition(&mut self, s: &str) -> Result<bool> {
        let expanded = self.expand(s)?;
        Ok(eval(self, &expanded)?
            .values
            .iter()
            .any(|z| z.re != 0. || z.im != 0.))
    }
    fn statement(&mut self, s: &Statement, depth: usize) -> Result<Flow> {
        let cmd = s.command.as_str();
        let args = s.arguments.as_str();
        match &s.body {
            Block::If { yes, no } => {
                let cond = self.condition(args)?;
                self.block(if cond { yes } else { no }, depth)
            }
            Block::Loop(body) => {
                let mut count = None;
                let mut var = String::new();
                let mut items = vec![].into_iter();
                if cmd == "repeat" && !args.is_empty() {
                    let args = self.expand(args)?;
                    count = Some(integer(eval(self, &args)?.number()?, usize::MAX)?);
                }
                if cmd == "foreach" {
                    let (name, words) = split(args);
                    var = name.to_ascii_lowercase();
                    items = self
                        .expand(words)?
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                        .into_iter();
                }
                loop {
                    if count == Some(0) || cmd == "while" && !self.condition(args)? {
                        break;
                    }
                    if cmd == "foreach" {
                        let Some(v) = items.next() else { break };
                        self.vars.insert(var.clone(), v);
                    }
                    self.iteration(cmd)?;
                    if let Some(c) = &mut count {
                        *c -= 1
                    }
                    match self.block(body, depth + 1)? {
                        Flow::Quit => return Ok(Flow::Quit),
                        Flow::Break => break,
                        _ => {}
                    }
                    if cmd == "dowhile" && !self.condition(args)? {
                        break;
                    }
                }
                Ok(Flow::Next)
            }
            Block::Command => {
                let args = self.expand(args)?;
                self.command(cmd, &args, depth)
            }
        }
    }
    fn add_plot(&mut self, a: AnalysisResult, grid: Option<(f64, f64, f64)>) {
        let prefix = plot::prefix(a.kind);
        let count = self.counts.entry(prefix).or_default();
        *count += 1;
        self.plots.push(Plot {
            name: format!("{prefix}{count}"),
            analysis: Some(a),
            grid,
            locals: BTreeMap::new(),
        });
        self.current = self.plots.len() - 1;
    }
    fn analysis(&mut self, cmd: &str, args: &str) -> Result<()> {
        let mut deck = self.deck.clone();
        deck.control_probes.clone_from(&self.probes);
        if cmd != "run" {
            let cards = cards(cmd, args)?;
            if let [
                AnalysisCard::Four {
                    frequency, outputs, ..
                },
            ] = cards.analyses.as_slice()
            {
                let tran = self.plots[self.current].snapshot()?;
                let a = sansho_engine::fourier(&tran, *frequency, outputs)?;
                self.add_plot(a, None);
                return Ok(());
            }
            deck.analyses = cards.analyses;
            deck.prints.clear();
            deck.measures.clear();
        }
        if deck.analyses.is_empty() {
            return Err(error("no analysis cards to run"));
        }
        let mut grids = deck.analyses.iter().filter_map(|a| {
            if let AnalysisCard::Tran {
                tstart,
                tstop,
                tstep,
                ..
            } = a
            {
                Some((*tstart, *tstop, *tstep))
            } else {
                None
            }
        });
        let run = sansho_engine::run_in_current_pool(&deck).map_err(|e| first(e.diagnostics))?;
        for d in run.warnings {
            if !self.diagnostics.contains(&d) {
                self.diagnostics.push(d)
            }
        }
        let first_plot = self.plots.len();
        for a in run.results.analyses {
            let grid = if a.kind == sansho_output::AnalysisKind::Tran {
                grids.next()
            } else {
                None
            };
            self.add_plot(a, grid);
        }
        for table in run.results.prints {
            if let Some(p) = self.plots[first_plot..].iter().find(|p| {
                p.analysis
                    .as_ref()
                    .is_some_and(|a| a.kind == table.analysis)
            }) {
                self.transcript.prints.push(Print {
                    plot: p.name.clone(),
                    expressions: table.headings.clone(),
                    columns: table
                        .columns
                        .iter()
                        .map(|c| c.iter().map(|x| (*x, 0.)).collect())
                        .collect(),
                });
            }
            self.transcript
                .lines
                .push(format!("Index\t{}", table.headings.join("\t")));
            for i in 0..table.columns.first().map_or(0, Vec::len) {
                let mut row = i.to_string();
                for c in &table.columns {
                    if let Some(v) = c.get(i) {
                        row.push_str(&format!("\t{v:e}"))
                    }
                }
                self.transcript.lines.push(row);
            }
            self.card_prints.push(table);
        }
        for m in run.results.measures {
            let plot = self
                .plots
                .iter()
                .enumerate()
                .skip(first_plot)
                .find(|(_, p)| {
                    p.analysis
                        .as_ref()
                        .is_some_and(|a| plot::prefix(a.kind) == m.analysis)
                })
                .map_or(self.current, |(i, _)| i);
            self.measurement(m, plot)?;
        }
        Ok(())
    }
    fn measurement(&mut self, m: Measurement, plot: usize) -> Result<()> {
        let value = m.value;
        let name = m.name.clone();
        let reason = m.error.clone().unwrap_or_default();
        self.transcript.measurements.push(m);
        let x = value.ok_or_else(|| error(format!("{name}: {reason}")))?;
        self.plots[plot]
            .locals
            .insert(name.clone(), Vector::scalar(x));
        self.transcript.lines.push(format!("{name} = {x:e}"));
        Ok(())
    }
    fn set(&mut self, args: &str) -> Result<()> {
        if args.is_empty() {
            self.transcript
                .lines
                .extend(self.vars.iter().map(|(k, v)| format!("{k} = {v}")));
            return Ok(());
        }
        let (k, v) = args.split_once('=').unwrap_or((args, ""));
        let key = k.trim().to_ascii_lowercase();
        if key.split_whitespace().count() != 1 {
            return Err(error("expected one variable name"));
        }
        let v = v.trim();
        let v = v
            .strip_prefix('(')
            .and_then(|v| v.strip_suffix(')'))
            .unwrap_or(v)
            .trim()
            .trim_matches('"');
        if key == "seed" {
            self.seed = Some(v.parse().map_err(|_| error("invalid seed"))?)
        }
        if key == "temp" {
            self.deck
                .options
                .extend(cards("options", &format!("temp={v}"))?.options)
        }
        self.vars.insert(key, v.into());
        Ok(())
    }
    fn command(&mut self, cmd: &str, args: &str, depth: usize) -> Result<Flow> {
        match cmd {
            "op" | "dc" | "ac" | "tran" | "noise" | "tf" | "sens" | "pz" | "sp" | "disto"
            | "four" | "fourier" | "run" => {
                self.analysis(if cmd == "fourier" { "four" } else { cmd }, args)?
            }
            "set" => self.set(args)?,
            "unset" => {
                for k in args.to_ascii_lowercase().split_whitespace() {
                    if k == "seed" {
                        self.seed = None
                    }
                    self.vars.remove(k);
                }
            }
            "setseed" => self.seed = Some(args.parse().map_err(|_| error("invalid seed"))?),
            "let" => {
                let (name, rhs) = args
                    .split_once('=')
                    .ok_or_else(|| error("expected name = expression"))?;
                let name = name.trim().to_ascii_lowercase();
                let mut v = eval(self, rhs)?;
                let (i, n) = self.resolve(&name);
                if i == 0
                    && self
                        .initial
                        .csparams
                        .contains_key(n.split('[').next().unwrap_or(n))
                {
                    return Err(error(format!("read-only .csparam {n}")));
                }
                if n.is_empty() {
                    return Err(error("missing vector name"));
                }
                if let Some((base, index)) = n.strip_suffix(']').and_then(|s| s.split_once('[')) {
                    let idx = integer(eval(self, index)?.number()?, usize::MAX)?;
                    let mut dest = self.plots[i]
                        .get(base)
                        .ok_or_else(|| error("unknown indexed vector"))?;
                    if v.values.len() != 1 {
                        return Err(error("indexed assignment requires a scalar"));
                    }
                    *dest
                        .values
                        .get_mut(idx)
                        .ok_or_else(|| error("index outside vector"))? = v.values[0];
                    dest.complex |= v.complex;
                    self.plots[i].locals.insert(base.into(), dest);
                } else {
                    if v.scale.is_none()
                        && let Some(scale) = self.plots[i].scale()
                        && scale.len() == v.values.len()
                    {
                        v.scale = Some(scale)
                    }
                    self.plots[i].locals.insert(n.into(), v);
                }
            }
            "unlet" => {
                for name in args.to_ascii_lowercase().split_whitespace() {
                    let (i, n) = self.resolve(name);
                    if i == 0 && self.initial.csparams.contains_key(n) {
                        return Err(error(format!("read-only .csparam {n}")));
                    }
                    self.plots[i].remove(n)?;
                }
            }
            "label" if args.split_whitespace().count() == 1 => {}
            "echo" => self.transcript.lines.push(args.trim_matches('"').into()),
            "print" | "wrdata" | "write" => self.output(cmd, args)?,
            "linearize" => self.linearize(args)?,
            "meas" | "measure" => {
                let p = &self.plots[self.current];
                let input = sansho_netlist::lex_source(&format!("meas {args}"), 0, false);
                let line = input
                    .lines
                    .first()
                    .ok_or_else(|| error("missing measurement"))?;
                let c = sansho_netlist::parse_control_meas(line, &p.names())?;
                // Keep the type of real derived vectors on complex plots. The measure engine
                // interprets a plain complex trace as magnitude.
                let mut a = p.snapshot()?;
                if let sansho_output::Data::Complex { columns } = &a.data
                    && c.kind
                        .signals()
                        .iter()
                        .all(|sig| sig.is_vector() && p.get(&sig.pos).is_some_and(|v| !v.complex))
                {
                    a.data = sansho_output::Data::Real {
                        columns: columns
                            .iter()
                            .map(|col| col.iter().map(|v| v.0).collect())
                            .collect(),
                    }
                }
                let r = SimResults {
                    title: self.deck.title.clone(),
                    analyses: vec![a],
                    measures: self.transcript.measurements.clone(),
                    prints: vec![],
                };
                for m in sansho_engine::measure(&[c], &r) {
                    self.measurement(m, self.current)?;
                }
            }
            "alter" | "altermod" | "alterparam" => {
                let (lhs, rhs) = args
                    .split_once('=')
                    .map_or_else(|| split(args), |(k, v)| (k.trim(), v.trim()));
                let x = eval(self, rhs)?.number()?;
                let lhs = lhs.to_ascii_lowercase();
                if cmd == "alterparam" {
                    self.params.insert(lhs, x);
                } else {
                    let key = lhs.trim_start_matches('@');
                    let (target, param) = key
                        .strip_suffix(']')
                        .and_then(|s| s.split_once('['))
                        .or_else(|| key.split_once(char::is_whitespace))
                        .unwrap_or((key, "value"));
                    if cmd == "altermod" && !self.deck.models.iter().any(|m| m.name == target) {
                        return Err(error(format!("unknown model {target}")));
                    }
                    crate::alter::apply(
                        &mut self.deck,
                        &crate::Change {
                            target: target.into(),
                            param: param.trim().into(),
                            value: x,
                        },
                    )?;
                }
            }
            "reset" => {
                self.deck = if self.params.is_empty() {
                    self.initial.clone()
                } else {
                    let source = self
                        .initial
                        .parameter_source
                        .as_deref()
                        .ok_or_else(|| error("alterparam requires a global .param"))?;
                    sansho_netlist::parse_parameters(source, &self.params).map_err(first)?
                };
            }
            "option" | "options" if args.is_empty() => {
                self.transcript
                    .lines
                    .extend(self.deck.options.iter().map(|o| {
                        o.value
                            .map_or_else(|| o.name.clone(), |v| format!("{} = {v}", o.name))
                    }))
            }
            "option" | "options" => self.deck.options.extend(cards("options", args)?.options),
            "save" => self.deck.saves.extend(cards("save", args)?.saves),
            "setplot" => {
                if args.is_empty() {
                    self.transcript
                        .lines
                        .extend(self.plots.iter().rev().map(|p| p.name.clone()))
                } else if args.eq_ignore_ascii_case("new") {
                    let count = self.counts.entry("unknown").or_default();
                    *count += 1;
                    self.plots.push(Plot {
                        name: format!("unknown{count}"),
                        analysis: None,
                        grid: None,
                        locals: Default::default(),
                    });
                    self.current = self.plots.len() - 1;
                } else {
                    self.current = self
                        .plots
                        .iter()
                        .position(|p| p.name.eq_ignore_ascii_case(args))
                        .ok_or_else(|| error(format!("unknown plot {args}")))?
                }
            }
            "destroy" => {
                let names = if args.is_empty() {
                    vec![self.plots[self.current].name.clone()]
                } else {
                    args.to_ascii_lowercase()
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect()
                };
                let active = self.plots[self.current].name.clone();
                for n in &names {
                    if n != "all" && !self.plots.iter().skip(1).any(|p| p.name == *n) {
                        return Err(error(format!("cannot destroy {n}")));
                    }
                }
                self.plots.retain(|p| {
                    p.name == "const" || !names.iter().any(|n| n == "all" || *n == p.name)
                });
                self.current = self
                    .plots
                    .iter()
                    .position(|p| p.name == active)
                    .unwrap_or(self.plots.len() - 1);
            }
            "plot" | "gnuplot" | "hardcopy" => self.diagnostics.push(Error::warning(
                "W-CONTROL-GRAPHICS",
                format!("{cmd} skipped in batch mode"),
            )),
            "break" | "continue" if depth == 0 => return Err(error(format!("{cmd} outside loop"))),
            "break" => return Ok(Flow::Break),
            "continue" => return Ok(Flow::Continue),
            "quit" | "exit" => return Ok(Flow::Quit),
            _ => return Err(error(format!("unsupported command {cmd}"))),
        }
        Ok(Flow::Next)
    }
    fn linearize(&mut self, args: &str) -> Result<()> {
        let p = &self.plots[self.current];
        let (start, stop, step) = p
            .grid
            .ok_or_else(|| error("linearize requires a transient plot"))?;
        let steps = integer(((stop - start) / step + 1e-9).floor(), expr::MAX_POINTS - 1)?;
        let target: Vec<_> = (0..=steps).map(|i| start + i as f64 * step).collect();
        let names = if args.is_empty() {
            p.names()
        } else {
            args.split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect()
        };
        let mut selected = vec![];
        for n in names {
            let v = p
                .get(&n)
                .ok_or_else(|| error(format!("unknown vector {n}")))?;
            if v.scale.is_none() && args.is_empty() {
                continue;
            }
            selected.push((n, expr::resample(&v, &target)?))
        }
        let mut base = p
            .analysis
            .clone()
            .ok_or_else(|| error("no transient plot"))?;
        base.scale_values = target;
        let linear = Plot {
            name: String::new(),
            analysis: Some(base),
            grid: p.grid,
            locals: Default::default(),
        };
        self.add_plot(linear.result(&selected)?, linear.grid);
        Ok(())
    }
}
fn cards(cmd: &str, args: &str) -> Result<Netlist> {
    if args.contains(['\n', '\r']) {
        return Err(error("unexpected newline"));
    }
    let nl = sansho_netlist::parse(&format!("control command\n.{cmd} {args}\n")).map_err(first)?;
    if let Some(d) = nl.warnings.first() {
        return Err(error(d.message.clone()));
    }
    Ok(nl)
}
