//! Splitting netlist text into logical lines and tokens.
//!
//! Rules (SPICE conventions): the first line is the title; lines starting with `*` are comments;
//! lines starting with `+` continue the previous line; `;`, `$ ` and `//` start inline comments.
//! Whitespace, `,`, `(` and `)` separate tokens; `=` is its own token.
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
}

impl Token {
    /// Lower-cased token text.
    pub fn lower(&self) -> String {
        self.text.to_ascii_lowercase()
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
}

impl Line {
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
            });
        }
    };
    for (i, ch) in text.char_indices() {
        let col = col_offset + i as u32 + 1;
        match ch {
            c if c.is_whitespace() || c == ',' || c == '(' || c == ')' => {
                flush(&mut cur, cur_col, out);
            }
            '=' => {
                flush(&mut cur, cur_col, out);
                out.push(Token {
                    text: "=".into(),
                    span: Span::new(line_no, col),
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

/// Lex netlist text.
pub fn lex(text: &str) -> Lexed {
    let mut physical = text.lines();
    let title = physical.next().unwrap_or("").trim_end().to_string();
    let mut lines: Vec<Line> = Vec::new();
    for (idx, raw) in physical.enumerate() {
        let line_no = idx as u32 + 2;
        let trimmed = raw.trim_start();
        let lead = (raw.len() - trimmed.len()) as u32;
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('+') {
            let body = strip_inline_comment(rest);
            let mut toks = Vec::new();
            tokenize(body, line_no, lead + 1, &mut toks);
            if let Some(last) = lines.last_mut() {
                last.tokens.extend(toks);
            } else if !toks.is_empty() {
                lines.push(Line { tokens: toks });
            }
            continue;
        }
        let body = strip_inline_comment(trimmed);
        let mut toks = Vec::new();
        tokenize(body, line_no, lead, &mut toks);
        if !toks.is_empty() {
            lines.push(Line { tokens: toks });
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
    fn equals_and_dollar_comments() {
        let lx = lex("t\nR1 a b r=10 $ comment\n.options reltol = 1e-4\n");
        assert_eq!(texts(&lx.lines[0]), ["R1", "a", "b", "r", "=", "10"]);
        assert_eq!(texts(&lx.lines[1]), [".options", "reltol", "=", "1e-4"]);
    }
}
