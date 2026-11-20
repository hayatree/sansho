//! Analog A-card port syntax and bracketed model values.
//! Provenance: ngspice XSPICE netlist conventions, original parser.

use crate::{ModelCard, Token};
use sansho_diag::Diagnostic;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodePort {
    /// Empty selects the model's default port type.
    pub modifier: String,
    pub vector: bool,
    /// Consecutive differential pairs for d-suffixed types. Empty denotes NULL.
    pub nodes: Vec<String>,
}

impl CodePort {
    pub fn differential(&self) -> bool {
        self.modifier.ends_with('d')
    }

    pub fn text(&self) -> String {
        let modifier = if self.modifier.is_empty() {
            String::new()
        } else {
            format!("%{} ", self.modifier)
        };
        let nodes = self.nodes.join(" ");
        let body = if self.nodes.is_empty() {
            "NULL".into()
        } else if self.vector {
            format!("[{nodes}]")
        } else {
            nodes
        };
        format!("{modifier}{body}")
    }
}

/// Split brackets only in A-card grammar. Ordinary node names and probes may contain brackets.
pub(crate) fn brackets(tokens: &[Token]) -> Vec<Token> {
    let mut out = Vec::new();
    for t in tokens {
        if t.text.starts_with(['{', '\'', '"']) {
            out.push(t.clone());
            continue;
        }
        let mut start = 0;
        for (pos, _) in t.text.match_indices(['[', ']']) {
            if start < pos {
                out.push(Token {
                    text: t.text[start..pos].into(),
                    ..t.clone()
                });
            }
            out.push(Token {
                text: t.text[pos..pos + 1].into(),
                ..t.clone()
            });
            start = pos + 1;
        }
        if start < t.text.len() {
            out.push(Token {
                text: t.text[start..].into(),
                ..t.clone()
            });
        }
    }
    out
}

pub(crate) fn ports(tokens: &[Token]) -> Result<Vec<CodePort>, String> {
    let mut out = Vec::new();
    let mut it = tokens.iter().peekable();
    while let Some(mut first) = it.next() {
        let mut modifier = String::new();
        if let Some(m) = first.lower().strip_prefix('%') {
            modifier = m.to_owned();
            first = it.next().ok_or("missing port after modifier")?;
        }
        let mut port = CodePort {
            modifier,
            vector: first.text == "[",
            nodes: Vec::new(),
        };
        if first.text != "~" && !first.text.eq_ignore_ascii_case("null") {
            let count = if port.vector {
                let n = it.clone().take_while(|t| t.text != "]").count();
                if n == 0 || port.differential() && !n.is_multiple_of(2) {
                    return Err("empty or incomplete port vector".into());
                }
                n
            } else {
                port.nodes.push(first.lower());
                usize::from(port.differential())
            };
            for _ in 0..count {
                port.nodes
                    .push(it.next().ok_or("missing port terminal")?.lower());
            }
            if port.vector && !it.next().is_some_and(|t| t.text == "]") {
                return Err("unterminated port vector".into());
            }
            for n in &mut port.nodes {
                if n.starts_with('%') || matches!(n.as_str(), "[" | "]" | "~" | "null" | "=") {
                    return Err(format!("invalid port terminal {n}"));
                }
                if n == "gnd" {
                    *n = "0".into();
                }
            }
        }
        out.push(port);
    }
    Ok(out)
}

pub fn analog_model(kind: &str) -> bool {
    matches!(
        kind,
        "sine"
            | "triangle"
            | "square"
            | "slew"
            | "aswitch"
            | "filesource"
            | "gain"
            | "summer"
            | "mult"
            | "divide"
            | "limit"
            | "climit"
            | "pwl"
            | "d_dt"
            | "int"
            | "s_xfer"
    )
}

impl ModelCard {
    /// Check required code-model parameters in their declared scalar, array or file form.
    pub fn check_code_model_params(&self) -> Result<(), Diagnostic> {
        let (scalars, arrays): (&[&str], &[&str]) = match self.kind.as_str() {
            "d_dt" | "int" => (&["out_lower_limit", "out_upper_limit"], &[]),
            "sine" | "triangle" | "square" => (&[], &["cntl_array", "freq_array"]),
            "pwl" => (&[], &["x_array", "y_array"]),
            "s_xfer" => (&[], &["num_coeff", "den_coeff"]),
            _ => (&[], &[]),
        };
        let missing = scalars
            .iter()
            .find(|&&key| !self.params.iter().any(|(k, _)| k == key))
            .or_else(|| {
                arrays
                    .iter()
                    .find(|&&key| !self.arrays.iter().any(|(k, _)| k == key))
            })
            .copied()
            .or_else(|| (self.kind == "filesource" && self.table.is_none()).then_some("file"));
        if let Some(key) = missing {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                format!(
                    "model {} ({}): missing required parameter {key}",
                    self.name, self.kind
                ),
            )
            .with_span(self.span));
        }
        Ok(())
    }
}

/// Time and channel columns captured at parse time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceTable {
    pub filename: String,
    pub rows: Vec<Vec<f64>>,
}

impl SourceTable {
    pub(crate) fn read(
        &mut self,
        loader: &dyn crate::SourceLoader,
        from: Option<&str>,
    ) -> Result<(), String> {
        let path = std::path::Path::new(&self.filename);
        if path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err("filesource path must be relative and cannot contain ..".into());
        }
        let data = loader.waveform(path, from)?;
        for (i, line) in data.lines().enumerate() {
            let data = line
                .split(['#', ';', '*'])
                .next()
                .unwrap_or_default()
                .trim();
            if data.is_empty() {
                continue;
            }
            let row: Vec<f64> = data
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .map(|s| {
                    crate::parse_number(s)
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("{}:{}: invalid sample", self.filename, i + 1))
                })
                .collect::<Result<_, _>>()?;
            if row.len() < 2 || self.rows.first().is_some_and(|r| r.len() != row.len()) {
                return Err(format!(
                    "{}:{}: inconsistent filesource columns",
                    self.filename,
                    i + 1
                ));
            }
            self.rows.push(row);
        }
        if self.rows.is_empty() {
            return Err(format!("{}: empty waveform", self.filename));
        }
        Ok(())
    }
}
