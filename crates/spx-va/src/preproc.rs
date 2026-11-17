//! Directive expansion and byte-position source mapping.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
use crate::VaLoader;
use spx_diag::{Diagnostic, Span};
use std::collections::BTreeMap;
/// Expanded source and per-byte original source positions.
#[derive(Debug, Clone)]
pub struct Preprocessed {
    /// Expanded source text.
    pub text: String,
    /// Original source position for each byte of expanded text.
    pub positions: Vec<Span>,
    /// Canonical source names indexed by span file IDs.
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

    expansion_positions: Vec<Span>,
    expansions: usize,
}
/// Expand directives and macros using only the supplied include loader.
pub fn preprocess(text: &str, loader: &dyn VaLoader) -> (Preprocessed, Vec<Diagnostic>) {
    let mut p = Processor {
        loader,
        macros: ["__VAMS_ENABLE__", "__VAMS_COMPACT_MODELING__"]
            .into_iter()
            .map(|n| {
                (
                    n.into(),
                    Macro {
                        args: None,
                        body: "1".into(),
                    },
                )
            })
            .collect(),
        out: Preprocessed {
            text: String::new(),
            positions: Vec::new(),
            sources: vec!["<input>".into()],
        },
        errors: Vec::new(),

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
        let text = mask_comments(text).unwrap_or_else(|offset| {
            let prefix = &text[..offset];
            self.error(
                "E-VA-SYNTAX",
                "unterminated block comment",
                Span::new(
                    prefix.bytes().filter(|&b| b == b'\n').count() as u32 + 1,
                    prefix.rsplit('\n').next().unwrap_or("").len() as u32 + 1,
                )
                .in_file(file),
            );
            text.to_string()
        });
        let mut pending = String::new();
        let mut pending_positions = Vec::new();
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
            let directive = directive.filter(|(name, _)| {
                matches!(
                    *name,
                    "ifdef"
                        | "ifndef"
                        | "elsif"
                        | "else"
                        | "endif"
                        | "define"
                        | "undef"
                        | "include"
                        | "resetall"
                        | "timescale"
                        | "default_nettype"
                )
            });
            if let Some((name, rest)) = directive {
                if let Some(&start) = pending_positions.first() {
                    self.expand_emit(&pending, start, &pending_positions);
                    pending.clear();
                    pending_positions.clear();
                }
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
                            let from = self.out.sources.get(file as usize).map(String::as_str);
                            let loaded = if let Some(header) = builtin(path) {
                                self.loader.standard_header(path, from).map(|local| {
                                    local.unwrap_or_else(|| (path.to_owned(), header.to_owned()))
                                })
                            } else {
                                self.loader.load(path, from)
                            };
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
                    _ => {}
                }
            } else if active {
                pending.push_str(&s);
                pending.push('\n');
                pending_positions.extend(positions);
                pending_positions.push(span);
            }
        }
        if !pending.is_empty() {
            self.expand_emit(&pending, pending_positions[0], &pending_positions);
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
        let before = self.errors.len();
        let expanded = self.expand(s, span, 0);
        for e in &mut self.errors[before..] {
            if let Some(p) = e.span {
                e.span = positions
                    .get(p.col.saturating_sub(span.col) as usize)
                    .copied()
                    .or(Some(p));
            }
        }
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
            if depth == 0 {
                map_segment(s, &out, previous, (i, out.len()), span, &mut mapping);
            }
            previous = (i, out.len());
            let tail = &s[i..];
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
                let invocation = Span {
                    col: if depth == 0 {
                        span.col + i as u32
                    } else {
                        span.col
                    },
                    ..span
                };
                let name = &tail[1..end];
                i += end;
                if let Some(m) = self.macros.get(name).cloned() {
                    self.expansions += 1;
                    if self.expansions >= 100_000 {
                        if self.expansions == 100_000 {
                            self.error(
                                "E-VA-PREPROC",
                                "macro expansion budget exceeded",
                                invocation,
                            );
                        }
                        continue;
                    }
                    let mut body = m.body;
                    if let Some(params) = m.args {
                        while s[i..].starts_with(char::is_whitespace) {
                            i += s[i..].chars().next().map_or(0, char::len_utf8);
                        }
                        if !s[i..].starts_with('(') {
                            self.error("E-VA-PREPROC", "expected macro arguments", invocation);
                            continue;
                        }
                        i += 1;
                        let start = i;
                        let mut level = 0usize;
                        let mut args = Vec::new();
                        let mut part = start;
                        let mut string = false;
                        while i < s.len() {
                            let b = s.as_bytes()[i];
                            if string && b == b'\\' {
                                i = (i + 2).min(s.len());
                                continue;
                            }
                            if b == b'"' {
                                string = !string;
                            }
                            if !string {
                                if matches!(b, b'(' | b'[' | b'{') {
                                    level += 1;
                                }
                                if matches!(b, b')' | b']' | b'}') {
                                    if level == 0 && b == b')' {
                                        args.push(s[part..i].trim().to_string());
                                        break;
                                    }
                                    level = level.saturating_sub(1);
                                }
                                if b == b',' && level == 0 {
                                    args.push(s[part..i].trim().to_string());
                                    part = i + 1;
                                }
                            }
                            i += 1;
                        }
                        if i == s.len() {
                            self.error("E-VA-PREPROC", "unterminated macro call", invocation);
                        } else {
                            i += 1;
                        }
                        if params.is_empty() && args == [""] {
                            args.clear();
                        }
                        if args.len() != params.len() {
                            self.error("E-VA-PREPROC", "macro argument count mismatch", invocation);
                        }
                        let mut replaced = String::new();
                        let mut pos = 0;
                        let mut in_string = false;
                        let mut escaped = false;
                        while pos < body.len() {
                            let c = body[pos..].chars().next().unwrap_or(' ');
                            if c == '"' && !escaped {
                                in_string = !in_string;
                            }
                            escaped = in_string && c == '\\' && !escaped;
                            if !in_string && ident(c) {
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
                    out.push_str(&self.expand(&body, invocation, depth + 1));
                } else {
                    self.error(
                        "E-VA-PREPROC",
                        format!("undefined macro `{name}"),
                        invocation,
                    );
                }
            } else if let Some(c) = tail.chars().next() {
                out.push(c);
                i += c.len_utf8();
            }
        }
        if depth == 0 {
            map_segment(s, &out, previous, (i, out.len()), span, &mut mapping);
        }
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
// Preserve byte offsets while removing comments before preprocessing directives.
fn mask_comments(text: &str) -> Result<String, usize> {
    let mut bytes = text.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        if quoted && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        let tail = &text.as_bytes()[i..];
        let len = if !quoted && tail.starts_with(b"//") {
            tail.iter().position(|&b| b == b'\n').unwrap_or(tail.len())
        } else if !quoted && tail.starts_with(b"/*") {
            tail[2..].windows(2).position(|w| w == b"*/").ok_or(i)? + 4
        } else {
            i += 1;
            continue;
        };
        for b in &mut bytes[i..i + len] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        i += len;
    }
    Ok(String::from_utf8(bytes).unwrap_or_default())
}
fn builtin(path: &str) -> Option<&'static str> {
    match path {
        "constants.vams" | "constants.h" => Some(include_str!("headers/constants.vams")),
        "disciplines.vams" | "discipline.h" => Some(include_str!("headers/disciplines.vams")),
        _ => None,
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
