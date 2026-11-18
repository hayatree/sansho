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
    // Columns with only one active input need no scatter or elimination.
    direct: Vec<(usize, usize)>,
    coupled: Vec<usize>,
    forward: Vec<usize>,
    // CSC entries that were zero when the symbolic update schedule was recorded.
    zero_mask: Vec<u64>,
    input: Vec<(usize, usize)>,
    updates: Vec<Range<usize>>,
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
    peak_index: usize,
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
            // A byte addresses the whole small-system workspace. This permits
            // bounds-check-free indexed updates without unsafe memory access.
            work: vec![0.; n.max(256)],
            plan: Plan::default(),
            peak_index: 0,
        }))
    }

    fn factor(&mut self, values: &[f64]) -> Result<(), SolverError> {
        if values.len() != self.matrix.row_idx().len() {
            return Err(SolverError::DimensionMismatch {
                expected: self.matrix.row_idx().len(),
                actual: values.len(),
            });
        }
        // A single entry gives a lower bound for the input peak. Every active
        // entry flows to a checked pivot, U or L candidate through only sums
        // and products. A successful refactor therefore also proves finiteness.
        if !self.plan.cols.is_empty() {
            let floor = values[self.peak_index].abs();
            if floor.is_finite()
                && self
                    .plan
                    .refactor(values, &mut self.work, (floor * 1e8).min(f64::MAX))
            {
                return Ok(());
            }
        }
        let Some((index, peak)) = values
            .iter()
            .enumerate()
            .max_by_key(|(_, x)| x.abs().to_bits())
        else {
            return Err(SolverError::Singular { column: None });
        };
        if !peak.is_finite() {
            return Err(SolverError::Backend("non-finite matrix entry".into()));
        }
        if index != self.peak_index
            && !self.plan.cols.is_empty()
            && self
                .plan
                .refactor(values, &mut self.work, (peak.abs() * 1e8).min(f64::MAX))
        {
            self.peak_index = index;
            return Ok(());
        }
        self.peak_index = index;
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
        for &j in &p.forward {
            let c = &p.cols[j];
            let v = x[j];
            subtract_rows(
                x,
                &p.lower_row[c.lower.clone()],
                &p.lower[c.lower.clone()],
                v,
            );
        }
        for (j, c) in p.cols.iter().enumerate().rev() {
            let v = x[j] * p.diagonal[j];
            x[j] = v;
            subtract_rows(
                x,
                &p.upper_row[c.upper.clone()],
                &p.upper[c.upper.clone()],
                v,
            );
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

#[inline]
fn subtract_rows(x: &mut [f64], rows: &[usize], factors: &[f64], value: f64) {
    if let Ok(small) = <&mut [f64; 256]>::try_from(&mut *x) {
        for (&r, &factor) in rows.iter().zip(factors) {
            small[usize::from(r as u8)] -= factor * value;
        }
    } else {
        for (&r, &factor) in rows.iter().zip(factors) {
            x[r] -= factor * value;
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
        let dump =
            self.cols.is_empty() && std::env::var("SPX_PROFILE_MATRIX").as_deref() == Ok("entries");
        if dump {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "spx-csc {:?} {:?} {:?}",
                a.symbolic().col_ptr(),
                a.symbolic().row_idx(),
                a.val()
            );
        }
        self.cols.clear();
        self.direct.clear();
        self.coupled.clear();
        self.forward.clear();
        self.lower_row.clear();
        self.lower.clear();
        self.upper_row.clear();
        self.upper.clear();
        self.diagonal.clear();
        self.input.clear();
        self.zero_mask.resize(a.val().len(), u64::MAX);
        for (mask, &v) in self.zero_mask.iter_mut().zip(a.val()) {
            if v.to_bits() != 0 {
                *mask = 0;
            }
        }
        self.updates.clear();
        let l = lu.l_factor_unsorted();
        let u = lu.u_factor_unsorted();
        // Overflow in fresh pivoting can contaminate even structural zeroes
        // through 0 * infinity. Keep those entries for the singular solve.
        if (0..order.len()).any(|j| {
            l.val_of_col(j)
                .iter()
                .chain(u.val_of_col(j))
                .any(|v| !v.is_finite())
        }) {
            self.zero_mask.fill(0);
        }
        let mut present = vec![false; order.len()];
        for (j, &orig) in order.iter().enumerate() {
            let start = self.input.len();
            for k in a.symbolic().col_range(orig) {
                if self.zero_mask[k] == 0 {
                    let r = inverse[a.symbolic().row_idx()[k]];
                    present[r] = true;
                    self.input.push((k, r));
                }
            }
            // Each reached upper entry updates exactly the lower column recorded
            // for its pivot. Keep faer's visitation order for every subtraction.
            let upper = self.upper.len();
            for (i, &v) in u.row_idx_of_col(j).zip(u.val_of_col(j)) {
                if i != j && present[i] {
                    present[i] = false;
                    let rows = self.cols[i].lower.clone();
                    for &r in &self.lower_row[rows.clone()] {
                        present[r] = true;
                    }
                    self.updates.push(rows);
                    self.upper_row.push(i);
                    self.upper.push(v);
                }
            }
            let lower = self.lower.len();
            for (i, &v) in l.row_idx_of_col(j).zip(l.val_of_col(j)) {
                if i != j && present[i] {
                    present[i] = false;
                    self.lower_row.push(i);
                    self.lower.push(v);
                }
            }
            present[j] = false;
            let diag = u
                .row_idx_of_col(j)
                .zip(u.val_of_col(j))
                .find_map(|(i, &v)| (i == j).then_some(v))
                .unwrap_or(0.);
            self.diagonal.push(1. / diag);
            if lower != self.lower.len() {
                self.forward.push(j);
            }
            if lower == self.lower.len()
                && upper == self.upper.len()
                && let [(src, row)] = self.input[start..]
                && row == j
            {
                self.direct.push((j, src));
            } else {
                self.coupled.push(j);
            }
            self.cols.push(Column {
                input: start..self.input.len(),
                lower: lower..self.lower.len(),
                upper: upper..self.upper.len(),
            });
        }
        if std::env::var_os("SPX_PROFILE_MATRIX").is_some() {
            let n = order.len();
            let nnz = a.val().len();
            let full: usize = (0..n)
                .map(|j| l.val_of_col(j).len() + u.val_of_col(j).len() - 1)
                .sum();
            let active = self.lower.len() + self.upper.len() + n;
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "spx-matrix n={n} A={nnz} LU={full} fill={} active_A={} active_LU={active}",
                full - nnz,
                self.input.len()
            );
        }
    }

    fn refactor(&mut self, values: &[f64], x: &mut [f64], growth_limit: f64) -> bool {
        if self
            .zero_mask
            .iter()
            .zip(values)
            .fold(0, |bits, (&mask, v)| bits | (mask & v.to_bits()))
            != 0
        {
            return false;
        }
        for &(j, src) in &self.direct {
            let diag = values[src];
            if !diag.is_normal() || diag.abs() > growth_limit {
                return false;
            }
            self.diagonal[j] = 1. / diag;
        }
        x.fill(0.);
        for &j in &self.coupled {
            let c = &self.cols[j];
            for &(src, dst) in &self.input[c.input.clone()] {
                x[dst] = values[src];
            }
            for ((&i, u), update) in self.upper_row[c.upper.clone()]
                .iter()
                .zip(&mut self.upper[c.upper.clone()])
                .zip(&self.updates[c.upper.clone()])
            {
                let v = std::mem::take(&mut x[i]);
                if v.abs().to_bits() > growth_limit.to_bits() {
                    return false;
                }
                *u = v;
                subtract_rows(
                    x,
                    &self.lower_row[update.clone()],
                    &self.lower[update.clone()],
                    v,
                );
            }
            let diag = std::mem::take(&mut x[j]);
            if !diag.is_normal() || diag.abs() > growth_limit {
                return false;
            }
            let reciprocal = 1. / diag;
            self.diagonal[j] = reciprocal;
            for (&r, l) in self.lower_row[c.lower.clone()]
                .iter()
                .zip(&mut self.lower[c.lower.clone()])
            {
                let v = std::mem::take(&mut x[r]);
                // A tie may have a different first candidate in the pivot search.
                // Bounding every multiplier also catches growth that invalidates the saved pivot.
                if v.abs().to_bits() >= diag.abs().to_bits() {
                    return false;
                }
                *l = v * reciprocal;
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
                original.factor(&a).unwrap();
                original.solve(&mut reference).unwrap();
                assert_eq!(x.map(f64::to_bits), reference.map(f64::to_bits));
            }
        }
        a[0] = f64::INFINITY;
        assert!(sparse.factor(&a).is_err());
        assert!(solver.factor(&[f64::NAN]).is_err());
        assert!(solver.solve(&mut [0.; 9]).is_err());
    }
}
