//! [`LinearSolver`] backend based on `faer`'s sparse LU (partial pivoting).
//!
//! Provenance: original glue code over the `faer` crate (MIT). See `docs/PROVENANCE.md`.

use faer::linalg::solvers::Solve;
use faer::sparse::linalg::LuError;
use faer::sparse::linalg::solvers::{Lu, SymbolicLu};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMat};
use faer::traits::ComplexField;

use crate::{CscPattern, LinearSolver, Scalar, SolverError};

/// Sparse LU solver backed by `faer`.
#[derive(Debug)]
pub struct FaerLu<T> {
    symbolic_mat: Option<SymbolicSparseColMat<usize>>,
    symbolic_lu: Option<SymbolicLu<usize>>,
    lu: Option<Lu<usize, T>>,
}

impl<T> Default for FaerLu<T> {
    fn default() -> Self {
        Self {
            symbolic_mat: None,
            symbolic_lu: None,
            lu: None,
        }
    }
}

impl<T> FaerLu<T> {
    /// Create an empty solver; call [`LinearSolver::analyze`] first.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<T: Scalar + ComplexField> LinearSolver<T> for FaerLu<T> {
    fn analyze(&mut self, pattern: &CscPattern) -> Result<(), SolverError> {
        let n = pattern.dim();
        let sym = SymbolicSparseColMat::new_checked(
            n,
            n,
            pattern.col_ptr().to_vec(),
            None,
            pattern.row_idx().to_vec(),
        );
        let slu = SymbolicLu::try_new(sym.as_ref())
            .map_err(|e| SolverError::Backend(format!("{e:?}")))?;
        self.symbolic_mat = Some(sym);
        self.symbolic_lu = Some(slu);
        self.lu = None;
        Ok(())
    }

    fn factor(&mut self, values: &[T]) -> Result<(), SolverError> {
        let (Some(sym), Some(slu)) = (&self.symbolic_mat, &self.symbolic_lu) else {
            return Err(SolverError::NotReady);
        };
        let nnz = sym.row_idx().len();
        if values.len() != nnz {
            return Err(SolverError::DimensionMismatch {
                expected: nnz,
                actual: values.len(),
            });
        }
        if values.iter().any(|v| !v.is_finite_value()) {
            return Err(SolverError::Backend("non-finite matrix entry".into()));
        }
        let mat = SparseColMatRef::new(sym.as_ref(), values);
        match Lu::try_new_with_symbolic(slu.clone(), mat) {
            Ok(lu) => {
                self.lu = Some(lu);
                Ok(())
            }
            Err(LuError::SymbolicSingular { index }) => {
                self.lu = None;
                Err(SolverError::Singular {
                    column: Some(index),
                })
            }
            Err(LuError::Generic(e)) => {
                self.lu = None;
                Err(SolverError::Backend(format!("{e:?}")))
            }
        }
    }

    fn solve(&self, rhs: &mut [T]) -> Result<(), SolverError> {
        let lu = self.lu.as_ref().ok_or(SolverError::NotReady)?;
        let n = rhs.len();
        let mat = faer::MatMut::from_column_major_slice_mut(rhs, n, 1);
        lu.solve_in_place(mat);
        if rhs.iter().any(|v| !v.is_finite_value()) {
            return Err(SolverError::Singular { column: None });
        }
        Ok(())
    }
}
