//! Directive expansion and byte-position source mapping.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
use crate::VaLoader;
use spx_diag::{Diagnostic, Span};
use std::collections::BTreeMap;
#[derive(Debug, Clone)]
pub struct Preprocessed {
    pub text: String,
    pub positions: Vec<Span>,
    pub sources: Vec<String>,
}
#[derive(Clone)]
struct Macro {
    args: Option<Vec<String>>,
    body: String,
}
struct Processor<'a> {
    loader: &'a dyn VaLoader,
    macros: BTreeMap<String, Macro>,
    out: Preprocessed,
    errors: Vec<Diagnostic>,
    comment: bool,
    expansion_positions: Vec<Span>,
    expansions: usize,
}
pub fn preprocess(text: &str, loader: &dyn VaLoader) -> (Preprocessed, Vec<Diagnostic>) {
    let mut p = Processor {
        loader,
        macros: BTreeMap::new(),
        out: Preprocessed {
            text: String::new(),
            positions: Vec::new(),
            sources: vec!["<input>".into()],
        },
        errors: Vec::new(),
        comment: false,
        expansion_positions: Vec::new(),
        expansions: 0,
    };
    p.file(text, 0, 0);
    (p.out, p.errors)
}
fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}
impl Processor<'_> {
    fn error(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.errors
            .push(Diagnostic::error(code, msg).with_span(span));
    }
    fn emit(&mut self, text: &str, span: Span) {
        self.out.text.push_str(text);
        self.out
            .positions
            .extend(std::iter::repeat_n(span, text.len()));
    }
    fn file(&mut self, text: &str, file: u32, depth: usize) {
        if depth >= 32 {
            self.error(
                "E-VA-INCLUDE",
                "include depth exceeds 32",
                Span::new(1, 1).in_file(file),
            );
            return;
        }
        let mut conditions: Vec<(bool, bool, bool)> = Vec::new();
        let mut lines = text.lines().enumerate();
        while let Some((line, first)) = lines.next() {
            let span = Span::new(line as u32 + 1, 1).in_file(file);
            let mut s = first.to_string();
            let mut positions: Vec<_> = (0..s.len())
                .map(|col| Span::new(line as u32 + 1, col as u32 + 1).in_file(file))
                .collect();
            while s.ends_with('\\') {
                s.pop();
                positions.pop();
                if let Some((next_line, next)) = lines.next() {
                    positions.extend(
                        (0..next.len()).map(|col| {
                            Span::new(next_line as u32 + 1, col as u32 + 1).in_file(file)
                        }),
                    );
                    s.push_str(next);
                } else {
                    break;
                }
            }
            let trimmed = s.trim_start();
            let active = conditions.last().is_none_or(|c| c.0);
            let directive = trimmed.strip_prefix('`').map(|r| {
                let end = r.find(|c: char| !ident(c)).unwrap_or(r.len());
                (&r[..end], r[end..].trim_start())
            });
            if !self.comment
                && let Some((name, rest)) = directive
            {
                match name {
                    "ifdef" | "ifndef" => {
                        let yes = self
                            .macros
                            .contains_key(rest.split_whitespace().next().unwrap_or(""))
                            == (name == "ifdef");
                        conditions.push((active && yes, yes, false));
                    }
                    "elsif" => {
                        if let Some(c) = conditions.pop() {
                            let parent = conditions.last().is_none_or(|v| v.0);
                            if c.2 {
                                self.error("E-VA-PREPROC", "elsif after else", span);
                            }
                            let yes = self
                                .macros
                                .contains_key(rest.split_whitespace().next().unwrap_or(""));
                            conditions.push((parent && !c.1 && yes, c.1 || yes, c.2));
                        } else {
                            self.error("E-VA-PREPROC", "unmatched elsif", span);
                        }
                    }
                    "else" => {
                        if let Some(c) = conditions.pop() {
                            let parent = conditions.last().is_none_or(|v| v.0);
                            if c.2 {
                                self.error("E-VA-PREPROC", "duplicate else", span);
                            }
                            conditions.push((parent && !c.1, true, true));
                        } else {
                            self.error("E-VA-PREPROC", "unmatched else", span);
                        }
                    }
                    "endif" => {
                        if conditions.pop().is_none() {
                            self.error("E-VA-PREPROC", "unmatched endif", span);
                        }
                    }
                    "define" if active => {
                        let end = rest.find(|c: char| !ident(c)).unwrap_or(rest.len());
                        let key = &rest[..end];
                        let tail = &rest[end..];
                        if key.is_empty() {
                            self.error("E-VA-PREPROC", "missing macro name", span);
                        } else if let Some(tail) = tail.strip_prefix('(') {
                            if let Some(end) = tail.find(')') {
                                let args = tail[..end]
                                    .split(',')
                                    .filter(|a| !a.trim().is_empty())
                                    .map(|a| a.trim().to_string())
                                    .collect();
                                self.macros.insert(
                                    key.into(),
                                    Macro {
                                        args: Some(args),
                                        body: tail[end + 1..].trim_start().into(),
                                    },
                                );
                            } else {
                                self.error("E-VA-PREPROC", "unterminated macro arguments", span);
                            }
                        } else {
                            self.macros.insert(
                                key.into(),
                                Macro {
                                    args: None,
                                    body: tail.trim_start().into(),
                                },
                            );
                        }
                    }
                    "undef" if active => {
                        self.macros
                            .remove(rest.split_whitespace().next().unwrap_or(""));
                    }
                    "include" if active => {
                        let expanded = self.expand(rest, span, 0);
                        if let Some(path) = expanded
                            .trim()
                            .strip_prefix('"')
                            .and_then(|s| s.split_once('"').map(|(path, _)| path))
                        {
                            let loaded = builtin(path)
                                .map(|s| (path.to_string(), s))
                                .map(Ok)
                                .unwrap_or_else(|| {
                                    self.loader.load(
                                        path,
                                        self.out.sources.get(file as usize).map(String::as_str),
                                    )
                                });
                            match loaded {
                                Ok((name, body)) => {
                                    let index = self.out.sources.len() as u32;
                                    self.out.sources.push(name);
                                    self.file(&body, index, depth + 1);
                                }
                                Err(e) => self.error("E-VA-INCLUDE", e, span),
                            }
                        } else {
                            self.error("E-VA-INCLUDE", "expected quoted include path", span);
                        }
                    }
                    "resetall" | "timescale" | "default_nettype" => {}
                    "define" | "undef" | "include" => {}
                    _ => {
                        if active {
                            self.expand_emit(&s, span, &positions);
                        }
                    }
                }
            } else if active {
                self.expand_emit(&s, span, &positions);
            }
            self.emit("\n", span);
        }
        if !conditions.is_empty() {
            self.error(
                "E-VA-PREPROC",
                "unterminated conditional",
                Span::new(text.lines().count() as u32, 1).in_file(file),
            );
        }
    }
    fn expand_emit(&mut self, s: &str, span: Span, positions: &[Span]) {
        let expanded = self.expand(s, span, 0);
        self.out.text.push_str(&expanded);
        self.out
            .positions
            .extend(self.expansion_positions.iter().map(|p| {
                positions
                    .get(p.col.saturating_sub(span.col) as usize)
                    .copied()
                    .unwrap_or(span)
            }));
    }
    fn expand(&mut self, s: &str, span: Span, depth: usize) -> String {
        if depth >= 64 {
            self.error("E-VA-PREPROC", "macro recursion exceeds 64", span);
            return String::new();
        }
        let mut out = String::new();
        let mut i = 0;
        let mut quoted = false;
        let mut mapping = Vec::new();
        let mut previous = (0, 0);
        while i < s.len() {
            map_segment(s, &out, previous, (i, out.len()), span, &mut mapping);
            previous = (i, out.len());
            let tail = &s[i..];
            if self.comment {
                if tail.starts_with("*/") {
                    self.comment = false;
                    out.push_str("*/");
                    i += 2;
                } else if let Some(c) = tail.chars().next() {
                    out.push(c);
                    i += c.len_utf8();
                }
                continue;
            }
            if !quoted && tail.starts_with("//") {
                out.push_str(tail);
                i = s.len();
                break;
            }
            if !quoted && tail.starts_with("/*") {
                self.comment = true;
                out.push_str("/*");
                i += 2;
                continue;
            }
            if tail.starts_with('"') {
                quoted = !quoted;
            }
            if quoted && tail.starts_with('\\') && tail.len() > 1 {
                let end = tail[1..].chars().next().map_or(1, |c| 1 + c.len_utf8());
                out.push_str(&tail[..end]);
                i += end;
                continue;
            }
            if !quoted && tail.starts_with('`') {
                let end = tail[1..]
                    .find(|c: char| !ident(c))
                    .map_or(tail.len(), |n| n + 1);
                let name = &tail[1..end];
                i += end;
                if let Some(m) = self.macros.get(name).cloned() {
                    self.expansions += 1;
                    if self.expansions >= 100_000 {
                        if self.expansions == 100_000 {
                            self.error("E-VA-PREPROC", "macro expansion budget exceeded", span);
                        }
                        continue;
                    }
                    let mut body = m.body;
                    if let Some(params) = m.args {
                        while s[i..].starts_with(char::is_whitespace) {
                            i += s[i..].chars().next().map_or(0, char::len_utf8);
                        }
                        if !s[i..].starts_with('(') {
                            self.error("E-VA-PREPROC", "expected macro arguments", span);
                            continue;
                        }
                        i += 1;
                        let start = i;
                        let mut level = 0;
                        let mut args = Vec::new();
                        let mut part = start;
                        let mut string = false;
                        while i < s.len() {
                            let b = s.as_bytes()[i];
                            if b == b'"' {
                                string = !string;
                            }
                            if !string {
                                if b == b'(' {
                                    level += 1;
                                }
                                if b == b')' {
                                    if level == 0 {
                                        args.push(s[part..i].trim().to_string());
                                        break;
                                    }
                                    level -= 1;
                                }
                                if b == b',' && level == 0 {
                                    args.push(s[part..i].trim().to_string());
                                    part = i + 1;
                                }
                            }
                            i += 1;
                        }
                        if i == s.len() {
                            self.error("E-VA-PREPROC", "unterminated macro call", span);
                        } else {
                            i += 1;
                        }
                        if params.is_empty() && args == [""] {
                            args.clear();
                        }
                        if args.len() != params.len() {
                            self.error("E-VA-PREPROC", "macro argument count mismatch", span);
                        }
                        let mut replaced = String::new();
                        let mut pos = 0;
                        while pos < body.len() {
                            let c = body[pos..].chars().next().unwrap_or(' ');
                            if ident(c) {
                                let end = body[pos..]
                                    .find(|c: char| !ident(c))
                                    .map_or(body.len(), |n| pos + n);
                                let word = &body[pos..end];
                                if let Some(k) = params.iter().position(|p| p == word) {
                                    replaced.push_str(args.get(k).map_or("", String::as_str));
                                } else {
                                    replaced.push_str(word);
                                }
                                pos = end;
                            } else {
                                replaced.push(c);
                                pos += c.len_utf8();
                            }
                        }
                        body = replaced;
                    }
                    out.push_str(&self.expand(&body, span, depth + 1));
                } else {
                    self.error("E-VA-PREPROC", format!("undefined macro `{name}"), span);
                }
            } else if let Some(c) = tail.chars().next() {
                out.push(c);
                i += c.len_utf8();
            }
        }
        map_segment(s, &out, previous, (i, out.len()), span, &mut mapping);
        if depth == 0 {
            self.expansion_positions = mapping;
        }
        out
    }
}
// Replacement bytes point to the invocation; untouched bytes retain their own columns.
fn map_segment(
    input: &str,
    output: &str,
    start: (usize, usize),
    end: (usize, usize),
    span: Span,
    mapping: &mut Vec<Span>,
) {
    let unchanged = input[start.0..end.0] == output[start.1..end.1];
    mapping.extend((0..end.1 - start.1).map(|offset| Span {
        col: span.col + start.0 as u32 + if unchanged { offset as u32 } else { 0 },
        ..span
    }));
}
fn builtin(path: &str) -> Option<String> {
    match path {
 "disciplines.vams" => Some("`ifndef SPX_BUILTIN_DISCIPLINES_VAMS\n`define SPX_BUILTIN_DISCIPLINES_VAMS\nnature Current; units = \"A\"; access = I; abstol = 1p; endnature\nnature Voltage; units = \"V\"; access = V; abstol = 1u; endnature\nnature Charge; units = \"C\"; access = Q; abstol = 1f; endnature\nnature Flux; units = \"Wb\"; access = Phi; abstol = 1p; endnature\nnature Temperature; units = \"K\"; access = Temp; abstol = 1u; endnature\nnature Power; units = \"W\"; access = Pwr; abstol = 1n; endnature\ndiscipline electrical; potential Voltage; flow Current; domain continuous; enddiscipline\ndiscipline magnetic; potential Flux; flow Current; domain continuous; enddiscipline\ndiscipline thermal; potential Temperature; flow Power; domain continuous; enddiscipline\ndiscipline kirchoff; potential Voltage; flow Current; domain continuous; enddiscipline\n`endif\n".into()),
 "constants.vams" => { let constants = [("M_PI","3.14159265358979323846"),("M_TWO_PI","6.28318530717958647692"),("M_PI_2","1.57079632679489661923"),("M_E","2.71828182845904523536"),("M_SQRT2","1.41421356237309504880"),("M_LN2","0.69314718055994530942"),("M_LN10","2.30258509299404568402"),("M_LOG2E","1.44269504088896340736"),("M_LOG10E","0.43429448190325182765"),("M_1_PI","0.31830988618379067154"),("M_2_PI","0.63661977236758134308"),("M_2_SQRTPI","1.12837916709551257390"),("M_SQRT1_2","0.70710678118654752440"),("P_Q","1.602176634e-19"),("P_C","299792458.0"),("P_K","1.380649e-23"),("P_H","6.62607015e-34"),("P_EPS0","8.8541878188e-12"),("P_U0","1.25663706127e-6"),("P_CELSIUS0","273.15")]; Some(constants.iter().map(|(n,v)| format!("`define {n} {v}\n")).collect()) },
 _ => None
 }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::NoVaFiles;
    #[test]
    fn nested_arguments_and_string_escapes() {
        let (source, errors) = preprocess(
            "`define F(a,b) ((a)+(b))\n`F(sin(1,2),3) \"literal\\\"value\"",
            &NoVaFiles,
        );
        assert!(errors.is_empty());
        assert!(source.text.contains("((sin(1,2))+(3))"));
        assert_eq!(source.text.len(), source.positions.len());
    }
}
