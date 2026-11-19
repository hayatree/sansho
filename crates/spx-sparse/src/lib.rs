//! Sparse linear solvers for circuit matrices.
//!
//! The matrix sparsity pattern is fixed once (see [`PatternBuilder`]) and every assembly afterwards
//! only rewrites the value array. Solvers implement [`LinearSolver`], which separates symbolic
//! analysis (done once per pattern) from numeric factorization (done every Newton iteration).
//!
//! Provenance: original work. The initial backend wraps the `faer` crate (MIT).
//! See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod btf;
mod faer_backend;
mod pattern;
mod refactor;
pub use refactor::NewtonLu;

pub use faer::c64;
pub use faer_backend::FaerLu;
pub use pattern::{CscPattern, PatternBuilder};

/// Scalar types supported by the solvers.
pub trait Scalar: Copy + Default + Send + Sync + std::fmt::Debug + 'static {
    /// Additive identity.
    fn zero() -> Self;
    /// Whether the value is finite.
    fn is_finite_value(self) -> bool;
}

impl Scalar for f64 {
    fn zero() -> Self {
        0.0
    }
    fn is_finite_value(self) -> bool {
        self.is_finite()
    }
}

impl Scalar for c64 {
    fn zero() -> Self {
        c64::new(0.0, 0.0)
    }
    fn is_finite_value(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

/// Errors reported by linear solvers.
#[derive(Debug, Clone, PartialEq)]
pub enum SolverError {
    /// The matrix is structurally or numerically singular.
    Singular {
        /// Column (unknown index) at which the factorization broke down, if known.
        column: Option<usize>,
    },
    /// The value array does not match the pattern.
    DimensionMismatch {
        /// Expected length.
        expected: usize,
        /// Actual length.
        actual: usize,
    },
    /// `factor` or `solve` was called before `analyze` / `factor`.
    NotReady,
    /// Any other backend failure.
    Backend(String),
}

impl std::fmt::Display for SolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Singular { column: Some(c) } => write!(f, "singular matrix at unknown {c}"),
            Self::Singular { column: None } => write!(f, "singular matrix"),
            Self::DimensionMismatch { expected, actual } => {
                write!(f, "value array has length {actual}, expected {expected}")
            }
            Self::NotReady => write!(f, "solver used before analysis/factorization"),
            Self::Backend(msg) => write!(f, "linear solver failure: {msg}"),
        }
    }
}

impl std::error::Error for SolverError {}

/// A sparse direct solver with separate symbolic and numeric phases.
pub trait LinearSolver<T: Scalar> {
    /// Symbolic analysis (ordering, fill-in structure). Called once per pattern.
    fn analyze(&mut self, pattern: &CscPattern) -> Result<(), SolverError>;
    /// Numeric factorization of the matrix with the analyzed pattern and the given values
    /// (in pattern order).
    fn factor(&mut self, values: &[T]) -> Result<(), SolverError>;
    /// Solve `A x = rhs` in place using the last factorization.
    fn solve(&self, rhs: &mut [T]) -> Result<(), SolverError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solve_real(n: usize, entries: &[(usize, usize, f64)], rhs: &[f64]) -> Vec<f64> {
        let mut pb = PatternBuilder::new(n);
        let slots: Vec<usize> = entries.iter().map(|&(r, c, _)| pb.add(r, c)).collect();
        let (pattern, map) = pb.build();
        let mut values = vec![0.0; pattern.nnz()];
        for (s, &(_, _, v)) in slots.iter().zip(entries) {
            values[map[*s]] += v;
        }
        let mut lu = FaerLu::<f64>::new();
        lu.analyze(&pattern).ok();
        lu.factor(&values).ok();
        let mut x = rhs.to_vec();
        assert!(lu.solve(&mut x).is_ok());
        x
    }

    #[test]
    fn solves_small_system_with_duplicates() {
        // [ 3 -1 ] x = [ 1 ]
        // [-1  2 ]     [ 0 ]   (diagonal entries given as sums of duplicates)
        let x = solve_real(
            2,
            &[
                (0, 0, 1.0),
                (0, 0, 2.0),
                (0, 1, -1.0),
                (1, 0, -1.0),
                (1, 1, 2.0),
            ],
            &[1.0, 0.0],
        );
        assert!((x[0] - 0.4).abs() < 1e-14);
        assert!((x[1] - 0.2).abs() < 1e-14);
    }

    #[test]
    fn needs_pivoting() {
        // Zero diagonal (typical of voltage-source MNA rows).
        let x = solve_real(2, &[(0, 1, 1.0), (1, 0, 1.0), (1, 1, 1.0)], &[2.0, 5.0]);
        assert!((x[0] - 3.0).abs() < 1e-14);
        assert!((x[1] - 2.0).abs() < 1e-14);
    }

    #[test]
    fn detects_singular() {
        let mut pb = PatternBuilder::new(2);
        pb.add(0, 0);
        pb.add(1, 1);
        let (pattern, _) = pb.build();
        let mut lu = FaerLu::<f64>::new();
        assert!(lu.analyze(&pattern).is_ok());
        // Numerical singularity may be detected at factorization or at solve time.
        let detected = match lu.factor(&[1.0, 0.0]) {
            Err(SolverError::Singular { .. }) => true,
            Ok(()) => {
                let mut x = vec![1.0, 1.0];
                matches!(lu.solve(&mut x), Err(SolverError::Singular { .. }))
            }
            Err(_) => false,
        };
        assert!(detected);
    }

    #[test]
    fn complex_solve() {
        let mut pb = PatternBuilder::new(1);
        pb.add(0, 0);
        let (pattern, _) = pb.build();
        let mut lu = FaerLu::<c64>::new();
        assert!(lu.analyze(&pattern).is_ok());
        assert!(lu.factor(&[c64::new(0.0, 2.0)]).is_ok());
        let mut x = vec![c64::new(2.0, 0.0)];
        assert!(lu.solve(&mut x).is_ok());
        assert!((x[0] - c64::new(0.0, -1.0)).norm() < 1e-15);
    }
}
