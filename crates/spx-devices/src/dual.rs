//! Forward-mode dual numbers for exact device Jacobians.
//!
//! A [`Dual<N>`] carries a value and its gradient with respect to `N` independent variables
//! (typically the terminal voltages of one device instance). Device equations are written once
//! in terms of `Dual` and both the residual contribution and its Jacobian follow, which avoids
//! hand-derived derivative mistakes.
//!
//! Provenance: original work; standard forward-mode automatic differentiation
//! (e.g. Griewank & Walther, *Evaluating Derivatives*, 2nd ed., SIAM 2008).

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub};

/// A value with gradient with respect to `N` variables.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dual<const N: usize> {
    /// Value.
    pub v: f64,
    /// Partial derivatives.
    pub d: [f64; N],
}

impl<const N: usize> Dual<N> {
    /// A constant (zero gradient).
    #[inline]
    pub fn cst(v: f64) -> Self {
        Self { v, d: [0.0; N] }
    }

    /// The independent variable `k` with value `v`.
    #[inline]
    pub fn var(v: f64, k: usize) -> Self {
        let mut d = [0.0; N];
        if let Some(s) = d.get_mut(k) {
            *s = 1.0;
        }
        Self { v, d }
    }

    #[inline]
    fn chain(self, v: f64, dv: f64) -> Self {
        let mut d = self.d;
        for x in &mut d {
            *x *= dv;
        }
        Self { v, d }
    }

    /// `e^self`
    #[inline]
    pub fn exp(self) -> Self {
        let e = self.v.exp();
        self.chain(e, e)
    }

    /// Natural logarithm.
    #[inline]
    pub fn ln(self) -> Self {
        self.chain(self.v.ln(), 1.0 / self.v)
    }

    /// Square root.
    #[inline]
    pub fn sqrt(self) -> Self {
        let s = self.v.sqrt();
        self.chain(s, 0.5 / s)
    }

    /// `self^p` for a constant exponent.
    #[inline]
    pub fn powf(self, p: f64) -> Self {
        let r = self.v.powf(p);
        self.chain(r, p * self.v.powf(p - 1.0))
    }

    /// Square.
    #[inline]
    pub fn sq(self) -> Self {
        self * self
    }
}

impl<const N: usize> Add for Dual<N> {
    type Output = Self;
    #[inline]
    fn add(mut self, o: Self) -> Self {
        self.v += o.v;
        for (a, b) in self.d.iter_mut().zip(o.d) {
            *a += b;
        }
        self
    }
}

impl<const N: usize> AddAssign for Dual<N> {
    #[inline]
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl<const N: usize> Sub for Dual<N> {
    type Output = Self;
    #[inline]
    fn sub(mut self, o: Self) -> Self {
        self.v -= o.v;
        for (a, b) in self.d.iter_mut().zip(o.d) {
            *a -= b;
        }
        self
    }
}

impl<const N: usize> Neg for Dual<N> {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        self.chain(-self.v, -1.0)
    }
}

impl<const N: usize> Mul for Dual<N> {
    type Output = Self;
    #[inline]
    fn mul(self, o: Self) -> Self {
        let mut d = [0.0; N];
        for (k, x) in d.iter_mut().enumerate() {
            *x = self.d[k] * o.v + self.v * o.d[k];
        }
        Self { v: self.v * o.v, d }
    }
}

impl<const N: usize> Div for Dual<N> {
    type Output = Self;
    #[inline]
    fn div(self, o: Self) -> Self {
        let inv = 1.0 / o.v;
        let v = self.v * inv;
        let mut d = [0.0; N];
        for (k, x) in d.iter_mut().enumerate() {
            *x = (self.d[k] - v * o.d[k]) * inv;
        }
        Self { v, d }
    }
}

impl<const N: usize> Add<f64> for Dual<N> {
    type Output = Self;
    #[inline]
    fn add(mut self, o: f64) -> Self {
        self.v += o;
        self
    }
}

impl<const N: usize> Sub<f64> for Dual<N> {
    type Output = Self;
    #[inline]
    fn sub(mut self, o: f64) -> Self {
        self.v -= o;
        self
    }
}

impl<const N: usize> Mul<f64> for Dual<N> {
    type Output = Self;
    #[inline]
    fn mul(self, o: f64) -> Self {
        self.chain(self.v * o, o)
    }
}

impl<const N: usize> Div<f64> for Dual<N> {
    type Output = Self;
    #[inline]
    fn div(self, o: f64) -> Self {
        self * (1.0 / o)
    }
}

impl<const N: usize> Mul<Dual<N>> for f64 {
    type Output = Dual<N>;
    #[inline]
    fn mul(self, o: Dual<N>) -> Dual<N> {
        o * self
    }
}

impl<const N: usize> Sub<Dual<N>> for f64 {
    type Output = Dual<N>;
    #[inline]
    fn sub(self, o: Dual<N>) -> Dual<N> {
        -o + self
    }
}

impl<const N: usize> Div<Dual<N>> for f64 {
    type Output = Dual<N>;
    #[inline]
    fn div(self, o: Dual<N>) -> Dual<N> {
        Dual::cst(self) / o
    }
}

impl<const N: usize> Add<Dual<N>> for f64 {
    type Output = Dual<N>;
    #[inline]
    fn add(self, o: Dual<N>) -> Dual<N> {
        o + self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivatives_match_finite_differences() {
        let f =
            |x: Dual<2>, y: Dual<2>| (x * y).exp() / (x + 2.0).sqrt() - (y.powf(1.5) * 3.0).ln();
        let (x0, y0) = (0.3, 0.7);
        let r = f(Dual::var(x0, 0), Dual::var(y0, 1));
        let h = 1e-6;
        let fx = |x: f64, y: f64| f(Dual::cst(x), Dual::cst(y)).v;
        let dx = (fx(x0 + h, y0) - fx(x0 - h, y0)) / (2.0 * h);
        let dy = (fx(x0, y0 + h) - fx(x0, y0 - h)) / (2.0 * h);
        assert!((r.d[0] - dx).abs() < 1e-8);
        assert!((r.d[1] - dy).abs() < 1e-8);
    }
}
