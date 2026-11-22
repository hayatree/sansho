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
    /// Invert the sign at a scalar real terminal.
    pub inverted: bool,
    /// Consecutive differential pairs for d-suffixed types. Empty denotes NULL.
    pub nodes: Vec<String>,
}

impl CodePort {
    pub fn differential(&self) -> bool {
        matches!(self.modifier.as_str(), "vd" | "id" | "gd" | "hd")
    }

    pub fn text(&self) -> String {
        let modifier = if self.modifier.is_empty() {
            String::new()
        } else {
            format!("%{} ", self.modifier)
        };
        let mut nodes = self.nodes.join(" ");
        if self.inverted {
            nodes.insert(0, '~');
        }
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
            inverted: false,
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
            | "pwlts"
            | "xfer"
            | "delay"
            | "hyst"
            | "ilimit"
            | "multi_input_pwl"
            | "zener"
            | "core"
            | "lcouple"
            | "oneshot"
            | "cmeter"
            | "lmeter"
            | "pswitch"
            | "sidiode"
            | "potentiometer"
            | "memristor"
            | "table2d"
            | "table3d"
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

/// Event-domain families. The model signature determines each port's default domain.
pub fn digital_model(kind: &str) -> bool {
    matches!(
        kind,
        "adc_bridge"
            | "dac_bridge"
            | "d_osc"
            | "d_buffer"
            | "d_inverter"
            | "d_and"
            | "d_nand"
            | "d_or"
            | "d_nor"
            | "d_xor"
            | "d_xnor"
            | "d_tristate"
            | "d_pullup"
            | "d_pulldown"
            | "d_open_c"
            | "d_open_e"
            | "d_dff"
            | "d_jkff"
            | "d_tff"
            | "d_srff"
            | "d_dlatch"
            | "d_srlatch"
            | "d_fdiv"
            | "d_source"
            | "d_lut"
            | "d_state"
            | "d_ram"
            | "d_genlut"
            | "bidi_bridge"
            | "real_gain"
            | "real_delay"
            | "real_to_v"
            | "d_to_real"
            | "d_pwm"
    )
}

pub(crate) fn code_model(kind: &str) -> bool {
    analog_model(kind) || digital_model(kind)
}

impl ModelCard {
    /// Check required code-model parameters in their declared scalar, array or file form.
    pub fn check_code_model_params(&self) -> Result<(), Diagnostic> {
        let (scalars, arrays): (&[&str], &[&str]) = match self.kind.as_str() {
            "d_dt" | "int" => (&["out_lower_limit", "out_upper_limit"], &[]),

            "multi_input_pwl" => (&[], &["x", "y"]),
            "zener" => (&["v_breakdown"], &[]),
            "core"
                if !self
                    .params
                    .iter()
                    .rev()
                    .find(|(k, _)| k == "mode")
                    .is_some_and(|(_, v)| *v == 2.) =>
            {
                (&["area", "length"], &["h_array", "b_array"])
            }
            "pwl" | "pwlts" => (&[], &["x_array", "y_array"]),
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
        let missing = missing
            .or_else(|| (self.kind == "d_source" && self.table.is_none()).then_some("input_file"))
            .or_else(|| {
                (matches!(self.kind.as_str(), "d_lut" | "d_genlut")
                    && self.arrays.iter().all(|(k, _)| k != "table_values"))
                .then_some("table_values")
            });
        let missing = missing.or_else(|| {
            (self.kind == "xfer"
                && self.table.is_none()
                && !self.arrays.iter().any(|(k, _)| k == "table"))
            .then_some("table or file")
        });
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
    /// Token records for model-specific state and frequency file grammars.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
}

impl SourceTable {
    pub(crate) fn read(
        &mut self,
        loader: &dyn crate::SourceLoader,
        from: Option<&str>,
        kind: &str,
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
        if matches!(kind, "d_state" | "xfer") {
            self.records = data.lines().map(str::to_owned).collect();
            return Ok(());
        }
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
                .enumerate()
                .map(|(col, s)| {
                    if kind == "d_source" && col > 0 {
                        return digital_code(s).map(f64::from).ok_or_else(|| {
                            format!("{}:{}: invalid digital value {s}", self.filename, i + 1)
                        });
                    }
                    crate::parse_number(s)
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("{}:{}: invalid sample", self.filename, i + 1))
                })
                .collect::<Result<_, _>>()?;
            if !matches!(kind, "table2d" | "table3d")
                && (row.len() < 2 || self.rows.first().is_some_and(|r| r.len() != row.len()))
            {
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

/// Stable packed state used in a digital stimulus file (level + 3 × strength).
pub fn digital_code(text: &str) -> Option<u8> {
    let mut bytes = text.bytes().map(|c| c.to_ascii_lowercase());
    let level = match bytes.next()? {
        b'0' => 0,
        b'1' => 1,
        b'u' => 2,
        _ => return None,
    };
    let drive = match bytes.next() {
        Some(b's') | None => 0,
        Some(b'r') => 1,
        Some(b'z') => 2,
        Some(b'u') => 3,
        _ => return None,
    };
    bytes.next().is_none().then_some(level + 3 * drive)
}
