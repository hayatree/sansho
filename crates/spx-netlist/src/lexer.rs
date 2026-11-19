//! Splitting netlist text into logical lines and tokens.
//!
//! Rules (SPICE conventions): the first line is the title; lines starting with `*` are comments;
//! lines starting with `+` continue the previous line; `;`, `$ ` and `//` start inline comments.
//! Whitespace, `,`, `(` and `)` separate tokens; `=` is its own token. `{...}` and `'...'`
//! (parameter expressions) are single tokens including their delimiters.
//!
//! Provenance: original work based on the SPICE input format described in Nagel (1975).
//! See `docs/PROVENANCE.md`.

use spx_diag::Span;

/// A token with its source position.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token text, as written (case preserved).
    pub text: String,
    /// Position of the first character.
    pub span: Span,
    /// The token is followed by a closing parenthesis.
    pub ends_group: bool,
}

impl Token {
    /// Lower-cased token text.
    pub fn lower(&self) -> String {
        self.text.to_ascii_lowercase()
    }

    /// A bound parameter may be represented as an integer in scientific notation.
    pub(crate) fn positive_integer(&self) -> Option<usize> {
        let value = self.text.parse::<f64>().ok()?;
        (value > 0. && value.fract() == 0. && value < usize::MAX as f64).then_some(value as usize)
    }
    /// Whether this token is `=`.
    pub fn is_eq(&self) -> bool {
        self.text == "="
    }
}

/// A logical line (after joining continuations) as a list of tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Tokens; never empty.
    pub tokens: Vec<Token>,
    /// Text of the logical line (continuations joined with a space, comments removed).
    pub raw: String,
}

impl Line {
    /// A linear E/G can legitimately have a control node named `poly`.
    pub(crate) fn polynomial(&self) -> bool {
        self.tokens
            .first()
            .is_some_and(|t| t.lower().starts_with(['e', 'g', 'f', 'h']))
            && self
                .tokens
                .get(3)
                .is_some_and(|t| t.text.eq_ignore_ascii_case("poly"))
            && (self.tokens.len() > 6
                || self
                    .raw
                    .split_whitespace()
                    .nth(3)
                    .is_some_and(|w| w.to_ascii_lowercase().starts_with("poly(")))
    }
    /// Span of the first token.
    pub fn span(&self) -> Span {
        self.tokens.first().map_or(Span::new(0, 0), |t| t.span)
    }
}

/// Result of lexing: the title line and the logical lines that follow it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lexed {
    /// The first line of the file.
    pub title: String,
    /// Logical lines (comments and blank lines removed).
    pub lines: Vec<Line>,
}

fn strip_inline_comment(s: &str) -> &str {
    let mut end = s.len();
    if let Some(p) = s.find(';') {
        end = end.min(p);
    }
    if let Some(p) = s.find("//") {
        end = end.min(p);
    }
    // `$` starts a comment when at line start or preceded by whitespace and followed by
    // whitespace or end of line.
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'$'
            && (i == 0 || b[i - 1].is_ascii_whitespace())
            && b.get(i + 1).is_none_or(|n| n.is_ascii_whitespace())
        {
            end = end.min(i);
            break;
        }
    }
    &s[..end]
}

fn tokenize(text: &str, line_no: u32, col_offset: u32, out: &mut Vec<Token>) {
    let mut cur = String::new();
    let mut cur_col = 0u32;
    let flush = |cur: &mut String, cur_col: u32, out: &mut Vec<Token>| {
        if !cur.is_empty() {
            out.push(Token {
                text: std::mem::take(cur),
                span: Span::new(line_no, cur_col),
                ends_group: false,
            });
        }
    };
    let mut close: Option<char> = None;
    let mut depth = 0u32;
    for (i, ch) in text.char_indices() {
        let col = col_offset + i as u32 + 1;
        if let Some(end) = close {
            // Inside an expression: keep everything up to the matching delimiter.
            cur.push(ch);
            if ch == '{' && end == '}' {
                depth += 1;
            } else if ch == end {
                if depth == 0 {
                    close = None;
                } else {
                    depth -= 1;
                }
            }
            continue;
        }
        match ch {
            '{' | '\'' | '"' => {
                if cur.is_empty() {
                    cur_col = col;
                }
                cur.push(ch);
                close = Some(if ch == '{' { '}' } else { ch });
                depth = 0;
            }
            ')' => {
                flush(&mut cur, cur_col, out);
                if let Some(token) = out.last_mut() {
                    token.ends_group = true;
                }
            }
            c if c.is_whitespace() || c == ',' || c == '(' => {
                flush(&mut cur, cur_col, out);
            }
            '=' => {
                flush(&mut cur, cur_col, out);
                out.push(Token {
                    text: "=".into(),
                    span: Span::new(line_no, col),
                    ends_group: false,
                });
            }
            c => {
                if cur.is_empty() {
                    cur_col = col;
                }
                cur.push(c);
            }
        }
    }
    flush(&mut cur, cur_col, out);
}

fn set_file(toks: &mut [Token], file: u32) {
    if file != 0 {
        for t in toks {
            t.span = t.span.in_file(file);
        }
    }
}

/// Lex netlist text (the first line is the title).
pub fn lex(text: &str) -> Lexed {
    lex_source(text, 0, true)
}

/// Lex the text of source file `file`; included files have no title line.
pub fn lex_source(text: &str, file: u32, has_title: bool) -> Lexed {
    let mut physical = text.lines();
    let title = if has_title {
        physical.next().unwrap_or("").trim_end().to_string()
    } else {
        String::new()
    };
    let first = if has_title { 2 } else { 1 };
    let mut lines: Vec<Line> = Vec::new();
    for (idx, raw) in physical.enumerate() {
        let line_no = idx as u32 + first;
        let trimmed = raw.trim_start();
        let lead = (raw.len() - trimmed.len()) as u32;
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('+') {
            let body = strip_inline_comment(rest);
            let mut toks = Vec::new();
            tokenize(body, line_no, lead + 1, &mut toks);
            set_file(&mut toks, file);
            if let Some(last) = lines.last_mut() {
                last.tokens.extend(toks);
                last.raw.push(' ');
                last.raw.push_str(body.trim());
            } else if !toks.is_empty() {
                lines.push(Line {
                    tokens: toks,
                    raw: body.trim().to_string(),
                });
            }
            continue;
        }
        let body = strip_inline_comment(trimmed);
        let mut toks = Vec::new();
        tokenize(body, line_no, lead, &mut toks);
        set_file(&mut toks, file);
        if !toks.is_empty() {
            lines.push(Line {
                tokens: toks,
                raw: body.trim().to_string(),
            });
        }
    }
    Lexed { title, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(l: &Line) -> Vec<&str> {
        l.tokens.iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn title_comments_and_continuations() {
        let src = "my title\n* comment\nR1 a b 1k ; inline\n\nV1 in 0\n+ PULSE(0 1 1n)\n.end\n";
        let lx = lex(src);
        assert_eq!(lx.title, "my title");
        assert_eq!(lx.lines.len(), 3);
        assert_eq!(texts(&lx.lines[0]), ["R1", "a", "b", "1k"]);
        assert_eq!(
            texts(&lx.lines[1]),
            ["V1", "in", "0", "PULSE", "0", "1", "1n"]
        );
        assert_eq!(lx.lines[1].tokens[3].span, Span::new(6, 3));
        assert_eq!(texts(&lx.lines[2]), [".end"]);
    }

    #[test]
    fn expressions_are_single_tokens() {
        let lx = lex("t\nR1 a b {r0 * (1 + k)} m={2*f(a, b)}\n.param x = 'a + b'\n");
        assert_eq!(
            texts(&lx.lines[0]),
            ["R1", "a", "b", "{r0 * (1 + k)}", "m", "=", "{2*f(a, b)}"]
        );
        assert_eq!(texts(&lx.lines[1]), [".param", "x", "=", "'a + b'"]);
        assert_eq!(lx.lines[1].raw, ".param x = 'a + b'");
    }

    #[test]
    fn equals_and_dollar_comments() {
        let lx = lex("t\nR1 a b r=10 $ comment\n.options reltol = 1e-4\n");
        assert_eq!(texts(&lx.lines[0]), ["R1", "a", "b", "r", "=", "10"]);
        assert_eq!(texts(&lx.lines[1]), [".options", "reltol", "=", "1e-4"]);
    }
}
