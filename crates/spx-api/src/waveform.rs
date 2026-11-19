//! Signal extraction and point-budgeted downsampling of waveforms.
//!
//! Downsampling uses Largest-Triangle-Three-Buckets (LTTB), which keeps the first and last
//! points and, per bucket, the point forming the largest triangle with its neighbours; it
//! preserves peaks and edges far better than decimation.
//!
//! Provenance: original implementation of LTTB as described in S. Steinarsson, *Downsampling
//! Time Series for Visual Representation*, MSc thesis, University of Iceland (2013).
//! See `docs/PROVENANCE.md`.

use serde::Serialize;
use spx_output::{AnalysisKind, AnalysisResult, Data, Quantity};

/// How complex (AC) values are reduced to a real series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reduce {
    /// Real data as is; dB for complex data.
    Auto,
    /// Real data as is; magnitude for complex data.
    Real,
    /// Magnitude.
    Mag,
    /// 20·log10 of the magnitude.
    Db,
    /// Phase in degrees.
    PhaseDeg,
    /// Real part.
    Re,
    /// Imaginary part.
    Im,
}

impl Reduce {
    /// Parse a name (`real`, `mag`, `db`, `phase`, `re`, `im`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "real" | "value" => Self::Real,
            "mag" | "abs" => Self::Mag,
            "db" => Self::Db,
            "phase" | "phase_deg" | "deg" => Self::PhaseDeg,
            "re" => Self::Re,
            "im" => Self::Im,
            _ => return None,
        })
    }

    fn unit(self, base: &'static str) -> &'static str {
        match self {
            Self::Db => "dB",
            Self::PhaseDeg => "deg",
            _ => base,
        }
    }
}

/// A sampled signal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waveform {
    /// Signal name as requested.
    pub signal: String,
    /// Unit of `y`.
    pub unit: &'static str,
    /// Name of the sweep axis (`time`, `frequency`, `v-sweep`, ...).
    pub x_name: String,
    /// Unit of `x`.
    pub x_unit: &'static str,
    /// Sweep values.
    pub x: Vec<f64>,
    /// Signal values.
    pub y: Vec<f64>,
    /// Number of stored points in the requested range before downsampling.
    pub source_points: usize,
    /// Whether `x`/`y` were downsampled to fit the point budget.
    pub downsampled: bool,
}

/// Values of `name` (a variable, or `v(a,b)` for a differential voltage) as complex pairs.
fn column(a: &AnalysisResult, name: &str) -> Option<(Vec<(f64, f64)>, Quantity)> {
    let name = name.trim().to_ascii_lowercase();
    let get = |n: &str| -> Option<(Vec<(f64, f64)>, Quantity)> {
        if n == "v(0)" || n == "v(gnd)" {
            return Some((vec![(0.0, 0.0); a.len()], Quantity::Voltage));
        }
        let i = a.index_of(n)?;
        let q = a.variables[i].quantity;
        let v = match &a.data {
            Data::Real { columns } => columns[i].iter().map(|&x| (x, 0.0)).collect(),
            Data::Complex { columns } => columns[i].clone(),
        };
        Some((v, q))
    };
    if let Some(inner) = name.strip_prefix("v(").and_then(|s| s.strip_suffix(')'))
        && let Some((p, n)) = inner.split_once(',')
    {
        let (vp, q) = get(&format!("v({})", p.trim()))?;
        let (vn, _) = get(&format!("v({})", n.trim()))?;
        let d = vp
            .iter()
            .zip(&vn)
            .map(|(a, b)| (a.0 - b.0, a.1 - b.1))
            .collect();
        return Some((d, q));
    }
    get(&name)
}

/// Extract a signal over `[from, to]` and downsample it to at most `max_points` points.
pub(crate) fn extract(
    a: &AnalysisResult,
    signal: &str,
    reduce: Reduce,
    from: Option<f64>,
    to: Option<f64>,
    max_points: usize,
) -> Option<Waveform> {
    let (vals, q) = column(a, signal)?;
    let complex = matches!(a.data, Data::Complex { .. });
    let reduce = match (complex, reduce) {
        (false, Reduce::Mag | Reduce::Db | Reduce::PhaseDeg | Reduce::Re | Reduce::Im) => reduce,
        (true, Reduce::Real) => Reduce::Mag,
        (true, Reduce::Auto) if a.kind == AnalysisKind::Pz => Reduce::Re,
        (true, Reduce::Auto) if a.kind == AnalysisKind::Sens => Reduce::Mag,
        (true, Reduce::Auto) => Reduce::Db,
        (false, Reduce::Auto) => Reduce::Real,
        _ => reduce,
    };
    let y_all: Vec<f64> = vals
        .iter()
        .map(|&(re, im)| match reduce {
            Reduce::Real | Reduce::Re | Reduce::Auto => re,
            Reduce::Im => im,
            Reduce::Mag => re.hypot(im),
            Reduce::Db => 20.0 * re.hypot(im).log10(),
            Reduce::PhaseDeg => im.atan2(re).to_degrees(),
        })
        .collect();
    let x_all: Vec<f64> = if a.scale.is_none() {
        vec![0.0; y_all.len()]
    } else {
        a.scale_values.clone()
    };
    let (x_name, x_unit) = match &a.scale {
        Some(v) => (v.name.clone(), v.quantity.unit()),
        None => ("index".to_string(), ""),
    };
    let lo = from.unwrap_or(f64::NEG_INFINITY);
    let hi = to.unwrap_or(f64::INFINITY);
    let (x, y): (Vec<f64>, Vec<f64>) = x_all
        .iter()
        .zip(&y_all)
        .filter(|&(&x, _)| x >= lo && x <= hi)
        .map(|(&x, &y)| (x, y))
        .unzip();
    let source_points = x.len();
    let budget = max_points.max(3);
    let (x, y, downsampled) = if x.len() > budget {
        // AC sweeps are logarithmic: choose buckets on a log axis.
        let log =
            matches!(a.kind, AnalysisKind::Ac | AnalysisKind::Noise) && x.iter().all(|&v| v > 0.0);
        let keep = lttb(&x, &y, budget, log);
        (
            keep.iter().map(|&i| x[i]).collect(),
            keep.iter().map(|&i| y[i]).collect(),
            true,
        )
    } else {
        (x, y, false)
    };
    Some(Waveform {
        signal: signal.trim().to_ascii_lowercase(),
        unit: reduce.unit(q.unit()),
        x_name,
        x_unit,
        x,
        y,
        source_points,
        downsampled,
    })
}

/// Indices selected by LTTB (`n_out >= 3`, `x.len() > n_out`).
pub(crate) fn lttb(x: &[f64], y: &[f64], n_out: usize, log_x: bool) -> Vec<usize> {
    let n = x.len();
    let xs: Vec<f64> = if log_x {
        x.iter().map(|v| v.log10()).collect()
    } else {
        x.to_vec()
    };
    let mut out = Vec::with_capacity(n_out);
    out.push(0);
    let buckets = n_out - 2;
    let width = (n - 2) as f64 / buckets as f64;
    let mut a = 0usize;
    for b in 0..buckets {
        let start = 1 + (b as f64 * width) as usize;
        let end = (1 + ((b + 1) as f64 * width) as usize).min(n - 1);
        // Average of the next bucket (or the last point).
        let (ns, ne) = if b + 1 < buckets {
            (end, (1 + ((b + 2) as f64 * width) as usize).min(n - 1))
        } else {
            (n - 1, n)
        };
        let cnt = (ne - ns).max(1) as f64;
        let ax: f64 = xs[ns..ne].iter().sum::<f64>() / cnt;
        let ay: f64 = y[ns..ne].iter().sum::<f64>() / cnt;
        let (px, py) = (xs[a], y[a]);
        let mut best = start;
        let mut best_area = -1.0;
        for i in start..end.max(start + 1) {
            let area = ((px - ax) * (y[i] - py) - (px - xs[i]) * (ay - py)).abs();
            if area > best_area {
                best_area = area;
                best = i;
            }
        }
        out.push(best);
        a = best;
    }
    out.push(n - 1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lttb_keeps_ends_and_peak() {
        let x: Vec<f64> = (0..1000).map(f64::from).collect();
        let y: Vec<f64> = x
            .iter()
            .map(|&v| if v == 500.0 { 10.0 } else { 0.0 })
            .collect();
        let k = lttb(&x, &y, 20, false);
        assert_eq!(k.len(), 20);
        assert_eq!(k[0], 0);
        assert_eq!(k[19], 999);
        assert!(k.contains(&500));
        assert!(k.windows(2).all(|w| w[0] < w[1]));
    }
}
