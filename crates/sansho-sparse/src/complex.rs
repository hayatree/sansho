//! Double-precision Cartesian complex arithmetic for AC and pole-zero analysis.
//! Provenance: original implementation of elementary complex arithmetic.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Complex64 {
    pub re: f64,
    pub im: f64,
}

impl Complex64 {
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }
    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}
impl From<f64> for Complex64 {
    fn from(x: f64) -> Self {
        Self::new(x, 0.)
    }
}
impl Add for Complex64 {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self::new(self.re + b.re, self.im + b.im)
    }
}
impl AddAssign for Complex64 {
    fn add_assign(&mut self, b: Self) {
        *self = *self + b;
    }
}
impl Neg for Complex64 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.re, -self.im)
    }
}
impl Sub for Complex64 {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self::new(self.re - b.re, self.im - b.im)
    }
}
impl SubAssign for Complex64 {
    fn sub_assign(&mut self, b: Self) {
        *self = *self - b;
    }
}
impl Mul for Complex64 {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self::new(
            self.re * b.re - self.im * b.im,
            self.re * b.im + self.im * b.re,
        )
    }
}
impl Mul<f64> for Complex64 {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self::new(self.re * b, self.im * b)
    }
}
impl Div<f64> for Complex64 {
    type Output = Self;
    fn div(self, b: f64) -> Self {
        Self::new(self.re / b, self.im / b)
    }
}
impl Div for Complex64 {
    type Output = Self;
    fn div(self, b: Self) -> Self {
        let scale = b.re.abs().max(b.im.abs());
        let unit = b / scale;
        let den = unit.norm_sqr();
        // Halve exceptionally large numerators before division by a scale
        // below one. The quotient can fit even if a single intermediate
        // component would overflow.
        let extra = if scale < 1. && self.re.abs().max(self.im.abs()) > f64::MAX * scale {
            0.5
        } else {
            1.
        };
        let a = (self * extra) / scale;
        Self::new(
            a.re * (unit.re / den) + a.im * (unit.im / den),
            a.im * (unit.re / den) - a.re * (unit.im / den),
        ) / extra
    }
}
impl Div<Complex64> for f64 {
    type Output = Complex64;
    fn div(self, b: Complex64) -> Complex64 {
        Complex64::from(self) / b
    }
}
impl Add<Complex64> for f64 {
    type Output = Complex64;
    fn add(self, b: Complex64) -> Complex64 {
        Complex64::new(self + b.re, b.im)
    }
}
impl std::iter::Sum for Complex64 {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), Add::add)
    }
}
