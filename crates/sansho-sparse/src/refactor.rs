//! Fixed-pattern left-looking LU for repeated real Newton updates.
//! Provenance: original implementation from Gilbert and Peierls (1988) and Davis
//! (2006), using the circuit block solver for numeric factorization.

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
}

impl NewtonLu {
    /// Start a fresh numeric solve using the batch's immutable column ordering.
    pub fn from_ordering(order: std::sync::Arc<crate::BatchOrdering>) -> Result<Self, SolverError> {
        let dim = order.dimension();
        Ok(Self {
            backend: Backend::Serial(Box::new(crate::SerialLu::from_ordering(order)?)),
            dim,
            ready: false,
        })
    }
    /// Prepare the column ordering and allocate pivoting workspace.
    pub fn new(pattern: &CscPattern) -> Result<Self, SolverError> {
        let n = pattern.dim();
        let selection = std::env::var("SANSHO_REAL_SOLVER").unwrap_or_default();
        let auto = matches!(selection.as_str(), "" | "auto");
        if !auto && !matches!(selection.as_str(), "newton" | "btf") {
            return Err(SolverError::Backend(format!(
                "unknown SANSHO_REAL_SOLVER value {selection:?} (expected auto, newton or btf)"
            )));
        }
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
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PatternBuilder;

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
