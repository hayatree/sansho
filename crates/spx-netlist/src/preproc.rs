//! Preprocessing: file inclusion, parameters and subcircuit flattening.
//!
//! The output is a flat list of logical lines in which every parameter expression has been
//! replaced by a number and every subcircuit instance by its expanded contents, so that the
//! typed parser never sees `.param`, `.func`, `.subckt`, `X` or `.include`.
//!
//! Naming of flattened objects follows the convention of SPICE3-derived simulators: nodes
//! inside instance `x1` become `x1.<node>`, elements become `<letter>.x1.<name>` (e.g.
//! `m.x1.xinv.m1`), and nested instances extend the path. Ground and `.global` nodes are never
//! renamed.
//!
//! Provenance: original work. Subcircuit and parameter semantics follow the SPICE input
//! format (Nagel 1975) and commonly documented ngspice conventions (`.subckt ... params:`,
//! `.lib file section`, `.global`). See `docs/PROVENANCE.md`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use spx_diag::{Confidence, Diagnostic, Span};

use crate::expr::{Expr, Func, Funcs, eval, is_number_literal, parse_expr};
use crate::lexer::{Line, Token, lex_source};

/// Supplies the text of `.include` / `.lib` files.
pub trait SourceLoader {
    /// Identify an immutable include during this parse, for lexed-library reuse.
    fn cache_path(&self, _path: &str, _from: Option<&str>) -> Option<PathBuf> {
        None
    }
    /// Load `path` as written in a netlist; `from` is the resolved path of the including
    /// file (`None` for the main netlist). Returns the resolved path and the file text.
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String>;
}

/// A loader that refuses all file access (for netlists given as text only).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFiles;

impl SourceLoader for NoFiles {
    fn load(&self, path: &str, _from: Option<&str>) -> Result<(String, String), String> {
        Err(format!(
            "cannot include `{path}`: file access is not available for this netlist"
        ))
    }
}

/// A loader that reads files relative to the including file (the main netlist resolves
/// relative to `base`).
#[derive(Debug, Clone)]
pub struct FsLoader {
    /// Directory of the main netlist.
    pub base: PathBuf,
}

impl FsLoader {
    /// Resolve a netlist reference using the same directory as the parser.
    pub fn resolve(&self, path: &str, from: Option<&str>) -> PathBuf {
        let expanded = match path.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME")
                .map_or_else(|| PathBuf::from(path), |h| PathBuf::from(h).join(rest)),
            None => PathBuf::from(path),
        };
        let dir = from
            .and_then(|f| Path::new(f).parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.base.clone());
        if expanded.is_absolute() {
            expanded
        } else {
            dir.join(expanded)
        }
    }
}

impl SourceLoader for FsLoader {
    fn cache_path(&self, path: &str, from: Option<&str>) -> Option<PathBuf> {
        Some(self.resolve(path, from))
    }

    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
        let full = self.resolve(path, from);
        let bytes =
            std::fs::read(&full).map_err(|e| format!("cannot read `{}`: {e}", full.display()))?;
        // Legacy SPICE libraries sometimes carry Latin-1 characters in comments.
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|e| e.into_bytes().into_iter().map(char::from).collect());
        Ok((full.display().to_string(), text))
    }
}

/// Result of preprocessing.
#[derive(Debug)]
pub(crate) struct Expanded {
    pub title: String,
    pub lines: Vec<Line>,
    /// Repeated model headers refer to the earlier card supplying their parameter tokens.
    pub model_scopes: HashMap<usize, Rc<ModelScope>>,
    pub model_sources: HashMap<usize, usize>,
    pub sources: Vec<String>,
    pub hdl: Vec<crate::HdlSource>,
    pub diags: Vec<Diagnostic>,
}

/// Maximum include / subcircuit nesting.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone)]
struct SubcktDef {
    ports: Vec<String>,
    /// Default parameters (`name = expr`), evaluated in the instance scope.
    defaults: Vec<(String, Rc<str>)>,
    /// Body lines for per-instance elaboration.
    body: Vec<Line>,
    leaf: RefCell<Option<Leaf>>,
    span: Span,
}

/// A substituted primitive/model body. Instance and net names are applied on use.
#[derive(Clone, Debug)]
struct Leaf {
    first_output: usize,
    arguments: Vec<(String, u64)>,
    functions: usize,
    body: Vec<Line>,
    local_models: HashSet<String>,
}

/// A parameter scope: instance overrides, body `.param`s and defaults, then the global scope.
#[derive(Debug)]
struct Scope<'p> {
    parent: Option<&'p Scope<'p>>,
    fixed: HashMap<String, f64>,
    defs: HashMap<String, Rc<str>>,
    expressions: Rc<RefCell<HashMap<String, Rc<Expr>>>>,
    cache: RefCell<HashMap<String, f64>>,
    busy: RefCell<HashSet<String>>,
}

impl<'p> Scope<'p> {
    fn new(parent: Option<&'p Scope<'p>>) -> Self {
        Self {
            parent,
            expressions: parent.map(|p| p.expressions.clone()).unwrap_or_default(),
            fixed: HashMap::new(),
            defs: HashMap::new(),
            cache: RefCell::new(HashMap::new()),
            busy: RefCell::new(HashSet::new()),
        }
    }

    fn get(&self, name: &str, funcs: &Funcs) -> Result<f64, String> {
        if let Some(v) = self.fixed.get(name) {
            return Ok(*v);
        }
        if let Some(v) = self.cache.borrow().get(name) {
            return Ok(*v);
        }
        if let Some(e) = self.defs.get(name) {
            if !self.busy.borrow_mut().insert(name.to_string()) {
                return Err(format!("parameter `{name}` depends on itself"));
            }
            let r = self.eval_text(e, funcs);
            self.busy.borrow_mut().remove(name);
            let v = r?;
            self.cache.borrow_mut().insert(name.to_string(), v);
            return Ok(v);
        }
        match self.parent {
            Some(p) => p.get(name, funcs),
            None => Err(format!("unknown parameter `{name}`")),
        }
    }

    fn expression(&self, text: &str) -> Result<Rc<Expr>, String> {
        if let Some(e) = self.expressions.borrow().get(text) {
            return Ok(e.clone());
        }
        let e = Rc::new(parse_expr(text)?);
        self.expressions.borrow_mut().insert(text.into(), e.clone());
        Ok(e)
    }

    fn eval_text(&self, text: &str, funcs: &Funcs) -> Result<f64, String> {
        let e = self.expression(text)?;
        eval(&e, &|n| self.get(n, funcs), funcs)
    }
}

/// Lexical environment retained by a model until a device selects it.
#[derive(Debug)]
pub(crate) struct ModelScope {
    local: Scope<'static>,
    global: Rc<Scope<'static>>,
    funcs: Funcs,
}

impl Scope<'_> {
    fn snapshot(&self) -> Scope<'static> {
        let mut saved = Scope::new(None);
        saved.fixed.clone_from(&self.fixed);
        saved.defs.clone_from(&self.defs);
        saved.expressions = self.expressions.clone();
        saved
    }
}

impl ModelScope {
    pub fn evaluator(
        &self,
        instance: &[(String, f64)],
    ) -> impl Fn(&str) -> Result<f64, String> + use<'_> {
        let mut local = Scope::new(Some(&self.global));
        local.fixed.clone_from(&self.local.fixed);
        local.defs.clone_from(&self.local.defs);
        local.fixed.extend(instance.iter().cloned());
        if !local.defs.contains_key("nf") {
            local.fixed.entry("nf".into()).or_insert(1.);
        }
        if !local.defs.contains_key("mult") {
            let m = local.fixed.get("m").copied().unwrap_or(1.);
            local.fixed.entry("mult".into()).or_insert(m);
        }
        move |text| local.eval_text(text, &self.funcs)
    }
}

/// Split `a=1 b = {x*2} c='y + 1'` into `(name, expression text)` pairs.
pub(crate) fn split_assignments(s: &str) -> Result<Vec<(String, String)>, String> {
    let b = s.as_bytes();
    // Positions of top-level `=` that are assignments (not part of ==, !=, <=, >=).
    let mut eqs = Vec::new();
    let (mut depth, mut quote) = (0i32, false);
    for i in 0..b.len() {
        match b[i] {
            b'\'' => quote = !quote,
            b'{' | b'(' if !quote => depth += 1,
            b'}' | b')' if !quote => depth -= 1,
            b'=' if !quote && depth == 0 => {
                let prev = if i > 0 { b[i - 1] } else { b' ' };
                let next = b.get(i + 1).copied().unwrap_or(b' ');
                if !matches!(prev, b'=' | b'!' | b'<' | b'>') && next != b'=' {
                    eqs.push(i);
                }
            }
            _ => {}
        }
    }
    let name_before = |eq: usize| -> Option<(usize, String)> {
        let mut end = eq;
        while end > 0 && b[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_') {
            start -= 1;
        }
        (start < end).then(|| (start, s[start..end].to_ascii_lowercase()))
    };
    let mut out = Vec::new();
    for (k, &eq) in eqs.iter().enumerate() {
        let (_, name) = name_before(eq).ok_or("expected `name = value`")?;
        let end = match eqs.get(k + 1) {
            Some(&next) => name_before(next).ok_or("expected `name = value`")?.0,
            None => s.len(),
        };
        let expr = s[eq + 1..end].trim();
        if expr.is_empty() {
            return Err(format!("missing value for `{name}`"));
        }
        out.push((name, expr.to_string()));
    }
    Ok(out)
}

/// `.func name(a, b) = expr` / `.func name(a, b) {expr}`.
fn parse_func(rest: &str) -> Result<(String, Func), String> {
    let open = rest.find('(').ok_or("expected `name(args)`")?;
    let close = rest[open..].find(')').ok_or("missing `)`")? + open;
    let name = rest[..open].trim().to_ascii_lowercase();
    if name.is_empty() {
        return Err("missing function name".into());
    }
    let args = rest[open + 1..close]
        .split(',')
        .map(|a| a.trim().to_ascii_lowercase())
        .filter(|a| !a.is_empty())
        .collect();
    let body = rest[close + 1..].trim();
    let body = body.strip_prefix('=').unwrap_or(body);
    Ok((
        name,
        Func {
            args,
            body: parse_expr(body)?,
        },
    ))
}

fn head(line: &Line) -> String {
    line.tokens[0].lower()
}

/// Text after the first word of the raw line.
fn raw_rest(line: &Line) -> &str {
    let r = line.raw.trim_start();
    r.get(line.tokens[0].text.len()..)
        .unwrap_or_default()
        .trim()
}

fn unquote(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'')
}

fn is_expr_token(t: &str) -> bool {
    t.starts_with('{') || (t.starts_with('\'') && t.len() > 1)
}

/// Format a number so that it parses back to the same value.
fn num_text(v: f64) -> String {
    format!("{v:e}")
}

struct Pre<'l> {
    libraries: HashMap<PathBuf, Rc<Vec<Line>>>,
    volatile_reads: usize,
    loader: &'l dyn SourceLoader,
    hdl: Vec<crate::HdlSource>,
    sources: Vec<String>,
    diags: Vec<Diagnostic>,
}

impl Pre<'_> {
    fn error(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::error(code, msg).with_span(span));
    }

    /// Load and lex a file, retaining its resolved path in the span source table.
    fn load(&mut self, path: &str, from: Option<&str>, span: Span) -> Option<Rc<Vec<Line>>> {
        let key = self.loader.cache_path(path, from);
        if let Some(lines) = key.as_ref().and_then(|k| self.libraries.get(k)) {
            return Some(lines.clone());
        }
        if key.is_none() {
            self.volatile_reads += 1;
        }
        match self.loader.load(path, from) {
            Ok((resolved, text)) => {
                let idx = self.sources.len() as u32;
                self.sources.push(resolved);
                let lines = Rc::new(lex_source(&text, idx, false).lines);
                if let Some(key) = key {
                    self.libraries.insert(key, lines.clone());
                }
                Some(lines)
            }
            Err(e) => {
                self.diags.push(
                    Diagnostic::error("E-PARSE-INCLUDE", e)
                        .with_span(span)
                        .with_suggestion(
                            "check the path; relative paths are resolved from the including file",
                            Confidence::Medium,
                        ),
                );
                None
            }
        }
    }

    /// Expand `.include` / `.lib`, drop `.control` blocks, stop at `.end`.
    fn expand(&mut self, lines: Vec<Line>, depth: usize, out: &mut Vec<Line>) {
        let mut it = lines.into_iter();
        while let Some(line) = it.next() {
            let h = head(&line);
            let file = line.span().file as usize;
            let origin = self.sources.get(file).filter(|s| !s.is_empty()).cloned();
            let from = origin.as_deref();
            match h.as_str() {
                ".if" | ".subckt" => {
                    // These bodies belong to another elaboration scope. In particular, an
                    // include in a discarded arm must never touch the filesystem.
                    let close = if h == ".if" { ".endif" } else { ".ends" };
                    let mut nesting = 1;
                    out.push(line);
                    for child in it.by_ref() {
                        let card = head(&child);
                        if card == h {
                            nesting += 1;
                        }
                        if card == close {
                            nesting -= 1;
                        }
                        out.push(child);
                        if nesting == 0 {
                            break;
                        }
                    }
                }
                ".hdl" => {
                    self.volatile_reads += 1;
                    if let Some(p) = line.tokens.get(1) {
                        match self.loader.load(unquote(&p.text), from) {
                            Ok((path, text)) => self.hdl.push(crate::HdlSource {
                                path,
                                text,
                                span: line.span(),
                            }),
                            Err(e) => self.error("E-PARSE-INCLUDE", e, line.span()),
                        }
                    } else {
                        self.error("E-PARSE-INCLUDE", "missing HDL file name", line.span());
                    }
                }
                ".end" => break,
                ".control" => {
                    self.diags.push(
                        Diagnostic::warning(
                            "W-PARSE-CONTROL",
                            ".control blocks are not supported and were skipped",
                        )
                        .with_span(line.span()),
                    );
                    for l in it.by_ref() {
                        if head(&l) == ".endc" {
                            break;
                        }
                    }
                }
                ".include" | ".inc" | ".lib" if depth >= MAX_DEPTH => {
                    self.error(
                        "E-PARSE-INCLUDE",
                        "includes nest too deeply (recursive include?)",
                        line.span(),
                    );
                }
                ".include" | ".inc" => {
                    let Some(p) = line.tokens.get(1) else {
                        self.error("E-PARSE-INCLUDE", "missing file name", line.span());
                        continue;
                    };
                    if let Some(lines) = self.load(unquote(&p.text), from, line.span()) {
                        self.expand(Rc::unwrap_or_clone(lines), depth + 1, out);
                    }
                }
                ".lib" if line.tokens.len() >= 3 => {
                    let file = unquote(&line.tokens[1].text).to_string();
                    let section = line.tokens[2].lower();
                    if let Some(lines) = self.load(&file, from, line.span()) {
                        match lib_section(&lines, &section) {
                            Some(sec) => self.expand(sec, depth + 1, out),
                            None => self.error(
                                "E-PARSE-INCLUDE",
                                format!("library `{file}` has no section `{section}`"),
                                line.span(),
                            ),
                        }
                    }
                }
                ".lib" => {
                    // A section definition in the main netlist: skip to `.endl`.
                    for l in it.by_ref() {
                        if head(&l) == ".endl" {
                            break;
                        }
                    }
                }
                ".endl" => {}
                _ => out.push(line),
            }
        }
    }
}

/// Lines of `.lib <name>` … `.endl` in a library file.
fn lib_section(lines: &[Line], name: &str) -> Option<Vec<Line>> {
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if head(l) == ".lib" && l.tokens.len() == 2 && l.tokens[1].lower() == name {
            let mut sec = Vec::new();
            for l in &lines[i + 1..] {
                if head(l) == ".endl" {
                    return Some(sec);
                }
                sec.push(l.clone());
            }
            return Some(sec);
        }
        i += 1;
    }
    None
}

/// Definitions collected from the expanded netlist.
#[derive(Default, Clone)]
struct Defs {
    function_version: usize,
    subckts: HashMap<String, Rc<SubcktDef>>,
    funcs: Funcs,
    globals: HashSet<String>,
    /// Every model name (top level and inside subcircuits), including binned base names.
    models: HashSet<String>,
}

fn model_names(name: &str, out: &mut HashSet<String>) {
    out.insert(name.to_string());
    // Binned models `nch.1`, `nch.2`, ... are referenced as `nch`.
    if let Some((base, bin)) = name.rsplit_once('.')
        && bin.chars().all(|c| c.is_ascii_digit())
    {
        out.insert(base.to_string());
    }
}

struct Ctx<'a> {
    copies: f64,
    prefix: String,
    ports: HashMap<String, String>,
    local_models: &'a HashSet<String>,
}

impl Ctx<'_> {
    fn node(&self, defs: &Defs, n: &str) -> String {
        let n = n.to_ascii_lowercase();
        if n == "0" || n == "gnd" || defs.globals.contains(&n) {
            return if n == "gnd" { "0".into() } else { n };
        }
        if let Some(m) = self.ports.get(&n) {
            return m.clone();
        }
        if self.prefix.is_empty() {
            n
        } else {
            format!("{}.{n}", self.prefix)
        }
    }

    fn element(&self, name: &str) -> String {
        let name = name.to_ascii_lowercase();
        if self.prefix.is_empty() {
            return name;
        }
        let letter = name.chars().next().unwrap_or('x');
        format!("{letter}.{}.{name}", self.prefix)
    }

    fn model(&self, m: &str) -> String {
        let m = m.to_ascii_lowercase();
        if !self.prefix.is_empty() && self.local_models.contains(&m) {
            format!("{}.{m}", self.prefix)
        } else {
            m
        }
    }
}

struct Flat<'l> {
    global_model_scope: Rc<Scope<'static>>,
    model_scopes: HashMap<usize, Rc<ModelScope>>,
    model_sources: HashMap<usize, usize>,
    next_version: usize,
    defs: Defs,
    pre: Pre<'l>,
    out: Vec<Line>,
    diags: Vec<Diagnostic>,
}

impl Flat<'_> {
    fn error(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::error(code, msg).with_span(span));
    }

    /// Evaluate an expression token to a number token.
    fn value(&mut self, t: &Token, scope: &Scope<'_>) -> Token {
        if !is_expr_token(&t.text) {
            return t.clone();
        }
        match scope.eval_text(&t.text, &self.defs.funcs) {
            Ok(v) => Token {
                text: num_text(v),
                span: t.span,
            },
            Err(e) => {
                self.error("E-PARSE-EXPR", format!("in `{}`: {e}", t.text), t.span);
                t.clone()
            }
        }
    }

    /// Evaluate the value after `=`: expressions, and bare parameter names or formulas.
    fn kv_value(&mut self, t: &Token, scope: &Scope<'_>) -> Token {
        if is_expr_token(&t.text) {
            return self.value(t, scope);
        }
        if is_number_literal(&t.text) {
            return t.clone();
        }
        match scope.eval_text(&t.text, &self.defs.funcs) {
            Ok(v) => Token {
                text: num_text(v),
                span: t.span,
            },
            // Not a parameter expression (e.g. `method=gear`, `rise=last`): keep as written.
            Err(_) => t.clone(),
        }
    }

    /// Substitute all values of a line: expression tokens anywhere, and `key = value`.
    fn substitute(&mut self, tokens: &[Token], scope: &Scope<'_>) -> Vec<Token> {
        let mut out = Vec::with_capacity(tokens.len());
        for (i, t) in tokens.iter().enumerate() {
            let after_eq = i > 0 && tokens[i - 1].is_eq();
            out.push(if after_eq {
                self.kv_value(t, scope)
            } else {
                self.value(t, scope)
            });
        }
        out
    }

    fn run(
        &mut self,
        lines: &[Line],
        ctx: &Ctx<'_>,
        scope: &Scope<'_>,
        depth: usize,
        substituted: bool,
        prior: Option<usize>,
    ) {
        let mut model_scope = None;
        for (offset, line) in lines.iter().enumerate() {
            let h = head(line);
            let span = line.span();
            if h.starts_with('.') {
                match h.as_str() {
                    ".param" | ".func" | ".global" => {}
                    ".model" => {
                        if let Some(start) = prior
                            && line.tokens.len() >= 3
                        {
                            let mut header = line.tokens[..3].to_vec();
                            header[1].text = ctx.model(&header[1].text);
                            self.model_sources.insert(self.out.len(), start + offset);
                            self.push(header, line);
                            continue;
                        }
                        // Keep model expressions in their lexical scope until the parser
                        // knows which geometry bin and primitive arguments are in use.
                        let deferred = line.tokens.get(3..).is_some_and(|tail| {
                            tail.chunks(3)
                                .any(|kv| kv.get(2).is_some_and(|v| !is_number_literal(&v.text)))
                        });
                        if deferred {
                            let captured = model_scope.get_or_insert_with(|| {
                                Rc::new(ModelScope {
                                    local: if scope.parent.is_some() {
                                        scope.snapshot()
                                    } else {
                                        Scope::new(None)
                                    },
                                    global: self.global_model_scope.clone(),
                                    funcs: self.defs.funcs.clone(),
                                })
                            });
                            self.model_scopes
                                .insert(self.out.len(), Rc::clone(captured));
                        }
                        let mut toks = if substituted || deferred {
                            line.tokens.clone()
                        } else {
                            self.substitute(&line.tokens, scope)
                        };
                        if let Some(name) = toks.get_mut(1) {
                            name.text = ctx.model(&name.text);
                        }
                        self.push(toks, line);
                    }
                    ".save" => {
                        let mut tokens = line.tokens.clone();
                        let mut fields = tokens[1..].iter_mut();
                        while let Some(probe) = fields.next() {
                            match probe.lower().as_str() {
                                "v" => {
                                    if let Some(node) = fields.next() {
                                        node.text = ctx.node(&self.defs, &node.text);
                                    }
                                }
                                "i" => {
                                    if let Some(device) = fields.next() {
                                        device.text = ctx.element(&device.text);
                                    }
                                }
                                _ => {}
                            }
                        }
                        self.push(tokens, line);
                    }
                    ".ic" | ".nodeset" => {
                        let mut toks = self.substitute(&line.tokens, scope);
                        // `v(node)=value` lexes as `v node = value`.
                        for i in 1..toks.len() {
                            if toks[i - 1].lower() == "v" && !toks[i].is_eq() {
                                toks[i].text = ctx.node(&self.defs, &toks[i].text);
                            }
                        }
                        self.push(toks, line);
                    }
                    _ if !ctx.prefix.is_empty() => {
                        self.diags.push(
                            Diagnostic::warning(
                                "W-PARSE-UNSUPPORTED",
                                format!("`{h}` inside a subcircuit is ignored"),
                            )
                            .with_span(span),
                        );
                    }
                    _ => {
                        let toks = self.substitute(&line.tokens, scope);
                        self.push(toks, line);
                    }
                }
                continue;
            }
            if h.starts_with('x') {
                self.instance(line, ctx, scope, depth);
                continue;
            }
            let mut toks = if substituted {
                line.tokens.clone()
            } else {
                self.substitute(&line.tokens, scope)
            };
            let letter = h.chars().next().unwrap_or(' ');
            let n = toks.len();
            let node_slots: Vec<usize> = match letter {
                'r' | 'c' | 'l' | 'v' | 'i' | 'd' | 'f' | 'h' => (1..3.min(n)).collect(),
                'e' | 'g' | 'm' => (1..5.min(n)).collect(),
                'n' => {
                    let end = (1..n)
                        .find(|&i| toks.get(i + 1).is_some_and(Token::is_eq))
                        .unwrap_or(n);
                    (1..end.saturating_sub(1)).collect()
                }
                'q' => {
                    let fourth_is_model = toks
                        .get(4)
                        .is_none_or(|t| self.defs.models.contains(&t.lower()) || t.is_eq());
                    let k = if fourth_is_model {
                        4
                    } else if toks
                        .get(6)
                        .is_some_and(|t| self.defs.models.contains(&t.lower()))
                    {
                        6
                    } else {
                        5
                    };
                    (1..k.min(n)).collect()
                }
                _ => Vec::new(),
            };
            for &i in &node_slots {
                toks[i].text = ctx.node(&self.defs, &toks[i].text);
            }
            if !ctx.prefix.is_empty() {
                toks[0].text = ctx.element(&toks[0].text);
                // Controlling source of F/H.
                if matches!(letter, 'f' | 'h')
                    && let Some(t) = toks.get_mut(3)
                {
                    t.text = ctx.element(&t.text);
                }
                // Model reference of D/Q/M: the first positional token after the nodes.
                if matches!(letter, 'd' | 'q' | 'm' | 'n') {
                    let at = node_slots.last().map_or(1, |&i| i + 1);
                    if let Some(t) = toks.get_mut(at)
                        && !t.is_eq()
                    {
                        t.text = ctx.model(&t.text);
                    }
                }
            }
            if ctx.copies != 1. && matches!(letter, 'm' | 'r' | 'l' | 'c' | 'd' | 'q') {
                let start = node_slots.last().copied().unwrap_or(0) + 1;
                if let Some(k) = toks[start..]
                    .windows(3)
                    .rposition(|kv| matches!(kv[0].lower().as_str(), "m" | "mult") && kv[1].is_eq())
                {
                    let value = &mut toks[start + k + 2];
                    if let Some(m) = crate::number::parse_number(&value.text) {
                        value.text = num_text(m * ctx.copies);
                    }
                } else {
                    toks.extend(
                        ["m".into(), "=".into(), num_text(ctx.copies)]
                            .map(|text| Token { text, span }),
                    );
                }
            }
            self.push(toks, line);
        }
    }

    /// Expand one lexical scope. Collect unconditional declarations first so forward
    /// parameter references work. Only descend into the arm selected in this scope.
    fn prepare(&mut self, input: Vec<Line>, scope: &mut Scope<'_>, depth: usize) -> Vec<Line> {
        if depth >= MAX_DEPTH {
            if let Some(line) = input.first() {
                self.error(
                    "E-PARSE-COND",
                    "conditional nesting limit reached",
                    line.span(),
                );
            }
            return vec![];
        }
        let mut lines = Vec::new();
        self.pre.expand(input, 0, &mut lines);
        let mut nested = 0usize;
        let mut temperature = None;
        for l in &lines {
            match head(l).as_str() {
                ".if" | ".subckt" => nested += 1,
                ".endif" | ".ends" => nested = nested.saturating_sub(1),
                ".temp" if nested == 0 && scope.parent.is_none() => temperature = l.tokens.get(1),
                ".option" | ".options" | ".opts" | ".opt"
                    if nested == 0 && scope.parent.is_none() =>
                {
                    for kv in l.tokens.windows(3) {
                        if kv[0].text.eq_ignore_ascii_case("temp") && kv[1].is_eq() {
                            temperature = Some(&kv[2]);
                        }
                    }
                }
                ".param" if nested == 0 => {
                    let parsed = split_assignments(raw_rest(l)).map(|pairs| {
                        for (name, value) in pairs {
                            scope.defs.insert(name, Rc::from(value));
                        }
                    });
                    if let Err(e) = parsed {
                        self.error("E-PARSE-EXPR", e, l.span());
                    }
                }
                ".func" if nested == 0 => match parse_func(raw_rest(l)) {
                    Ok((n, f)) => {
                        self.defs.funcs.insert(n, f);
                        self.next_version += 1;
                        self.defs.function_version = self.next_version;
                    }
                    Err(e) => self.error("E-PARSE-EXPR", e, l.span()),
                },
                _ => {}
            }
        }
        scope.cache.get_mut().clear();
        if let Some(t) = temperature
            && let Ok(v) = scope.eval_text(&t.text, &self.defs.funcs)
        {
            scope.fixed.insert("temper".into(), v);
            scope.cache.get_mut().clear();
        }
        let mut selected = Vec::new();
        let mut it = lines.into_iter();
        while let Some(line) = it.next() {
            let h = head(&line);
            if h == ".subckt" {
                let mut level = 1;
                selected.push(line);
                for l in it.by_ref() {
                    match head(&l).as_str() {
                        ".subckt" => level += 1,
                        ".ends" => level -= 1,
                        _ => {}
                    }
                    selected.push(l);
                    if level == 0 {
                        break;
                    }
                }
            } else if h == ".if" {
                let span = line.span();
                let mut active = self.condition(&line, scope);
                let mut taken = active;
                let mut saw_else = false;
                let mut level = 1;
                let mut arm = Vec::new();
                for l in it.by_ref() {
                    let card = head(&l);
                    match card.as_str() {
                        ".if" => level += 1,
                        ".endif" => {
                            level -= 1;
                            if level == 0 {
                                break;
                            }
                        }
                        ".elseif" | ".else" if level == 1 => {
                            if saw_else {
                                self.error("E-PARSE-COND", "branch after .else", l.span());
                            }
                            saw_else = card == ".else";
                            active = !taken && (saw_else || self.condition(&l, scope));
                            taken |= active;
                            continue;
                        }
                        _ => {}
                    }
                    if active {
                        arm.push(l);
                    }
                }
                if level > 0 {
                    self.error("E-PARSE-COND", "missing .endif", span);
                }
                selected.extend(self.prepare(arm, scope, depth + 1));
            } else if matches!(h.as_str(), ".else" | ".elseif" | ".endif") {
                self.error("E-PARSE-COND", format!("unexpected {h}"), line.span());
            } else {
                selected.push(line);
            }
        }
        selected
    }

    fn condition(&mut self, line: &Line, scope: &Scope<'_>) -> bool {
        match scope.eval_text(raw_rest(line), &self.defs.funcs) {
            Ok(v) => v != 0.,
            Err(e) => {
                self.error("E-PARSE-COND", e, line.span());
                false
            }
        }
    }

    fn push(&mut self, tokens: Vec<Token>, orig: &Line) {
        let raw = if tokens == orig.tokens {
            orig.raw.clone()
        } else {
            tokens
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        self.out.push(Line { tokens, raw });
    }

    fn instance(&mut self, line: &Line, ctx: &Ctx<'_>, scope: &Scope<'_>, depth: usize) {
        let t = &line.tokens;
        let span = line.span();
        let name = t[0].lower();
        // Positional tokens end at `params:` or the first `key =`.
        let mut end = t.len();
        for i in 1..t.len() {
            if t[i].lower() == "params:" || t.get(i + 1).is_some_and(Token::is_eq) {
                end = i;
                break;
            }
        }
        if end < 3 {
            self.error(
                "E-PARSE-SUBCKT",
                format!("`{name}`: expected `Xname nodes... subckt [params]`"),
                span,
            );
            return;
        }
        let sub = t[end - 1].lower();
        let nodes = &t[1..end - 1];
        let Some(def) = self.defs.subckts.get(&sub).cloned() else {
            self.error(
                "E-PARSE-SUBCKT",
                format!("`{name}` references undefined subcircuit `{sub}`"),
                span,
            );
            return;
        };
        if def.ports.len() != nodes.len() {
            self.diags.push(
                Diagnostic::error(
                    "E-PARSE-SUBCKT",
                    format!(
                        "`{name}` connects {} node(s), but subcircuit `{sub}` has {} port(s)",
                        nodes.len(),
                        def.ports.len()
                    ),
                )
                .with_span(span)
                .with_note(format!("ports: {}", def.ports.join(" "))),
            );
            return;
        }
        if depth >= MAX_DEPTH {
            self.error(
                "E-PARSE-SUBCKT",
                format!("subcircuit `{sub}` nests too deeply (recursive definition?)"),
                span,
            );
            return;
        }
        let diagnostics_before_args = self.diags.len();
        // Instance parameters are evaluated in the caller's scope.
        let mut inst = Scope::new(Some(global_of(scope)));
        let mut i = end;
        while i < t.len() {
            if t[i].lower() == "params:" {
                i += 1;
                continue;
            }
            if t.get(i + 1).is_some_and(Token::is_eq)
                && let Some(v) = t.get(i + 2)
            {
                let key = t[i].lower();
                match scope.eval_text(&v.text, &self.defs.funcs) {
                    Ok(x) => {
                        inst.fixed.insert(key, x);
                    }
                    Err(e) => self.error("E-PARSE-EXPR", format!("`{key}`: {e}"), v.span),
                }
                i += 3;
            } else {
                self.error(
                    "E-PARSE-SUBCKT",
                    format!("unexpected token `{}`", t[i].text),
                    t[i].span,
                );
                return;
            }
        }
        for (k, e) in &def.defaults {
            inst.defs.insert(k.clone(), e.clone());
        }
        let prefix = if ctx.prefix.is_empty() {
            name
        } else {
            format!("{}.{name}", ctx.prefix)
        };
        let ports = def
            .ports
            .iter()
            .zip(nodes)
            .map(|(p, n)| (p.clone(), ctx.node(&self.defs, &n.text)))
            .collect();
        let mut arguments: Vec<_> = inst
            .fixed
            .iter()
            .map(|(k, v)| (k.clone(), v.to_bits()))
            .collect();
        arguments.sort_unstable();
        // An explicitly declared multiplier is a wrapper parameter. An undeclared
        // m (or mult) denotes parallel copies of the instantiated subcircuit.
        let declared = |key: &str| {
            def.defaults.iter().any(|(n, _)| n == key)
                || def.body.iter().filter(|l| head(l) == ".param").any(|l| {
                    l.tokens
                        .windows(2)
                        .any(|w| w[0].text.eq_ignore_ascii_case(key) && w[1].is_eq())
                })
        };
        let copies = ctx.copies
            * ["m", "mult"]
                .iter()
                .filter_map(|&n| inst.fixed.get(n).filter(|_| !declared(n)))
                .product::<f64>();
        if !copies.is_finite() || copies <= 0. {
            self.error(
                "E-PARSE-VALUE",
                "subcircuit multiplicity must be positive",
                span,
            );
            return;
        }
        let version = self.defs.function_version;
        if let Some(leaf) = def.leaf.borrow().as_ref()
            && self.diags.len() == diagnostics_before_args
            && leaf.functions == version
            && leaf.arguments == arguments
        {
            let inner = Ctx {
                copies,
                prefix,
                ports,
                local_models: &leaf.local_models,
            };
            self.run(
                &leaf.body,
                &inner,
                &inst,
                depth + 1,
                true,
                Some(leaf.first_output),
            );
            return;
        }
        let before = (
            self.diags.len(),
            self.pre.diags.len(),
            self.pre.volatile_reads,
        );
        let outer = self.defs.clone();
        let lines = self.prepare(def.body.clone(), &mut inst, 0);
        for (index, tok) in t.iter().enumerate().skip(end) {
            if t.get(index + 1).is_some_and(Token::is_eq)
                && !inst.defs.contains_key(&tok.lower())
                && !matches!(tok.lower().as_str(), "m" | "mult")
            {
                self.diags.push(
                    Diagnostic::warning(
                        "W-PARSE-IGNORED",
                        format!(
                            "`{}`: subcircuit `{sub}` has no parameter `{}`",
                            t[0].text, tok.text
                        ),
                    )
                    .with_span(tok.span),
                );
            }
        }
        let mut models = HashSet::new();
        let body = collect(lines, &mut self.defs, &mut models, &mut self.diags);
        let inner = Ctx {
            copies,
            prefix,
            ports,
            local_models: &models,
        };
        // Cache expensive local model cards only when preparation was side-effect free.
        // Child overrides come from their caller, but the parent of inst is the global scope.
        if before.0 == diagnostics_before_args
            && before
                == (
                    self.diags.len(),
                    self.pre.diags.len(),
                    self.pre.volatile_reads,
                )
            && self.defs.globals == outer.globals
            && body
                .iter()
                .any(|line| line.tokens.len() > 64 && head(line) == ".model")
            && body.iter().all(|line| {
                matches!(head(line).as_str(), ".model" | ".param" | ".func")
                    || head(line).starts_with(['n', 'm'])
            })
        {
            let body: Vec<_> = body
                .iter()
                .filter(|l| !matches!(head(l).as_str(), ".param" | ".func"))
                .map(|l| Line {
                    tokens: if head(l) == ".model" {
                        l.tokens.clone()
                    } else {
                        self.substitute(&l.tokens, &inst)
                    },
                    raw: l.raw.clone(),
                })
                .collect();
            let first_output = self.out.len();
            self.run(&body, &inner, &inst, depth + 1, true, None);
            if self.diags.len() == before.0 {
                *def.leaf.borrow_mut() = Some(Leaf {
                    first_output,
                    arguments,
                    functions: version,
                    body,
                    local_models: models,
                });
            }
        } else {
            self.run(&body, &inner, &inst, depth + 1, false, None);
        }
        self.defs = outer;
    }
}

fn global_of<'a>(s: &'a Scope<'a>) -> &'a Scope<'a> {
    let mut cur = s;
    while let Some(p) = cur.parent {
        cur = p;
    }
    cur
}

/// Collect `.subckt` definitions, `.global` and model
/// names; returns the top-level lines.
fn collect(
    lines: Vec<Line>,
    defs: &mut Defs,
    local_models: &mut HashSet<String>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<Line> {
    let mut top = Vec::new();
    // Unconditional nested definitions keep the existing global visibility. A
    // conditional definition belongs to the instance that selects its arm.
    let mut stack: Vec<(String, SubcktDef)> = Vec::new();
    let mut conditional = 0usize;
    for line in lines {
        let h = head(&line);
        if let Some((_, def)) = stack.last_mut() {
            if h == ".if" {
                conditional += 1;
            }
            if conditional > 0 {
                if h == ".endif" {
                    conditional -= 1;
                }
                def.body.push(line);
                continue;
            }
        }
        match h.as_str() {
            ".subckt" => {
                let t = &line.tokens;
                let Some(name) = t.get(1).map(Token::lower) else {
                    diags.push(
                        Diagnostic::error("E-PARSE-SUBCKT", "missing subcircuit name")
                            .with_span(line.span()),
                    );
                    continue;
                };
                let mut ports = Vec::new();
                let mut defaults = Vec::new();
                let mut i = 2;
                while i < t.len() {
                    let lower = t[i].lower();
                    if lower == "params:" {
                        i += 1;
                    } else if t.get(i + 1).is_some_and(Token::is_eq) {
                        match t.get(i + 2) {
                            Some(v) => defaults.push((lower, Rc::from(v.text.as_str()))),
                            None => diags.push(
                                Diagnostic::error(
                                    "E-PARSE-EXPR",
                                    format!("missing value for `{lower}`"),
                                )
                                .with_span(t[i].span),
                            ),
                        }
                        i += 3;
                    } else {
                        ports.push(lower);
                        i += 1;
                    }
                }
                stack.push((
                    name,
                    SubcktDef {
                        ports,
                        defaults,
                        body: Vec::new(),
                        leaf: Default::default(),
                        span: line.span(),
                    },
                ));
            }
            ".ends" => match stack.pop() {
                Some((name, def)) => {
                    if defs.subckts.contains_key(&name) {
                        diags.push(
                            Diagnostic::error(
                                "E-PARSE-DUPLICATE",
                                format!("duplicate subcircuit name `{name}`"),
                            )
                            .with_span(def.span),
                        );
                    }
                    defs.subckts.insert(name, Rc::new(def));
                }
                None => diags.push(
                    Diagnostic::warning("W-PARSE-IGNORED", "`.ends` without `.subckt`")
                        .with_span(line.span()),
                ),
            },
            _ => {
                if h == ".global" {
                    for t in &line.tokens[1..] {
                        defs.globals.insert(t.lower());
                    }
                    continue;
                }
                if stack.is_empty()
                    && h == ".model"
                    && let Some(n) = line.tokens.get(1)
                {
                    model_names(&n.lower(), &mut defs.models);
                    model_names(&n.lower(), local_models);
                }
                match stack.last_mut() {
                    Some((_, def)) => def.body.push(line),
                    None => top.push(line),
                }
            }
        }
    }
    for (name, def) in stack {
        diags.push(
            Diagnostic::error(
                "E-PARSE-SUBCKT",
                format!("subcircuit `{name}` is missing `.ends`"),
            )
            .with_span(def.span),
        );
    }
    top
}

/// Preprocess a netlist: includes, libraries, parameters, functions and subcircuits.
pub(crate) fn preprocess(text: &str, loader: &dyn SourceLoader) -> Expanded {
    let _phase = spx_diag::Phase::start("netlist_pre");
    let lexed = lex_source(text, 0, true);
    let mut flat = Flat {
        global_model_scope: Rc::new(Scope::new(None)),
        model_scopes: HashMap::new(),
        model_sources: Default::default(),
        next_version: 0,
        defs: Defs::default(),
        pre: Pre {
            loader,
            libraries: Default::default(),
            volatile_reads: 0,
            hdl: Vec::new(),
            sources: vec![String::new()],
            diags: vec![],
        },
        out: vec![],
        diags: vec![],
    };
    let mut global = Scope::new(None);
    global.fixed.insert("temper".into(), 27.);
    let lines = flat.prepare(lexed.lines, &mut global, 0);
    let top = collect(lines, &mut flat.defs, &mut HashSet::new(), &mut flat.diags);
    flat.global_model_scope = Rc::new(global.snapshot());
    let empty = HashSet::new();
    let ctx = Ctx {
        copies: 1.,
        prefix: String::new(),
        ports: HashMap::new(),
        local_models: &empty,
    };
    drop(_phase);
    let _phase = spx_diag::Phase::start("flatten");
    flat.run(&top, &ctx, &global, 0, false, None);
    flat.diags.extend(flat.pre.diags);
    Expanded {
        hdl: flat.pre.hdl,
        title: lexed.title,
        lines: flat.out,
        model_sources: flat.model_sources,
        model_scopes: flat.model_scopes,
        sources: flat.pre.sources,
        diags: flat.diags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignments_split_at_top_level_equals() {
        let a = split_assignments("a=1 b = {x*2} c='y >= 1' d = max(a, b)").unwrap();
        let names: Vec<&str> = a.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "d"]);
        assert_eq!(a[1].1, "{x*2}");
        assert_eq!(a[2].1, "'y >= 1'");
        assert_eq!(a[3].1, "max(a, b)");
    }

    fn flat(src: &str) -> Vec<String> {
        let e = preprocess(src, &NoFiles);
        assert!(e.diags.iter().all(|d| !d.is_error()), "{:#?}", e.diags);
        e.lines.iter().map(|l| l.raw.clone()).collect()
    }

    #[test]
    fn parameters_and_nested_subcircuits() {
        let out = flat(
            "t\n.param vdd=1.8 wn={2*lmin} lmin=0.5u\n\
             .subckt inv in out vdd params: wn=1u\nM1 out in 0 0 nch w={wn} l=lmin\n\
             .model nch nmos vto={vdd/4}\n.ends\n\
             .subckt buf a y vdd\nXa a m vdd inv wn={wn*2}\nXb m y vdd inv\n.ends\n\
             X1 in out vdd buf\nV1 vdd 0 {vdd}\n.end\n",
        );
        let want = [
            "m.x1.xa.m1 x1.m in 0 0 x1.xa.nch w = 2e-6 l = 5e-7",
            ".model x1.xa.nch nmos vto = {vdd/4}",
            "m.x1.xb.m1 out x1.m 0 0 x1.xb.nch w = 1e-6 l = 5e-7",
            ".model x1.xb.nch nmos vto = {vdd/4}",
            "V1 vdd 0 1.8e0",
        ];
        assert_eq!(out, want);
    }

    #[test]
    fn selects_library_arms_and_instance_local_definitions() {
        struct Library;
        impl SourceLoader for Library {
            fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
                match (from,path) {
                    (None,"corners.lib") => Ok(("models/corners.lib".into(),
                         ".lib typ\n.if (corner)\n.include cell.inc\n.else\n.hdl absent.va\n.endif\n.endl\n".into())),
                    (Some("models/corners.lib"),"cell.inc") => Ok(("models/cell.inc".into(),
                        ".subckt cell a k=0\n.if(k < 1)\n.param r=2k\n.elseif (k == 1)\n.if (k)\n.param r=5k\n.endif\n.else\n.param r=8k\n.endif\nR1 a 0 'r'\n.ends\n".into())),
                    _ => Err(format!("unexpected input {path}"))
                }
            }
        }
        let e = preprocess(
            "t\n.lib corners.lib typ\n.param corner=floor(max(0,1.5))\nX1 a cell\nX2 b cell k='corner'\nX3 c cell k=2\n.if (0)\nXbad a missing\n.param bad=missing+1\n.elseif (corner)\nRtop a b 3\n.elseif (undefined)\n.include absent\n.endif\n",
            &Library,
        );
        assert!(e.diags.is_empty(), "{:#?}", e.diags);
        let cards = e.lines.iter().map(|l| l.raw.as_str()).collect::<Vec<_>>();
        assert_eq!(
            cards,
            [
                "r.x1.r1 a 0 2e3",
                "r.x2.r1 b 0 5e3",
                "r.x3.r1 c 0 8e3",
                "Rtop a b 3"
            ]
        );
    }

    #[test]
    fn conditional_errors_have_spans() {
        for body in [
            ".else",
            ".elseif (1)",
            ".endif",
            ".if (1)",
            ".if (missing)\n.endif",
            ".if (0)\n.else\n.elseif (1)\n.endif",
        ] {
            let e = preprocess(&format!("t\n{body}\n"), &NoFiles);
            assert!(
                e.diags
                    .iter()
                    .any(|d| d.code == "E-PARSE-COND" && d.span.is_some()),
                "{body}"
            );
        }
        assert_eq!(
            flat("t\n.if (temper == 60)\nR1 a 0 1\n.endif\n.option temp=60\n"),
            ["R1 a 0 1", ".option temp=60"]
        );
    }

    #[test]
    fn errors_are_reported() {
        let e = preprocess(
            "t\nX1 a b nosuch\n.subckt s a\n.ends\nX2 a b s\n.param p={q}\nR1 a 0 {p}\n",
            &NoFiles,
        );
        let codes: Vec<&str> = e.diags.iter().map(|d| d.code).collect();
        assert_eq!(
            codes.iter().filter(|c| **c == "E-PARSE-SUBCKT").count(),
            2,
            "{codes:?}"
        );
        assert!(codes.contains(&"E-PARSE-EXPR"));
        let e = preprocess("t\n.include missing.cir\n", &NoFiles);
        assert_eq!(e.diags[0].code, "E-PARSE-INCLUDE");
    }
}
