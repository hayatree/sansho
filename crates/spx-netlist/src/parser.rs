//! Parser from logical lines to a typed [`Netlist`].
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975) and
//! general SPICE3 conventions. See `docs/PROVENANCE.md`.

use spx_diag::{Diagnostic, Span};

use crate::lexer::{Line, Token, lex};
use crate::number::parse_number;
use crate::{AcSweep, AnalysisCard, Element, ElementKind, Netlist, OptionCard, SourceSpec, TranFn};

/// Parse netlist text.
///
/// On success the returned netlist carries any warnings. On failure, all diagnostics (errors and
/// warnings) found in the whole input are returned.
pub fn parse(text: &str) -> Result<Netlist, Vec<Diagnostic>> {
    let lexed = lex(text);
    let mut p = Parser::default();
    let mut in_control = false;
    for line in &lexed.lines {
        let head = line.tokens[0].lower();
        if in_control {
            if head == ".endc" {
                in_control = false;
            }
            continue;
        }
        if head == ".end" {
            break;
        }
        if head == ".control" {
            in_control = true;
            p.diags.push(
                Diagnostic::warning(
                    "W-PARSE-CONTROL",
                    ".control blocks are not supported and were skipped",
                )
                .with_span(line.span()),
            );
            continue;
        }
        if head.starts_with('.') {
            p.dot_card(&head, line);
        } else {
            p.element(&head, line);
        }
    }
    if p.diags.iter().any(Diagnostic::is_error) {
        return Err(p.diags);
    }
    Ok(Netlist {
        title: lexed.title,
        elements: p.elements,
        analyses: p.analyses,
        options: p.options,
        warnings: p.diags,
    })
}

#[derive(Default)]
struct Parser {
    elements: Vec<Element>,
    analyses: Vec<AnalysisCard>,
    options: Vec<OptionCard>,
    diags: Vec<Diagnostic>,
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
                value = Some(self.number(&t[i], "the element value")?);
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
                if uic {
                    self.diags.push(
                        Diagnostic::warning(
                            "W-PARSE-IGNORED",
                            "UIC is not supported yet; the initial operating point is computed",
                        )
                        .with_span(span),
                    );
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
                        let Some(v) = self.number(vt, &name) else {
                            return;
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
            ".title" => {}
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
        let src = "t\nR1 a b\nQ1 c b e mod\nC1 a 0 xyz\n";
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
