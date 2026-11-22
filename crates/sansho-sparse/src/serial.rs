//! Unblocked real and complex LU with shared symbolic ordering.
//! Provenance: original adapter for our Gilbert & Peierls (1988) factorization.

use crate::{BatchOrdering, CscPattern, LinearSolver, Scalar, SolverError, btf::Gp};
use std::sync::Arc;

#[derive(Debug)]
pub struct SerialLu<T: Scalar = f64> {
    gp: Gp<T>,
    nnz: usize,
    ready: bool,
}
impl<T: Scalar> SerialLu<T> {
    pub fn new(pattern: &CscPattern) -> Result<Self, SolverError> {
        Self::from_ordering(BatchOrdering::new(pattern)?)
    }
    pub fn from_ordering(order: Arc<BatchOrdering>) -> Result<Self, SolverError> {
        let nnz = order.input.iter().map(Vec::len).sum();
        Ok(Self {
            gp: Gp::new(order, false),
            nnz,
            ready: false,
        })
    }
    pub(crate) fn last_column(&self) -> Option<usize> {
        self.gp.symbolic.order.last().copied()
    }
    pub fn factor(&mut self, values: &[T]) -> Result<bool, SolverError> {
        let first = !self.ready;
        self.ready = false;
        check_len(self.nnz, values.len())?;
        let valid = self.gp.refactor(values);
        let reused = valid == self.gp.symbolic.dimension();
        if !reused {
            // A completed refactor already checked every pivot and every
            // reached entry. Distinguish invalid input on the fallback path.
            if values.iter().any(|x| !x.is_finite_value()) {
                return Err(SolverError::Backend("non-finite matrix entry".into()));
            }

            self.gp
                .factor_from(valid, values)
                .map_err(|c| SolverError::Singular { column: Some(c) })?;
        }
        self.ready = true;
        if first && std::env::var_os("SANSHO_PROFILE_MATRIX").is_some() {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "sansho-matrix A={} LU={}",
                self.nnz,
                self.gp.nnz()
            );
        }
        Ok(reused)
    }
    fn check_rhs(&self, rhs: &[T]) -> Result<(), SolverError> {
        if !self.ready {
            return Err(SolverError::NotReady);
        }
        check_len(self.gp.symbolic.dimension(), rhs.len())
    }
    pub fn solve(&mut self, rhs: &mut [T]) -> Result<(), SolverError> {
        self.check_rhs(rhs)?;
        self.gp.solve(rhs);
        check_result(rhs)
    }
    /// Solve the algebraic transpose without complex conjugation.
    pub fn solve_transpose(&mut self, rhs: &mut [T]) -> Result<(), SolverError> {
        self.check_rhs(rhs)?;
        self.gp.solve_transpose(rhs);
        check_result(rhs)
    }
}
fn check_len(expected: usize, actual: usize) -> Result<(), SolverError> {
    if expected == actual {
        Ok(())
    } else {
        Err(SolverError::DimensionMismatch { expected, actual })
    }
}
fn check_result<T: Scalar>(rhs: &[T]) -> Result<(), SolverError> {
    if rhs.iter().all(|v| v.is_finite_value()) {
        Ok(())
    } else {
        Err(SolverError::Singular { column: None })
    }
}
impl<T: Scalar> LinearSolver<T> for SerialLu<T> {
    fn analyze(&mut self, pattern: &CscPattern) -> Result<(), SolverError> {
        *self = Self::new(pattern)?;
        Ok(())
    }
    fn factor(&mut self, values: &[T]) -> Result<(), SolverError> {
        self.factor(values).map(|_| ())
    }
    fn solve(&self, rhs: &mut [T]) -> Result<(), SolverError> {
        self.check_rhs(rhs)?;
        self.gp.solve_shared(rhs);
        check_result(rhs)
    }
}
