//! Parser from logical lines to a typed [`Netlist`].
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975) and
//! general SPICE3 conventions. See `docs/PROVENANCE.md`.

use sansho_diag::{Confidence, Diagnostic, Span};

use crate::lexer::{Line, Token};
use crate::number::{parse_number, scale_suffix};
use crate::preproc::{NoFiles, SourceLoader, preprocess};
use crate::{
    AcSweep, AnalysisCard, Element, ElementKind, ModelCard, Netlist, NodeValue, OptionCard,
    SourceSpec, TranFn,
};

/// Parse netlist text.
///
/// On success the returned netlist carries any warnings. On failure, all diagnostics (errors and
/// warnings) found in the whole input are returned.
pub fn parse(text: &str) -> Result<Netlist, Vec<Diagnostic>> {
    parse_with(text, &NoFiles)
}

/// Parse netlist text, loading `.include` / `.lib` files through `loader`.
pub fn parse_with(text: &str, loader: &dyn SourceLoader) -> Result<Netlist, Vec<Diagnostic>> {
    parse_expanded(preprocess(text, loader), false).map(|p| p.nominal)
}

/// Compile the lexical parameter environments for a statistical batch.
pub fn prepare_parameters(
    text: &str,
    loader: &dyn SourceLoader,
) -> Result<crate::ParameterPlan, Vec<Diagnostic>> {
    parse_expanded(crate::preproc::preprocess_with(text, loader, true), true)
}

fn parse_expanded(
    mut pre: crate::preproc::Expanded,
    statistical: bool,
) -> Result<crate::ParameterPlan, Vec<Diagnostic>> {
    let mut elements = Vec::new();
    let mut directives = Vec::new();
    let _phase = sansho_diag::Phase::start("netlist_parse");
    let mut p = Parser {
        vdmos_models: pre
            .lines
            .iter()
            .filter(|l| {
                l.tokens
                    .get(2)
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("vdmos"))
                    && l.tokens[0].text.eq_ignore_ascii_case(".model")
            })
            .map(|l| l.tokens[1].lower())
            .collect(),
        has_hdl: !pre.hdl.is_empty(),
        diags: pre.diags,
        ..Parser::default()
    };
    let mut dynamic = std::collections::HashMap::new();
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
                pre.model_scopes.get(&original).map(|s| s.as_ref()),
            );
            if p.models.len() > before {
                p.model_names.insert(p.models[before].name.clone(), before);
            }
            if let Some(scope) = pre.model_scopes.get(&original)
                && p.models.len() > before
            {
                dynamic.insert(before, (original, scope.as_ref(), &pre.lines[original]));
            }
            if index == original && p.models.len() > before {
                parsed_models.insert(index, before);
            }
        } else if head.starts_with('.') {
            if let Some(fields) = pre.fields.remove(&index)
                && !fields.is_empty()
            {
                let offset = match head.as_str() {
                    ".meas" | ".measure" => Some(p.measures.len()),
                    ".dc" | ".ac" | ".tran" => Some(p.analyses.len()),
                    ".ic" => Some(p.ics.len()),
                    ".nodeset" => Some(p.nodesets.len()),
                    _ => None,
                };
                if let Some(index) = offset {
                    directives.push(crate::parameters::BoundLine {
                        index,
                        line: line.clone(),
                        fields,
                    })
                } else {
                    p.err(
                        "E-MC-PARAM",
                        format!("statistical fields on {head} are unsupported"),
                        line.span(),
                    );
                }
            }
            p.dot_card(&head, line);
        } else {
            if let Some(fields) = pre.fields.remove(&index)
                && !fields.is_empty()
            {
                elements.push(crate::parameters::BoundLine {
                    index: p.elements.len(),
                    line: line.clone(),
                    fields,
                });
            }
            p.element(&head, line);
        }
    }
    p.resolve_bjt_substrates();
    let families = if statistical {
        p.bin_candidates()
    } else {
        Default::default()
    };
    p.resolve_bins(&dynamic);
    let mut model_bindings = Vec::new();
    if statistical {
        p.check_models();
        if !p.diags.iter().any(Diagnostic::is_error) {
            model_bindings = crate::parameters::bind_models(
                &mut p.elements,
                &mut p.models,
                &dynamic,
                &elements,
                &families,
                p.options
                    .iter()
                    .rev()
                    .find(|o| o.name == "scale")
                    .and_then(|o| o.value)
                    .unwrap_or(1.),
            )?;
        }
    } else {
        p.bind_model_expressions(&dynamic);
        p.check_models();
    }
    if p.diags.iter().any(Diagnostic::is_error) {
        return Err(p.diags);
    }
    Ok(crate::ParameterPlan {
        elements,
        directives,
        models: model_bindings,
        nominal: Netlist {
            hdl: pre.hdl,
            title: p.title.unwrap_or(pre.title),
            sources: pre.sources,
            initial_conditions: p.ics,
            nodesets: p.nodesets,
            elements: p.elements,
            analyses: p.analyses,
            options: p.options,
            models: p.models,
            saves: p.saves,
            prints: p.prints,
            measures: p.measures,
            warnings: p.diags,
        },
    })
}

pub(crate) fn sample_directive(
    line: &Line,
    index: usize,
    nl: &mut Netlist,
) -> Result<(), Vec<Diagnostic>> {
    let mut p = Parser::default();
    p.dot_card(&line.tokens[0].lower(), line);
    if p.diags.iter().any(Diagnostic::is_error) {
        return Err(p.diags);
    }
    // The expressions replace numeric tokens. The card kind and number of entries stay fixed.
    for (to, from) in nl.analyses.iter_mut().skip(index).zip(p.analyses) {
        *to = from
    }
    for (to, from) in nl.measures.iter_mut().skip(index).zip(p.measures) {
        *to = from
    }
    for (to, from) in nl.initial_conditions.iter_mut().skip(index).zip(p.ics) {
        *to = from
    }
    for (to, from) in nl.nodesets.iter_mut().skip(index).zip(p.nodesets) {
        *to = from
    }
    nl.warnings.extend(p.diags);
    Ok(())
}

pub(crate) fn sample_element(line: &Line, nl: &Netlist) -> Result<Element, Vec<Diagnostic>> {
    let head = line.tokens[0].lower();
    let mut p = Parser::default();
    if head.starts_with('m')
        && line
            .tokens
            .iter()
            .any(|t| t.text.eq_ignore_ascii_case("thermal"))
    {
        p.vdmos_models = nl
            .models
            .iter()
            .filter(|m| m.kind == "vdmos")
            .map(|m| m.name.clone())
            .collect();
    }
    p.element(&head, line);
    if head.starts_with('q') {
        p.models = nl.models.clone();
        p.has_hdl = !nl.hdl.is_empty();
        p.resolve_bjt_substrates();
    }
    if p.diags.iter().any(Diagnostic::is_error) {
        return Err(p.diags);
    }
    p.elements.pop().ok_or(p.diags)
}

pub(crate) type DynamicCards<'a> =
    std::collections::HashMap<usize, (usize, &'a crate::preproc::ModelScope, &'a Line)>;

#[derive(Default)]
struct Parser {
    vdmos_models: std::collections::HashSet<String>,
    title: Option<String>,
    prints: Vec<crate::PrintCard>,
    has_hdl: bool,
    elements: Vec<Element>,
    element_names: std::collections::HashSet<String>,
    analyses: Vec<AnalysisCard>,
    options: Vec<OptionCard>,
    models: Vec<ModelCard>,
    model_names: std::collections::HashMap<String, usize>,
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

pub(crate) fn dotted_version(s: &str) -> bool {
    s.split('.').count() > 2
        && s.split('.')
            .all(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
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
        let mut ic = Vec::new();
        let mut stripped;
        let line = if head.starts_with(['d', 'q', 'm', 'j', 'z', 't', 'o'])
            && let Some(index) = line
                .tokens
                .windows(2)
                .position(|w| w[0].lower() == "ic" && w[1].is_eq())
        {
            let count = match head.as_bytes()[0] {
                b'd' => 1,
                b'm' => 3,
                b't' | b'o' => 4,
                _ => 2,
            };
            for token in line.tokens.iter().skip(index + 2).take(count) {
                if let Some(v) = parse_number(&token.text) {
                    ic.push(v);
                } else {
                    break;
                }
            }
            if ic.len() != count && !(head.starts_with('m') && ic.len() == 2) {
                self.err(
                    "E-PARSE-ARITY",
                    format!("{name} IC requires {count} values"),
                    line.tokens[index].span,
                );
                return;
            }
            stripped = line.clone();
            stripped.tokens.drain(index..index + 2 + ic.len());
            &stripped
        } else {
            line
        };
        let kind = match head.as_bytes().first() {
            Some(b'o') => {
                let Some(t) = self.need(line, 6, "Oname p1 n1 p2 n2 model") else {
                    return;
                };
                self.no_extra(t, 6);
                Some(ElementKind::Ltra {
                    ports: [1, 2, 3, 4].map(|i| node_name(&t[i])),
                    model: t[5].lower(),
                })
            }
            Some(b'u') => self.urc(line),
            Some(b't') => self.line_device(line),
            Some(b'j' | b'z') => self.jfet(line),
            Some(b'e' | b'f' | b'g' | b'h') if line.polynomial() => self.polynomial(line),
            Some(b'k') => self.coupling(line),
            Some(b's' | b'w') => self.switch(line),
            Some(b'b') => self.behavioral(line),
            Some(b'e' | b'g') if line.tokens.get(4).is_some_and(Token::is_eq) => {
                self.behavioral(line)
            }
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
            if !ic.is_empty()
                && (matches!(kind, ElementKind::Mosfet { .. }) && ic.len() != 3
                    || matches!(kind, ElementKind::Vdmos { .. }) && ic.len() != 2)
            {
                self.err(
                    "E-PARSE-ARITY",
                    format!("invalid IC tuple for {name}"),
                    span,
                );
                return;
            }
            if !self.element_names.insert(name.clone()) {
                self.err(
                    "E-PARSE-DUPLICATE",
                    format!("duplicate element name `{name}`"),
                    span,
                );
                return;
            }
            self.elements.push(Element {
                name,
                span,
                kind,
                ic,
            });
        }
    }

    fn line_device(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(
            line,
            8,
            "Tname p1 n1 p2 n2 z0=value td=value (or f=value [nl=value])",
        )?;
        let mut z0 = None;
        let mut td = None;
        let mut freq = None;
        let mut nl = None;
        for kv in t[5..].chunks(3) {
            let [key, eq, value] = kv else {
                return self.arity(line, "expected line parameter=value");
            };
            let slot = match key.lower().as_str() {
                "z0" => &mut z0,
                "td" => &mut td,
                "f" => &mut freq,
                "nl" => &mut nl,
                _ => {
                    self.err(
                        "E-LINE-PARAM",
                        format!("unknown line parameter {}", key.text),
                        key.span,
                    );
                    return None;
                }
            };
            let v = self.number(value, &key.text)?;
            if !eq.is_eq() || slot.replace(v).is_some() || !v.is_finite() || v <= 0. {
                self.err(
                    "E-LINE-PARAM",
                    "line parameters must be unique, positive finite assignments",
                    key.span,
                );
                return None;
            }
        }
        let delay = match (td, freq, nl) {
            (Some(t), None, None) => Some(t),
            (None, Some(f), nl) => Some(nl.unwrap_or(0.25) / f),
            _ => None,
        };
        if let (Some(z0), Some(td)) = (z0, delay.filter(|d| d.is_finite() && *d > 0.)) {
            Some(ElementKind::Line {
                ports: [1, 2, 3, 4].map(|i| node_name(&t[i])),
                z0,
                td,
            })
        } else {
            self.err(
                "E-LINE-PARAM",
                "expected Z0 and either TD or F with optional NL",
                line.span(),
            );
            None
        }
    }

    fn polynomial(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(line, 6, "E/F/G/Hname p n POLY(n) controls coefficients")?;
        let letter = t[0].lower();
        let voltage_control = letter.starts_with(['e', 'g']);
        let width = if voltage_control { 2 } else { 1 };
        let Some(count) = t[4]
            .positive_integer()
            .filter(|&n| n <= (t.len() - 6) / width)
        else {
            self.err("E-POLY-ARITY", "expected positive dimension, one control per variable and at least one coefficient", line.span());
            return None;
        };
        let end = 5 + count * width;
        let controls = t[5..end]
            .chunks(width)
            .map(|args| {
                if voltage_control {
                    crate::SwitchControl::Voltage(node_name(&args[0]), node_name(&args[1]))
                } else {
                    crate::SwitchControl::Current(args[0].lower())
                }
            })
            .collect();
        let coefficients = t[end..]
            .iter()
            .map(|v| self.number(v, "polynomial coefficient"))
            .collect::<Option<Vec<_>>>()?;
        if coefficients.iter().any(|v| !v.is_finite()) {
            self.err(
                "E-PARSE-VALUE",
                "POLY coefficients must be finite",
                line.span(),
            );
            return None;
        }
        Some(ElementKind::Polynomial {
            p: node_name(&t[1]),
            n: node_name(&t[2]),
            voltage: letter.starts_with(['e', 'h']),
            controls,
            coefficients,
        })
    }

    fn urc(&mut self, line: &Line) -> Option<ElementKind> {
        let f = self.device_fields(line)?;
        let [a, b, sub, model] = f.positional.as_slice() else {
            return self.arity(line, "Uname n1 n2 ref model L=length [N=lumps]");
        };
        Some(ElementKind::Urc {
            ports: [a, b, sub].map(node_name),
            model: model.lower(),
            params: f.params,
        })
    }

    fn jfet(&mut self, line: &Line) -> Option<ElementKind> {
        let mut f = self.device_fields(line)?;
        let p = &f.positional;
        if !(4..=5).contains(&p.len()) {
            return self.arity(line, "Jname drain gate source model [area] [off]");
        }
        self.area(p.get(4), &mut f.params)?;
        if line.tokens[0].lower().starts_with('z') {
            return Some(ElementKind::Mesfet {
                d: node_name(&p[0]),
                g: node_name(&p[1]),
                s: node_name(&p[2]),
                model: p[3].lower(),
                params: f.params,
                off: f.off,
            });
        }
        Some(ElementKind::Jfet {
            d: node_name(&p[0]),
            g: node_name(&p[1]),
            s: node_name(&p[2]),
            model: p[3].lower(),
            params: f.params,
            off: f.off,
        })
    }

    fn coupling(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(line, 4, "Kname L1 L2 coefficient")?;
        self.no_extra(t, 4);
        let k = self.number(&t[3], "coupling coefficient")?;
        if !(-1.0..=1.0).contains(&k) || t[1].lower() == t[2].lower() {
            self.err(
                "E-COUPLING-K",
                "coupling needs distinct windings and a coefficient between -1 and 1",
                line.span(),
            );
            return None;
        }
        Some(ElementKind::Coupling {
            first: t[1].lower(),
            second: t[2].lower(),
            k,
        })
    }

    fn switch(&mut self, line: &Line) -> Option<ElementKind> {
        let voltage = line.tokens[0].lower().starts_with('s');
        let count = if voltage { 6 } else { 5 };
        let t = self.need(
            line,
            count,
            "Sname p n cp cn model [on|off] or Wname p n Vcontrol model [on|off]",
        )?;
        let control = if voltage {
            crate::SwitchControl::Voltage(node_name(&t[3]), node_name(&t[4]))
        } else {
            crate::SwitchControl::Current(t[3].lower())
        };
        let on = match t.get(count).map(Token::lower).as_deref() {
            Some("on") => true,
            None | Some("off") => false,
            _ => {
                self.err("E-SWITCH-MODEL", "expected on or off", t[count].span);
                return None;
            }
        };
        self.no_extra(t, count + 1);
        Some(ElementKind::Switch {
            p: node_name(&t[1]),
            n: node_name(&t[2]),
            control,
            model: t[count - 1].lower(),
            on,
        })
    }

    fn behavioral(&mut self, line: &Line) -> Option<ElementKind> {
        let t = self.need(line, 6, "Bname p n V={expression} or I={expression}")?;
        let name = t[0].lower();
        let key = t[3].lower();
        let voltage = match (name.as_bytes()[0], key.as_str()) {
            (b'b', "v") | (b'e', "value" | "table") => true,
            (b'b', "i") | (b'g', "value" | "table") => false,
            _ => {
                self.err(
                    "E-BSOURCE-EXPR",
                    "invalid behavioural source specification",
                    line.span(),
                );
                return None;
            }
        };
        if !t[4].is_eq() {
            self.err(
                "E-BSOURCE-EXPR",
                "expected '=' before expression",
                line.span(),
            );
            return None;
        }
        let mut table = vec![];
        let expr = crate::expr::strip_delims(&t[5].text).to_owned();
        if key == "table" {
            let mut prev = f64::NEG_INFINITY;
            if t.len() < 8 || !(t.len() - 6).is_multiple_of(2) {
                self.err(
                    "E-BSOURCE-EXPR",
                    "expected table coordinate pairs",
                    line.span(),
                );
                return None;
            }
            for pair in t[6..].as_chunks::<2>().0 {
                let x = self.number(&pair[0], "table coordinate")?;
                if x <= prev {
                    self.err(
                        "E-BSOURCE-EXPR",
                        "table coordinates must increase",
                        pair[0].span,
                    );
                    return None;
                }
                prev = x;
                table.push((x, self.number(&pair[1], "table value")?));
            }
        } else {
            self.no_extra(t, 6);
        }
        Some(ElementKind::Behavioral {
            p: node_name(&t[1]),
            n: node_name(&t[2]),
            voltage,
            expr,
            params: Vec::new(),
            table,
        })
    }

    /// `Xname n+ n- value [key=value ...]` or `Xname n+ n- key=value ...`
    fn two_terminal(&mut self, line: &Line, letter: &str, key: &str) -> Option<ElementKind> {
        let usage = format!("{letter}name n+ n- value");
        let t = self.need(line, 3, &usage)?;
        let (p, n) = (node_name(&t[1]), node_name(&t[2]));
        let mut expr = None;
        let mut value = None;
        let mut model = None;
        let mut mult = 1.;
        let mut params = vec![];
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
                if k == key && vt.text.starts_with('{') {
                    expr = Some(
                        vt.text
                            .trim_start_matches('{')
                            .trim_end_matches('}')
                            .to_owned(),
                    );
                    i += 3;
                    continue;
                }
                let v = self.number(vt, &k)?;
                if k == "tc" {
                    params.push(("tc1".into(), v));
                    if let Some(second) = t.get(i + 3).and_then(|t| parse_number(&t.text)) {
                        params.push(("tc2".into(), second));
                        i += 1;
                    }
                } else if k == key {
                    value = Some(v);
                } else if matches!(k.as_str(), "m" | "mult") {
                    mult = v;
                } else if (matches!(k.as_str(), "noisy" | "ac") && letter == "R")
                    || (k == "ic" && letter != "R")
                    || matches!(k.as_str(), "tc1" | "tc2" | "temp" | "dtemp")
                {
                    params.push((k, v));
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
            } else if model.is_none()
                && parse_number(&t[i].text).is_none()
                && (value.is_some()
                    || t.get(i + 1)
                        .is_some_and(|t| parse_number(&t.text).is_some()))
            {
                model = Some(t[i].lower());
                i += 1;
            } else if value.is_none() {
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
        if mult <= 0. || !mult.is_finite() {
            self.err(
                "E-PARSE-VALUE",
                "multiplicity must be positive",
                line.span(),
            );
            return None;
        }
        if let Some(expr) = expr {
            if model.is_some() || value.is_some() {
                return self.arity(line, "expected one behavioral value expression");
            }
            if mult != 1. {
                params.push(("m".into(), mult));
            }
            return Some(ElementKind::Behavioral {
                p,
                n,
                voltage: letter == "L",
                expr,
                params,
                table: Vec::new(),
            });
        }
        let Some(value) = value else {
            self.diags.push(
                Diagnostic::error("E-PARSE-ARITY", "missing element value")
                    .with_span(line.span())
                    .with_note(format!("usage: {usage}")),
            );
            return None;
        };
        if let Some((_, ac)) = params.iter_mut().find(|(k, _)| k == "ac") {
            *ac /= mult;
            if *ac == 0. {
                self.err(
                    "E-PARSE-VALUE",
                    "AC resistance must be nonzero",
                    line.span(),
                );
                return None;
            }
        }
        let value = if letter == "C" {
            value * mult
        } else {
            value / mult
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
            "R" => ElementKind::Resistor {
                mult,
                p,
                n,
                value,
                model,
                params,
            },
            "C" => ElementKind::Capacitor {
                p,
                n,
                value,
                model,
                params,
            },
            _ => ElementKind::Inductor {
                p,
                n,
                value,
                model,
                params,
            },
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
        let mut port = None;
        let mut z0 = None;
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
                "portnum" | "z0" => {
                    if !t[0].lower().starts_with('v')
                        || !t.get(i + 1).is_some_and(Token::is_eq)
                        || i + 2 >= t.len()
                    {
                        self.err(
                            "E-PARSE-VALUE",
                            "expected voltage-source portnum=n or z0=r",
                            t[i].span,
                        );
                        return None;
                    }
                    let value = &t[i + 2];
                    if word == "portnum" {
                        let Some(number) = value.positive_integer() else {
                            self.err(
                                "E-PARSE-VALUE",
                                "port number must be a positive integer",
                                value.span,
                            );
                            return None;
                        };
                        port = Some(number);
                    } else {
                        let r = self.number(value, "port reference resistance")?;
                        if !(r.is_finite() && r > 0.) {
                            self.err(
                                "E-PARSE-VALUE",
                                "Z0 must be finite and positive",
                                value.span,
                            );
                            return None;
                        }
                        z0 = Some(r);
                    }
                    i += 3;
                }
                "distof1" | "distof2" => {
                    i += 1;
                    let mut tone = [1., 0.];
                    for item in &mut tone {
                        if let Some(value) = t.get(i).and_then(|x| parse_number(&x.text)) {
                            *item = value;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    spec.disto[usize::from(word == "distof2")] = Some((tone[0], tone[1]));
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
                "pulse" | "sin" | "exp" | "pwl" | "sffm" | "am" | "trnoise" | "trrandom" => {
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
        if z0.is_some() && port.is_none() {
            self.err("E-PARSE-VALUE", "Z0 requires a port number", line.span());
            return None;
        }
        spec.port = port.map(|n| (n, z0.unwrap_or(50.)));
        Some(if t[0].lower().starts_with('v') {
            ElementKind::VoltageSource { p, n, source: spec }
        } else {
            ElementKind::CurrentSource { p, n, source: spec }
        })
    }

    fn tran_fn(&mut self, word: &str, vals: Vec<f64>, span: Span) -> Option<TranFn> {
        let (min, max) = match word {
            "pulse" | "sffm" | "am" => (2, 8),
            "trnoise" => (2, 7),
            "trrandom" => (2, 5),
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
            "trrandom" => {
                if vals.iter().any(|v| !v.is_finite())
                    || !matches!(vals[0], 1. | 2. | 3. | 4.)
                    || vals[1] <= 0.
                    || vals.iter().skip(2).take(2).any(|&v| v < 0.)
                {
                    self.err("E-PARSE-VALUE","TRRANDOM requires type 1 through 4, positive TS and nonnegative delay and scale",span);
                    return None;
                }
                let mut parameters = [0., 0., 0., 1., 0.];
                parameters[..vals.len()].copy_from_slice(&vals);
                TranFn::TrRandom(parameters)
            }
            "trnoise" => {
                if vals.iter().any(|&v| !v.is_finite() || v < 0.)
                    || vals.get(2).is_some_and(|&v| v > 2.)
                    || (vals.get(4).is_some_and(|&v| v != 0.)
                        && (vals.len() != 7 || vals[5] <= 0. || vals[6] <= 0.))
                {
                    self.err(
                        "E-PARSE-VALUE",
                        "invalid TRNOISE amplitude, interval, exponent or RTS dwell time",
                        span,
                    );
                    return None;
                }
                let mut parameters = [0.; 7];
                parameters[..vals.len()].copy_from_slice(&vals);
                TranFn::TrNoise(parameters)
            }
            "pulse" => TranFn::Pulse(vals),
            "sin" => TranFn::Sin(vals),
            "sffm" => TranFn::Sffm(vals),
            "am" => TranFn::Am(vals),
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
        let mut f = self.device_fields(line)?;
        if let [nodes @ .., model, flag] = f.positional.as_slice()
            && flag.text.eq_ignore_ascii_case("thermal")
            && (nodes.len() == 5
                || matches!(nodes.len(), 3 | 4) && self.vdmos_models.contains(&model.lower()))
        {
            f.positional.pop();
            f.params.push(("thermal".into(), 1.));
        }
        if f.positional.len() != 6
            && f.params
                .iter()
                .rfind(|(k, _)| k == "thermal")
                .is_some_and(|(_, v)| *v != 0.)
            && f.positional
                .last()
                .is_some_and(|m| self.vdmos_models.contains(&m.lower()))
        {
            self.err(
                "E-PARSE-ARITY",
                format!(
                    "thermal VDMOS `{}` requires exactly five nodes (drain, gate, source, junction, case)",
                    line.tokens[0].text
                ),
                line.span(),
            );
            return None;
        }
        let pins = match f.positional.as_slice() {
            [_, _, _, tj, tc, _] => Some([node_name(tj), node_name(tc)]),
            _ => None,
        };
        if let [d, g, s, .., model] = f.positional.as_slice()
            && (pins.is_some() || f.positional.len() == 4)
        {
            return Some(ElementKind::Vdmos {
                thermal_pins: pins,
                d: node_name(d),
                g: node_name(g),
                s: node_name(s),
                model: model.lower(),
                params: f.params,
                off: f.off,
            });
        }
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

    fn bin_candidates(&self) -> std::collections::HashMap<usize, Vec<usize>> {
        let families = self.bin_families();
        self.elements
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let model = e.kind.model()?;
                if self.model_names.contains_key(model) {
                    return None;
                }
                Some((i, families.get(model)?.clone()))
            })
            .collect()
    }
    fn bin_families(&self) -> std::collections::HashMap<String, Vec<usize>> {
        let mut families = std::collections::HashMap::<_, Vec<_>>::new();
        for (index, card) in self.models.iter().enumerate() {
            if let Some((base, bin)) = card.name.rsplit_once('.')
                && !bin.is_empty()
                && bin.bytes().all(|c| c.is_ascii_digit())
            {
                families.entry(base.to_owned()).or_default().push(index);
            }
        }
        families
    }
    /// Report instances that reference undefined models or models of the wrong type.
    /// Resolve references to binned models (`nch` → `nch.1`, `nch.2`, ...) by the instance
    /// geometry: the bin with `lmin <= l < lmax` and `wmin <= w < wmax` is chosen.
    fn resolve_bins(&mut self, dynamic: &DynamicCards<'_>) {
        let scale = self
            .options
            .iter()
            .rev()
            .find(|o| o.name == "scale")
            .and_then(|o| o.value)
            .unwrap_or(1.);
        let mut errs = Vec::new();
        let families = self.bin_families();
        for e in &mut self.elements {
            let ElementKind::Mosfet { model, params, .. } = &mut e.kind else {
                continue;
            };
            if self.model_names.contains_key(model) {
                continue;
            }
            let Some(bins) = families.get(model.as_str()) else {
                continue;
            };
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
            let mut selected = None;
            for &idx in bins {
                let card = &self.models[idx];
                let mut bounds = [1., 0., f64::INFINITY, 0., f64::INFINITY];
                let keys = ["level", "lmin", "lmax", "wmin", "wmax"];
                for (name, value) in &card.params {
                    if let Some(k) = keys.iter().position(|&n| n == name) {
                        bounds[k] = *value;
                    }
                }
                if let Some((_, scope, line)) = dynamic.get(&idx) {
                    let eval = scope.evaluator(params);
                    for kv in line.tokens[3..].as_chunks::<3>().0 {
                        if let Some(k) =
                            keys.iter().position(|n| kv[0].text.eq_ignore_ascii_case(n))
                            && !crate::expr::is_number_literal(&kv[2].text)
                        {
                            match eval(&kv[2].text) {
                                Ok(v) if v.is_finite() => bounds[k] = v,
                                _ => errs.push(
                                    Diagnostic::error(
                                        "E-PARSE-EXPR",
                                        format!(
                                            "{}: invalid {} in bin {}",
                                            e.name, keys[k], card.name
                                        ),
                                    )
                                    .with_span(kv[2].span),
                                ),
                            }
                        }
                    }
                }
                if crate::parameters::in_bin(bounds, l * scale, w * scale, get("nf").unwrap_or(1.))
                {
                    selected = Some(card);
                    break;
                }
            }
            match selected {
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

    /// Bind only the selected model in the raw (pre-scale) instance geometry.
    fn bind_model_expressions(&mut self, dynamic: &DynamicCards<'_>) {
        use std::collections::HashMap;
        if dynamic.is_empty() {
            return;
        }
        let index: HashMap<_, _> = self
            .models
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name.clone(), i))
            .collect();
        let mut arguments = HashMap::new();
        let mut first = std::collections::HashSet::new();
        let mut shared: HashMap<_, Vec<(String, f64)>> = HashMap::new();
        let mut canonical = HashMap::new();
        'element: for e in &mut self.elements {
            let mut empty = Vec::new();
            let (model, params) = match &mut e.kind {
                ElementKind::Resistor {
                    model: Some(model),
                    params,
                    ..
                }
                | ElementKind::Inductor {
                    model: Some(model),
                    params,
                    ..
                }
                | ElementKind::Capacitor {
                    model: Some(model),
                    params,
                    ..
                } => (model, params),
                ElementKind::Ltra { model, .. } | ElementKind::Switch { model, .. } => {
                    (model, &mut empty)
                }
                ElementKind::Mosfet { model, params, .. }
                | ElementKind::Jfet { model, params, .. }
                | ElementKind::Mesfet { model, params, .. }
                | ElementKind::Vdmos { model, params, .. }
                | ElementKind::Urc { model, params, .. }
                | ElementKind::Diode { model, params, .. }
                | ElementKind::Bjt { model, params, .. }
                | ElementKind::Va { model, params, .. } => (model, params),
                _ => continue,
            };
            let Some((&idx, (original, scope, line))) = index
                .get(model)
                .and_then(|i| dynamic.get(i).map(|v| (i, v)))
            else {
                continue;
            };
            let key = (
                idx,
                params
                    .iter()
                    .map(|(n, v)| (n.clone(), v.to_bits()))
                    .collect::<Vec<_>>(),
            );
            if let Some(name) = arguments.get(&key) {
                model.clone_from(name);
                continue;
            }
            let shared_key = (*original, key.1.clone());
            let values = if let Some(cached) = shared.get(&shared_key) {
                cached.clone()
            } else {
                let eval = scope.evaluator(params);
                let mut values = vec![];
                for kv in line.tokens[3..].as_chunks::<3>().0 {
                    let token = &kv[2];
                    if kv[0].text.eq_ignore_ascii_case("version") && dotted_version(&token.text) {
                        continue;
                    }
                    let value = if crate::expr::is_number_literal(&token.text) {
                        parse_number(&token.text)
                            .ok_or_else(|| format!("invalid number {}", token.text))
                    } else {
                        eval(&token.text)
                    };
                    match value {
                        Ok(v) if v.is_finite() => values.push((kv[0].lower(), v)),
                        err => {
                            let msg = err.err().unwrap_or_else(|| "non-finite value".into());
                            self.diags.push(
                                Diagnostic::error(
                                    "E-PARSE-EXPR",
                                    format!("{}: {} in model {}: {msg}", e.name, kv[0].text, model),
                                )
                                .with_span(token.span),
                            );
                            continue 'element;
                        }
                    }
                }
                shared.insert(shared_key, values.clone());
                values
            };
            let signature = (
                idx,
                values
                    .iter()
                    .map(|(n, v)| (n.clone(), v.to_bits()))
                    .collect::<Vec<_>>(),
            );
            let name = if let Some(name) = canonical.get(&signature) {
                String::clone(name)
            } else {
                let name = if first.insert(idx) {
                    self.models[idx].params = values;
                    model.clone()
                } else {
                    let mut card = self.models[idx].clone();
                    let mut suffix = self.models.len();
                    loop {
                        card.name = format!("{model}__sansho_{suffix}");
                        if !index.contains_key(&card.name)
                            && !self.model_names.contains_key(&card.name)
                        {
                            break;
                        }
                        suffix += 1;
                    }
                    card.params = values;
                    let name = card.name.clone();
                    self.model_names.insert(name.clone(), self.models.len());
                    self.models.push(card);
                    name
                };
                canonical.insert(signature, name.clone());
                name
            };
            arguments.insert(key, name.clone());
            *model = name;
        }
    }

    fn check_models(&mut self) {
        let mut errs = Vec::new();
        for e in &self.elements {
            let Some(name) = e.kind.model() else { continue };
            let expected: &[&str] = match e.kind {
                ElementKind::Switch {
                    control: crate::SwitchControl::Voltage(..),
                    ..
                } => &["sw"],
                ElementKind::Switch { .. } => &["csw"],
                ElementKind::Resistor { .. } => &["r"],
                ElementKind::Capacitor { .. } => &["c"],
                ElementKind::Inductor { .. } => &["l"],
                ElementKind::Jfet { .. } => &["njf", "pjf"],
                ElementKind::Urc { .. } => &["urc"],
                ElementKind::Ltra { .. } => &["ltra"],
                ElementKind::Mesfet { .. } => &["nmf", "pmf"],
                ElementKind::Vdmos { .. } => &["vdmos"],
                ElementKind::Diode { .. } => &["d"],
                ElementKind::Bjt { .. } => &["npn", "pnp"],
                ElementKind::Va { .. } => &[],
                _ => &["nmos", "pmos"],
            };
            let code = if matches!(e.kind, ElementKind::Switch { .. }) {
                "E-SWITCH-MODEL"
            } else {
                "E-PARSE-MODEL"
            };
            match self.model_names.get(name).map(|&i| &self.models[i]) {
                None => errs.push(
                    Diagnostic::error(
                        code,
                        format!("`{}` references undefined model `{name}`", e.name),
                    )
                    .with_span(e.span),
                ),
                Some(m) if !expected.is_empty() && !expected.contains(&m.kind.as_str()) => errs
                    .push(
                        Diagnostic::error(
                            code,
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

    fn model_card(
        &mut self,
        line: &Line,
        parameters: &Line,
        cached: Option<usize>,
        scope: Option<&crate::preproc::ModelScope>,
    ) {
        let Some(t) = self.need(line, 3, ".model name type [(param=value ...)]") else {
            return;
        };
        let name = t[1].lower();
        let kind = t[2].lower();
        if self.model_names.contains_key(&name) {
            self.err(
                "E-PARSE-DUPLICATE",
                format!("duplicate model name `{name}`"),
                t[1].span,
            );
            return;
        }
        if let Some(index) = cached {
            let params = self.models[index].params.clone();
            let version = self.models[index].version.clone();
            self.models.push(ModelCard {
                name,
                kind,
                version,
                params,
                span: line.span(),
            });
            return;
        }
        let t = &parameters.tokens;
        let mut params = Vec::new();
        let mut version = None;
        let mut i = 3;
        while i < t.len() {
            let k = t[i].lower();
            if !t.get(i + 1).is_some_and(Token::is_eq) {
                if kind == "vdmos" && matches!(k.as_str(), "pchan" | "nchan") {
                    params.push((k, 1.));
                    i += 1;
                    continue;
                }
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
            if k == "version"
                && matches!(kind.as_str(), "nmos" | "pmos")
                && dotted_version(&vt.text)
            {
                version = Some(vt.text.clone());
                i += 3;
                continue;
            }
            if scope.is_some() && !crate::expr::is_number_literal(&vt.text) {
                i += 3;
                continue;
            }
            let Some(v) = self.number(vt, &k) else {
                return;
            };
            params.push((k, v));
            i += 3;
        }
        self.models.push(ModelCard {
            name,
            kind,
            version,
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

    fn frequency_sweep(&mut self, t: &[Token], span: Span) -> Option<crate::FrequencySweep> {
        if t.len() != 4 {
            self.err("E-PARSE-ARITY", "expected dec|oct|lin n fstart fstop", span);
            return None;
        }
        let sweep = match t[0].lower().as_str() {
            "dec" => AcSweep::Dec,
            "oct" => AcSweep::Oct,
            "lin" => AcSweep::Lin,
            _ => {
                self.err(
                    "E-PARSE-VALUE",
                    format!(
                        "unknown AC sweep type `{}` (use dec, oct or lin)",
                        t[0].text
                    ),
                    t[0].span,
                );
                return None;
            }
        };
        let (Some(np), Some(fstart), Some(fstop)) = (
            self.number(&t[1], "number of points"),
            self.number(&t[2], "fstart"),
            self.number(&t[3], "fstop"),
        ) else {
            return None;
        };
        if !np.is_finite()
            || np.fract() != 0.
            || np < 1.0
            || np > u32::MAX as f64
            || !fstart.is_finite()
            || !fstop.is_finite()
            || fstart <= 0.0
            || fstop < fstart
        {
            self.err(
                "E-PARSE-VALUE",
                "AC sweep needs n >= 1 and 0 < fstart <= fstop",
                span,
            );
            return None;
        }
        Some(crate::FrequencySweep {
            sweep,
            points: np as u32,
            fstart,
            fstop,
        })
    }

    fn analysis_probe(&mut self, t: &[Token], span: Span) -> Option<crate::Probe> {
        use crate::Probe;
        if let Some(k) = t.iter().position(|t| t.ends_group)
            && k + 1 < t.len()
        {
            self.err(
                "E-PARSE-ARITY",
                "unexpected argument after output probe",
                t[k + 1].span,
            );
            return None;
        }
        match t {
            [k, p] if k.lower() == "v" => Some(Probe::Voltage(node_name(p), "0".into())),
            [k, p, n] if k.lower() == "v" => Some(Probe::Voltage(node_name(p), node_name(n))),
            [k, b] if k.lower() == "i" => Some(Probe::Current(b.lower())),
            _ => {
                self.err("E-PARSE-VALUE", "expected v(node[,ref]) or i(source)", span);
                None
            }
        }
    }

    fn dot_card(&mut self, head: &str, line: &Line) {
        let span = line.span();
        let t = &line.tokens;
        match head {
            ".op" if t.len() == 1 => self.analyses.push(AnalysisCard::Op { span }),
            ".op" => self.err("E-PARSE-ARITY", ".op takes no arguments", t[1].span),
            ".dc" => {
                if !matches!(t.len(), 5 | 9) {
                    self.err(
                        "E-PARSE-ARITY",
                        ".dc requires one or two source/start/stop/step groups",
                        span,
                    );
                    return;
                }
                let mut axis = |t: &[Token]| {
                    let sweep = crate::DcSweep {
                        source: t[0].lower(),
                        start: self.number(&t[1], "start")?,
                        stop: self.number(&t[2], "stop")?,
                        step: self.number(&t[3], "increment")?,
                    };
                    if sweep.count().is_none() {
                        self.err(
                            "E-PARSE-VALUE",
                            "invalid DC sweep range or increment",
                            t[3].span,
                        );
                        return None;
                    }
                    Some(sweep)
                };
                let Some(inner) = axis(&t[1..5]) else { return };
                let outer = if t.len() == 9 {
                    let Some(outer) = axis(&t[5..]) else { return };
                    if outer.source == inner.source {
                        self.err("E-PARSE-VALUE", "DC sweep axes must be distinct", t[5].span);
                        return;
                    }
                    Some(outer)
                } else {
                    None
                };
                self.analyses.push(AnalysisCard::Dc { span, inner, outer });
            }
            ".tf" | ".sens" => {
                let mut end = t.len() - usize::from(head == ".tf");
                let mut ac = None;
                if head == ".sens" && t.len() >= 8 && t[t.len() - 5].lower() == "ac" {
                    end = t.len() - 5;
                    let Some(grid) = self.frequency_sweep(&t[end + 1..], span) else {
                        return;
                    };
                    ac = Some(grid);
                }
                let Some(output) = self.analysis_probe(t.get(1..end).unwrap_or_default(), span)
                else {
                    return;
                };
                self.analyses.push(if head == ".tf" {
                    AnalysisCard::Tf {
                        span,
                        output,
                        source: t[end].lower(),
                    }
                } else {
                    AnalysisCard::Sens { span, output, ac }
                });
            }
            ".four" | ".fourier" => {
                let Some(t) = self.need(line, 4, ".four frequency outvar ...") else {
                    return;
                };
                let Some(frequency) = self.number(&t[1], "fundamental frequency") else {
                    return;
                };
                if frequency <= 0. || !frequency.is_finite() {
                    self.err("E-PARSE-VALUE", "Fourier frequency must be positive", span);
                    return;
                }
                let mut outputs = Vec::new();
                // Work back from the last probe so a parameter expression for
                // the frequency can contain parenthesized function calls.
                let mut end = t.len();
                for argument in line
                    .raw
                    .rsplit(')')
                    .filter_map(|s| s.rsplit_once('(').map(|(_, a)| a))
                {
                    if end == 2 {
                        break;
                    }
                    let nodes = argument
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|s| !s.is_empty())
                        .count();
                    let Some(start) = end.checked_sub(1 + nodes).filter(|&k| k >= 2) else {
                        self.err("E-PARSE-VALUE", "malformed Fourier output probe", span);
                        return;
                    };
                    let Some(probe) = self.analysis_probe(&t[start..end], span) else {
                        return;
                    };
                    outputs.push(probe);
                    end = start;
                }
                if end != 2 || outputs.is_empty() {
                    self.err(
                        "E-PARSE-VALUE",
                        "expected Fourier output probes with parentheses",
                        span,
                    );
                    return;
                }
                outputs.reverse();
                self.analyses.push(AnalysisCard::Four {
                    span,
                    frequency,
                    outputs,
                });
            }
            ".pz" => {
                if t.len() != 7 {
                    self.err(
                        "E-PARSE-ARITY",
                        ".pz in+ in- out+ out- vol|cur pol|zer|pz",
                        span,
                    );
                    return;
                }
                let mode = t[6].lower();
                let input = t[5].lower();
                if !matches!(mode.as_str(), "pol" | "zer" | "pz")
                    || !matches!(input.as_str(), "vol" | "cur")
                {
                    self.err(
                        "E-PARSE-VALUE",
                        "expected vol or cur followed by pol, zer or pz",
                        span,
                    );
                    return;
                }
                self.analyses.push(AnalysisCard::Pz {
                    span,
                    nodes: std::array::from_fn(|i| node_name(&t[i + 1])),
                    voltage: input == "vol",
                    poles: mode != "zer",
                    zeros: mode != "pol",
                });
            }
            ".sp" | ".disto" => {
                if !(5..=6).contains(&t.len()) {
                    self.err(
                        "E-PARSE-ARITY",
                        format!("{head} requires a frequency grid and at most one option"),
                        span,
                    );
                    return;
                }
                let Some(grid) = self.frequency_sweep(&t[1..5], span) else {
                    return;
                };
                let value = if let Some(token) = t.get(5) {
                    let Some(n) = self.number(token, "analysis option") else {
                        return;
                    };
                    Some(n)
                } else {
                    None
                };
                let card = if head == ".sp" {
                    if value.is_some_and(|n| n != 0. && n != 1.) {
                        self.err("E-PARSE-VALUE", "SP donoise must be 0 or 1", span);
                        return;
                    }
                    AnalysisCard::Sp {
                        span,
                        grid,
                        noise: value == Some(1.),
                    }
                } else {
                    if value.is_some_and(|n| !(n > 0. && n < 1.)) {
                        self.err("E-PARSE-VALUE", "DISTO requires 0 < f2overf1 < 1", span);
                        return;
                    }
                    AnalysisCard::Disto {
                        span,
                        grid,
                        ratio: value,
                    }
                };
                self.analyses.push(card);
            }
            ".ac" | ".noise" => {
                let noise = if head == ".noise" {
                    let Some(k) = (4..=5).find(|&i| {
                        t.get(i)
                            .is_some_and(|v| matches!(v.lower().as_str(), "lin" | "dec" | "oct"))
                    }) else {
                        self.err(
                            "E-PARSE-ARITY",
                            ".noise v(out[,ref]) source dec|oct|lin n start stop [summary]",
                            span,
                        );
                        return;
                    };
                    let Some(output) = self.analysis_probe(&t[1..k - 1], span) else {
                        return;
                    };
                    if !matches!(output, crate::Probe::Voltage(..))
                        || !(k + 4..=k + 5).contains(&t.len())
                    {
                        self.err(
                            "E-PARSE-VALUE",
                            "expected a voltage output and a frequency sweep",
                            span,
                        );
                        return;
                    }
                    let summary = match t.get(k + 4) {
                        Some(v) => match v.text.parse::<u32>() {
                            Ok(n) => n,
                            Err(_) => {
                                self.err(
                                    "E-PARSE-VALUE",
                                    "summary interval must be a nonnegative integer",
                                    v.span,
                                );
                                return;
                            }
                        },
                        None => 0,
                    };
                    Some((output, t[k - 1].lower(), summary, k - 1))
                } else {
                    None
                };
                let Some(t) = self.need(line, 5, ".ac dec|oct|lin n fstart fstop") else {
                    return;
                };
                let t = &t[noise.as_ref().map_or(0, |n| n.3)..];
                let Some(crate::FrequencySweep {
                    sweep,
                    points,
                    fstart,
                    fstop,
                }) = self.frequency_sweep(if noise.is_some() { &t[1..5] } else { &t[1..] }, span)
                else {
                    return;
                };
                if let Some((output, source, summary, _)) = noise {
                    self.analyses.push(AnalysisCard::Noise {
                        span,
                        output,
                        source,
                        summary,
                        sweep,
                        points,
                        fstart,
                        fstop,
                    });
                    return;
                }
                self.analyses.push(AnalysisCard::Ac {
                    span,
                    sweep,
                    points,
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
                    if tok.lower() == "uic" && tok.span == t[t.len() - 1].span {
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
                            if !matches!(vt.lower().as_str(), "gear" | "trap" | "trapezoidal") {
                                self.diags.push(
                                    Diagnostic::warning(
                                        "W-OPT-IGNORED",
                                        format!("unsupported integration method {}", vt.text),
                                    )
                                    .with_span(vt.span),
                                );
                                i += 3;
                                continue;
                            }
                            // Store the chosen integration method as a flag.
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
                        _ if name.starts_with('@') => name,
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
                self.no_extra(t, 2);
                if let Some(v) = self.number(&t[1], "the temperature") {
                    self.options.push(OptionCard {
                        name: "temp".into(),
                        value: Some(v),
                        span,
                    });
                }
            }
            ".title" => {
                self.title = Some(
                    line.raw
                        .trim_start()
                        .get(6..)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                )
            }
            ".width" => {}
            ".plot" => self.diags.push(
                Diagnostic::warning("W-PLOT-IGNORED", "ASCII plotting is unavailable")
                    .with_span(span),
            ),
            ".print" => match crate::meas::parse_print(line) {
                Ok(card) => self.prints.push(card),
                Err(d) => self.diags.push(d),
            },
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
    fn passive_initial_values_and_geometry_scale_survive_export() {
        let src = "initial conditions\nC1 a b 20p ic=-2.5\nL1 b 0 4u ic=7m\n.options scale=0.5u\n";
        let nl = parse(src).unwrap();
        let copy = parse(&crate::write(&nl)).unwrap();
        assert_eq!(
            nl.elements.iter().map(|e| &e.kind).collect::<Vec<_>>(),
            copy.elements.iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
        assert_eq!(copy.options[0].value, Some(0.5e-6));
    }

    #[test]
    fn invalid_coupling_and_switch_models_have_card_spans() {
        for (line, code) in [
            ("Kbad Lp Ls -1.01", "E-COUPLING-K"),
            ("Sbad p 0 s 0 wrong\n.model wrong csw", "E-SWITCH-MODEL"),
        ] {
            let ds = parse(&format!("bad card\n{line}\n")).unwrap_err();
            assert!(ds.iter().any(|d| d.code == code && d.span.is_some()));
        }
    }

    #[test]
    fn parameterized_bins_bind_only_live_expressions() {
        let nl=parse("device bins\n.option scale=1u\n.param bad={undefined}\n.model n.1 nmos level=54 version=4.8.2 wmax={w/2*1u} vth0={bad}\n.model n.2 nmos level=54 wmin={w/2*1u} vth0={l+nf}\n.model idle nmos level=54 k1={missing}\nM1 d g 0 0 n l=0.2 w=4 nf=2\nM2 d g 0 0 n l=0.3 w=4 nf=2\nM3 d g 0 0 n l=0.2 w=4 nf=2\n.end").unwrap();
        let cards: Vec<_> = nl
            .elements
            .iter()
            .map(|e| {
                nl.models
                    .iter()
                    .find(|m| Some(m.name.as_str()) == e.kind.model())
                    .unwrap()
            })
            .collect();
        let vth = |i: usize| cards[i].params.iter().find(|(k, _)| k == "vth0").unwrap().1;
        assert_eq!([vth(0), vth(1), vth(2)], [2.2, 2.3, 2.2]);
        assert_eq!(cards[0].name, cards[2].name);
        assert_ne!(cards[0].name, cards[1].name);
        assert_eq!(nl.models[0].version.as_deref(), Some("4.8.2"));
    }

    #[test]
    fn statistical_parameters_and_parallel_subcircuits() {
        let src = "nominal\n.param mc_mm_switch=0 mc_pr_switch=0\n.subckt cell a\nR1 a 0 {agauss(80,2,3)+gauss(10,1,3)+unif(5,1)+aunif(5,1)}\n.ends\n.subckt nest a\nX1 a cell mult=2\n.ends\nX2 d nest m=5\n.end";
        let nl = parse(src).unwrap();
        assert!(matches!(nl.elements[0].kind,ElementKind::Resistor {value,..} if value==10.));
        assert!(parse(&src.replace("mc_mm_switch=0", "mc_mm_switch=1")).is_ok());
    }

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
            errors.iter().filter(|d| d.code == "E-PARSE-EXPR").count(),
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
    fn thermal_vdmos_requires_both_thermal_terminals() {
        for flag in ["THERMAL", "thermal=1"] {
            for nodes in ["d g s", "d g s tj"] {
                let deck = format!("pin count\nM1 {nodes} power {flag} m=2\n.model power vdmos\n");
                let errors = parse(&deck).unwrap_err();
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].code, "E-PARSE-ARITY");
                assert_eq!(errors[0].span, Some(Span::new(2, 1)));
                assert!(
                    errors[0]
                        .message
                        .contains("drain, gate, source, junction, case")
                );
            }
        }
        let valid = "pins\nM1 d g s tj 0 power thermal\nM2 d g s power\nM3 d g s b thermal\n.model power vdmos\n.model thermal nmos\n";
        let nl = parse(valid).unwrap();
        assert!(
            matches!(&nl.elements[0].kind, ElementKind::Vdmos { thermal_pins: Some(pins), .. } if pins == &["tj", "0"])
        );
        assert!(matches!(
            nl.elements[1].kind,
            ElementKind::Vdmos {
                thermal_pins: None,
                ..
            }
        ));
        assert!(matches!(nl.elements[2].kind, ElementKind::Mosfet { .. }));
    }

    #[test]
    fn reports_all_errors_with_spans() {
        let src = "t\nR1 a b\nY1 c b e mod\nC1 a 0 xyz\n";
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

#[cfg(test)]
mod analysis_arity_tests {
    #[test]
    fn trailing_analysis_arguments_are_diagnosed() {
        for card in [
            ".op junk",
            ".dc v1 0 1 1 temp 0 10",
            ".dc v1 0 1 1 temp 0 10 10 junk",
            ".ac dec 1 1 10 extra",
            ".tran 1n 1u uic uic",
            ".tran 1n 1u 0 1n 1n",
            ".noise v(a) v1 lin 1 1 10 2 junk",
            ".sens v(a) ac lin 1 1 10 extra",
            ".tf v(a) v1 junk",
            ".sens v(a) junk",
            ".pz a 0 b 0 vol pol junk",
            ".four 100 v(a) junk",
        ] {
            assert!(
                crate::parse(&format!("arity\nV1 a 0\nR1 a 0 1k\n{card}\n")).is_err(),
                "{card}"
            );
        }
    }
}
