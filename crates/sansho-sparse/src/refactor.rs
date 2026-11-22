//! Fixed-pattern left-looking LU for repeated real Newton updates.
//! Provenance: original implementation from Gilbert and Peierls (1988) and Davis
//! (2006), using the circuit block solver for numeric factorization. The optional
//! mixed-precision backend is classical iterative refinement (Wilkinson 1963; Moler,
//! J. ACM 14(2), 1967) with the componentwise backward error of Oettli and Prager
//! (Numer. Math. 6, 1964) as stopping test, as described in Higham, *Accuracy and
//! Stability of Numerical Algorithms*, 2nd ed., §12.

use crate::{CscPattern, SerialLu, SolverError};

/// Real Newton solver.
#[derive(Debug)]
pub struct NewtonLu {
    backend: Backend,
    dim: usize,
    ready: bool,
}

#[derive(Debug)]
enum Backend {
    Serial(Box<SerialLu>),
    Btf(Box<crate::btf::Btf>, CscPattern),
    Mixed(Box<Mixed>),
}

/// Refinement steps before falling back to a double-precision factorization.
const REFINE_STEPS: usize = 6;
/// Componentwise backward error accepted as converged.
const BACKWARD_ERROR: f64 = 8.0 * f64::EPSILON;

/// Single-precision factors refined to double-precision accuracy.
///
/// Every solve is refined with double-precision residuals until the componentwise
/// backward error is at the level of double rounding. A matrix too ill-conditioned for
/// that (roughly cond(A) > 1e7) is factored again in double precision, so accuracy
/// never depends on the low-precision factors.
#[derive(Debug)]
struct Mixed {
    low: SerialLu<f32>,
    exact: SerialLu<f64>,
    input: std::sync::Arc<crate::BatchOrdering>,
    values: Vec<f64>,
    low_values: Vec<f32>,
    /// The current matrix needed the double-precision factors.
    use_exact: bool,
    exact_ready: bool,
    low_ready: bool,
    /// Factorizations that skip the low-precision attempt after recent fallbacks.
    skip: u32,
    backoff: u32,
    stats: MixedStats,
}

/// Counters of the mixed-precision backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MixedStats {
    /// Solves finished by refinement.
    pub refined: u64,
    /// Refinement steps over all refined solves.
    pub steps: u64,
    /// Solves that used double-precision factors, after a failed refinement or while
    /// recent failures suspend the low-precision attempt.
    pub fallbacks: u64,
}

impl Drop for Mixed {
    fn drop(&mut self) {
        if std::env::var_os("SANSHO_PROFILE_MIXED").is_some() {
            use std::io::Write;
            let s = self.stats;
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-mixed refined={} steps={} fallbacks={}",
                s.refined,
                s.steps,
                s.fallbacks
            );
        }
    }
}

impl Mixed {
    fn new(order: std::sync::Arc<crate::BatchOrdering>) -> Result<Self, SolverError> {
        Ok(Self {
            low: SerialLu::from_ordering(order.clone())?,
            exact: SerialLu::from_ordering(order.clone())?,
            input: order,
            values: Vec::new(),
            low_values: Vec::new(),
            use_exact: false,
            exact_ready: false,
            low_ready: false,
            skip: 0,
            backoff: 0,
            stats: MixedStats::default(),
        })
    }

    fn factor(&mut self, values: &[f64]) -> Result<bool, SolverError> {
        self.values.clear();
        self.values.extend_from_slice(values);
        self.low_values.clear();
        self.low_values.extend(values.iter().map(|&v| v as f32));
        self.exact_ready = false;
        if self.skip > 0 {
            // Recent matrices needed double factors; Newton matrices change slowly.
            self.skip -= 1;
            self.low_ready = false;
            self.use_exact = true;
            return self.factor_exact();
        }
        let representable = values
            .iter()
            .zip(&self.low_values)
            .all(|(&v, &l)| !v.is_finite() || l.is_finite());
        self.low_ready = representable && self.low.factor(&self.low_values).is_ok();
        self.use_exact = !self.low_ready;
        if self.use_exact {
            self.back_off();
            return self.factor_exact();
        }
        Ok(false)
    }

    /// Skip exponentially more low-precision attempts after each failure.
    fn back_off(&mut self) {
        self.backoff = (2 * self.backoff + 1).min(63);
        self.skip = self.backoff;
    }

    fn factor_exact(&mut self) -> Result<bool, SolverError> {
        let reused = self.exact.factor(&self.values)?;
        self.exact_ready = true;
        Ok(reused)
    }

    /// `r = b - A x` and the componentwise backward error max |r| / (|A||x| + |b|).
    fn residual(&self, b: &[f64], x: &[f64], r: &mut [f64], scale: &mut [f64]) -> f64 {
        r.copy_from_slice(b);
        for (s, v) in scale.iter_mut().zip(b) {
            *s = v.abs();
        }
        for (col, entries) in self.input.input.iter().enumerate() {
            let xc = x[col];
            for &(row, k) in entries {
                let a = self.values[k];
                r[row] -= a * xc;
                scale[row] += (a * xc).abs();
            }
        }
        r.iter().zip(scale.iter()).fold(0.0, |w: f64, (&ri, &si)| {
            if ri == 0.0 {
                w
            } else if si > 0.0 {
                w.max(ri.abs() / si)
            } else {
                f64::INFINITY
            }
        })
    }

    fn solve(&mut self, rhs: &mut [f64]) -> Result<(), SolverError> {
        if !self.use_exact {
            let b = rhs.to_vec();
            let n = b.len();
            let mut low: Vec<f32> = b.iter().map(|&v| v as f32).collect();
            if self.low.solve(&mut low).is_ok() {
                let mut x: Vec<f64> = low.iter().map(|&v| f64::from(v)).collect();
                let (mut r, mut scale) = (vec![0.0; n], vec![0.0; n]);
                let mut previous = f64::INFINITY;
                for step in 0..=REFINE_STEPS {
                    let omega = self.residual(&b, &x, &mut r, &mut scale);
                    if omega <= BACKWARD_ERROR {
                        rhs.copy_from_slice(&x);
                        self.stats.refined += 1;
                        self.stats.steps += step as u64;
                        self.backoff = 0;
                        return Ok(());
                    }
                    let stalled = omega.is_nan() || omega >= 0.5 * previous;
                    if step == REFINE_STEPS || stalled {
                        break;
                    }
                    previous = omega;
                    for (l, &v) in low.iter_mut().zip(&r) {
                        *l = v as f32;
                    }
                    if self.low.solve(&mut low).is_err() {
                        break;
                    }
                    for (xi, &d) in x.iter_mut().zip(&low) {
                        *xi += f64::from(d);
                    }
                }
            }
            // Keep the double factors for the remaining solves of this matrix and
            // skip exponentially more low-precision attempts.
            self.use_exact = true;
            self.back_off();
        }
        if !self.exact_ready {
            self.factor_exact()?;
        }
        self.stats.fallbacks += 1;
        self.exact.solve(rhs)
    }
}

impl NewtonLu {
    /// Start a fresh numeric solve using the batch's immutable column ordering.
    pub fn from_ordering(order: std::sync::Arc<crate::BatchOrdering>) -> Result<Self, SolverError> {
        let dim = order.dimension();
        let backend = if mixed_requested()? {
            Backend::Mixed(Box::new(Mixed::new(order)?))
        } else {
            Backend::Serial(Box::new(crate::SerialLu::from_ordering(order)?))
        };
        Ok(Self {
            backend,
            dim,
            ready: false,
        })
    }

    /// Mixed-precision counters, if this solver uses that backend.
    pub fn mixed_stats(&self) -> Option<MixedStats> {
        match &self.backend {
            Backend::Mixed(m) => Some(m.stats),
            _ => None,
        }
    }
    /// Prepare the column ordering and allocate pivoting workspace.
    pub fn new(pattern: &CscPattern) -> Result<Self, SolverError> {
        let n = pattern.dim();
        if mixed_requested()? {
            return Ok(Self {
                backend: Backend::Mixed(Box::new(Mixed::new(crate::BatchOrdering::new(pattern)?)?)),
                dim: n,
                ready: false,
            });
        }
        let selection = std::env::var("SANSHO_REAL_SOLVER").unwrap_or_default();
        let auto = matches!(selection.as_str(), "" | "auto");
        if (selection == "btf" || auto && n >= 2048 && pattern.nnz() <= n.saturating_mul(16))
            && let Some(btf) = crate::btf::Btf::new(pattern, selection == "btf")?
        {
            return Ok(Self {
                backend: Backend::Btf(Box::new(btf), pattern.clone()),
                dim: n,
                ready: false,
            });
        }
        Self::unblocked(pattern)
    }

    fn unblocked(pattern: &CscPattern) -> Result<Self, SolverError> {
        Ok(Self {
            backend: Backend::Serial(Box::new(SerialLu::new(pattern)?)),
            dim: pattern.dim(),
            ready: false,
        })
    }

    /// Original column eliminated last by the simplicial Newton factorization.
    pub fn last_column(&self) -> Option<usize> {
        match &self.backend {
            Backend::Serial(s) => s.last_column(),
            Backend::Mixed(m) => m.exact.last_column(),
            Backend::Btf(..) => None,
        }
    }

    /// Factor a new numeric assembly, reverting to partial pivoting if needed.
    pub fn factor(&mut self, values: &[f64]) -> Result<(), SolverError> {
        self.factor_with_status(values).map(|_| ())
    }

    /// Factor and report whether the saved pivot sequence was reused.
    pub fn factor_with_status(&mut self, values: &[f64]) -> Result<bool, SolverError> {
        self.ready = false;
        if let Backend::Btf(b, pattern) = &self.backend
            && !b.supports(values)
        {
            *self = Self::unblocked(pattern)?;
        }
        let result = match &mut self.backend {
            Backend::Btf(b, _) => b.factor(values),
            Backend::Serial(g) => g.factor(values),
            Backend::Mixed(m) => m.factor(values),
        };
        self.ready = result.is_ok();
        result
    }

    /// Apply the two triangular solves in place.
    pub fn solve(&mut self, rhs: &mut [f64]) -> Result<(), SolverError> {
        if !self.ready {
            return Err(SolverError::NotReady);
        }
        if rhs.len() != self.dim {
            return Err(SolverError::DimensionMismatch {
                expected: self.dim,
                actual: rhs.len(),
            });
        }
        match &mut self.backend {
            Backend::Btf(b, _) => b.solve(rhs),
            Backend::Serial(g) => g.solve(rhs),
            Backend::Mixed(m) => m.solve(rhs),
        }
    }
}

/// Whether `SANSHO_REAL_SOLVER` selects the experimental mixed-precision backend.
fn mixed_requested() -> Result<bool, SolverError> {
    let selection = std::env::var("SANSHO_REAL_SOLVER").unwrap_or_default();
    match selection.as_str() {
        "mixed" => Ok(true),
        "" | "auto" | "newton" | "btf" => Ok(false),
        _ => Err(SolverError::Backend(format!(
            "unknown SANSHO_REAL_SOLVER value {selection:?} (expected auto, newton, btf or mixed)"
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PatternBuilder;

    /// Tridiagonal system with an embedded 2x2 block [[1, 1], [1, 1 + eps]], so the
    /// condition number grows like 1/eps.
    fn chain(n: usize, eps: f64) -> (CscPattern, Vec<f64>) {
        let k = n / 2;
        let block = |i: usize, j: usize| -> Option<f64> {
            match (i, j) {
                (i, j) if i == k && j == k => Some(1.0),
                (i, j) if (i == k && j == k + 1) || (i == k + 1 && j == k) => Some(1.0),
                (i, j) if i == k + 1 && j == k + 1 => Some(1.0 + eps),
                (i, j) if (i == k || i == k + 1 || j == k || j == k + 1) && i != j => Some(0.0),
                _ => None,
            }
        };
        let mut b = PatternBuilder::new(n);
        let mut entries = Vec::new();
        for i in 0..n {
            let d = block(i, i).unwrap_or(4.0 + 1e-3 * i as f64);
            entries.push((b.add(i, i), d));
            if i + 1 < n {
                entries.push((b.add(i, i + 1), block(i, i + 1).unwrap_or(-1.0)));
                entries.push((
                    b.add(i + 1, i),
                    block(i + 1, i).unwrap_or(-1.0 + 1e-4 * i as f64),
                ));
            }
        }
        let (pattern, map) = b.build();
        let mut values = vec![0.0; pattern.nnz()];
        for (slot, v) in entries {
            values[map[slot]] += v;
        }
        (pattern, values)
    }

    fn reference(pattern: &CscPattern, values: &[f64], rhs: &[f64]) -> Vec<f64> {
        let mut lu = SerialLu::<f64>::new(pattern).unwrap();
        lu.factor(values).unwrap();
        let mut x = rhs.to_vec();
        lu.solve(&mut x).unwrap();
        x
    }

    #[test]
    fn mixed_refinement_reaches_double_accuracy_or_falls_back() {
        for (weak, refined) in [(1.0, true), (1e-11, false)] {
            let (pattern, values) = chain(40, weak);
            let rhs: Vec<f64> = (0..40).map(|i| (i as f64 * 0.37).sin()).collect();
            let want = reference(&pattern, &values, &rhs);
            let mut m = Mixed::new(crate::BatchOrdering::new(&pattern).unwrap()).unwrap();
            m.factor(&values).unwrap();
            let mut x = rhs.clone();
            m.solve(&mut x).unwrap();
            let scale = want.iter().fold(0f64, |a, v| a.max(v.abs()));
            let err = x
                .iter()
                .zip(&want)
                .fold(0f64, |a, (p, q)| a.max((p - q).abs()));
            assert!(
                err <= 1e-12 * scale,
                "weak {weak}: error {err:e} of {scale:e}"
            );
            assert_eq!(m.stats.refined == 1, refined, "{:?}", m.stats);
            assert_eq!(m.stats.fallbacks == 1, !refined, "{:?}", m.stats);
        }
    }

    #[test]
    fn mixed_backoff_skips_low_precision_after_fallbacks() {
        let (pattern, values) = chain(40, 1e-11);
        let mut m = Mixed::new(crate::BatchOrdering::new(&pattern).unwrap()).unwrap();
        for _ in 0..8 {
            m.factor(&values).unwrap();
            let mut x = vec![1.0; 40];
            m.solve(&mut x).unwrap();
        }
        // Attempts at factorizations 1, 2 and 5; the others reuse double factors directly.
        assert_eq!(m.stats.fallbacks, 8);
        assert_eq!(m.backoff, 7);
    }

    #[test]
    fn reuses_and_replaces_pivots_against_fresh_factorizations() {
        let mut pattern = PatternBuilder::new(9);
        for j in 0..9 {
            for i in [j, (j + 1) % 9, (j + 5) % 9, 8] {
                pattern.add(i, j);
            }
        }
        let (pattern, _) = pattern.build();
        let mut solver = NewtonLu::new(&pattern).unwrap();

        for iter in 0..80 {
            let mut a = vec![0.; pattern.nnz()];
            for j in 0..9 {
                for (k, entry) in a
                    .iter_mut()
                    .enumerate()
                    .take(pattern.col_ptr()[j + 1])
                    .skip(pattern.col_ptr()[j])
                {
                    let i = pattern.row_idx()[k];
                    let scale = if (iter / 10) % 2 == 0 && i == j
                        || (iter / 10) % 2 == 1 && i == (j + 1) % 9
                    {
                        9.
                    } else {
                        0.4
                    };
                    *entry = scale + ((j + k + 3 * iter) as f64).sin() * 0.22;
                }
            }
            let mut x: Vec<_> = (0..9).map(|i| ((i + iter) as f64).cos()).collect();
            let mut other = x.clone();
            solver.factor(&a).unwrap();
            solver.solve(&mut x).unwrap();
            let mut original = SerialLu::new(&pattern).unwrap();
            original.factor(&a).unwrap();
            original.solve(&mut other).unwrap();
            assert!(
                x.iter().zip(other).all(|(x, y)| (x - y).abs() < 1e-13),
                "assembly {iter}"
            );
        }
        // Entries can become active after the plan has been recorded.
        let mut a = vec![0.; pattern.nnz()];
        for col in 0..9 {
            let entries = pattern.col_ptr()[col]..pattern.col_ptr()[col + 1];
            for (&row, value) in pattern.row_idx()[entries.clone()]
                .iter()
                .zip(&mut a[entries])
            {
                if row == col {
                    *value = 3.;
                }
            }
        }
        let mut sparse = NewtonLu::new(&pattern).unwrap();
        for k in 0..a.len() {
            if a[k] == 0. {
                a[k] = 0.125;
            }
            for _ in 0..2 {
                let mut x = [1.; 9];
                let mut reference = x;
                sparse.factor(&a).unwrap();
                sparse.solve(&mut x).unwrap();
                let mut original = SerialLu::new(&pattern).unwrap();
                original.factor(&a).unwrap();
                original.solve(&mut reference).unwrap();
                assert!(x.iter().zip(reference).all(|(x, y)| (x - y).abs() < 1e-13));
            }
        }
        a[0] = f64::INFINITY;
        assert!(sparse.factor(&a).is_err());
        assert!(solver.factor(&[f64::NAN]).is_err());
        assert!(solver.solve(&mut [0.; 9]).is_err());
    }
}
