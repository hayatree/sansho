//! Local Taylor coefficients for Volterra current and charge generators.
//! Provenance: Griewank & Walther, Evaluating Derivatives (2008), higher-order
//! forward differentiation; Wambacq & Sansen, Distortion Analysis (1998).

use crate::{Node, volt};
use std::ops::{Add, Div, Mul, Neg, Sub};

/// One monomial in the multivariate expansion about the operating point.
#[derive(Debug)]
pub struct Term {
    pub row: usize,
    pub axes: [usize; 3],
    pub order: usize,
    pub current: f64,
    pub charge: f64,
    /// For C(x) dv/dt, differentiate only the last factor of the monomial.
    pub differential: bool,
}

// Coefficients of products of subsets of three independent perturbations.
// Each perturbation squares to zero. Repeated seeds along one voltage give
// repeated partials, including mixed third derivatives, without differencing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Jet([f64; 8]);
impl Jet {
    pub fn value(self) -> f64 {
        self.0[0]
    }
    pub fn constant(v: f64) -> Self {
        let mut x = Self::default();
        x.0[0] = v;
        x
    }
    fn compose(self, value: f64, d1: f64, d2: f64, d3: f64) -> Self {
        let delta = self - self.value();
        Self::constant(value)
            + delta * d1
            + (delta * delta) * (0.5 * d2)
            + (delta * delta * delta) * (d3 / 6.)
    }
    pub fn exp(self) -> Self {
        let e = self.value().exp();
        self.compose(e, e, e, e)
    }
    pub fn powf(self, p: f64) -> Self {
        let x = self.value();
        let slope = |k: i32| {
            let c = (0..k).fold(1., |s, i| s * (p - f64::from(i)));
            if c == 0. {
                0.
            } else {
                c * x.powf(p - f64::from(k))
            }
        };
        self.compose(x.powf(p), slope(1), slope(2), slope(3))
    }
    pub fn diode(self, is: f64, nvt: f64) -> Self {
        let v = self / nvt;
        let exp = if v.value() > 80. {
            (v - 79.) * 80f64.exp()
        } else {
            v.exp()
        };
        (exp - 1.) * is
    }
    pub fn depletion(self, c: f64, vj: f64, m: f64, fc: f64) -> Self {
        if c == 0. {
            return Self::default();
        }
        let x = self.value();
        let a = 1. - (x / vj).min(fc);
        let cap = c * a.powf(-m);
        let slope = cap * m / (vj * a);
        // The primitive's constant is irrelevant to a terminal charge generator.
        if x >= fc * vj {
            self.compose(0., cap + slope * (x - fc * vj), slope, 0.)
        } else {
            self.compose(0., cap, slope, slope * (m + 1.) / (vj * a))
        }
    }
}
impl Add for Jet {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] + b.0[i]))
    }
}
impl Neg for Jet {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.map(|a| -a))
    }
}
impl Sub for Jet {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        self + (-b)
    }
}
impl Sub<f64> for Jet {
    type Output = Self;
    fn sub(mut self, b: f64) -> Self {
        self.0[0] -= b;
        self
    }
}
impl Add<f64> for Jet {
    type Output = Self;
    fn add(mut self, b: f64) -> Self {
        self.0[0] += b;
        self
    }
}
impl Mul<f64> for Jet {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self(self.0.map(|a| a * b))
    }
}
impl Div<f64> for Jet {
    type Output = Self;
    fn div(self, b: f64) -> Self {
        Self(self.0.map(|a| a / b))
    }
}
impl Mul for Jet {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self(std::array::from_fn(|mask| {
            let mut sub = mask;
            let mut sum = 0.;
            loop {
                sum += self.0[sub] * b.0[mask ^ sub];
                if sub == 0 {
                    break;
                }
                sub = (sub - 1) & mask;
            }
            sum
        }))
    }
}
#[allow(clippy::suspicious_arithmetic_impl)]
impl Div for Jet {
    type Output = Self;
    fn div(self, b: Self) -> Self {
        self * b.powf(-1.)
    }
}

pub(crate) fn expand<const N: usize>(
    nodes: [Node; N],
    bias: &[f64],
    out: &mut Vec<Term>,
    eval: impl Fn([Jet; N]) -> ([Jet; N], [Jet; N]),
) {
    let base = nodes.map(|n| Jet::constant(volt(bias, n)));
    for (i, ni) in nodes.iter().enumerate() {
        let Some(i_node) = *ni else { continue };
        for (j, nj) in nodes.iter().enumerate().skip(i) {
            let Some(j_node) = *nj else { continue };
            for k in j..=N {
                let third = nodes.get(k);
                if third == Some(&None) {
                    continue;
                }
                let mut inputs = base;
                inputs[i].0[1] = 1.;
                inputs[j].0[2] = 1.;
                let (mask, order, divisor) = if third.is_some() {
                    inputs[k].0[4] = 1.;
                    (
                        7,
                        3,
                        if i == k {
                            6.
                        } else if i == j || j == k {
                            2.
                        } else {
                            1.
                        },
                    )
                } else {
                    (3, 2, if i == j { 2. } else { 1. })
                };
                let (f, q) = eval(inputs);
                for (r, node) in nodes.iter().enumerate() {
                    let Some(row) = *node else { continue };
                    let (current, charge) = (f[r].0[mask] / divisor, q[r].0[mask] / divisor);
                    if current != 0. || charge != 0. {
                        out.push(Term {
                            row,
                            axes: [i_node, j_node, third.copied().flatten().unwrap_or(i_node)],
                            order,
                            current,
                            charge,
                            differential: false,
                        });
                    }
                }
            }
        }
    }
}

/// Expand a differential branch capacitance C(x) d(Vp-Vn)/dt.
pub(crate) fn capacitance<const N: usize>(
    nodes: [Node; N],
    pins: [Node; 2],
    bias: &[f64],
    out: &mut Vec<Term>,
    eval: impl Fn([Jet; N]) -> Jet,
) {
    let base = nodes.map(|n| Jet::constant(volt(bias, n)));
    for (i, &first) in nodes.iter().enumerate() {
        let Some(first) = first else { continue };
        for j in i..=N {
            let mut x = base;
            x[i].0[1] = 1.;
            let (second, order, mask, factor) = if j < N {
                let Some(n) = nodes[j] else { continue };
                x[j].0[2] = 1.;
                (n, 3, 3, if i == j { 0.5 } else { 1. })
            } else {
                (first, 2, 1, 1.)
            };
            let c = eval(x).0[mask] * factor;
            if c == 0. {
                continue;
            }
            for (r, &row) in pins.iter().enumerate() {
                for (t, &end) in pins.iter().enumerate() {
                    if let (Some(row), Some(end)) = (row, end) {
                        let axes = if order == 2 {
                            [first, end, end]
                        } else {
                            [first, second, end]
                        };
                        out.push(Term {
                            row,
                            order,
                            axes,
                            current: 0.,
                            charge: if r == t { c } else { -c },
                            differential: true,
                        });
                    }
                }
            }
        }
    }
}
