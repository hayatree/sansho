//! Fixed-pattern left-looking LU for repeated real Newton updates.
//! Provenance: original implementation from Gilbert and Peierls (1988) and Davis
//! (2006), using faer's public ordering and pivoted simplicial factor interfaces.

use crate::{CscPattern, FaerLu, LinearSolver, SolverError};
use faer::dyn_stack::{MemBuffer, MemStack};
use faer::sparse::linalg::lu::{factorize_symbolic_lu, simplicial};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMat};
use std::ops::Range;

/// Real Newton solver. Dense systems keep the backend's block kernels.
#[derive(Debug)]
pub struct NewtonLu {
    backend: Backend,
    dim: usize,
    ready: bool,
}

#[derive(Debug)]
enum Backend {
    Sparse(Box<Sparse>),
    General(Box<FaerLu<f64>>),
}

impl NewtonLu {
    /// Prepare the column ordering and allocate pivoting workspace.
    pub fn new(pattern: &CscPattern) -> Result<Self, SolverError> {
        let n = pattern.dim();
        let sparse = if n > 0 && pattern.nnz() <= n.saturating_mul(16) {
            Sparse::new(pattern)?
        } else {
            None
        };
        let backend = if let Some(sparse) = sparse {
            Backend::Sparse(Box::new(sparse))
        } else {
            let mut lu = FaerLu::new();
            lu.analyze(pattern)?;
            Backend::General(Box::new(lu))
        };
        Ok(Self {
            backend,
            dim: n,
            ready: false,
        })
    }

    /// Factor a new numeric assembly, reverting to partial pivoting if needed.
    pub fn factor(&mut self, values: &[f64]) -> Result<(), SolverError> {
        let result = match &mut self.backend {
            Backend::Sparse(s) => s.factor(values),
            Backend::General(s) => s.factor(values),
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
            Backend::Sparse(s) => s.solve(rhs),
            Backend::General(s) => s.solve(rhs),
        }
    }
}

#[derive(Debug)]
struct Column {
    input: Range<usize>,
    lower: Range<usize>,
    upper: Range<usize>,
}

#[derive(Debug, Default)]
struct Plan {
    cols: Vec<Column>,
    scatter: Vec<usize>,
    lower_row: Vec<usize>,
    lower: Vec<f64>,
    upper_row: Vec<usize>,
    upper: Vec<f64>,
    diagonal: Vec<f64>,
}

struct Sparse {
    matrix: SymbolicSparseColMat<usize>,
    order: faer::sparse::linalg::lu::SymbolicLu<usize>,
    pivoted: simplicial::SimplicialLu<usize, f64>,
    row: Vec<usize>,
    inverse: Vec<usize>,
    stack: MemBuffer,
    work: Vec<f64>,
    plan: Plan,
}

impl std::fmt::Debug for Sparse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sparse")
            .field("dimension", &self.row.len())
            .finish_non_exhaustive()
    }
}

impl Sparse {
    fn new(pattern: &CscPattern) -> Result<Option<Self>, SolverError> {
        let n = pattern.dim();
        let matrix = SymbolicSparseColMat::new_checked(
            n,
            n,
            pattern.col_ptr().to_vec(),
            None,
            pattern.row_idx().to_vec(),
        );
        let order = factorize_symbolic_lu(matrix.as_ref(), Default::default())
            .map_err(|e| SolverError::Backend(format!("{e:?}")))?;
        let req = simplicial::factorize_simplicial_numeric_lu_scratch::<usize, f64>(n, n);
        // faer 0.24's block factorization additionally reserves the transposed
        // matrix and block work arrays. Its workspace is strictly larger for
        // our square patterns with a structural diagonal. Keep that backend
        // whenever the automatic symbolic analysis requests a different layout.
        let auto = order.factorize_numeric_lu_scratch::<f64>(faer::Par::Seq, Default::default());
        if (auto.size_bytes(), auto.align_bytes()) != (req.size_bytes(), req.align_bytes()) {
            return Ok(None);
        }
        let stack = MemBuffer::try_new(req).map_err(|e| SolverError::Backend(format!("{e:?}")))?;
        Ok(Some(Self {
            matrix,
            order,
            pivoted: simplicial::SimplicialLu::new(),
            row: vec![0; n],
            inverse: vec![0; n],
            stack,
            work: vec![0.; n],
            plan: Plan::default(),
        }))
    }

    fn factor(&mut self, values: &[f64]) -> Result<(), SolverError> {
        if values.len() != self.matrix.row_idx().len() {
            return Err(SolverError::DimensionMismatch {
                expected: self.matrix.row_idx().len(),
                actual: values.len(),
            });
        }
        // The signless IEEE encoding orders finite magnitudes below infinities and NaNs.
        let peak = f64::from_bits(values.iter().map(|x| x.abs().to_bits()).max().unwrap_or(0));
        if !peak.is_finite() {
            return Err(SolverError::Backend("non-finite matrix entry".into()));
        }
        if !self.plan.cols.is_empty()
            && self
                .plan
                .refactor(values, &mut self.work, (peak * 1e8).min(f64::MAX))
        {
            return Ok(());
        }
        let a = SparseColMatRef::new(self.matrix.as_ref(), values);
        let perm = self.order.col_perm();
        simplicial::factorize_simplicial_numeric_lu(
            &mut self.row,
            &mut self.inverse,
            &mut self.pivoted,
            a,
            perm,
            MemStack::new(&mut self.stack),
        )
        .map_err(|err| match err {
            faer::sparse::linalg::LuError::SymbolicSingular { index } => SolverError::Singular {
                column: Some(index),
            },
            e => SolverError::Backend(format!("{e:?}")),
        })?;
        self.plan
            .record(a, perm.arrays().0, &self.inverse, &self.pivoted);
        Ok(())
    }
    fn solve(&mut self, rhs: &mut [f64]) -> Result<(), SolverError> {
        let x = &mut self.work;
        for (v, &r) in x.iter_mut().zip(&self.row) {
            *v = rhs[r];
        }
        let p = &self.plan;
        for (j, c) in p.cols.iter().enumerate() {
            let v = x[j];
            for k in c.lower.clone() {
                x[p.lower_row[k]] -= p.lower[k] * v;
            }
        }
        for (j, c) in p.cols.iter().enumerate().rev() {
            let v = x[j] * (1. / p.diagonal[j]);
            x[j] = v;
            for k in c.upper.clone() {
                x[p.upper_row[k]] -= p.upper[k] * v;
            }
        }
        for (&v, &r) in x.iter().zip(self.order.col_perm().arrays().0) {
            rhs[r] = v;
        }
        if rhs.iter().all(|x| x.is_finite()) {
            Ok(())
        } else {
            Err(SolverError::Singular { column: None })
        }
    }
}

impl Plan {
    fn record(
        &mut self,
        a: SparseColMatRef<'_, usize, f64>,
        order: &[usize],
        inverse: &[usize],
        lu: &simplicial::SimplicialLu<usize, f64>,
    ) {
        self.cols.clear();
        self.lower_row.clear();
        self.lower.clear();
        self.upper_row.clear();
        self.upper.clear();
        self.diagonal.clear();
        self.scatter.clear();
        self.scatter
            .extend(a.symbolic().row_idx().iter().map(|&i| inverse[i]));
        let l = lu.l_factor_unsorted();
        let u = lu.u_factor_unsorted();
        for (j, &orig) in order.iter().enumerate() {
            let lower = self.lower.len();
            let upper = self.upper.len();
            for (i, &v) in l.row_idx_of_col(j).zip(l.val_of_col(j)) {
                if i != j {
                    self.lower_row.push(i);
                    self.lower.push(v);
                }
            }
            let mut diag = 0.;
            for (i, &v) in u.row_idx_of_col(j).zip(u.val_of_col(j)) {
                if i == j {
                    diag = v;
                } else {
                    self.upper_row.push(i);
                    self.upper.push(v);
                }
            }
            self.diagonal.push(diag);
            self.cols.push(Column {
                input: a.symbolic().col_range(orig),
                lower: lower..self.lower.len(),
                upper: upper..self.upper.len(),
            });
        }
    }

    fn refactor(&mut self, values: &[f64], x: &mut [f64], growth_limit: f64) -> bool {
        x.fill(0.);
        for (j, c) in self.cols.iter().enumerate() {
            for k in c.input.clone() {
                x[self.scatter[k]] = values[k];
            }
            for k in c.upper.clone() {
                let i = self.upper_row[k];
                let v = std::mem::take(&mut x[i]);
                if !v.is_finite() || v.abs() > growth_limit {
                    return false;
                }
                self.upper[k] = v;
                for h in self.cols[i].lower.clone() {
                    x[self.lower_row[h]] -= self.lower[h] * v;
                }
            }
            let diag = std::mem::take(&mut x[j]);
            if !diag.is_normal() || diag.abs() > growth_limit {
                return false;
            }
            self.diagonal[j] = diag;
            let reciprocal = 1. / diag;
            for k in c.lower.clone() {
                let v = std::mem::take(&mut x[self.lower_row[k]]);
                // A tie may have a different first candidate in the pivot search.
                // Bounding every multiplier also catches growth that invalidates the saved pivot.
                if !v.is_finite() || v.abs() >= diag.abs() {
                    return false;
                }
                self.lower[k] = v * reciprocal;
            }
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PatternBuilder;

    #[test]
    fn reuses_and_replaces_pivots_without_changing_the_solution_bits() {
        let mut pattern = PatternBuilder::new(9);
        for j in 0..9 {
            for i in [j, (j + 1) % 9, (j + 5) % 9, 8] {
                pattern.add(i, j);
            }
        }
        let (pattern, _) = pattern.build();
        let mut solver = NewtonLu::new(&pattern).unwrap();
        let mut original = FaerLu::<f64>::new();
        original.analyze(&pattern).unwrap();
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
            original.factor(&a).unwrap();
            original.solve(&mut other).unwrap();
            assert!(
                x.iter().zip(other).all(|(x, y)| x.to_bits() == y.to_bits()),
                "assembly {iter}"
            );
        }
        assert!(solver.factor(&[f64::NAN]).is_err());
        assert!(solver.solve(&mut [0.; 9]).is_err());
    }
}
