//! Finite generalized eigenvalues of a small real circuit pencil.
//! Provenance: original real QZ from Moler & Stewart (1973) and Golub &
//! Van Loan, Matrix Computations (Hessenberg-triangular and implicit QZ steps).

use crate::{SolverError, c64};
use std::ops::{Index, IndexMut, RangeInclusive};

struct Matrix {
    n: usize,
    data: Vec<f64>,
}
impl Index<(usize, usize)> for Matrix {
    type Output = f64;
    fn index(&self, (i, j): (usize, usize)) -> &f64 {
        &self.data[i * self.n + j]
    }
}
impl IndexMut<(usize, usize)> for Matrix {
    fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut f64 {
        &mut self.data[i * self.n + j]
    }
}
#[derive(Clone, Copy)]
struct Rotation {
    c: f64,
    s: f64,
}
impl Rotation {
    // Map (x,y) to (hypot(x,y),0).
    fn new(x: f64, y: f64) -> Self {
        let r = x.hypot(y);
        if r == 0. {
            Self { c: 1., s: 0. }
        } else {
            Self { c: x / r, s: y / r }
        }
    }
    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (self.c * x + self.s * y, self.c * y - self.s * x)
    }
}
struct Pencil {
    a: Matrix,
    b: Matrix,
}
impl Pencil {
    fn left(&mut self, row: usize, cols: RangeInclusive<usize>, g: Rotation) {
        for m in [&mut self.a, &mut self.b] {
            for j in cols.clone() {
                (m[(row, j)], m[(row + 1, j)]) = g.apply(m[(row, j)], m[(row + 1, j)]);
            }
        }
    }
    // Apply to the ordered column pair (j+1,j), annihilating its left entry.
    fn right(&mut self, col: usize, rows: RangeInclusive<usize>, g: Rotation) {
        for m in [&mut self.a, &mut self.b] {
            for i in rows.clone() {
                (m[(i, col + 1)], m[(i, col)]) = g.apply(m[(i, col + 1)], m[(i, col)]);
            }
        }
    }
    // Column-pivoted QR on the active B. Compare each trailing column to
    // its own initial scale: a tiny independent column can carry a finite
    // mode, whereas roundoff after eliminating a dependent column cannot.
    fn triangularize(&mut self, n: usize) -> usize {
        let mut scale: Vec<_> = (0..n)
            .map(|j| (0..n).fold(0f64, |m, i| m.max(self.b[(i, j)].abs())))
            .collect();
        for j in 0..n {
            let mut best = 0.;
            let mut pivot = j;
            for (k, &base) in scale.iter().enumerate().skip(j) {
                if base == 0. {
                    continue;
                }
                let residual = (j..n).fold(0f64, |m, i| m.max(self.b[(i, k)].abs())) / base;
                if residual > best || residual == best && base > scale[pivot] {
                    best = residual;
                    pivot = k;
                }
            }
            if best <= 8. * n as f64 * f64::EPSILON {
                for i in j..n {
                    for k in j..n {
                        self.b[(i, k)] = 0.;
                    }
                }
                return j;
            }
            if j != pivot {
                for m in [&mut self.a, &mut self.b] {
                    for i in 0..n {
                        m.data.swap(i * m.n + j, i * m.n + pivot);
                    }
                }
                scale.swap(j, pivot);
            }
            for i in (j + 1..n).rev() {
                if self.b[(i, j)] != 0. {
                    let g = Rotation::new(self.b[(i - 1, j)], self.b[(i, j)]);
                    self.left(i - 1, 0..=n - 1, g);
                    self.b[(i, j)] = 0.;
                }
            }
        }
        n
    }
    fn reduce(&mut self) -> Result<usize, SolverError> {
        let mut n = self.a.n;
        loop {
            let rank = self.triangularize(n);
            if rank == n {
                break;
            }
            // Each null row of B gives a zero beta. Isolate its nonzero
            // alpha by right rotations of A, then test the smaller pencil.
            // Repeating the reduction handles chains of infinite modes.
            for i in (rank..n).rev() {
                for j in 0..i {
                    if self.a[(i, j)] != 0. {
                        let g = Rotation::new(self.a[(i, j + 1)], self.a[(i, j)]);
                        self.right(j, 0..=i, g);
                        self.a[(i, j)] = 0.;
                    }
                }
                if self.a[(i, i)] == 0. {
                    return Err(SolverError::Singular { column: None });
                }
            }
            n = rank;
        }
        for j in 0..n {
            for i in (j + 2..n).rev() {
                if self.a[(i, j)] != 0. {
                    let g = Rotation::new(self.a[(i - 1, j)], self.a[(i, j)]);
                    self.left(i - 1, j..=n - 1, g);
                    self.a[(i, j)] = 0.;
                    let g = Rotation::new(self.b[(i, i)], self.b[(i, i - 1)]);
                    self.right(i - 1, 0..=n - 1, g);
                    self.b[(i, i - 1)] = 0.;
                }
            }
        }
        Ok(n)
    }
    // Coefficients of det(A₂ − s B₂), with B₂ triangular.
    fn polynomial(&self, j: usize) -> [f64; 3] {
        let (a, b) = (&self.a, &self.b);
        let k = j + 1;
        [
            b[(j, j)] * b[(k, k)],
            a[(k, j)] * b[(j, k)] - a[(j, j)] * b[(k, k)] - a[(k, k)] * b[(j, j)],
            a[(j, j)] * a[(k, k)] - a[(j, k)] * a[(k, j)],
        ]
    }
    // Normalize the two blocks separately before forming quadratic products.
    // Taking the discriminant from the diagonal difference retains small
    // complex splittings around a repeated nonzero real part.
    fn pair(&self, j: usize) -> [c64; 2] {
        let k = j + 1;
        let rows = |m: &Matrix| [m[(j, j)], m[(j, k)], m[(k, j)], m[(k, k)]];
        let a = rows(&self.a);
        let b = rows(&self.b);
        let norm = |v: [f64; 4]| v.into_iter().fold(0f64, |m, x| m.max(x.abs()));
        let an = norm(a);
        let bn = norm(b);
        let [a11, a12, a21, a22] = a.map(|x| x / an);
        let [b11, b12, _, b22] = b.map(|x| x / bn);
        let beta = b11 * b22;
        let left = a11 * b22;
        let right = a22.mul_add(b11, -a21 * b12);
        let trace = left + right;
        let delta = left - right;
        let cross = a12.mul_add(b11, -a11 * b12) * (a21 * b22);
        let disc = delta.mul_add(delta, 4. * cross);
        let roots = if disc < 0. {
            let re = 0.5 * trace / beta;
            let im = 0.5 * (-disc).sqrt() / beta.abs();
            [c64::new(re, im), c64::new(re, -im)]
        } else {
            let t = 0.5 * (trace + disc.sqrt().copysign(trace));
            let det = a11.mul_add(a22, -a21 * a12);
            [
                c64::from(t / beta),
                c64::from(if t == 0. { 0. } else { det / t }),
            ]
        };
        roots.map(|z| z * an / bn)
    }

    fn sweep(&mut self, lo: usize, hi: usize, iteration: usize) {
        let (a, b) = (&self.a, &self.b);
        let [p, q, r] = if iteration.is_multiple_of(16) {
            // A repeated real shift breaks cycles with a symmetric trailing block.
            let x = a[(hi, hi)] + 0.75 * a[(hi, hi - 1)].abs();
            let y = b[(hi, hi)];
            [y * y, -2. * x * y, x * x]
        } else {
            self.polynomial(hi - 1)
        };
        let j = lo + 1;
        let x = a[(lo, lo)] * b[(j, j)] - a[(j, lo)] * b[(lo, j)];
        let y = b[(lo, lo)] * a[(j, lo)];
        let z = b[(lo, lo)] * b[(j, j)];
        // First column of the shift polynomial in A B⁻¹, multiplied through
        // by the denominators. No triangular inverse is formed.
        let mut v = [0.; 3];
        for (i, vi) in v.iter_mut().enumerate() {
            *vi = (p * x + q * z) * a[(lo + i, lo)] + p * y * a[(lo + i, j)];
        }
        v[0] += r * z * b[(lo, lo)];
        for k in lo..hi {
            let end = hi.min(k + 2);
            if k != lo {
                for i in k..=end {
                    v[i - k] = self.a[(i, k - 1)];
                }
            }
            for i in (k + 1..=end).rev() {
                let g = Rotation::new(v[i - k - 1], v[i - k]);
                v[i - k - 1] = v[i - k - 1].hypot(v[i - k]);
                self.left(i - 1, k.saturating_sub(1).max(lo)..=hi, g);
                if k > lo {
                    self.a[(i, k - 1)] = 0.;
                }
            }
            // Restore B by an RQ reduction of the two- or three-row window.
            for i in (k + 1..=end).rev() {
                for col in k..i {
                    let g = Rotation::new(self.b[(i, col + 1)], self.b[(i, col)]);
                    self.right(col, lo..=hi.min(end + 1), g);
                    self.b[(i, col)] = 0.;
                }
            }
        }
    }

    // Move a zero beta down the triangle. The left rotation transfers the
    // zero to the next diagonal, the right rotation repairs Hessenberg form.
    fn infinite(&mut self, lo: usize, hi: usize, start: usize) -> Result<(), SolverError> {
        self.b[(start, start)] = 0.;
        for k in start..hi {
            let g = Rotation::new(self.b[(k, k + 1)], self.b[(k + 1, k + 1)]);
            self.left(k, k.saturating_sub(1).max(lo)..=hi, g);
            self.b[(k + 1, k + 1)] = 0.;
            if k > lo {
                let g = Rotation::new(self.a[(k + 1, k)], self.a[(k + 1, k - 1)]);
                self.right(k - 1, lo..=k + 1, g);
                self.a[(k + 1, k - 1)] = 0.;
            }
        }
        if hi > lo {
            let g = Rotation::new(self.a[(hi, hi)], self.a[(hi, hi - 1)]);
            self.right(hi - 1, lo..=hi, g);
            self.a[(hi, hi - 1)] = 0.;
        }
        if self.a[(hi, hi)] == 0. {
            Err(SolverError::Singular { column: None })
        } else {
            Ok(())
        }
    }

    fn roots(&mut self) -> Result<Vec<c64>, SolverError> {
        let mut end = self.reduce()?;
        let mut roots = Vec::new();
        let mut steps = 0;
        while end > 0 {
            let hi = end - 1;
            let mut lo = hi;
            while lo > 0 {
                let diag = self.a[(lo, lo)].abs() + self.a[(lo - 1, lo - 1)].abs();
                if self.a[(lo, lo - 1)].abs() <= f64::EPSILON * diag {
                    self.a[(lo, lo - 1)] = 0.;
                    break;
                }
                lo -= 1;
            }
            // Test a beta against its local triangular row. An isolated small
            // nonzero beta still represents a finite, possibly very large root.
            if let Some(k) = (lo..end).find(|&k| {
                let off = if k < hi { self.b[(k, k + 1)].abs() } else { 0. }
                    + if k > lo { self.b[(k - 1, k)].abs() } else { 0. };
                self.b[(k, k)].abs() <= f64::EPSILON * off
            }) {
                self.infinite(lo, hi, k)?;
                end -= 1;
            } else if lo == hi {
                roots.push(c64::from(self.a[(hi, hi)] / self.b[(hi, hi)]));
                end -= 1;
            } else if lo + 1 == hi {
                roots.extend(self.pair(lo));
                end -= 2;
            } else {
                steps += 1;
                if steps > 160 * (end - lo) {
                    return Err(SolverError::Backend("QZ iteration did not converge".into()));
                }
                self.sweep(lo, hi, steps);
                continue;
            }
            steps = 0;
        }
        Ok(roots)
    }
}

/// Finite roots of det(G + s C), sorted by real and imaginary part.
/// The 256 circuit unknowns may be augmented by one port constraint.
pub fn pencil_roots(g: &[f64], c: &[f64], n: usize) -> Result<Vec<c64>, SolverError> {
    if !(1..=257).contains(&n) || g.len() != n * n || c.len() != n * n {
        return Err(SolverError::Backend(
            "invalid dense pencil dimensions".into(),
        ));
    }
    if !g.iter().chain(c).all(|x| x.is_finite()) {
        return Err(SolverError::Backend("non-finite pencil entry".into()));
    }
    let norm = |row: &[f64]| row.iter().fold(0f64, |m, x| m.max(x.abs()));
    let mut ratios: Vec<_> = g
        .chunks_exact(n)
        .zip(c.chunks_exact(n))
        .map(|(a, b)| norm(a) / norm(b))
        .filter(|r| r.is_normal())
        .collect();
    ratios.sort_by(f64::total_cmp);
    let scale = ratios.get(ratios.len() / 2).copied().unwrap_or(1.);
    let mut p = Pencil {
        a: Matrix {
            n,
            data: g.iter().map(|x| -x).collect(),
        },
        b: Matrix {
            n,
            data: c.to_vec(),
        },
    };
    for i in 0..n {
        let row = i * n..(i + 1) * n;
        let max = norm(&p.a.data[row.clone()]).max(norm(&p.b.data[row]));
        if max == 0. {
            return Err(SolverError::Singular { column: None });
        }
        for j in 0..n {
            p.a[(i, j)] /= max;
            p.b[(i, j)] = (p.b[(i, j)] / max) * scale;
        }
    }
    // Normalize the scaled equations before column equilibration.
    for (a, b) in
        p.a.data
            .chunks_exact_mut(n)
            .zip(p.b.data.chunks_exact_mut(n))
    {
        let max = norm(a).max(norm(b));
        if max == 0. {
            return Err(SolverError::Singular { column: None });
        }
        for (x, y) in a.iter_mut().zip(b) {
            *x /= max;
            *y /= max;
        }
    }
    for j in 0..n {
        let max = (0..n).fold(0f64, |m, i| m.max(p.a[(i, j)].abs()).max(p.b[(i, j)].abs()));
        if max == 0. {
            return Err(SolverError::Singular { column: Some(j) });
        }
        for i in 0..n {
            p.a[(i, j)] /= max;
            p.b[(i, j)] /= max;
        }
    }
    let mut roots = p.roots()?;
    for z in &mut roots {
        *z = *z * scale;
        if !z.re.is_finite() || !z.im.is_finite() {
            return Err(SolverError::Backend("non-finite pencil root".into()));
        }
    }
    roots.sort_by(|a, b| a.re.total_cmp(&b.re).then_with(|| a.im.total_cmp(&b.im)));
    Ok(roots)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn check(a: &[f64], b: &[f64], n: usize, expected: &[c64], tol: f64) {
        let g: Vec<_> = a.iter().map(|v| -v).collect();
        let mut roots = pencil_roots(&g, b, n).unwrap();
        assert_eq!(roots.len(), expected.len(), "{roots:?}");
        for &want in expected {
            let (index, dist) = roots
                .iter()
                .enumerate()
                .map(|(i, &v)| (i, (v - want).norm()))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(
                dist <= tol * want.norm().max(1.),
                "{want:?} missing from {roots:?}"
            );
            roots.swap_remove(index);
        }
    }
    fn identity(n: usize) -> Vec<f64> {
        (0..n * n).map(|i| f64::from(i / n == i % n)).collect()
    }

    #[test]
    fn diagonal_finite_zero_and_infinite() {
        check(
            &[0., 0., 0., 0., 6., 0., 0., 0., 1.],
            &[2., 0., 0., 0., 2., 0., 0., 0., 0.],
            3,
            &[0.0.into(), 3.0.into()],
            1e-15,
        );
        assert!(pencil_roots(&[0.], &[0.], 1).is_err());
    }
    #[test]
    fn nearly_repeated_complex_pair_and_extreme_equation_units() {
        let d = 1e-10;
        check(
            &[1., -d, d, 1.],
            &identity(2),
            2,
            &[c64::new(1., d), c64::new(1., -d)],
            1e-14,
        );
        check(
            &[1e300, 0., 0., 1e300],
            &[1e100, 0., 0., 1e300],
            2,
            &[1.0.into(), 1e200.into()],
            1e-14,
        );
    }
    #[test]
    fn defective_finite_and_infinite_jordan_blocks() {
        let a = [-2., 0., 0., 1., -2., 0., 0., 1., -2.];
        check(&a, &identity(3), 3, &[(-2.).into(); 3], 3e-5);
        check(
            &identity(3),
            &[0., 1., 0., 0., 0., 1., 0., 0., 0.],
            3,
            &[],
            1e-14,
        );
    }
    #[test]
    fn dense_equivalent_companion() {
        // (s+2)(s-3)(s²+1). Mix rows and columns of both matrices with
        // nonsingular shears so the reduction and the bulge chase both run.
        let mut a = [
            0., 0., 0., 6., 1., 0., 0., 1., 0., 1., 0., 5., 0., 0., 1., 1.,
        ];
        let mut b = identity(4);
        for m in [&mut a[..], &mut b] {
            for i in 0..4 {
                for j in 0..4 {
                    m[4 * i + j] += 0.25 * m[4 * ((i + 1) % 4) + j];
                }
            }
            for j in 0..4 {
                for i in 0..4 {
                    m[4 * i + j] -= 0.5 * m[4 * i + (j + 2) % 4];
                }
            }
        }
        check(
            &a,
            &b,
            4,
            &[
                (-2.).into(),
                3.0.into(),
                c64::new(0., 1.),
                c64::new(0., -1.),
            ],
            1e-12,
        );
    }

    #[test]
    fn dense_equivalence_preserves_multiple_infinite_modes() {
        let n = 32;
        let mut a = vec![0.; n * n];
        let mut b = identity(n);
        let mut expected = Vec::new();
        for i in 0..n {
            a[n * i + i] = (i + 1) as f64;
            if i % 13 == 3 {
                b[n * i + i] = 0.;
            } else {
                expected.push(c64::from((i + 1) as f64));
            }
        }
        let mut seed = 9u64;
        let mut index = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as usize
        };
        for _ in 0..8 * n {
            let mut pair = || {
                let i = index() % n;
                (i, (i + 1 + index() % (n - 1)) % n)
            };
            let (i, j) = pair();
            let (u, v) = pair();
            for m in [&mut a, &mut b] {
                for k in 0..n {
                    let (x, y) = (m[i * n + k], m[j * n + k]);
                    (m[i * n + k], m[j * n + k]) = (0.6 * x + 0.8 * y, 0.6 * y - 0.8 * x);
                }
                for k in 0..n {
                    let (x, y) = (m[k * n + u], m[k * n + v]);
                    (m[k * n + u], m[k * n + v]) = (0.8 * x - 0.6 * y, 0.6 * x + 0.8 * y);
                }
            }
        }
        check(&a, &b, n, &expected, 1e-13);
    }
    #[test]
    fn interior_zero_beta_in_unreduced_pencil() {
        let a = [2., 1., 0., 1., 1., 1., 0., 1., 5.];
        // Eliminating the algebraic middle equation leaves [[1,-1],[-1,4]].
        check(
            &a,
            &[1., 0., 0., 0., 0., 0., 0., 0., 1.],
            3,
            &[
                ((5. - 13f64.sqrt()) / 2.).into(),
                ((5. + 13f64.sqrt()) / 2.).into(),
            ],
            1e-14,
        );
    }
}
