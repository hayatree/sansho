//! Quasi-static intrinsic and overlap terminal charges.
//! Provenance: UC Berkeley BSIM3v3.3 manual, §§4.3–4.5 and Appendix B.2.

use super::{
    D, EPS_SI, Model,
    channel::{Bias, softplus},
    floor, smooth_min,
};
use crate::junction::safe_exp;

// Positive gate charge per unit oxide capacitance in depletion. The
// accumulation continuation is also the limiting slope at zero drive.
fn depletion(v: D, k: f64) -> D {
    if v.v <= 0. {
        v
    } else if k <= 0. {
        D::cst(0.)
    } else {
        2. * v / (1. + (1. + 4. * v / (k * k)).sqrt())
    }
}

impl Model {
    pub(super) fn charges(&self, b: &Bias) -> [D; 4] {
        let p = &self.p;
        let area = self.wcv * self.lcv;
        let oxide = area * self.cox;
        let body = self.phi - b.root.sq();
        let ctm = self.capmod == 3;
        let (mut drive, mut qb) = if self.capmod == 0 {
            let threshold = p.vfbcv + self.phi + self.k1 * b.root;
            let drive = b.gate - threshold;
            if drive.v <= 0. {
                let qg = oxide * depletion(b.gate - body - p.vfbcv, self.k1);
                return [D::cst(0.), qg, D::cst(0.), -qg];
            }
            (drive, -oxide * self.k1 * b.root)
        } else {
            let slope = p.noff * b.swing * self.vt;
            let drive = slope * softplus((b.gate - b.threshold - p.voffcv) / slope);
            let vgb = b.gate - body;
            if self.capmod == 1 {
                (
                    drive,
                    -oxide * depletion(vgb - self.flat_cv - drive, self.k1),
                )
            } else {
                let x = self.flat_cv - vgb - 0.02;
                let acc = 0.5 * (x + (x.sq() + 0.08 * self.flat_cv.abs()).sqrt());
                let dep = depletion(vgb - self.flat_cv + acc - drive, self.k1);
                let cap = if ctm {
                    let xmax = self.debye / 3.;
                    let field = (vgb - self.flat_cv) / (self.tox * 1e8);
                    let xdc = xmax * safe_exp(self.centroid_acde * field);
                    let thickness = smooth_min(D::cst(xmax), xdc, 0.001 * self.tox);
                    oxide / (1. + thickness * (self.cox / EPS_SI))
                } else {
                    D::cst(oxide)
                };
                (drive, cap * (acc - dep))
            }
        };
        let mut cap = D::cst(oxide);
        if ctm {
            let field = floor(
                (b.overdrive + 4. * (b.threshold - self.flat_cv - self.phi)) / (2e8 * self.tox),
                1e-18,
            );
            let thickness = 1.9e-9 / (1. + field.powf(0.7));
            cap = oxide / (1. + thickness * (self.cox / EPS_SI));
            let k = self.k1.max(1e-8);
            let phi_delta = self.vt
                * (drive * (drive + 2. * k * self.phi.sqrt()) / (p.moin * k * k * self.vt)).ln_1p();
            // The 44.2 weak-inversion thermal floor resolves a 1 mV transition
            // between the surface-potential correction and the available drive.
            drive = drive - smooth_min(phi_delta, drive, 0.001);
        }
        if drive.v < 1e-35 {
            return [D::cst(0.), -qb, D::cst(0.), qb];
        }
        let length = if self.capmod < 2 { self.l } else { self.lcv };
        let bulk = floor(b.bulk0 * (1. + (p.clc / length).powf(p.cle)), 0.01);
        let sat = drive / bulk;
        let eff = if self.capmod >= 2 {
            smooth_min(sat, b.drain, 0.02)
        } else if b.drain.v < sat.v {
            b.drain
        } else {
            sat
        };
        // Normalize the partition polynomials to avoid powers of tiny overdrive.
        let t = bulk * eff / drive;
        let denom = 1. - 0.5 * t;
        let correction = t.sq() / (12. * denom);
        let inv = -cap * drive * (denom + correction);
        qb += cap * (1. - bulk) * eff * (0.5 - t / (12. * denom));
        let qs = if self.xpart < 0.5 {
            -0.5 * cap * drive * (1. + t * (-4. / 3. + t * (2. / 3. - (2. / 15.) * t))) / denom.sq()
        } else if self.xpart > 0.5 {
            -cap * drive * (0.5 + 0.25 * t - 0.5 * correction)
        } else {
            0.5 * inv
        };
        [inv - qs, -(qb + inv), qs, qb]
    }

    pub(super) fn overlap_charge(&self, side: usize, v: D) -> D {
        let base = self.overlap[side] * v;
        if self.capmod == 0 {
            return base;
        }
        let k = self.p.ckappa;
        let negative = if self.capmod == 1 {
            if v.v >= 0. {
                return base + self.ldd[side] * v;
            }
            v
        } else {
            let t = v + 0.02;
            0.5 * (t - (t.sq() + 0.08).sqrt())
        };
        base + self.ldd[side]
            * (v - negative + 2. * negative / (1. + (1. - 4. * negative / k).sqrt()))
    }
}
