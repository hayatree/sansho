//! [`LinearSolver`] backend based on `faer`'s sparse LU (partial pivoting).
//!
//! Provenance: original glue code over the `faer` crate (MIT). See `docs/PROVENANCE.md`.

use faer::linalg::solvers::Solve;
use faer::sparse::linalg::LuError;
use faer::sparse::linalg::solvers::{Lu, SymbolicLu};
use faer::sparse::{SparseColMatRef, SymbolicSparseColMat};
use faer::traits::ComplexField;

use crate::{CscPattern, LinearSolver, Scalar, SolverError, c64};

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
    pub fn new(pattern: &CscPattern) -> Result<Self, SolverError> {
        Self::from_ordering(BatchOrdering::new(pattern)?)
    }
    pub fn from_ordering(shared: std::sync::Arc<BatchOrdering>) -> Result<Self, SolverError> {
        let order = &shared.order;
        let req = order
            .factorize_numeric_lu_scratch::<T>(faer::Par::Seq, Default::default())
            .or(order.solve_in_place_scratch::<T>(1, faer::Par::Seq))
            .or(order.solve_transpose_in_place_scratch::<T>(1, faer::Par::Seq));
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
        self.triangular(rhs, false)
    }
    /// Solve A-transpose x = b with the already factored real or complex matrix.
    pub fn solve_transpose(&mut self, rhs: &mut [T]) -> Result<(), SolverError> {
        self.triangular(rhs, true)
    }
    fn triangular(&mut self, rhs: &mut [T], transpose: bool) -> Result<(), SolverError> {
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

        let factors = LuRef::new_unchecked(&self.shared.order, &self.numeric);
        let col = faer::MatMut::from_column_major_slice_mut(rhs, expected, 1);
        let stack = faer::dyn_stack::MemStack::new(&mut self.scratch);
        if transpose {
            factors.solve_transpose_in_place_with_conj(faer::Conj::No, col, faer::Par::Seq, stack);
        } else {
            factors.solve_in_place_with_conj(faer::Conj::No, col, faer::Par::Seq, stack);
        }
        if rhs.iter().all(|x| x.is_finite_value()) {
            Ok(())
        } else {
            Err(SolverError::Singular { column: None })
        }
    }
}

/// Finite roots of det(G+sC), for a small row-major dense pencil.
/// Scale the frequency unit and equilibrate the equations before QZ.
pub fn pencil_roots(g: &[f64], c: &[f64], n: usize) -> Result<Vec<c64>, SolverError> {
    if n == 0 || n > 258 || g.len() != n * n || c.len() != n * n {
        return Err(SolverError::Backend(
            "invalid dense pencil dimensions".into(),
        ));
    }
    let mut scales: Vec<_> = g
        .chunks(n)
        .zip(c.chunks(n))
        .filter_map(|(a, b)| {
            let norm = |r: &[f64]| r.iter().fold(0.0f64, |s, v| s.max(v.abs()));
            let ratio = norm(a) / norm(b);
            (ratio.is_finite() && ratio > 0.).then_some(ratio)
        })
        .collect();
    scales.sort_by(f64::total_cmp);
    let scale = scales.get(scales.len() / 2).copied().unwrap_or(1.);
    let mut a = faer::Mat::from_fn(n, n, |i, j| -g[i * n + j]);
    let mut b = faer::Mat::from_fn(n, n, |i, j| c[i * n + j] * scale);
    for i in 0..n {
        let max = (0..n).fold(0.0f64, |v, j| v.max(a[(i, j)].abs()).max(b[(i, j)].abs()));
        if max == 0. || !max.is_finite() {
            return Err(SolverError::Singular { column: None });
        }
        for j in 0..n {
            a[(i, j)] /= max;
            b[(i, j)] /= max;
        }
    }
    for j in 0..n {
        let max = (0..n).fold(0.0f64, |v, i| v.max(a[(i, j)].abs()).max(b[(i, j)].abs()));
        if max == 0. {
            return Err(SolverError::Singular { column: Some(j) });
        }
        for i in 0..n {
            a[(i, j)] /= max;
            b[(i, j)] /= max;
        }
    }
    // Only eigenvalues are needed. In particular, algebraic constraint rows
    // need no back-substitution for their infinite-eigenvalue eigenvectors.
    use faer::{
        Par,
        diag::Diag,
        dyn_stack::{MemBuffer, MemStack},
        linalg::{evd::ComputeEigenvectors::No, gevd},
    };
    let req = gevd::gevd_scratch::<f64>(n, No, No, Par::Seq, Default::default());
    let mut buf = MemBuffer::try_new(req).map_err(|e| SolverError::Backend(e.to_string()))?;
    let mut re = Diag::zeros(n);
    let mut im = Diag::zeros(n);
    let mut denominator = Diag::zeros(n);
    gevd::gevd_real(
        a.as_mut(),
        b.as_mut(),
        re.as_mut(),
        im.as_mut(),
        denominator.as_mut(),
        None,
        None,
        Par::Seq,
        MemStack::new(&mut buf),
        Default::default(),
    )
    .map_err(|e| SolverError::Backend(format!("generalized eigenvalue iteration: {e:?}")))?;
    let mut roots = Vec::new();
    for i in 0..n {
        let (alpha, beta) = (c64::new(re[i], im[i]), denominator[i]);
        if !alpha.is_finite_value()
            || !beta.is_finite_value()
            || (alpha.norm() == 0. && beta.abs() == 0.)
        {
            return Err(SolverError::Singular { column: None });
        }
        if beta.abs() > 16. * f64::EPSILON * alpha.norm() {
            let root = alpha / beta * scale;
            if !root.is_finite_value() {
                return Err(SolverError::Backend("non-finite root".into()));
            }
            roots.push(root);
        }
    }
    roots.sort_by(|a, b| a.re.total_cmp(&b.re).then_with(|| a.im.total_cmp(&b.im)));
    Ok(roots)
}
