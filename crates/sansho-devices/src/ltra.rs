//! Distributed lossy line with piecewise-linear convolution of port histories.
//! Provenance: Roychowdhury and Pederson, "Efficient transient simulation
//! of lossy interconnect", DAC (1991); inverse Laplace transforms of the
//! telegrapher equations. See docs/distributed-lines.md.

use crate::{
    DeviceGroup, EvalCtx, Mode, ModelParams, Node, Slots, StampBuilder, StateRef, Unknowns, volt,
};
use sansho_diag::Diagnostic;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy)]
pub struct LineConstants {
    pub r: f64,
    pub l: f64,
    pub g: f64,
    pub c: f64,
}

impl LineConstants {
    pub fn new(p: &ModelParams) -> Result<Self, Diagnostic> {
        let len = p.get("len", 0.);
        let v = Self {
            r: p.get("r", 0.) * len,
            l: p.get("l", 0.) * len,
            g: p.get("g", 0.) * len,
            c: p.get("c", 0.) * len,
        };
        if !len.is_finite()
            || len <= 0.
            || ![v.r, v.l, v.g, v.c]
                .iter()
                .all(|a| a.is_finite() && *a >= 0.)
            || !(v.g == 0. && v.c > 0. && (v.r > 0. || v.l > 0.)
                || v.r > 0. && v.g > 0. && v.l == 0. && v.c == 0.)
        {
            return Err(Diagnostic::error(
                "E-LINE-PARAM",
                "LTRA needs positive LEN and an RLC, RC, LC or RG line",
            ));
        }
        Ok(v)
    }
    pub fn delay(self) -> f64 {
        (self.l * self.c).sqrt()
    }
    /// Driving-point and transfer admittances; I1 = Y11 V1 - Y12 V2.
    pub fn dc(self) -> [f64; 2] {
        if self.g == 0. {
            return [1. / self.r; 2];
        }
        let z = (self.r * self.g).sqrt();
        let y = (self.g / self.r).sqrt();
        let h = (-z).exp();
        let denom = -(-2. * z).exp_m1();
        [y * (1. + h * h) / denom, 2. * y * h / denom]
    }
}

#[derive(Debug)]
pub struct Ltra {
    name: String,
    ends: [[Node; 2]; 2],
    currents: [Node; 2],
    pub values: LineConstants,
    pub step_limit: bool,
}

impl Ltra {
    pub fn new(
        name: &str,
        [p, n, q, m]: [Node; 4],
        values: LineConstants,
        step_limit: bool,
    ) -> Self {
        Self {
            name: name.into(),
            ends: [[p, n], [q, m]],
            currents: [None; 2],
            values,
            step_limit,
        }
    }
    pub fn sample(&self, x: &[f64]) -> [f64; 4] {
        let port = |k: usize| volt(x, self.ends[k][0]) - volt(x, self.ends[k][1]);
        [
            port(0),
            volt(x, self.currents[0]),
            port(1),
            volt(x, self.currents[1]),
        ]
    }
    /// Rows of the voltage terms in the two branch-current constraints.
    pub fn admittance_entries(&self) -> impl Iterator<Item = (Node, Node, usize, f64)> {
        [0, 1].into_iter().flat_map(move |k| {
            [0, 1].into_iter().flat_map(move |other| {
                let [p, n] = self.ends[k ^ other];
                let sign = if other == 0 { -1. } else { 1. };
                [
                    (self.currents[k], p, other, sign),
                    (self.currents[k], n, other, -sign),
                ]
            })
        })
    }
    fn stamp(&self, own: f64, cross: f64, remote_current: f64, out: &mut Slots) {
        for row in [0, 1] {
            out.g[row * 8..row * 8 + 8].copy_from_slice(&[
                1.,
                -1.,
                1.,
                -own,
                own,
                cross,
                -cross,
                remote_current,
            ]);
        }
    }
    /// Integrate the convolution once per trial step, before Newton begins.
    /// The latest interval contributes an implicit local voltage coefficient.
    pub fn trial(&self, t: f64, hist: &[(f64, [f64; 4])], out: &mut Slots) {
        let c = self.values;
        let Some(&(last, prev)) = hist.last() else {
            return;
        };
        if c.g > 0. {
            return;
        }
        let initial = hist[0].1;
        if c.l == 0. {
            let [mut own, mut cross] = [1. / c.r; 2];
            let mut rhs = [0.; 2];
            for pair in hist.windows(2) {
                let [(t0, left), (t1, right)] = [pair[0], pair[1]];
                let w = rc_integral(c, t - t1, t - t0);
                for k in [0, 1] {
                    rhs[k] += (w[0] * (right[2 * k] - left[2 * k])
                        - w[1] * (right[2 * (1 - k)] - left[2 * (1 - k)]))
                        / (t1 - t0);
                }
            }
            let h = t - last;
            let [a, b] = rc_integral(c, 0., h).map(|w| w / h);
            own += a;
            cross += b;
            for k in [0, 1] {
                rhs[k] += -a * prev[2 * k] + b * prev[2 * (1 - k)];
            }
            self.stamp(own, cross, 0., out);
            out.b.copy_from_slice(&rhs);
        } else {
            let tau = c.delay();
            let y = (c.c / c.l).sqrt();
            let a = 0.5 * c.r / c.l;
            let atten = (-a * tau).exp();
            // Work in deviations from the constant prehistory. The DC additive
            // term is the initial port current even for a nonzero DC voltage.
            let mut rhs = [initial[1] - y * initial[0], initial[3] - y * initial[2]];
            let mut own = y;
            let mut cross = 0.;
            let mut remote_current = 0.;
            for i in 0..hist.len() {
                let (left_time, left) = hist[i];
                let (right_time, right) = hist.get(i + 1).copied().unwrap_or((t, [0.; 4]));
                let h = right_time - left_time;
                for k in [0, 1] {
                    let remote = 2 * (1 - k);
                    let retarded = t - tau;
                    if left_time < retarded && retarded <= right_time {
                        let f = (retarded - left_time) / h;
                        let wave = |s: [f64; 4]| {
                            (s[remote + 1] - initial[remote + 1])
                                + y * (s[remote] - initial[remote])
                        };
                        rhs[k] -= atten * (wave(left) * (1. - f) + wave(right) * f);
                        if i + 1 == hist.len() && k == 0 {
                            cross += atten * y * f;
                            remote_current += atten * f;
                        }
                    }
                }
                let mut integrate = |lo: f64, hi: f64, delayed: bool| {
                    quadrature(lo, hi, |lag, w| {
                        let f = ((t - lag - left_time) / h).clamp(0., 1.);
                        let radial = if delayed {
                            ((lag - tau) * (lag + tau)).max(0.).sqrt()
                        } else {
                            lag
                        };
                        let (b0, b1) = scaled_bessels(a * radial);
                        let decay = if delayed {
                            (-a * tau * (tau / (lag + radial))).exp()
                        } else {
                            1.
                        };
                        let b1_over_r = if radial > 0. { b1 / radial } else { a * 0.5 };
                        for k in [0, 1] {
                            let port = 2 * (if delayed { 1 - k } else { k });
                            let sample = |j: usize| (1. - f) * left[j] + f * right[j] - initial[j];
                            if delayed {
                                if k == 0 && i + 1 == hist.len() {
                                    cross += w * decay * a * y * (lag * b1_over_r - b0) * f;
                                    remote_current += w * decay * a * tau * b1_over_r * f;
                                }
                                rhs[k] -= w
                                    * decay
                                    * a
                                    * (tau * b1_over_r * sample(port + 1)
                                        + y * (lag * b1_over_r - b0) * sample(port));
                            } else {
                                let kernel = y * a * (b1 - b0);
                                rhs[k] += w * kernel * sample(port);
                                if k == 0 && i + 1 == hist.len() {
                                    own += w * kernel * f;
                                }
                            }
                        }
                    });
                };
                integrate(t - right_time, t - left_time, false);
                integrate((t - right_time).max(tau), t - left_time, true);
            }
            self.stamp(own, cross, remote_current, out);
            out.b.copy_from_slice(&rhs);
        }
    }
}

fn quadrature(lo: f64, hi: f64, mut sample: impl FnMut(f64, f64)) {
    if hi <= lo {
        return;
    }
    let half = (hi - lo) * 0.5;
    for (node, w) in [
        (0.1834346424956498, 0.362683783378362),
        (0.525532409916329, 0.3137066458778873),
        (0.7966664774136267, 0.2223810344533745),
        (0.9602898564975363, 0.1012285362903763),
    ] {
        for sign in [-1., 1.] {
            sample(lo + half * (1. + sign * node), half * w);
        }
    }
}

/// Integral of the RC *excess* step admittance. Use images at short times
/// and the eigenfunction series at long times; both converge exponentially.
fn rc_integral(c: LineConstants, lo: f64, hi: f64) -> [f64; 2] {
    let rc = c.r * c.c;
    let mut sum = [0.; 2];
    let split = 0.2 * rc;
    // Integrate the long-time eigenmodes analytically. Their short initial
    // decay can be much faster than the trial interval on a fast RC line.
    let start = lo.max(split);
    if hi > start {
        for n in 1..16 {
            let rate = (PI * n as f64).powi(2) / rc;
            let decay = (-rate * start).exp();
            if decay < 1e-16 {
                break;
            }
            let area = 2. * decay * (-(-rate * (hi - start)).exp_m1()) / (c.r * rate);
            sum[0] += area;
            sum[1] += if n % 2 == 0 { area } else { -area };
        }
    }
    let left = lo.max(0.).sqrt();
    let right = hi.min(split).sqrt();
    let mut image = |root: f64, w: f64| {
        let u = root * root / rc;
        let mut own = 1.;
        let mut transfer = 0.;
        for n in 0..16 {
            let half = n as f64 + 0.5;
            let odd = (-half * half / u).exp();
            if odd < 1e-16 {
                break;
            }
            own += 2. * (-((n + 1) as f64).powi(2) / u).exp();
            transfer += 2. * odd;
        }
        // The root Jacobian cancels the 1/sqrt(t) singularity.
        let scale = (rc / PI).sqrt();
        sum[0] += 2. * w * (scale * own - root) / c.r;
        sum[1] += 2. * w * (scale * transfer - root) / c.r;
    };
    let middle = (left + right) * 0.5;
    quadrature(left, middle, &mut image);
    quadrature(middle, right, image);
    sum
}

/// exp(-x) I0(x) and exp(-x) I1(x). Sum the defining series for modest
/// arguments and use the large-argument expansion above the crossover.
fn scaled_bessels(x: f64) -> (f64, f64) {
    if x < 32. {
        let mut term = 1.;
        let mut sum0 = 1.;
        let mut sum1 = 0.5 * x;
        for k in 1..96 {
            term *= (0.5 * x / k as f64).powi(2);
            sum0 += term;
            sum1 += term * 0.5 * x / (k + 1) as f64;
            if term < 1e-16 * sum0 {
                break;
            }
        }
        let e = (-x).exp();
        (e * sum0, e * sum1)
    } else {
        let series = |mu: f64| {
            let mut term = 1.;
            let mut sum = 1.;
            for k in 1..12 {
                term *= (((2 * k - 1) as f64).powi(2) - mu) / (8. * x * k as f64);
                sum += term;
            }
            sum / (2. * PI * x).sqrt()
        };
        (series(0.), series(4.))
    }
}

impl DeviceGroup for Ltra {
    fn type_name(&self) -> &'static str {
        "ltra"
    }
    fn ltra(&self) -> Option<&Ltra> {
        Some(self)
    }
    fn len(&self) -> usize {
        1
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for k in [0, 1] {
            self.currents[k] = Some(u.alloc_branch(&format!("{}#{}", self.name, k + 1)));
        }
    }
    fn instance_ic(&self, _: usize, ic: &[f64], x: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [v1, i1, v2, i2] = ic {
            for (k, (v, i)) in [(*v1, *i1), (*v2, *i2)].into_iter().enumerate() {
                edges.push((self.ends[k][0], self.ends[k][1], v));
                if let Some(j) = self.currents[k] {
                    x[j] = i;
                }
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for k in [0, 1] {
            let [p, n] = self.ends[k];
            let [q, m] = self.ends[1 - k];
            let i = self.currents[k];
            for (row, col) in [
                (p, i),
                (n, i),
                (i, i),
                (i, p),
                (i, n),
                (i, q),
                (i, m),
                (i, self.currents[1 - k]),
            ] {
                b.g(row, col);
            }
            for row in [p, n, i] {
                b.f(row);
            }
            b.b(i);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        if matches!(ctx.mode, Mode::Ac | Mode::Noise) && self.values.c > 0. {
            self.stamp(0., 0., 0., out);
            out.b.fill(0.);
        } else if ctx.mode != Mode::Tran || ctx.time == 0. || self.values.g > 0. {
            let [own, cross] = self.values.dc();
            self.stamp(own, cross, 0., out);
            out.b.fill(0.);
        }
        let port = self.sample(x);
        for k in [0, 1] {
            let i = port[2 * k + 1];
            let row = &out.g[k * 8..];
            out.f[k * 3..k * 3 + 3].copy_from_slice(&[
                i,
                -i,
                i + row[3] * port[2 * k]
                    + row[5] * port[2 * (1 - k)]
                    + row[7] * port[2 * (1 - k) + 1],
            ]);
        }
        if let Some(o) = out.outputs.first_mut() {
            o.extend([
                ("v1", port[0]),
                ("i1", port[1]),
                ("v2", port[2]),
                ("i2", port[3]),
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rc_moments_survive_steps_much_longer_than_the_diffusion_time() {
        let line = LineConstants {
            r: 2e3,
            c: 1e-12,
            g: 0.,
            l: 0.,
        };
        let w = rc_integral(line, 0., 1000. * line.r * line.c);
        for (sum, want) in w.into_iter().zip([line.c / 3., -line.c / 6.]) {
            assert!((sum - want).abs() < 3e-6 * line.c, "{sum} vs {want}");
        }
    }
}
