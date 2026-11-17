//! Reader for ASCII raw files (as written by this crate or by other SPICE simulators).
//!
//! Provenance: original work following the publicly documented SPICE3 rawfile layout.
//! See `docs/PROVENANCE.md`.

use crate::{AnalysisKind, AnalysisResult, Data, Quantity, SimResults, Variable};

/// Error while reading a raw file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawError {
    /// 1-based line number.
    pub line: usize,
    /// Description.
    pub message: String,
}

impl std::fmt::Display for RawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "raw file line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for RawError {}

fn err(line: usize, message: impl Into<String>) -> RawError {
    RawError {
        line,
        message: message.into(),
    }
}

fn kind_of(plot: &str) -> Option<AnalysisKind> {
    let p = plot.to_ascii_lowercase();
    if p.starts_with("operating point") {
        Some(AnalysisKind::Op)
    } else if p.starts_with("dc transfer") {
        Some(AnalysisKind::Dc)
    } else if p.starts_with("ac analysis") {
        Some(AnalysisKind::Ac)
    } else if p.starts_with("transient analysis") {
        Some(AnalysisKind::Tran)
    } else {
        None
    }
}

fn quantity_of(t: &str) -> Quantity {
    match t.to_ascii_lowercase().as_str() {
        "time" => Quantity::Time,
        "frequency" => Quantity::Frequency,
        "current" => Quantity::Current,
        _ => Quantity::Voltage,
    }
}

/// Parse an ASCII raw file. Plots of unknown analysis types are skipped.
pub fn read_raw_ascii(text: &str) -> Result<SimResults, RawError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut results = SimResults {
        measures: Vec::new(),
        title: String::new(),
        analyses: Vec::new(),
    };
    let mut i = 0;
    while i < lines.len() {
        let Some((key, val)) = lines[i].split_once(':') else {
            i += 1;
            continue;
        };
        if key.trim().eq_ignore_ascii_case("title") {
            results.title = val.trim().to_string();
            i += 1;
            continue;
        }
        if !key.trim().eq_ignore_ascii_case("plotname") {
            i += 1;
            continue;
        }
        let plot = val.trim().to_string();
        let plot_line = i + 1;
        let (mut complex, mut nvars, mut npts) = (false, None, None);
        i += 1;
        while i < lines.len()
            && !lines[i]
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("variables:")
        {
            if let Some((k, v)) = lines[i].split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "flags" => complex = v.to_ascii_lowercase().contains("complex"),
                    "no. variables" => nvars = v.trim().parse::<usize>().ok(),
                    "no. points" => npts = v.trim().parse::<usize>().ok(),
                    _ => {}
                }
            }
            i += 1;
        }
        let (Some(nv), Some(np)) = (nvars, npts) else {
            return Err(err(plot_line, "missing variable or point count"));
        };
        i += 1;
        let mut vars = Vec::with_capacity(nv);
        for _ in 0..nv {
            let f: Vec<&str> = lines
                .get(i)
                .map_or(Vec::new(), |l| l.split_whitespace().collect());
            if f.len() < 3 {
                return Err(err(i + 1, "malformed variable line"));
            }
            vars.push(Variable {
                name: f[1].to_ascii_lowercase(),
                quantity: quantity_of(f[2]),
            });
            i += 1;
        }
        if !lines
            .get(i)
            .is_some_and(|l| l.trim().eq_ignore_ascii_case("values:"))
        {
            return Err(err(
                i + 1,
                "expected `Values:` (binary raw files are not supported)",
            ));
        }
        i += 1;
        // Values: each point is an index followed by nv values, whitespace separated.
        let mut toks: Vec<&str> = Vec::with_capacity(np * (nv + 1));
        while toks.len() < np * (nv + 1) && i < lines.len() {
            let l = lines[i].trim_start();
            if l.starts_with(|c: char| c.is_ascii_alphabetic()) && l.contains(':') {
                break;
            }
            toks.extend(lines[i].split_whitespace());
            i += 1;
        }
        if toks.len() < np * (nv + 1) {
            return Err(err(i, "fewer values than declared"));
        }
        let mut re: Vec<Vec<f64>> = vec![Vec::with_capacity(np); nv];
        let mut cx: Vec<Vec<(f64, f64)>> = vec![Vec::with_capacity(np); nv];
        for p in 0..np {
            for k in 0..nv {
                let t = toks[p * (nv + 1) + 1 + k];
                if complex {
                    let (a, b) = t.split_once(',').unwrap_or((t, "0"));
                    let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>()) else {
                        return Err(err(i, format!("bad complex value `{t}`")));
                    };
                    cx[k].push((a, b));
                } else {
                    let Ok(v) = t.parse::<f64>() else {
                        return Err(err(i, format!("bad value `{t}`")));
                    };
                    re[k].push(v);
                }
            }
        }
        let Some(kind) = kind_of(&plot) else {
            continue;
        };
        let has_scale = kind != AnalysisKind::Op;
        let (scale, scale_values, vars) = if has_scale && !vars.is_empty() {
            let s = vars[0].clone();
            let vals = if complex {
                cx[0].iter().map(|c| c.0).collect()
            } else {
                re[0].clone()
            };
            (Some(s), vals, vars[1..].to_vec())
        } else {
            (None, Vec::new(), vars)
        };
        let skip = usize::from(has_scale);
        let data = if complex {
            Data::Complex {
                columns: cx.into_iter().skip(skip).collect(),
            }
        } else {
            Data::Real {
                columns: re.into_iter().skip(skip).collect(),
            }
        };
        results.analyses.push(AnalysisResult {
            kind,
            scale,
            scale_values,
            variables: vars,
            data,
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write_raw_ascii;

    #[test]
    fn round_trip() {
        let r = SimResults {
            measures: Vec::new(),
            title: "t".into(),
            analyses: vec![
                AnalysisResult {
                    kind: AnalysisKind::Op,
                    scale: None,
                    scale_values: vec![],
                    variables: vec![Variable {
                        name: "v(a)".into(),
                        quantity: Quantity::Voltage,
                    }],
                    data: Data::Real {
                        columns: vec![vec![1.5]],
                    },
                },
                AnalysisResult {
                    kind: AnalysisKind::Ac,
                    scale: Some(Variable {
                        name: "frequency".into(),
                        quantity: Quantity::Frequency,
                    }),
                    scale_values: vec![1.0, 10.0],
                    variables: vec![Variable {
                        name: "i(v1)".into(),
                        quantity: Quantity::Current,
                    }],
                    data: Data::Complex {
                        columns: vec![vec![(1.0, -2.0), (0.5, 0.25)]],
                    },
                },
            ],
        };
        let mut buf = Vec::new();
        write_raw_ascii(&r, &mut buf).unwrap();
        let back = read_raw_ascii(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
