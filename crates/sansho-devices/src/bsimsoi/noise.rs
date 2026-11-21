//! One-sided local channel and junction noise for the SOI adjoint analysis.
//! Provenance: BSIMSOIv4.4 Users' Manual, chapter 8, equations 8.1–8.11.
use super::{Model, Point};
use crate::{BOLTZMANN, CHARGE, ModelParams};

#[derive(Debug)]
pub(super) struct Noise {
    flicker: u8,
    thermal: u8,
    kf: f64,
    af: f64,
    bf: f64,
    pub ef: f64,
    w0: f64,
    traps: [f64; 3],
    em: f64,
    nt: f64,
    r: [f64; 2],
    t: [f64; 2],
    pub noif: f64,
}
impl Noise {
    pub fn bind(c: &ModelParams, sign: f64) -> Result<Self, String> {
        let f = c.get("fnoimod", 1.);
        let t = c.get("tnoimod", 0.);
        if ![0., 1.].contains(&f) || ![0., 1., 2.].contains(&t) {
            return Err("invalid fnoimod or tnoimod".into());
        }
        Ok(Self {
            flicker: f as u8,
            thermal: t as u8,
            kf: c.get("kf", 0.),
            af: c.get("af", 1.),
            bf: c.get("bf", 2.),
            ef: c.get("ef", 1.),
            w0: c.get("w0flk", 1e-5),
            traps: [
                c.get("noia", if sign > 0. { 6.25e41 } else { 6.188e40 }),
                c.get("noib", if sign > 0. { 3.125e26 } else { 1.5e25 }),
                c.get("noic", 8.75e9),
            ],
            em: c.get("em", 4.1e7),
            nt: c.get("ntnoi", 1.),
            r: [c.get("rnoia", 0.577), c.get("rnoib", 0.37)],
            t: [c.get("tnoia", 1.5), c.get("tnoib", 3.5)],
            noif: c.get("noif", 1.),
        })
    }
    pub fn powers(&self, m: &Model, v: &Point) -> [f64; 3] {
        let a = &v.bias;
        let kt = BOLTZMANN * a.temp.v;
        let width = m.w * m.nf;
        let ids = a.ids.v.abs();
        let (gm, gb, gd) = (a.ids.d[1].abs(), a.ids.d[3].abs(), a.ids.d[v.drain].abs());
        let mut source_voltage = 0.;
        let conductance = match self.thermal {
            0 => {
                let g = a.mu.v * v.qinv.abs() / m.l.powi(2);
                self.nt * g / (1. + a.rds.v * g)
            }
            1 => {
                let field = m.l * (a.over.v / (a.esat.v * m.l)).powi(2);
                let beta = self.r[0] * (1. + self.t[0] * field);
                let theta = self.r[1] * (1. + self.t[1] * field);
                source_voltage = 4. * kt * theta.powi(2) / (m.copies * a.conductance.v.max(1e-30));
                ((gd + beta * (gm + gb)).powi(2) - (theta * (gm + gb + gd)).powi(2))
                    / a.conductance.v.max(1e-30)
            }
            _ => 2. / 3. * self.nt * (gm + gb + gd),
        };
        let flicker = if self.flicker == 0 {
            (width / self.w0).powf(1. - self.af) * self.kf * ids.powf(self.af)
                / (m.cox * m.l.powf(self.bf))
        } else {
            let n0 = m.cox * a.over.v / CHARGE;
            let nl = n0 * (1. - a.bulk.v * a.eff.v / (a.over.v + 2. * a.vt.v)).max(0.);
            let star = a.vt.v * m.cox * a.swing.v / CHARGE;
            let [na, nb, nc] = self.traps;
            let integral = na * ((n0 - nl) / (nl + star)).ln_1p()
                + nb * (n0 - nl)
                + 0.5 * nc * (n0 - nl) * (n0 + nl);
            let litl = (super::EPS_SI / m.cox * m.tsi).sqrt();
            let dl = litl
                * (((a.vd.v - a.eff.v) / litl + self.em) / a.esat.v)
                    .max(1.)
                    .ln();
            let strong = kt / (m.l.powi(2) * 1e10)
                * (CHARGE.powi(2) * a.mu.v * ids / (m.cox * a.bulk.v) * integral
                    + ids.powi(2) * dl / width * (na + nb * nl + nc * nl * nl)
                        / (nl + star).powi(2));
            let weak = na * kt * ids.powi(2) / (width * m.l * star.powi(2) * 1e10);
            if strong + weak > 0. {
                strong * weak / (strong + weak)
            } else {
                0.
            }
        };
        [
            4. * kt * conductance.max(0.) * m.copies,
            flicker.max(0.) * m.copies,
            source_voltage,
        ]
    }
}
