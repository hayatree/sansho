//! Parser from logical lines to a typed [`Netlist`].
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975) and
//! general SPICE3 conventions. See `docs/PROVENANCE.md`.

use spx_diag::{Confidence, Diagnostic, Span};

use crate::lexer::{Line, Token};
use crate::number::{parse_number, scale_suffix};
use crate::preproc::{NoFiles, SourceLoader, preprocess};
use crate::{
    AcSweep, AnalysisCard, Element, ElementKind, ModelCard, Netlist, NodeValue, OptionCard,
    SourceSpec, TranFn,
};

/// Model types accepted on `.model` cards.
const MODEL_TYPES: &[&str] = &["d", "npn", "pnp", "nmos", "pmos"];

/// Parse netlist text.
///
/// On success the returned netlist carries any warnings. On failure, all diagnostics (errors and
/// warnings) found in the whole input are returned.
pub fn parse(text: &str) -> Result<Netlist, Vec<Diagnostic>> {
    parse_with(text, &NoFiles)
}

/// Parse netlist text, loading `.include` / `.lib` files through `loader`.
pub fn parse_with(text: &str, loader: &dyn SourceLoader) -> Result<Netlist, Vec<Diagnostic>> {
    let pre = preprocess(text, loader);
    let _phase = spx_diag::Phase::start("netlist_parse");
    let mut p = Parser {
        has_hdl: !pre.hdl.is_empty(),
        diags: pre.diags,
        ..Parser::default()
    };
    let mut parsed_models = std::collections::HashMap::new();
    for (index, line) in pre.lines.iter().enumerate() {
        let head = line.tokens[0].lower();
        if head == ".model" {
            let original = pre.model_sources.get(&index).copied().unwrap_or(index);
            let before = p.models.len();
            p.model_card(
                line,
                &pre.lines[original],
                parsed_models.get(&original).copied(),
            );
            if index == original && p.models.len() > before {
                parsed_models.insert(index, before);
            }
        } else if head.starts_with('.') {
            p.dot_card(&head, line);
        } else {
            p.element(&head, line);
        }
    }
    p.resolve_bjt_substrates();
    p.resolve_bins();
    p.check_models();
    if p.diags.iter().any(Diagnostic::is_error) {
        return Err(p.diags);
    }
    Ok(Netlist {
        hdl: pre.hdl,
        title: pre.title,
        sources: pre.sources,
        initial_conditions: p.ics,
        nodesets: p.nodesets,
        elements: p.elements,
        analyses: p.analyses,
        options: p.options,
        models: p.models,
        saves: p.saves,
        measures: p.measures,
        warnings: p.diags,
    })
}

#[derive(Default)]
struct Parser {
    has_hdl: bool,
    elements: Vec<Element>,
    analyses: Vec<AnalysisCard>,
    options: Vec<OptionCard>,
    models: Vec<ModelCard>,
    saves: Vec<String>,
    measures: Vec<crate::MeasureCard>,
    ics: Vec<NodeValue>,
    nodesets: Vec<NodeValue>,
    diags: Vec<Diagnostic>,
    /// BJTs whose 4th/5th positional fields are ambiguous until all models are known:
    /// (element index, positional tokens after the three terminals).
    bjt_tail: Vec<(usize, Vec<Token>)>,
}

/// Fields of a semiconductor instance line after the name.
struct DeviceFields {
    positional: Vec<Token>,
    params: Vec<(String, f64)>,
    off: bool,
}

fn node_name(t: &Token) -> String {
    let s = t.lower();
    if s == "gnd" { crate::GROUND.into() } else { s }
}

impl Parser {
    fn err(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags
            .push(Diagnostic::error(code, msg).with_span(span));
    }

    fn number(&mut self, t: &Token, what: &str) -> Option<f64> {
        let v = parse_number(&t.text);
        if v.is_none() {
            self.err(
                "E-PARSE-NUMBER",
                format!("expected a number for {what}, found `{}`", t.text),
                t.span,
            );
        }
        v
    }

    /// Warn about component values that are probably unit mistakes.
    fn lint_value(&mut self, letter: &str, t: &Token, value: f64) {
        let suffix = scale_suffix(&t.text);
        if matches!(letter, "R" | "C")
            && suffix.starts_with('M')
            && !suffix.to_ascii_lowercase().starts_with("meg")
            && !suffix.to_ascii_lowercase().starts_with("mil")
        {
            let mantissa = &t.text[..t.text.len() - suffix.len()];
            let unit = if letter == "R" { "Ω" } else { "F" };
            self.diags.push(
                Diagnostic::warning(
                    "W-LINT-UNIT-MILLI",
                    format!(
                        "`{}` is {value:e} {unit}: SPICE reads `M` as milli, not mega",
                        t.text
                    ),
                )
                .with_span(t.span)
                .with_suggestion(
                    format!("write `{mantissa}meg` for mega, or `{mantissa}m` if milli is meant"),
                    Confidence::High,
                ),
            );
            return;
        }
        let implausible = match letter {
            "C" => value.abs() >= 1.0,
            "L" => value.abs() >= 1e3,
            _ => false,
        };
        if implausible {
            let unit = if letter == "C" { "F" } else { "H" };
            self.diags.push(
                Diagnostic::warning(
                    "W-LINT-UNIT-RANGE",
                    format!(
                        "`{}` is {value:e} {unit}, which is implausibly large",
                        t.text
                    ),
                )
                .with_span(t.span)
                .with_suggestion(
                    "add the intended scale suffix (`u`, `n`, `p`, ...)",
                    Confidence::Medium,
                ),
            );
        }
    }

    fn need<'a>(&mut self, line: &'a Line, count: usize, usage: &str) -> Option<&'a [Token]> {
        if line.tokens.len() < count {
            self.diags.push(
                Diagnostic::error(
                    "E-PARSE-ARITY",
                    format!(
                        "`{}` needs at least {} fields",
                        line.tokens[0].text,
                        count - 1
                    ),
                )
                .with_span(line.span())
                .with_note(format!("usage: {usage}")),
            );
            return None;
        }
        Some(&line.tokens)
    }

    fn element(&mut self, head: &str, line: &Line) {
        let span = line.span();
        let name = head.to_string();
        let kind = match head.as_bytes().first() {
            Some(b'r') => self.two_terminal(line, "R", "r"),
            Some(b'c') => self.two_terminal(line, "C", "c"),
            Some(b'l') => self.two_terminal(line, "L", "l"),
            Some(b'v') | Some(b'i') => self.source(line),
            Some(b'e') | Some(b'g') => self.vcontrolled(line),
            Some(b'f') | Some(b'h') => self.ccontrolled(line),
            Some(b'd') => self.diode(line),
            Some(b'q') => self.bjt(line),
            Some(b'm') => self.mosfet(line),
            Some(b'n') => self.va(line),
            _ => {
                self.err(
                    "E-PARSE-ELEMENT",
                    format!("unsupported element type `{}`", line.tokens[0].text),
                    span,
                );
                None
            }
        };
        if let Some(kind) = kind {
            if self.elements.iter().any(|e| e.name == name) {
                self.err(
                    "E-PARSE-DUPLICATE",
                    format!("duplicate element name `{name}`"),
                    span,
                );
                return;
            }
            self.elements.push(Element { name, span, kind });
        }
    }

    /// `Xname n+ n- value [key=value ...]` or `Xname n+ n- key=value ...`
    fn two_terminal(&mut self, line: &Line, letter: &str, key: &str) -> Option<ElementKind> {
        let usage = format!("{letter}name n+ n- value");
        let t = self.need(line, 3, &usage)?;
        let (p, n) = (node_name(&t[1]), node_name(&t[2]));
        let mut value = None;
        let mut i = 3;
        while i < t.len() {
            if t.get(i + 1).is_some_and(Token::is_eq) {
                let k = t[i].lower();
                let Some(vt) = t.get(i + 2) else {
                    self.err(
                        "E-PARSE-ARITY",
                        format!("missing value for `{k}`"),
                        t[i].span,
                    );
                    return None;
                };
                let v = self.number(vt, &k)?;
                if k == key {
                    value = Some(v);
                } else if k == "ic" {
                    self.diags.push(
                        Diagnostic::warning(
                            "W-PARSE-IGNORED",
                            "initial conditions (ic=) are not supported yet and were ignored",
                        )
                        .with_span(t[i].span),
                    );
                } else {
                    self.diags.push(
                        Diagnostic::warning(
                            "W-PARSE-IGNORED",
                            format!("unknown parameter `{k}` was ignored"),
                        )
                        .with_span(t[i].span),
                    );
                }
                i += 3;
            } else if value.is_none() && i == 3 {
                let v = self.number(&t[i], "the element value")?;
                self.lint_value(letter, &t[i], v);
                value = Some(v);
                i += 1;
            } else {
                self.err(
                    "E-PARSE-UNEXPECTED",
                    format!("unexpected token `{}`", t[i].text),
                    t[i].span,
                );
                return None;
            }
        }
        let Some(value) = value else {
            self.diags.push(
                Diagnostic::error("E-PARSE-ARITY", "missing element value")
                    .with_span(line.span())
                    .with_note(format!("usage: {usage}")),
            );
            return None;
        };
        if !value.is_finite() || (letter == "R" && value == 0.0) {
            self.err(
                "E-PARSE-VALUE",
                format!("invalid value {value} for {}", line.tokens[0].text),
                line.span(),
            );
            return None;
        }
        Some(match letter {
            "R" => ElementKind::Resistor { p, n, value },
            "C" => ElementKind::Capacitor { p, n, value },
            _ => ElementKind::Inductor { p, n, value },
        })
    }

    fn source(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(
            line,
            3,
            "Vname n+ n- [DC v] [AC mag [phase]] [PULSE|SIN|EXP|PWL(...)]",
        )?;
        let (p, n) = (node_name(&t[1]), node_name(&t[2]));
        let mut spec = SourceSpec::default();
        let mut i = 3;
        while i < t.len() {
            let word = t[i].lower();
            match word.as_str() {
                "dc" => {
                    let Some(vt) = t.get(i + 1) else {
                        self.err("E-PARSE-ARITY", "missing value after DC", t[i].span);
                        return None;
                    };
                    spec.dc = Some(self.number(vt, "DC")?);
                    i += 2;
                }
                "ac" => {
                    i += 1;
                    spec.ac_mag = 1.0;
                    if let Some(v) = t.get(i).and_then(|x| parse_number(&x.text)) {
                        spec.ac_mag = v;
                        i += 1;
                        if let Some(ph) = t.get(i).and_then(|x| parse_number(&x.text)) {
                            spec.ac_phase_deg = ph;
                            i += 1;
                        }
                    }
                }
                "pulse" | "sin" | "exp" | "pwl" => {
                    let start = i;
                    i += 1;
                    let mut vals = Vec::new();
                    while let Some(v) = t.get(i).and_then(|x| parse_number(&x.text)) {
                        vals.push(v);
                        i += 1;
                    }
                    spec.tran = Some(self.tran_fn(&word, vals, t[start].span)?);
                }
                _ => {
                    if i == 3
                        && let Some(v) = parse_number(&t[i].text)
                    {
                        spec.dc = Some(v);
                        i += 1;
                        continue;
                    }
                    self.err(
                        "E-PARSE-UNEXPECTED",
                        format!("unexpected token `{}` in source specification", t[i].text),
                        t[i].span,
                    );
                    return None;
                }
            }
        }
        Some(if t[0].lower().starts_with('v') {
            ElementKind::VoltageSource { p, n, source: spec }
        } else {
            ElementKind::CurrentSource { p, n, source: spec }
        })
    }

    fn tran_fn(&mut self, word: &str, vals: Vec<f64>, span: Span) -> Option<TranFn> {
        let (min, max) = match word {
            "pulse" => (2, 8),
            "sin" => (2, 6),
            "exp" => (2, 6),
            _ => (2, usize::MAX),
        };
        if vals.len() < min || vals.len() > max || (word == "pwl" && !vals.len().is_multiple_of(2))
        {
            self.err(
                "E-PARSE-ARITY",
                format!(
                    "{} has {} parameters; expected {}",
                    word.to_uppercase(),
                    vals.len(),
                    if word == "pwl" {
                        "an even number (time/value pairs)".to_string()
                    } else {
                        format!("{min} to {max}")
                    }
                ),
                span,
            );
            return None;
        }
        Some(match word {
            "pulse" => TranFn::Pulse(vals),
            "sin" => TranFn::Sin(vals),
            "exp" => TranFn::Exp(vals),
            _ => {
                let pts: Vec<(f64, f64)> = vals
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| (c[0], c[1]))
                    .collect();
                if pts.windows(2).any(|w| w[1].0 < w[0].0) {
                    self.err(
                        "E-PARSE-VALUE",
                        "PWL time points must be non-decreasing",
                        span,
                    );
                    return None;
                }
                TranFn::Pwl(pts)
            }
        })
    }

    fn vcontrolled(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(line, 6, "Ename n+ n- nc+ nc- gain")?;
        let (p, n, cp, cn) = (
            node_name(&t[1]),
            node_name(&t[2]),
            node_name(&t[3]),
            node_name(&t[4]),
        );
        let v = self.number(&t[5], "the gain")?;
        self.no_extra(t, 6);
        Some(if t[0].lower().starts_with('e') {
            ElementKind::Vcvs {
                p,
                n,
                cp,
                cn,
                gain: v,
            }
        } else {
            ElementKind::Vccs {
                p,
                n,
                cp,
                cn,
                gm: v,
            }
        })
    }

    fn ccontrolled(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(line, 5, "Fname n+ n- vname gain")?;
        let (p, n, control) = (node_name(&t[1]), node_name(&t[2]), t[3].lower());
        let v = self.number(&t[4], "the gain")?;
        self.no_extra(t, 5);
        Some(if t[0].lower().starts_with('f') {
            ElementKind::Cccs {
                p,
                n,
                control,
                gain: v,
            }
        } else {
            ElementKind::Ccvs {
                p,
                n,
                control,
                r: v,
            }
        })
    }

    /// Split instance fields into positional tokens, `key=value` pairs and the `off` flag.
    fn device_fields(&mut self, line: &Line) -> Option<DeviceFields> {
        let t = &line.tokens;
        let mut f = DeviceFields {
            positional: Vec::new(),
            params: Vec::new(),
            off: false,
        };
        let mut i = 1;
        while i < t.len() {
            if t.get(i + 1).is_some_and(Token::is_eq) {
                let k = t[i].lower();
                let Some(vt) = t.get(i + 2) else {
                    self.err(
                        "E-PARSE-ARITY",
                        format!("missing value for `{k}`"),
                        t[i].span,
                    );
                    return None;
                };
                let v = self.number(vt, &k)?;
                f.params.push((k, v));
                i += 3;
            } else if t[i].lower() == "off" {
                f.off = true;
                i += 1;
            } else {
                f.positional.push(t[i].clone());
                i += 1;
            }
        }
        Some(f)
    }

    /// Parse an optional positional area factor.
    fn area(&mut self, tok: Option<&Token>, params: &mut Vec<(String, f64)>) -> Option<()> {
        if let Some(t) = tok {
            let v = self.number(t, "the area factor")?;
            params.insert(0, ("area".into(), v));
        }
        Some(())
    }

    fn va(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(line, 2, "Nname nodes... model [key=value ...]")?;
        let end = (1..t.len())
            .find(|&i| t.get(i + 1).is_some_and(Token::is_eq))
            .unwrap_or(t.len());
        if end < 2 {
            return self.arity(line, "Nname nodes... model [key=value ...]");
        }
        let nodes = t[1..end - 1].iter().map(node_name).collect();
        let model = t[end - 1].lower();
        let mut params = Vec::new();
        let mut i = end;
        while i < t.len() {
            if !t.get(i + 1).is_some_and(Token::is_eq) || t.get(i + 2).is_none() {
                self.err("E-PARSE-ARITY", "expected key=value", t[i].span);
                return None;
            }
            params.push((t[i].lower(), self.number(&t[i + 2], "VA parameter")?));
            i += 3;
        }
        Some(ElementKind::Va {
            nodes,
            model,
            params,
        })
    }

    fn diode(&mut self, line: &Line) -> Option<ElementKind> {
        let usage = "Dname n+ n- model [area] [off]";
        let mut f = self.device_fields(line)?;
        if f.positional.len() < 3 || f.positional.len() > 4 {
            return self.arity(line, usage);
        }
        self.area(f.positional.get(3), &mut f.params)?;
        Some(ElementKind::Diode {
            p: node_name(&f.positional[0]),
            n: node_name(&f.positional[1]),
            model: f.positional[2].lower(),
            params: f.params,
            off: f.off,
        })
    }

    fn bjt(&mut self, line: &Line) -> Option<ElementKind> {
        let usage = "Qname nc nb ne [ns] model [area] [off]";
        let f = self.device_fields(line)?;
        if f.positional.len() < 4 || f.positional.len() > 6 {
            return self.arity(line, usage);
        }
        let p = &f.positional;
        // Resolved after all `.model` cards are known; see `resolve_bjt_substrates`.
        self.bjt_tail.push((self.elements.len(), p[3..].to_vec()));
        Some(ElementKind::Bjt {
            c: node_name(&p[0]),
            b: node_name(&p[1]),
            e: node_name(&p[2]),
            s: None,
            model: String::new(),
            params: f.params,
            off: f.off,
        })
    }

    fn mosfet(&mut self, line: &Line) -> Option<ElementKind> {
        let usage = "Mname nd ng ns nb model [l=..] [w=..] [ad=..] [as=..] [pd=..] [ps=..] [off]";
        let f = self.device_fields(line)?;
        if f.positional.len() != 5 {
            return self.arity(line, usage);
        }
        let p = &f.positional;
        Some(ElementKind::Mosfet {
            d: node_name(&p[0]),
            g: node_name(&p[1]),
            s: node_name(&p[2]),
            b: node_name(&p[3]),
            model: p[4].lower(),
            params: f.params,
            off: f.off,
        })
    }

    fn arity(&mut self, line: &Line, usage: &str) -> Option<ElementKind> {
        self.diags.push(
            Diagnostic::error(
                "E-PARSE-ARITY",
                format!("wrong number of fields for `{}`", line.tokens[0].text),
            )
            .with_span(line.span())
            .with_note(format!("usage: {usage}")),
        );
        None
    }

    /// Decide for every BJT whether the 4th positional field is a substrate node or the model
    /// name. It is the model name if a model of that name exists, or if no 5th field could be
    /// one.
    fn resolve_bjt_substrates(&mut self) {
        let tails = std::mem::take(&mut self.bjt_tail);
        for (idx, tail) in tails {
            if self.has_hdl
                && let [substrate, thermal, model] = tail.as_slice()
                && self.models.iter().any(|m| {
                    m.name == model.lower()
                        && matches!(m.kind.as_str(), "npn" | "pnp")
                        && m.params.iter().any(|(k, v)| k == "level" && *v == 9.)
                })
                && let Some(element) = self.elements.get_mut(idx)
                && let ElementKind::Bjt {
                    c, b, e, params, ..
                } = &mut element.kind
            {
                element.kind = ElementKind::Va {
                    nodes: vec![
                        c.clone(),
                        b.clone(),
                        e.clone(),
                        node_name(substrate),
                        node_name(thermal),
                    ],
                    model: model.lower(),
                    params: std::mem::take(params),
                };
                continue;
            }
            let is_model = |t: &Token| self.models.iter().any(|m| m.name == t.lower());
            let (sub, model, rest) = if is_model(&tail[0]) || tail.len() == 1 {
                (None, &tail[0], &tail[1..])
            } else if tail.len() >= 2
                && (is_model(&tail[1]) || parse_number(&tail[1].text).is_none())
            {
                (Some(node_name(&tail[0])), &tail[1], &tail[2..])
            } else {
                (None, &tail[0], &tail[1..])
            };
            let model = model.lower();
            let mut area = Vec::new();
            match rest {
                [] => {}
                [a] => {
                    if self.area(Some(a), &mut area).is_none() {
                        continue;
                    }
                }
                [_, extra, ..] => {
                    self.err(
                        "E-PARSE-UNEXPECTED",
                        format!("unexpected token `{}`", extra.text),
                        extra.span,
                    );
                    continue;
                }
            }
            if let Some(Element {
                kind:
                    ElementKind::Bjt {
                        s,
                        model: m,
                        params,
                        ..
                    },
                ..
            }) = self.elements.get_mut(idx)
            {
                *s = sub;
                *m = model;
                for a in area.into_iter().rev() {
                    params.insert(0, a);
                }
            }
        }
    }

    /// Report instances that reference undefined models or models of the wrong type.
    /// Resolve references to binned models (`nch` → `nch.1`, `nch.2`, ...) by the instance
    /// geometry: the bin with `lmin <= l < lmax` and `wmin <= w < wmax` is chosen.
    fn resolve_bins(&mut self) {
        let mut errs = Vec::new();
        for e in &mut self.elements {
            let ElementKind::Mosfet { model, params, .. } = &mut e.kind else {
                continue;
            };
            if self.models.iter().any(|m| m.name == *model) {
                continue;
            }
            let prefix = format!("{model}.");
            let bins: Vec<&ModelCard> = self
                .models
                .iter()
                .filter(|m| {
                    m.name
                        .strip_prefix(&prefix)
                        .is_some_and(|b| !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()))
                })
                .collect();
            if bins.is_empty() {
                continue;
            }
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|p| p.1);
            let (Some(l), Some(w)) = (get("l"), get("w")) else {
                errs.push(
                    Diagnostic::error(
                        "E-PARSE-MODEL",
                        format!(
                            "`{}`: binned model `{model}` needs explicit l= and w=",
                            e.name
                        ),
                    )
                    .with_span(e.span),
                );
                continue;
            };
            let p = |m: &ModelCard, k: &str, d: f64| {
                m.params.iter().find(|(n, _)| n == k).map_or(d, |p| p.1)
            };
            // Relative tolerance for geometry exactly on a bin boundary.
            let tol = 1e-9;
            let fits = |m: &&ModelCard| {
                let (lmin, lmax) = (p(m, "lmin", 0.0), p(m, "lmax", f64::INFINITY));
                let (wmin, wmax) = (p(m, "wmin", 0.0), p(m, "wmax", f64::INFINITY));
                l >= lmin * (1.0 - tol)
                    && l < lmax * (1.0 - tol)
                    && w >= wmin * (1.0 - tol)
                    && w < wmax * (1.0 - tol)
            };
            match bins.iter().find(|m| fits(m)) {
                Some(m) => *model = m.name.clone(),
                None => errs.push(
                    Diagnostic::error(
                        "E-PARSE-MODEL",
                        format!(
                            "`{}`: no bin of model `{model}` covers l = {l:e}, w = {w:e}",
                            e.name
                        ),
                    )
                    .with_span(e.span),
                ),
            }
        }
        self.diags.extend(errs);
    }

    fn check_models(&mut self) {
        let mut errs = Vec::new();
        for e in &self.elements {
            let Some(name) = e.kind.model() else { continue };
            let expected: &[&str] = match e.kind {
                ElementKind::Diode { .. } => &["d"],
                ElementKind::Bjt { .. } => &["npn", "pnp"],
                ElementKind::Va { .. } => &[],
                _ => &["nmos", "pmos"],
            };
            match self.models.iter().find(|m| m.name == name) {
                None => errs.push(
                    Diagnostic::error(
                        "E-PARSE-MODEL",
                        format!("`{}` references undefined model `{name}`", e.name),
                    )
                    .with_span(e.span),
                ),
                Some(m) if !expected.is_empty() && !expected.contains(&m.kind.as_str()) => errs
                    .push(
                        Diagnostic::error(
                            "E-PARSE-MODEL",
                            format!(
                                "`{}` needs a model of type {}, but `{name}` is of type `{}`",
                                e.name,
                                expected.join("/"),
                                m.kind
                            ),
                        )
                        .with_span(e.span),
                    ),
                Some(_) => {}
            }
        }
        self.diags.extend(errs);
    }

    fn model_card(&mut self, line: &Line, parameters: &Line, cached: Option<usize>) {
        let Some(t) = self.need(line, 3, ".model name type [(param=value ...)]") else {
            return;
        };
        let name = t[1].lower();
        let kind = t[2].lower();
        if !self.has_hdl && !MODEL_TYPES.contains(&kind.as_str()) {
            self.diags.push(
                Diagnostic::error(
                    "E-PARSE-MODEL",
                    format!("unsupported model type `{}`", t[2].text),
                )
                .with_span(t[2].span)
                .with_note(format!("supported types: {}", MODEL_TYPES.join(", "))),
            );
            return;
        }
        if self.models.iter().any(|m| m.name == name) {
            self.err(
                "E-PARSE-DUPLICATE",
                format!("duplicate model name `{name}`"),
                t[1].span,
            );
            return;
        }
        if let Some(index) = cached {
            let params = self.models[index].params.clone();
            self.models.push(ModelCard {
                name,
                kind,
                params,
                span: line.span(),
            });
            return;
        }
        let t = &parameters.tokens;
        let mut params = Vec::new();
        let mut i = 3;
        while i < t.len() {
            let k = t[i].lower();
            if !t.get(i + 1).is_some_and(Token::is_eq) {
                self.err(
                    "E-PARSE-UNEXPECTED",
                    format!("expected `name=value` in .model, found `{}`", t[i].text),
                    t[i].span,
                );
                return;
            }
            let Some(vt) = t.get(i + 2) else {
                self.err(
                    "E-PARSE-ARITY",
                    format!("missing value for `{k}`"),
                    t[i].span,
                );
                return;
            };
            let Some(v) = self.number(vt, &k) else {
                return;
            };
            params.push((k, v));
            i += 3;
        }
        self.models.push(ModelCard {
            name,
            kind,
            params,
            span: line.span(),
        });
    }

    fn no_extra(&mut self, t: &[Token], used: usize) {
        if let Some(x) = t.get(used) {
            self.diags.push(
                Diagnostic::warning(
                    "W-PARSE-IGNORED",
                    format!("extra token `{}` was ignored", x.text),
                )
                .with_span(x.span),
            );
        }
    }

    fn dot_card(&mut self, head: &str, line: &Line) {
        let span = line.span();
        let t = &line.tokens;
        match head {
            ".op" => self.analyses.push(AnalysisCard::Op { span }),
            ".dc" => {
                let Some(t) = self.need(line, 5, ".dc srcname start stop incr") else {
                    return;
                };
                let (Some(start), Some(stop), Some(step)) = (
                    self.number(&t[2], "start"),
                    self.number(&t[3], "stop"),
                    self.number(&t[4], "increment"),
                ) else {
                    return;
                };
                if step == 0.0 || (stop - start) * step < 0.0 {
                    self.err(
                        "E-PARSE-VALUE",
                        "DC sweep increment must be non-zero and point from start to stop",
                        t[4].span,
                    );
                    return;
                }
                if t.len() > 5 {
                    self.diags.push(
                        Diagnostic::warning(
                            "W-PARSE-IGNORED",
                            "nested DC sweeps are not supported yet; the second sweep was ignored",
                        )
                        .with_span(t[5].span),
                    );
                }
                self.analyses.push(AnalysisCard::Dc {
                    span,
                    source: t[1].lower(),
                    start,
                    stop,
                    step,
                });
            }
            ".ac" => {
                let Some(t) = self.need(line, 5, ".ac dec|oct|lin n fstart fstop") else {
                    return;
                };
                let sweep = match t[1].lower().as_str() {
                    "dec" => AcSweep::Dec,
                    "oct" => AcSweep::Oct,
                    "lin" => AcSweep::Lin,
                    _ => {
                        self.err(
                            "E-PARSE-VALUE",
                            format!(
                                "unknown AC sweep type `{}` (use dec, oct or lin)",
                                t[1].text
                            ),
                            t[1].span,
                        );
                        return;
                    }
                };
                let (Some(np), Some(fstart), Some(fstop)) = (
                    self.number(&t[2], "number of points"),
                    self.number(&t[3], "fstart"),
                    self.number(&t[4], "fstop"),
                ) else {
                    return;
                };
                if np < 1.0 || fstart <= 0.0 || fstop < fstart {
                    self.err(
                        "E-PARSE-VALUE",
                        "AC sweep needs n >= 1 and 0 < fstart <= fstop",
                        span,
                    );
                    return;
                }
                self.analyses.push(AnalysisCard::Ac {
                    span,
                    sweep,
                    points: np as u32,
                    fstart,
                    fstop,
                });
            }
            ".tran" => {
                let Some(t) = self.need(line, 3, ".tran tstep tstop [tstart [tmax]] [uic]") else {
                    return;
                };
                let mut uic = false;
                let mut nums = Vec::new();
                for tok in &t[1..] {
                    if tok.lower() == "uic" {
                        uic = true;
                    } else if let Some(v) = self.number(tok, ".tran parameter") {
                        nums.push(v);
                    } else {
                        return;
                    }
                }
                if nums.len() < 2 || nums.len() > 4 {
                    self.err("E-PARSE-ARITY", ".tran expects 2 to 4 numbers", span);
                    return;
                }
                let (tstep, tstop) = (nums[0], nums[1]);
                let tstart = nums.get(2).copied().unwrap_or(0.0);
                let tmax = nums.get(3).copied();
                if tstep <= 0.0 || tstop <= 0.0 || tstart < 0.0 || tstart >= tstop {
                    self.err(
                        "E-PARSE-VALUE",
                        ".tran needs tstep > 0, tstop > 0 and 0 <= tstart < tstop",
                        span,
                    );
                    return;
                }
                self.analyses.push(AnalysisCard::Tran {
                    span,
                    tstep,
                    tstop,
                    tstart,
                    tmax,
                    uic,
                });
            }
            ".options" | ".option" | ".opt" | ".opts" => {
                let mut i = 1;
                while i < t.len() {
                    let name = t[i].lower();
                    if t.get(i + 1).is_some_and(Token::is_eq) {
                        let Some(vt) = t.get(i + 2) else {
                            self.err(
                                "E-PARSE-ARITY",
                                format!("missing value for `{name}`"),
                                t[i].span,
                            );
                            return;
                        };
                        if name == "method" {
                            // `method=gear|trap` is stored as a flag named after the method.
                            self.options.push(OptionCard {
                                name: vt.lower(),
                                value: None,
                                span: vt.span,
                            });
                            i += 3;
                            continue;
                        }
                        let Some(v) = parse_number(&vt.text) else {
                            self.diags.push(
                                Diagnostic::warning(
                                    "W-OPT-IGNORED",
                                    format!("unsupported option `{name}={}` was ignored", vt.text),
                                )
                                .with_span(vt.span),
                            );
                            i += 3;
                            continue;
                        };
                        self.options.push(OptionCard {
                            name,
                            value: Some(v),
                            span: t[i].span,
                        });
                        i += 3;
                    } else {
                        self.options.push(OptionCard {
                            name,
                            value: None,
                            span: t[i].span,
                        });
                        i += 1;
                    }
                }
            }
            ".save" => {
                if self.need(line, 2, ".save v(node) i(vsrc) | all").is_none() {
                    return;
                }
                let mut fields = t[1..].iter();
                while let Some(field) = fields.next() {
                    let name = field.lower();
                    let signal = match name.as_str() {
                        "all" => name,
                        "v" | "i" => {
                            let Some(arg) = fields.next() else {
                                self.err("E-PARSE-ARITY", "missing .save signal name", field.span);
                                return;
                            };
                            format!("{name}({})", arg.lower())
                        }
                        _ => {
                            self.err(
                                "E-PARSE-ARITY",
                                format!("unsupported .save signal {name}"),
                                field.span,
                            );
                            return;
                        }
                    };
                    if !self.saves.contains(&signal) {
                        self.saves.push(signal);
                    }
                }
            }

            ".meas" | ".measure" => match crate::meas::parse_meas(line) {
                Ok(m) => self.measures.push(m),
                Err(d) => self.diags.push(d),
            },
            ".temp" => {
                let Some(t) = self.need(line, 2, ".temp value") else {
                    return;
                };
                if let Some(v) = self.number(&t[1], "the temperature") {
                    self.options.push(OptionCard {
                        name: "temp".into(),
                        value: Some(v),
                        span,
                    });
                }
            }
            ".title" => {}
            ".ic" | ".nodeset" => {
                // `v(node)=value` lexes as `v node = value`.
                let mut out = Vec::new();
                let mut i = 1;
                while i < t.len() {
                    if t[i].lower() == "v"
                        && t.get(i + 2).is_some_and(Token::is_eq)
                        && let Some(vt) = t.get(i + 3)
                    {
                        let Some(v) = self.number(vt, "the node value") else {
                            return;
                        };
                        out.push(NodeValue {
                            node: node_name(&t[i + 1]),
                            value: v,
                            span: t[i].span,
                        });
                        i += 4;
                    } else {
                        self.err(
                            "E-PARSE-UNEXPECTED",
                            format!("expected `v(node)=value`, found `{}`", t[i].text),
                            t[i].span,
                        );
                        return;
                    }
                }
                if head == ".ic" {
                    self.ics.extend(out);
                } else {
                    self.nodesets.extend(out);
                }
            }
            _ => self.diags.push(
                Diagnostic::warning(
                    "W-PARSE-UNSUPPORTED",
                    format!("unsupported control card `{}` was ignored", t[0].text),
                )
                .with_span(span),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_local_models_keep_their_scope_and_diagnostics() {
        let fields = (0..24)
            .map(|i| format!("p{i}={{gain(a)}}"))
            .collect::<Vec<_>>()
            .join(" ");
        let deck = format!(
            "templates\n.func gain(x) {{x+1}}\n.subckt leaf pin params: a=0\n.model device d {fields}\nN1 pin device\n.ends\n.subckt wrapper pin\n.func gain(x) {{x+2}}\nxinner pin leaf a=3\n.ends\nx1 one leaf a=3\nx2 two leaf a=3\nx3 three wrapper\nx4 four leaf a=4\n.op\n.end"
        );
        let nl = parse(&deck).unwrap();
        for (index, (name, value)) in [
            ("x1.device", 4.),
            ("x2.device", 4.),
            ("x3.xinner.device", 5.),
            ("x4.device", 5.),
        ]
        .into_iter()
        .enumerate()
        {
            let model = &nl.models[index];
            assert_eq!(model.name, name);
            assert_eq!(nl.elements[index].kind.model(), Some(name));
            assert_eq!(model.params.len(), 24);
            assert!(model.params.iter().all(|(_, v)| *v == value));
        }
        let broken = deck.replace("p12={gain(a)}", "p12=missing");
        let errors = parse(&broken).unwrap_err();
        assert_eq!(
            errors.iter().filter(|d| d.code == "E-PARSE-NUMBER").count(),
            4
        );
    }

    #[test]
    fn unknown_string_option_warns_and_parsing_continues() {
        let nl = parse("options\n.option filetype=ascii reltol=0.001\nR1 a 0 1k\n.op\n").unwrap();
        assert!(nl.warnings.iter().any(|d| d.code == "W-OPT-IGNORED"));
        assert_eq!(nl.options[0].name, "reltol");
    }

    #[test]
    fn parses_elements_and_analyses() {
        let src = "\
rc test
V1 in 0 DC 1 AC 1 0 PULSE(0 1 1n 1n 1n 5u 10u)
R1 in out 1k
C1 out GND 1n
.options reltol=1e-4 nopage
.op
.dc V1 0 5 0.1
.ac dec 10 1 1meg
.tran 10n 20u
.end
R2 ignored after end 1
";
        let nl = parse(src).unwrap_or_else(|d| panic!("{d:?}"));
        assert_eq!(nl.title, "rc test");
        assert_eq!(nl.elements.len(), 3);
        match &nl.elements[0].kind {
            ElementKind::VoltageSource { p, n, source } => {
                assert_eq!((p.as_str(), n.as_str()), ("in", "0"));
                assert_eq!(source.dc, Some(1.0));
                assert_eq!(source.ac_mag, 1.0);
                assert!(matches!(&source.tran, Some(TranFn::Pulse(v)) if v.len() == 7));
            }
            k => panic!("unexpected {k:?}"),
        }
        assert!(matches!(
            &nl.elements[2].kind,
            ElementKind::Capacitor { n, value, .. } if n == "0" && (*value - 1e-9).abs() < 1e-24
        ));
        assert_eq!(nl.analyses.len(), 4);
        assert_eq!(nl.options.len(), 2);
        assert_eq!(nl.options[0].value, Some(1e-4));
        assert!(nl.warnings.is_empty());
    }

    #[test]
    fn reports_all_errors_with_spans() {
        let src = "t\nR1 a b\nZ1 c b e mod\nC1 a 0 xyz\n";
        let diags = parse(src).err().unwrap_or_default();
        let codes: Vec<_> = diags.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            ["E-PARSE-ARITY", "E-PARSE-ELEMENT", "E-PARSE-NUMBER"]
        );
        assert_eq!(diags[2].span, Some(Span::new(4, 8)));
    }

    #[test]
    fn controlled_sources() {
        let src = "t\nE1 o 0 a b 10\nG1 o 0 a b 1m\nF1 o 0 vs 2\nH1 o 0 vs 1k\n";
        let nl = parse(src).unwrap_or_else(|d| panic!("{d:?}"));
        assert!(
            matches!(&nl.elements[3].kind, ElementKind::Ccvs { control, r, .. } if control == "vs" && *r == 1e3)
        );
    }

    #[test]
    fn bare_dc_value_and_r_equals() {
        let nl =
            parse("t\nV1 a 0 5\nR1 a 0 r=2k\nI1 0 a dc 1m\n").unwrap_or_else(|d| panic!("{d:?}"));
        assert!(
            matches!(&nl.elements[0].kind, ElementKind::VoltageSource { source, .. } if source.dc == Some(5.0))
        );
        assert!(
            matches!(&nl.elements[1].kind, ElementKind::Resistor { value, .. } if *value == 2e3)
        );
    }
}
