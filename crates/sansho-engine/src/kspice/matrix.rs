//! Small dense matrices for the one to eight conductor telegrapher equations.
//! Provenance: elementary matrix algebra, cyclic Jacobi rotations and scaling-and-squaring.

use sansho_sparse::c64;
use std::ops::{Add, Index, IndexMut, Mul};

#[derive(Debug, Clone)]
pub(super) struct Matrix<T> {
    pub n: usize,
    pub data: Vec<T>,
}
impl<T> Index<(usize, usize)> for Matrix<T> {
    type Output = T;
    fn index(&self, (i, j): (usize, usize)) -> &T {
        &self.data[i * self.n + j]
    }
}
impl<T> IndexMut<(usize, usize)> for Matrix<T> {
    fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut T {
        &mut self.data[i * self.n + j]
    }
}
impl<T> Matrix<T> {
    pub fn build(n: usize, mut f: impl FnMut(usize, usize) -> T) -> Self {
        let mut data = Vec::with_capacity(n * n);
        for i in 0..n {
            for j in 0..n {
                data.push(f(i, j));
            }
        }
        Self { n, data }
    }
}
impl<T: Copy + Default + Add<Output = T> + Mul<Output = T>> Matrix<T> {
    pub fn product(&self, b: &Self) -> Self {
        Self::build(self.n, |i, j| {
            (0..self.n).fold(T::default(), |sum, k| sum + self[(i, k)] * b[(k, j)])
        })
    }
}

impl Matrix<f64> {
    pub fn transpose(&self) -> Self {
        Self::build(self.n, |i, j| self[(j, i)])
    }
    /// Columns of Q are eigenvectors. Relative stopping scale follows the input units.
    pub fn eigen(&self) -> (Vec<f64>, Self) {
        let n = self.n;
        let mut a = self.clone();
        let mut q = Self::build(n, |i, j| f64::from(i == j));
        for _ in 0..64 {
            let mut changed = false;
            for i in 0..n {
                for j in i + 1..n {
                    let off = a[(i, j)];
                    if off.abs() <= f64::EPSILON * a[(i, i)].abs().sqrt() * a[(j, j)].abs().sqrt() {
                        continue;
                    }
                    changed = true;
                    let diff = (a[(j, j)] - a[(i, i)]) * 0.5;
                    let t = if diff == 0. {
                        1.
                    } else {
                        off / (diff + diff.hypot(off).copysign(diff))
                    };
                    let c = 1. / (1. + t * t).sqrt();
                    let s = t * c;
                    a[(i, i)] -= t * off;
                    a[(j, j)] += t * off;
                    a[(i, j)] = 0.;
                    a[(j, i)] = 0.;
                    for k in 0..n {
                        if k != i && k != j {
                            let (x, y) = (a[(k, i)], a[(k, j)]);
                            a[(k, i)] = c * x - s * y;
                            a[(i, k)] = a[(k, i)];
                            a[(k, j)] = s * x + c * y;
                            a[(j, k)] = a[(k, j)];
                        }
                        let (x, y) = (q[(k, i)], q[(k, j)]);
                        q[(k, i)] = c * x - s * y;
                        q[(k, j)] = s * x + c * y;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        ((0..n).map(|i| a[(i, i)]).collect(), q)
    }
}

impl Matrix<c64> {
    fn identity(n: usize) -> Self {
        Self::build(n, |i, j| f64::from(i == j).into())
    }
    fn inverse(&self) -> Self {
        // The argument is a wave transmission block of a short passive section,
        // or the feedback denominator for two identical passive sections.
        let n = self.n;
        let mut a = self.clone();
        let mut inv = Self::identity(n);
        for k in 0..n {
            let mut pivot = k;
            for i in k + 1..n {
                if a[(i, k)].norm() > a[(pivot, k)].norm() {
                    pivot = i;
                }
            }
            for j in 0..n {
                a.data.swap(k * n + j, pivot * n + j);
                inv.data.swap(k * n + j, pivot * n + j);
            }
            let diag = a[(k, k)];
            for j in 0..n {
                a[(k, j)] = a[(k, j)] / diag;
                inv[(k, j)] = inv[(k, j)] / diag;
            }
            for i in 0..n {
                if i == k {
                    continue;
                }
                let f = a[(i, k)];
                for j in 0..n {
                    let v = a[(k, j)] * f;
                    a[(i, j)] -= v;
                    let v = inv[(k, j)] * f;
                    inv[(i, j)] -= v;
                }
            }
        }
        inv
    }
}

/// Port scattering at a positive real reference impedance. Start with the
/// exponential of a short section in wave coordinates. Cascade by doubling in
/// scattering coordinates so growing backward exponentials never overflow.
pub(super) fn scattering(z: &Matrix<c64>, y: &Matrix<c64>) -> Matrix<c64> {
    let n = z.n;
    let ports = 2 * n;
    let mut a = Matrix::build(ports, |i, j| {
        let ij = (i % n, j % n);
        let sum = (z[ij] + y[ij]) * 0.5;
        let diff = (y[ij] - z[ij]) * 0.5;
        match (i < n, j < n) {
            (true, true) => sum,
            (true, false) => diff,
            (false, true) => -diff,
            (false, false) => -sum,
        }
    });
    let norm = a
        .data
        .chunks(ports)
        .map(|row| row.iter().map(|v| v.norm()).sum::<f64>())
        .fold(0., f64::max);
    let shifts = (norm.log2().ceil() + 1.).clamp(0., 1023.) as i32;
    let scale = 2f64.powi(-shifts);
    for v in &mut a.data {
        *v = *v * scale;
    }
    let mut transfer = Matrix::identity(ports);
    let mut term = transfer.clone();
    for k in 1..=16 {
        term = term.product(&a);
        for (sum, v) in transfer.data.iter_mut().zip(&mut term.data) {
            *v = *v / (k as f64);
            *sum += *v;
        }
    }
    let mut through = Matrix::build(n, |i, j| transfer[(i, j)]).inverse();
    let mut refl = Matrix::build(n, |i, j| transfer[(i + n, j)]).product(&through);
    for _ in 0..shifts {
        let rr = refl.product(&refl);
        let feedback = Matrix::build(n, |i, j| c64::from(f64::from(i == j)) - rr[(i, j)]).inverse();
        let t = through.product(&feedback);
        let extra = t.product(&refl).product(&through);
        for (v, e) in refl.data.iter_mut().zip(extra.data) {
            *v += e;
        }
        through = t.product(&through);
    }
    Matrix::build(ports, |i, j| {
        if i / n == j / n {
            refl[(i % n, j % n)]
        } else {
            through[(i % n, j % n)]
        }
    })
}
