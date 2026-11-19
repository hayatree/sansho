//! One-sided channel and diffusion resistance noise.
//! Provenance: UC Berkeley BSIM3v3.3 manual, equations 8.1–8.10 and Table 8-2.

use super::{D, Model, channel::Bias};
use crate::{BOLTZMANN, CHARGE, ModelParams};

#[derive(Debug)]
pub(super) struct Noise {
    mode: u8,
    a: f64,
    b: f64,
    c: f64,
    em: f64,
    kf: f64,
    af: f64,
    pub ef: f64,
    length: f64,
}

impl Noise {
    pub fn new(p: &ModelParams, mode: u8, sign: f64, l: f64) -> Result<Self, String> {
        let length = l - 2. * p.get("lintnoi", 0.);
        if length <= 0. {
            return Err("nonpositive effective noise length".into());
        }
        Ok(Self {
            mode,
            a: p.get("noia", if sign > 0. { 1e20 } else { 9.9e18 }),
            b: p.get("noib", if sign > 0. { 5e4 } else { 2.4e3 }),
            c: p.get("noic", if sign > 0. { -1.4e-12 } else { 1.4e-12 }),
            em: p.get("em", 4.1e7),
            af: p.get("af", 1.),
            ef: p.get("ef", 1.),
            kf: p.get("kf", 0.),
            length,
        })
    }
    fn strong(&self, m: &Model, b: &Bias, overdrive: f64) -> f64 {
        let n0 = m.cox * overdrive / CHARGE;
        let nl = (n0 - m.cox * b.drain.v.min(b.sat.v) / CHARGE).max(0.);
        let offset = 2e14;
        let traps = self.a * ((n0 + offset) / (nl + offset)).ln()
            + (n0 - nl) * (self.b + 0.5 * self.c * (n0 + nl));
        let litl = (3. * m.p.xj * m.tox).sqrt();
        let dl = if b.drain.v > b.sat.v {
            let field = (b.drain.v - b.sat.v) / litl + self.em;
            litl * (field * b.mu.v / (2. * m.vsat)).max(1.).ln()
        } else {
            0.
        };
        let density = self.a + nl * (self.b + self.c * nl);
        let kt_ev = m.vt;
        kt_ev / (1e8 * self.length.powi(2))
            * (CHARGE * CHARGE * b.mu.v * b.ids.v * traps / (m.cox * b.bulk.v)
                + b.ids.v.powi(2) * dl * density / (b.width.v * (nl + offset).powi(2)))
    }

    pub fn powers(&self, m: &Model, vg: f64, vd: f64, vb: f64) -> [f64; 2] {
        // Lanes are Vgs, Vds and Vbs in the normalized source frame.
        let b = m.channel(D::var(vg, 0), D::var(vd, 1), D::var(vb, 2));
        let conductance = if matches!(self.mode, 2 | 4) {
            let q = m.charges(&b);
            let mobility_charge = b.mu.v * (q[0].v + q[2].v).abs();
            mobility_charge / (m.l * m.l + mobility_charge * b.rds.v)
        } else {
            let sum = (b.ids.d[0] + b.ids.d[1] + b.ids.d[2]).abs();
            2. / 3. * sum
        };
        let flicker = if matches!(self.mode, 1 | 4) {
            self.kf * b.ids.v.abs().powf(self.af) / (m.cox * m.l * m.l)
        } else if vg > b.threshold.v + 0.1 {
            self.strong(m, &b, vg - b.threshold.v)
        } else {
            let limit = m.channel(D::cst(b.threshold.v + 0.1), D::cst(vd), D::cst(vb));
            let s = self.strong(m, &limit, 0.1).max(0.);
            let weak = self.a * m.vt * b.ids.v.powi(2) / (b.width.v * m.l * 4e36);
            if weak + s > 0. {
                s * weak / (s + weak)
            } else {
                0.
            }
        };
        [
            4. * BOLTZMANN * m.temp * conductance * m.mult,
            flicker.max(0.) * m.mult,
        ]
    }
}
