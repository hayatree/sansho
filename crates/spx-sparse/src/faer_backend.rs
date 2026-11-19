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

/// General real backend for irreducible systems. The ordering,
/// numerical storage and scratch live for the lifetime of the Newton solve.
/// Use the same sequential frontal kernels in every host thread pool.
#[derive(Debug)]
pub struct BatchOrdering {
    matrix: SymbolicSparseColMat<usize>,
    order: faer::sparse::linalg::lu::SymbolicLu<usize>,
}

/// Run-local numeric LU and scratch using sequential kernels.
pub struct SerialLu<T = f64> {
    ready: bool,
    shared: std::sync::Arc<BatchOrdering>,
    numeric: faer::sparse::linalg::lu::NumericLu<usize, T>,
    scratch: faer::dyn_stack::MemBuffer,
}

impl<T> std::fmt::Debug for SerialLu<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialLu")
            .field("dimension", &self.shared.order.nrows())
            .finish_non_exhaustive()
    }
}

impl BatchOrdering {
    pub fn dimension(&self) -> usize {
        self.order.nrows()
    }
    pub fn new(pattern: &CscPattern) -> Result<std::sync::Arc<Self>, SolverError> {
        use faer::sparse::linalg::lu::factorize_symbolic_lu;
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
        Ok(std::sync::Arc::new(Self { matrix, order }))
    }
}

impl<T: Scalar + ComplexField> SerialLu<T> {
    pub(crate) fn new(pattern: &CscPattern) -> Result<Self, SolverError> {
        Self::from_ordering(BatchOrdering::new(pattern)?)
    }
    pub fn from_ordering(shared: std::sync::Arc<BatchOrdering>) -> Result<Self, SolverError> {
        let order = &shared.order;
        let req = order
            .factorize_numeric_lu_scratch::<T>(faer::Par::Seq, Default::default())
            .or(order.solve_in_place_scratch::<T>(1, faer::Par::Seq));
        let scratch = faer::dyn_stack::MemBuffer::try_new(req)
            .map_err(|e| SolverError::Backend(format!("{e:?}")))?;
        Ok(Self {
            ready: false,
            shared,
            numeric: faer::sparse::linalg::lu::NumericLu::new(),
            scratch,
        })
    }

    pub fn factor(&mut self, values: &[T]) -> Result<bool, SolverError> {
        self.ready = false;
        let expected = self.shared.matrix.row_idx().len();
        if values.len() != expected {
            return Err(SolverError::DimensionMismatch {
                expected,
                actual: values.len(),
            });
        }
        if values.iter().any(|v| !v.is_finite_value()) {
            return Err(SolverError::Backend("non-finite matrix entry".into()));
        }
        self.shared
            .order
            .factorize_numeric_lu(
                &mut self.numeric,
                SparseColMatRef::new(self.shared.matrix.as_ref(), values),
                faer::Par::Seq,
                faer::dyn_stack::MemStack::new(&mut self.scratch),
                Default::default(),
            )
            .map_err(|err| match err {
                LuError::SymbolicSingular { index } => SolverError::Singular {
                    column: Some(index),
                },
                LuError::Generic(e) => SolverError::Backend(format!("{e:?}")),
            })?;
        self.ready = true;
        Ok(false)
    }

    pub fn solve(&mut self, rhs: &mut [T]) -> Result<(), SolverError> {
        use faer::sparse::linalg::lu::LuRef;
        if !self.ready {
            return Err(SolverError::NotReady);
        }
        let expected = self.shared.order.nrows();
        if rhs.len() != expected {
            return Err(SolverError::DimensionMismatch {
                expected,
                actual: rhs.len(),
            });
        }

        LuRef::new_unchecked(&self.shared.order, &self.numeric).solve_in_place_with_conj(
            faer::Conj::No,
            faer::MatMut::from_column_major_slice_mut(rhs, rhs.len(), 1),
            faer::Par::Seq,
            faer::dyn_stack::MemStack::new(&mut self.scratch),
        );
        if rhs.iter().all(|x| x.is_finite_value()) {
            Ok(())
        } else {
            Err(SolverError::Singular { column: None })
        }
    }
}
