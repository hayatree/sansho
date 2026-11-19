//! Source and drain diffusion IV and depletion charge.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, chapters 11, 12.2.1 and 13.5–7.

use super::params::bandgap;
use crate::{
    ModelParams,
    dual::Dual,
    junction::{depletion_charge, safe_exp},
    thermal_voltage,
};

#[derive(Debug)]
pub(super) struct Junction {
    mode: u8,
    pub(super) isat: f64,
    pub(super) nvt: f64,
    pub(super) vcrit: f64,
    bv: f64,
    xjbv: f64,
    forward: f64,
    reverse: f64,
    cap: [[f64; 3]; 3],
    forward_cap: [f64; 2],
    traps: [[f64; 3]; 3],
}

impl Junction {
    pub(super) fn pair(
        p: &ModelParams,
        area: [f64; 2],
        perim: [f64; 2],
        width: f64,
        nf: f64,
        t: f64,
        tn: f64,
    ) -> [Self; 2] {
        let edge = width * nf;
        let mode = p.get("diomod", 1.) as u8;
        std::array::from_fn(|side| {
            let get = |src: &str, drain: &str, default: f64| {
                let s = if src == "pbs" {
                    p.opt(&["pb", "pbs"]).unwrap_or(default)
                } else {
                    p.get(src, default)
                };
                if side == 0 { p.get(drain, s) } else { s }
            };
            let n = get("njs", "njd", 1.);
            let nvt = n * thermal_voltage(t);
            let jtemp = ((bandgap(tn) / thermal_voltage(tn) - bandgap(t) / thermal_voltage(t)
                + get("xtis", "xtid", 3.) * (t / tn).ln())
                / n)
                .exp();
            let surface = area[side] * get("jss", "jsd", 1e-4)
                + perim[side].max(0.) * get("jsws", "jswd", 0.);
            let gate_edge = edge * get("jswgs", "jswgd", 0.);
            let isat = jtemp * (surface + gate_edge);
            let bv = get("bvs", "bvd", 10.);
            let xjbv = get("xjbvs", "xjbvd", 1.);
            let fwd = get("ijthsfwd", "ijthdfwd", 0.1);
            let rev = get("ijthsrev", "ijthdrev", 0.1);
            let forward = if isat > 0. {
                nvt * (fwd / isat).ln_1p()
            } else {
                f64::INFINITY
            };
            let reverse = if isat > 0. && xjbv > 0. && rev > isat {
                -bv - nvt * ((rev / isat - 1.) / xjbv).ln()
            } else {
                f64::NEG_INFINITY
            };
            let cap = std::array::from_fn(|k| {
                let (cs, cd, ps, pd, ms, md, tc, tp, geom, c0, m0) = match k {
                    0 => (
                        "cjs", "cjd", "pbs", "pbd", "mjs", "mjd", "tcj", "tpb", area[side], 5e-4,
                        0.5,
                    ),
                    1 => (
                        "cjsws",
                        "cjswd",
                        "pbsws",
                        "pbswd",
                        "mjsws",
                        "mjswd",
                        "tcjsw",
                        "tpbsw",
                        perim[side],
                        5e-10,
                        0.33,
                    ),
                    _ => (
                        "cjswgs",
                        "cjswgd",
                        "pbswgs",
                        "pbswgd",
                        "mjswgs",
                        "mjswgd",
                        "tcjswg",
                        "tpbswg",
                        edge,
                        p.get("cjsws", 5e-10),
                        p.get("mjsws", 0.33),
                    ),
                };
                [
                    geom * get(cs, cd, c0) * (1. + p.get(tc, 0.) * (t - tn)).max(0.),
                    (get(ps, pd, 1.) - p.get(tp, 0.) * (t - tn)).max(0.01),
                    get(ms, md, m0),
                ]
            });
            // In forward bias all three capacitances are affine in V. Combine the
            // signed perimeter correction before enforcing a passive continuation.
            let forward_cap = [
                cap.iter().map(|&[c, _, _]| c).sum::<f64>().max(0.),
                cap.iter().map(|&[c, p, m]| c * m / p).sum::<f64>().max(0.),
            ];
            let traps = std::array::from_fn(|k| {
                let (js, jd, vs, vd, ns, nd, xs, xd, ts, td, geo) = match k {
                    0 => (
                        "jtss", "jtsd", "vtss", "vtsd", "njts", "njtsd", "xtss", "xtsd", "tnjts",
                        "tnjtsd", area[side],
                    ),
                    1 => (
                        "jtssws",
                        "jtsswd",
                        "vtssws",
                        "vtsswd",
                        "njtssw",
                        "njtsswd",
                        "xtssws",
                        "xtsswd",
                        "tnjtssw",
                        "tnjtsswd",
                        perim[side].max(0.),
                    ),
                    _ => (
                        "jtsswgs",
                        "jtsswgd",
                        "vtsswgs",
                        "vtsswgd",
                        "njtsswg",
                        "njtsswgd",
                        "xtsswgs",
                        "xtsswgd",
                        "tnjtsswg",
                        "tnjtsswgd",
                        edge * (1. + (p.get("jtweff", 0.).max(0.) / width).sqrt()),
                    ),
                };
                let rel = t / tn - 1.;
                let density = get(js, jd, 0.)
                    * (bandgap(tn) / thermal_voltage(t) * get(xs, xd, 0.02) * rel).exp();
                [
                    geo * density,
                    get(vs, vd, 10.).max(1e-6),
                    thermal_voltage(tn) * get(ns, nd, 20.) * (1. + get(ts, td, 0.) * rel),
                ]
            });
            Self {
                traps,
                forward_cap,
                mode,
                isat,
                vcrit: crate::junction::vcrit(isat.max(1e-30), nvt),
                nvt,
                bv,
                xjbv,
                forward,
                reverse,
                cap,
            }
        })
    }
    fn exponential<const N: usize>(&self, v: Dual<N>) -> Dual<N> {
        let forward = safe_exp(v / self.nvt) - 1.;
        let correction = if self.mode == 1 {
            Dual::cst(0.)
        } else {
            self.xjbv
                * (Dual::cst((-self.bv / self.nvt).exp()) - safe_exp(-(v + self.bv) / self.nvt))
        };
        (forward + correction) * self.isat
    }
    pub(super) fn eval<const N: usize>(&self, v: Dual<N>, gmin: f64) -> (Dual<N>, Dual<N>) {
        let bound = if self.mode != 0 && v.v > self.forward {
            Some(self.forward)
        } else if self.mode == 2 && v.v < self.reverse {
            Some(self.reverse)
        } else {
            None
        };
        let current = if self.isat == 0. {
            Dual::cst(0.)
        } else if let Some(b) = bound {
            let at = self.exponential(Dual::<1>::var(b, 0));
            at.v + (v - b) * at.d[0]
        } else {
            self.exponential(v)
        };
        let q = if v.v >= 0. {
            v * (self.forward_cap[0] + 0.5 * self.forward_cap[1] * v)
        } else {
            // A reverse-biased depletion region needs a nonnegative physical extent.
            self.cap.iter().fold(Dual::cst(0.), |sum, &[c, p, m]| {
                sum + depletion_charge(v, c.max(0.), p, m, 0.)
            })
        };
        let mut total = current + gmin * v;
        for [amplitude, voltage, slope] in self.traps {
            if amplitude > 0. && slope > 0. {
                // On the forward side the reverse tunneling branch approaches its asymptote.
                let den = voltage - v;
                let trap = if den.v <= 1e-4 * voltage {
                    Dual::cst(-1.)
                } else {
                    safe_exp(-v * voltage / (slope * den)) - 1.
                };
                total += -amplitude * trap;
            }
        }
        (total, q)
    }
}
