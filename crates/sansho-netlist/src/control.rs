//! Structured, source-located batch control programs.
//! Provenance: original parser for the ngspice user manual's control structures.

use crate::{Diagnostic, Line, Span};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Statement {
    pub span: Span,
    pub command: String,
    /// Literal arguments, expanded in the active plot and variable scope.
    pub arguments: String,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Block {
    Command,
    If {
        yes: Vec<Statement>,
        no: Vec<Statement>,
    },
    Loop(Vec<Statement>),
}

pub(crate) fn parse(lines: &[Line]) -> Result<Vec<Statement>, Diagnostic> {
    fn sequence(
        lines: &[Line],
        pos: &mut usize,
        depth: usize,
    ) -> Result<Vec<Statement>, Diagnostic> {
        let mut statements = vec![];
        while let Some(line) = lines.get(*pos) {
            let (name, arguments) = line
                .raw
                .split_once(char::is_whitespace)
                .unwrap_or((&line.raw, ""));
            let head = name.to_ascii_lowercase();
            if head == "codemodel" {
                return Err(Diagnostic::error(
                    "E-UNSUPPORTED",
                    "codemodel requires loading an external shared library",
                )
                .with_span(line.span()));
            }
            if matches!(head.as_str(), "else" | "end") {
                break;
            }
            let err = |message| Diagnostic::error("E-CONTROL", message).with_span(line.span());
            if depth >= 64 {
                return Err(err("control nesting limit exceeded"));
            }
            *pos += 1;
            let body = match head.as_str() {
                "if" | "while" | "dowhile" | "repeat" | "foreach" => {
                    let yes = sequence(lines, pos, depth + 1)?;
                    let block = if head == "if" {
                        let no = if lines
                            .get(*pos)
                            .is_some_and(|l| l.tokens[0].lower() == "else")
                        {
                            *pos += 1;
                            sequence(lines, pos, depth + 1)?
                        } else {
                            vec![]
                        };
                        Block::If { yes, no }
                    } else {
                        Block::Loop(yes)
                    };
                    if !lines
                        .get(*pos)
                        .is_some_and(|l| l.raw.eq_ignore_ascii_case("end"))
                    {
                        return Err(err("expected end of control structure"));
                    }
                    *pos += 1;
                    block
                }
                _ => Block::Command,
            };
            statements.push(Statement {
                span: line.span(),
                command: head,
                arguments: arguments.trim().into(),
                body,
            });
        }
        Ok(statements)
    }
    let mut pos = 0;
    let statements = sequence(lines, &mut pos, 0)?;
    if let Some(l) = lines.get(pos) {
        return Err(
            Diagnostic::error("E-CONTROL", format!("unexpected {}", l.raw)).with_span(l.span()),
        );
    }
    Ok(statements)
}

pub(crate) fn write(body: &[Statement], out: &mut String) {
    for s in body {
        out.push_str(&s.command);
        if !s.arguments.is_empty() {
            out.push(' ');
            out.push_str(&s.arguments)
        }
        out.push('\n');
        match &s.body {
            Block::Command => continue,
            Block::Loop(body) => write(body, out),
            Block::If { yes, no } => {
                write(yes, out);
                if !no.is_empty() {
                    out.push_str("else\n");
                    write(no, out);
                }
            }
        }
        out.push_str("end\n");
    }
}
