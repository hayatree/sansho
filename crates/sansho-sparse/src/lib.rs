//! Sparse linear solvers for circuit matrices.
//!
//! The matrix sparsity pattern is fixed once (see [`PatternBuilder`]) and every assembly afterwards
//! only rewrites the value array. Solvers implement [`LinearSolver`], which separates symbolic
//! analysis (done once per pattern) from numeric factorization (done every Newton iteration).
//!
//! Provenance: original work based on Gilbert & Peierls (1988).
//! See `docs/PROVENANCE.md`.

#![forbid(unsafe_code)]

mod btf;
mod complex;
mod ordering;
mod pattern;
mod qz;
mod refactor;
mod serial;
pub use refactor::{MixedStats, NewtonLu};

pub use complex::Complex64 as c64;
pub use ordering::BatchOrdering;
pub use pattern::{CscPattern, PatternBuilder};
pub use qz::pencil_roots;
pub use serial::SerialLu;

/// Scalar types supported by the solvers.
pub trait Scalar:
    PartialEq
    + std::ops::SubAssign
    + std::ops::Mul<Output = Self>
    + std::ops::Div<Output = Self>
    + Copy
    + Default
    + Send
    + Sync
    + std::fmt::Debug
    + 'static
{
    #[doc(hidden)]
    type Cell: btf::FactorValue<Self>;
    /// Magnitude used in the relative pivot test.
    fn magnitude(self) -> f64;
    /// Additive identity.
    fn zero() -> Self;
    /// Whether the value is finite.
    fn is_finite_value(self) -> bool;
}

impl Scalar for f64 {
    type Cell = btf::Value;
    fn magnitude(self) -> f64 {
        self.abs()
    }
    fn zero() -> Self {
        0.0
    }
    fn is_finite_value(self) -> bool {
        self.is_finite()
    }
}

impl Scalar for f32 {
    type Cell = btf::Value32;
    fn magnitude(self) -> f64 {
        f64::from(self.abs())
    }
    fn zero() -> Self {
        0.0
    }
    fn is_finite_value(self) -> bool {
        self.is_finite()
    }
}

impl Scalar for c64 {
    type Cell = [btf::Value; 2];
    fn magnitude(self) -> f64 {
        self.norm()
    }
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
#[allow(clippy::unwrap_used)]
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
        let mut lu = SerialLu::<f64>::new(&pattern).unwrap();
        lu.factor(&values).unwrap();
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
        let mut lu = SerialLu::<f64>::new(&pattern).unwrap();
        assert!(lu.analyze(&pattern).is_ok());
        // Numerical singularity may be detected at factorization or at solve time.
        let detected = match lu.factor(&[1.0, 0.0]) {
            Err(SolverError::Singular { .. }) => true,
            Ok(_) => {
                let mut x = vec![1.0, 1.0];
                matches!(lu.solve(&mut x), Err(SolverError::Singular { .. }))
            }
            Err(_) => false,
        };
        assert!(detected);
    }

    #[test]
    fn complex_transpose_after_permutations_and_factor_failure() {
        let mut builder = PatternBuilder::new(5);
        for j in 0..5 {
            builder.add((j + 2) % 5, j);
            builder.add((j + 3) % 5, j);
        }
        let (p, _) = builder.build();
        let mut lu = SerialLu::new(&p).unwrap();
        let want: Vec<_> = (0..5)
            .map(|i| c64::new(i as f64 - 2., i as f64 / 3.))
            .collect();
        let mut a = vec![c64::default(); p.nnz()];
        for exchange in [2, 3, 3, 2] {
            let mut rhs = vec![c64::default(); 5];
            for (j, v) in rhs.iter_mut().enumerate() {
                let entries = p.col_ptr()[j]..p.col_ptr()[j + 1];
                for (&r, entry) in p.row_idx()[entries.clone()].iter().zip(&mut a[entries]) {
                    *entry = if r == (j + exchange) % 5 {
                        c64::new(-4., 3.)
                    } else {
                        c64::new(0.02, r as f64 * 0.01)
                    };
                    *v += *entry * want[r];
                }
            }
            lu.factor(&a).unwrap();
            lu.solve_transpose(&mut rhs).unwrap();
            for (v, x) in rhs.iter().zip(&want) {
                assert!((*v - *x).norm() < 2e-14);
            }
        }
        assert!(lu.factor(&[]).is_err());
        assert_eq!(lu.solve_transpose(&mut a), Err(SolverError::NotReady));
    }

    #[test]
    fn complex_solve() {
        let mut pb = PatternBuilder::new(1);
        pb.add(0, 0);
        let (pattern, _) = pb.build();
        let mut lu = SerialLu::<c64>::new(&pattern).unwrap();
        assert!(lu.analyze(&pattern).is_ok());
        assert!(lu.factor(&[c64::new(0.0, 2.0)]).is_ok());
        let mut x = vec![c64::new(2.0, 0.0)];
        assert!(lu.solve(&mut x).is_ok());
        assert!((x[0] - c64::new(0.0, -1.0)).norm() < 1e-15);
    }
}
