//! Analog tokens with original source positions.
//! Provenance: original implementation from the Accellera Verilog-AMS LRM 2.4; see docs/PROVENANCE.md.
use crate::preproc::Preprocessed;
use spx_diag::{Diagnostic, Span};
/// Lexical token categories.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Identifier reference.
    Ident(String),
    /// System identifier.
    System(String),
    /// Integer literal.
    Int(i64),
    /// Real literal.
    Real(f64),
    /// String literal.
    Str(String),
    /// Operator or punctuation.
    Symbol(String),
    /// End-of-input marker.
    Eof,
}
/// A lexical token and its original source position.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Syntactic form.
    pub kind: TokenKind,
    /// Original source position.
    pub span: Span,
}
/// Tokenize preprocessed text and collect lexical diagnostics.
pub fn lex(source: &Preprocessed) -> (Vec<Token>, Vec<Diagnostic>) {
    let s = &source.text;
    let b = s.as_bytes();
    let mut i = 0;
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let span = source.positions.get(i).copied().unwrap_or(Span::new(1, 1));
        if s[i..].starts_with("//") {
            i = s[i..].find('\n').map_or(b.len(), |n| i + n);
            continue;
        }
        if s[i..].starts_with("/*") || s[i..].starts_with("(*") {
            let close = if b[i] == b'/' { "*/" } else { "*)" };
            if let Some(n) = s[i + 2..].find(close) {
                i += n + 4;
            } else {
                errors.push(
                    Diagnostic::error("E-VA-SYNTAX", "unterminated comment or attribute")
                        .with_span(span),
                );
                i = b.len();
            }
            continue;
        }
        let kind = if b[i] == b'\\' {
            i += 1;
            let begin = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            TokenKind::Ident(s[begin..i].into())
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] == b'$' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            if b[start] == b'$' {
                TokenKind::System(s[start..i].into())
            } else {
                TokenKind::Ident(s[start..i].into())
            }
        } else if b[i] == b'"' {
            i += 1;
            let mut value = String::new();
            let mut closed = false;
            while i < b.len() {
                if b[i] == b'"' {
                    i += 1;
                    closed = true;
                    break;
                }
                if b[i] == b'\\' {
                    i += 1;
                    if i == b.len() {
                        break;
                    }
                    if matches!(b[i], b'0'..=b'7') {
                        let mut code = 0u32;
                        let mut digits = 0;
                        while digits < 3 && i < b.len() && matches!(b[i], b'0'..=b'7') {
                            code = code * 8 + u32::from(b[i] - b'0');
                            i += 1;
                            digits += 1;
                        }
                        value.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    } else {
                        value.push(match b[i] {
                            b'n' => '\n',
                            b't' => '\t',
                            b'r' => '\r',
                            b'f' => '\u{000c}',
                            b'b' => '\u{0008}',
                            b'v' => '\u{000b}',
                            b'a' => '\u{0007}',
                            c => char::from(c),
                        });
                        i += 1;
                    }
                } else if let Some(c) = s[i..].chars().next() {
                    value.push(c);
                    i += c.len_utf8();
                }
            }
            if !closed {
                errors
                    .push(Diagnostic::error("E-VA-SYNTAX", "unterminated string").with_span(span));
            }
            TokenKind::Str(value)
        } else if b[i].is_ascii_digit()
            || b[i] == b'\''
            || (b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let result = if b.get(i) == Some(&b'\'') {
                i += 1;
                if matches!(b.get(i), Some(b's' | b'S')) {
                    i += 1;
                }
                let radix = match b.get(i) {
                    Some(b'b' | b'B') => 2,
                    Some(b'o' | b'O') => 8,
                    Some(b'd' | b'D') => 10,
                    Some(b'h' | b'H') => 16,
                    _ => 0,
                };
                i = (i + 1).min(b.len());
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let digits = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if radix == 0 {
                    None
                } else {
                    i64::from_str_radix(&s[digits..i].replace('_', ""), radix)
                        .ok()
                        .map(TokenKind::Int)
                }
            } else {
                if b.get(i) == Some(&b'.') {
                    i += 1;
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                        i += 1;
                    }
                }
                if matches!(b.get(i), Some(b'e' | b'E')) {
                    i += 1;
                    if matches!(b.get(i), Some(b'+' | b'-')) {
                        i += 1;
                    }
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                        i += 1;
                    }
                }
                let number = s[start..i].replace('_', "");
                let scale = match b.get(i) {
                    Some(b'T') => Some(1e12),
                    Some(b'G') => Some(1e9),
                    Some(b'M') => Some(1e6),
                    Some(b'K' | b'k') => Some(1e3),
                    Some(b'm') => Some(1e-3),
                    Some(b'u') => Some(1e-6),
                    Some(b'n') => Some(1e-9),
                    Some(b'p') => Some(1e-12),
                    Some(b'f') => Some(1e-15),
                    Some(b'a') => Some(1e-18),
                    _ => None,
                };
                if scale.is_some() {
                    i += 1;
                }
                if scale.is_none() && !number.contains(['.', 'e', 'E']) {
                    number.parse::<i64>().ok().map(TokenKind::Int)
                } else {
                    number
                        .parse::<f64>()
                        .ok()
                        .map(|v| v * scale.unwrap_or(1.0))
                        .filter(|v| v.is_finite())
                        .map(TokenKind::Real)
                }
            };
            match result {
                Some(k) => k,
                None => {
                    errors.push(
                        Diagnostic::error("E-VA-SYNTAX", "invalid or unsupported numeric literal")
                            .with_span(span),
                    );
                    TokenKind::Int(0)
                }
            }
        } else {
            let op = [
                "<<<", ">>>", "===", "!==", "<+", "**", "<<", ">>", "<=", ">=", "==", "!=", "&&",
                "||", "~^", "^~", "~&", "~|",
            ]
            .into_iter()
            .find(|op| s[i..].starts_with(op));
            if let Some(op) = op {
                i += op.len();
                TokenKind::Symbol(op.into())
            } else {
                let c = s[i..].chars().next().unwrap_or(' ');
                i += c.len_utf8();
                if !"()[]{};,:?.+-*/%<>=!~&|^@".contains(c) {
                    errors.push(
                        Diagnostic::error("E-VA-SYNTAX", format!("unexpected character {c}"))
                            .with_span(span),
                    );
                }
                TokenKind::Symbol(c.to_string())
            }
        };
        tokens.push(Token { kind, span });
    }
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: source.positions.last().copied().unwrap_or(Span::new(1, 1)),
    });
    (tokens, errors)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{NoVaFiles, preproc::preprocess};
    #[test]
    fn malformed_literals_report_positions() {
        for input in ["1e+", "'hxyz", "\"unclosed", "/* unclosed", "(* unclosed"] {
            let (source, _) = preprocess(input, &NoVaFiles);
            let (_, errors) = lex(&source);
            assert!(!errors.is_empty(), "{input}");
            assert_eq!(errors[0].span, Some(Span::new(1, 1)));
        }
    }
}
