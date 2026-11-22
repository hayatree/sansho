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
    for kind in [
        AnalysisKind::Pss,
        AnalysisKind::PssFreq,
        AnalysisKind::Vectors,
        AnalysisKind::Sp,
        AnalysisKind::Harmonic2,
        AnalysisKind::Harmonic3,
        AnalysisKind::Sum,
        AnalysisKind::Difference,
        AnalysisKind::Intermod3,
        AnalysisKind::Noise,
        AnalysisKind::IntegratedNoise,
        AnalysisKind::Tf,
        AnalysisKind::Sens,
        AnalysisKind::Four,
        AnalysisKind::Pz,
    ] {
        if p.starts_with(&kind.plot_name().to_ascii_lowercase()) {
            return Some(kind);
        }
    }
    if p == "periodic steady state (time domain)" {
        Some(AnalysisKind::Pss)
    } else if p == "periodic steady state (frequency domain)" {
        Some(AnalysisKind::PssFreq)
    } else if p.starts_with("s-parameter analysis") {
        Some(AnalysisKind::Sp)
    } else if p.starts_with("operating point") {
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
        "power" => Quantity::Power,
        "capacitance" => Quantity::Capacitance,
        "charge" => Quantity::Charge,
        "voltage-density" => Quantity::VoltageDensity,
        "current-density" => Quantity::CurrentDensity,
        "current-noise-power" => Quantity::CurrentNoisePower,
        "resistance" => Quantity::Resistance,
        "admittance" => Quantity::Admittance,
        "notype" => Quantity::Scalar,
        "decibel" | "db" => Quantity::Decibels,
        "phase" => Quantity::Phase,
        "pole" => Quantity::PoleZero,
        "percent" => Quantity::Percent,
        "temperature" => Quantity::Temperature,
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
        batch_raw: Default::default(),
        prints: Vec::new(),
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
        let kind = kind_of(val.trim());
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
        let fields = nv
            .checked_add(1)
            .and_then(|n| np.checked_mul(n))
            .filter(|&n| n <= text.len())
            .ok_or_else(|| err(plot_line, "raw dimensions exceed input length"))?;
        if nv > lines.len().saturating_sub(i) {
            return Err(err(plot_line, "incomplete variable table"));
        }
        let mut vars = Vec::with_capacity(nv);
        for _ in 0..nv {
            let f: Vec<&str> = lines
                .get(i)
                .map_or(Vec::new(), |l| l.split_whitespace().collect());
            if f.len() < 3 {
                return Err(err(i + 1, "malformed variable line"));
            }
            let name = f[1].to_ascii_lowercase();
            let inner = name
                .strip_prefix("v(")
                .or_else(|| name.strip_prefix("i("))
                .and_then(|s| s.strip_suffix(')'));
            let unwrapped = inner.filter(|s| {
                s.starts_with('@')
                    || kind == Some(AnalysisKind::Sens)
                    || kind == Some(AnalysisKind::IntegratedNoise)
                        && crate::integrated_noise_name(s)
            });
            vars.push(Variable {
                name: unwrapped.unwrap_or(&name).to_owned(),
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
        let mut toks: Vec<&str> = Vec::with_capacity(fields);
        while toks.len() < fields && i < lines.len() {
            let l = lines[i].trim_start();
            if l.starts_with(|c: char| c.is_ascii_alphabetic()) && l.contains(':') {
                break;
            }
            toks.extend(lines[i].split_whitespace());
            i += 1;
        }
        if toks.len() < fields {
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
        let Some(kind) = kind else {
            continue;
        };
        let has_scale = kind.is_distortion()
            || matches!(
                kind,
                AnalysisKind::Sp
                    | AnalysisKind::Pss
                    | AnalysisKind::PssFreq
                    | AnalysisKind::Ac
                    | AnalysisKind::Dc
                    | AnalysisKind::Tran
                    | AnalysisKind::Noise
                    | AnalysisKind::Four
            )
            || (kind == AnalysisKind::Sens && vars.first().is_some_and(|v| v.name == "frequency"));
        let (scale, scale_values, vars) = if has_scale && !vars.is_empty() {
            let mut s = vars[0].clone();
            if kind == AnalysisKind::Dc && s.name == "v(v-sweep)" {
                s.name = "v-sweep".into();
            }
            let vals = if complex {
                cx[0].iter().map(|c| c.0).collect()
            } else {
                re[0].clone()
            };
            (Some(s), vals, vars[1..].to_vec())
        } else {
            (None, Vec::new(), vars)
        };
        let mut vars = vars;
        restore_special_names(kind, &mut vars);
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
            events: Default::default(),
            kind,
            scale,
            scale_values,
            variables: vars,
            data,
        });
    }
    Ok(results)
}

/// Undo the batch spelling of transfer-function, pole-zero and S-parameter
/// vectors, which ngspice writes as `v(...)` voltages, so that a Sansho raw
/// file reads back with its native names and units.
fn restore_special_names(kind: AnalysisKind, vars: &mut [Variable]) {
    if !matches!(kind, AnalysisKind::Tf | AnalysisKind::Pz | AnalysisKind::Sp) {
        return;
    }
    let unwrap = |name: &str| {
        name.strip_prefix("v(")
            .and_then(|s| s.strip_suffix(')'))
            .map(str::to_owned)
    };
    // In a TF plot the output vector says whether the output is a current
    // and the input impedance names the source, which fixes the gain unit.
    let current_out = vars.iter().any(|v| v.name.ends_with("#output_impedance)"));
    let current_in = vars
        .iter()
        .any(|v| v.name.starts_with("v(i") && v.name.ends_with("#input_impedance)"));
    for v in vars.iter_mut() {
        let Some(inner) = unwrap(&v.name) else {
            continue;
        };
        let quantity = match kind {
            AnalysisKind::Pz if inner.starts_with("pole(") || inner.starts_with("zero(") => {
                Quantity::PoleZero
            }
            AnalysisKind::Sp => match inner.split('_').next() {
                Some("s") if inner.matches('_').count() == 2 => Quantity::Scalar,
                Some("y") if inner.matches('_').count() == 2 => Quantity::Admittance,
                Some("z") if inner.matches('_').count() == 2 => Quantity::Resistance,
                _ => continue,
            },
            AnalysisKind::Tf if inner == "transfer_function" => match (current_out, current_in) {
                (false, true) => Quantity::Resistance,
                (true, false) => Quantity::Admittance,
                _ => Quantity::Scalar,
            },
            AnalysisKind::Tf
                if inner.ends_with("#input_impedance")
                    || inner.ends_with("#output_impedance")
                    || inner.starts_with("output_impedance_at_") =>
            {
                Quantity::Resistance
            }
            _ => continue,
        };
        v.name = if kind == AnalysisKind::Sp {
            inner.to_ascii_uppercase()
        } else {
            inner
        };
        v.quantity = quantity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write_raw_ascii;

    #[test]
    fn round_trip() {
        let r = SimResults {
            batch_raw: Default::default(),
            prints: Vec::new(),
            measures: Vec::new(),
            title: "t".into(),
            analyses: vec![
                AnalysisResult {
                    events: Default::default(),
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
                    events: Default::default(),
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
