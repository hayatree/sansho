//! Dynamic forward-mode differentiation.
//! Provenance: original implementation from the chain rule and general AD literature.
#[derive(Debug, Clone)]
pub(crate) struct Dual {
    pub v: f64,
    pub d: Vec<f64>,
}
impl Dual {
    pub fn constant(v: f64, n: usize) -> Self {
        Self { v, d: vec![0.; n] }
    }
    pub fn variable(v: f64, n: usize, i: usize) -> Self {
        let mut a = Self::constant(v, n);
        if let Some(d) = a.d.get_mut(i) {
            *d = 1.;
        }
        a
    }
    pub fn map(&self, v: f64, s: f64) -> Self {
        Self {
            v,
            d: self
                .d
                .iter()
                .map(|d| if *d == 0. { 0. } else { d * s })
                .collect(),
        }
    }
    pub fn combine(&self, b: &Self, v: f64, sa: f64, sb: f64) -> Self {
        Self {
            v,
            d: self
                .d
                .iter()
                .zip(&b.d)
                .map(|(a, b)| {
                    (if *a == 0. { 0. } else { a * sa }) + (if *b == 0. { 0. } else { b * sb })
                })
                .collect(),
        }
    }
    pub fn add(&self, b: &Self) -> Self {
        self.combine(b, self.v + b.v, 1., 1.)
    }
    pub fn sub(&self, b: &Self) -> Self {
        self.combine(b, self.v - b.v, 1., -1.)
    }
    pub fn mul(&self, b: &Self) -> Self {
        self.combine(b, self.v * b.v, b.v, self.v)
    }
    pub fn div(&self, b: &Self) -> Self {
        self.combine(b, self.v / b.v, 1. / b.v, -self.v / (b.v * b.v))
    }
}
