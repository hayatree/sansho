//! Diffusion junction IV and integrated area, field-side and gate-side charge.
//! Provenance: UC Berkeley BSIM3v3.3 manual, §§9.1–9.2 and Appendix D.

use super::{D, params::Bins};
use crate::{junction::vcrit, mos1::Mos1Geometry, temperature::gap, thermal_voltage};

#[derive(Debug)]
pub(super) struct Junction {
    isat: f64,
    pub nvt: f64,
    pub critical: f64,
    knee: f64,
    limit: f64,
    layers: [(f64, f64, f64); 3],
}

impl Junction {
    pub fn pair(
        bin: &Bins<'_>,
        g: Mos1Geometry,
        w: f64,
        t: f64,
        nom: f64,
    ) -> Result<[Self; 2], String> {
        let p = bin.card;
        let nj = bin.get("nj", 1.);
        let limit = p.get("ijth", 0.1);
        let dt = t - nom;
        let nvt = nj * thermal_voltage(t);
        let scale = ((gap(nom) / thermal_voltage(nom) - gap(t) / thermal_voltage(t)
            + bin.get("xti", 3.) * (t / nom).ln())
            / nj)
            .exp();
        let js = p.get("js", 1e-4) * scale;
        // The 44.2 temperature sweep follows area-only saturation current.
        p.accept(&["jssw"]);
        let cj = p.get("cj", 5e-4);
        let csw = p.get("cjsw", 5e-10);
        let psw = p.get("pbsw", 1.);
        let msw = p.get("mjsw", 0.33);
        let regions = [
            (
                cj * (1. + dt * p.get("tcj", 0.)),
                p.get("pb", 1.) - dt * p.get("tpb", 0.),
                p.get("mj", 0.5),
            ),
            (
                csw * (1. + dt * p.get("tcjsw", 0.)),
                psw - dt * p.get("tpbsw", 0.),
                msw,
            ),
            (
                p.get("cjswg", csw) * (1. + dt * p.get("tcjswg", 0.)),
                p.get("pbswg", psw) - dt * p.get("tpbswg", 0.),
                p.get("mjswg", msw),
            ),
        ];
        if nj <= 0.
            || limit < 0.
            || !js.is_finite()
            || regions.iter().any(|&(c, v, m)| c < 0. || v <= 0. || m < 0.)
        {
            return Err("invalid junction coefficient at operating temperature".into());
        }
        Ok([(g.ad, g.pd), (g.as_, g.ps)].map(|(area, perimeter)| {
            let isat = (area * js).max(0.);
            let geometry = [area, (perimeter - w).max(0.), perimeter.min(w)];
            Self {
                isat,
                limit,
                nvt,
                critical: vcrit(isat.max(1e-30), nvt),
                knee: if limit > 0. && isat > 0. {
                    nvt * (limit / isat).ln_1p()
                } else {
                    f64::INFINITY
                },
                layers: std::array::from_fn(|i| {
                    (regions[i].0 * geometry[i], regions[i].1, regions[i].2)
                }),
            }
        }))
    }

    pub fn evaluate(&self, v: D, gmin: f64) -> (D, D) {
        let i = if self.isat == 0. {
            D::cst(0.)
        } else if v.v > self.knee {
            self.limit + (self.limit + self.isat) / self.nvt * (v - self.knee)
        } else {
            let x = v / self.nvt;
            // Combine the logarithms before a large forward exponential. The
            // physical current can still be representable when exp(x) overflows.
            if x.v > 80. {
                (x + self.isat.ln()).exp() - self.isat
            } else {
                self.isat * x.exp_m1()
            }
        };
        let mut q = D::cst(0.);
        for (c, pb, m) in self.layers {
            if c == 0. {
                continue;
            }
            q += if v.v >= 0. {
                c * v * (1. + 0.5 * m / pb * v)
            } else if m == 1. {
                -c * pb * (-v / pb).ln_1p()
            } else {
                -c * pb / (1. - m) * ((1. - m) * (-v / pb).ln_1p()).exp_m1()
            };
        }
        (i + gmin * v, q)
    }
}
