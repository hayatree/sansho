//! Quasi-static terminal charge in the channel frame and overlap charge.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, §§7.2–7.5.

use super::{EPS_SI, Size, channel::CvBias, floor, smooth_min, softplus};
use crate::dual::Dual;
use crate::{ModelParams, junction::safe_exp};

#[derive(Debug)]
pub(super) struct ChargeModel {
    mode: u8,
    area: f64,
    pub(super) dimensions: [f64; 2],
    pub(super) bulk_scale: f64,
    noff: f64,
    pub(super) voff: f64,
    mstar: Option<f64>,
    vfbcv: f64,
    xpart: f64,
    moin: f64,
    pub(super) acde: f64,
    edge: [f64; 2],
    kappa: [f64; 2],
    pub(super) flat_zero: f64,
}

impl ChargeModel {
    pub(super) fn new(p: &ModelParams, bin: &super::params::Bin, l: f64, w: f64, nf: f64) -> Self {
        Self {
            mode: p.get("capmod", 2.) as u8,
            area: l * w * nf,
            dimensions: [l, w],
            bulk_scale: 1. + (bin.get("clc", 1e-7) / l).powf(bin.get("cle", 0.6)),
            noff: bin.get("noff", 1.),
            mstar: (p.get("cvchargemod", 0.) == 1.)
                .then(|| 0.5 + bin.get("minvcv", 0.).atan() / std::f64::consts::PI),
            voff: bin.get("voffcv", 0.),
            vfbcv: bin.get("vfbcv", -1.),
            xpart: p.get("xpart", 0.),
            moin: bin.get("moin", 15.),
            acde: bin.get("acde", 1.),
            edge: [bin.get("cgdl", 0.) * w * nf, bin.get("cgsl", 0.) * w * nf],
            kappa: [
                bin.get("ckappad", bin.get("ckappas", 0.6)),
                bin.get("ckappas", 0.6),
            ],
            flat_zero: 0.,
        }
    }
}
impl Size {
    pub(super) fn overlap_charge<const N: usize>(&self, v: Dual<N>, side: usize) -> Dual<N> {
        let q = self.overlap[side] * v;
        if self.cv.mode == 0 {
            return q;
        }
        q + self.cv.edge[side] * ldd_overlap(v, self.cv.kappa[side])
    }

    pub(super) fn charges<const N: usize>(&self, b: &CvBias<N>) -> [Dual<N>; 4] {
        let cv = &self.cv;
        let zero = Dual::cst(0.);
        if cv.xpart < 0. {
            return [zero; 4];
        }
        let abulk = floor(b.abulk0 * cv.bulk_scale, 0.05);
        let (over, base, cap) = if cv.mode == 0 {
            let gate_bulk = b.gate - b.bulk - cv.vfbcv;
            let over = b.gate - cv.vfbcv - self.phi - self.k1ox * (self.phi - b.bulk).sqrt();
            let c = cv.area * self.coxe;
            if gate_bulk.v <= 0. || over.v <= 0. {
                let qg = if gate_bulk.v <= 0. {
                    c * gate_bulk
                } else {
                    c * depletion(gate_bulk, self.k1ox)
                };
                return [zero, qg, zero, -qg];
            }
            (
                over,
                -c * self.k1ox * (self.phi - b.bulk).sqrt(),
                Dual::cst(c),
            )
        } else {
            let nvt = b.swing * self.vt * cv.noff;
            let vgst = b.gate - b.threshold;
            let raw = if let Some(m) = cv.mstar {
                let nv = b.swing * self.vt;
                nv * softplus(m * vgst / nv)
                    / (m + b.swing * self.coxe * self.xdep0 / EPS_SI
                        * safe_exp((cv.voff - (1. - m) * vgst) / nv))
            } else {
                nvt * softplus((vgst - cv.voff) / nvt)
            };
            let flat = cv.flat_zero;
            let delta = flat + b.bulk - b.gate - 0.02;
            let drop = 0.5 * (delta + (delta.sq() + 0.08 * flat.abs().max(1e-30)).sqrt());
            let flat_eff = flat - drop;
            let sub0 = depletion(b.gate - flat_eff - b.bulk - raw, self.k1ox);
            if cv.mode == 1 {
                let cap = cv.area * self.coxe;
                (raw, cap * (drop - sub0), Dual::cst(cap))
            } else {
                let xmax = self.xdep0 * (self.vt / (2. * self.phi)).sqrt() / 3.;
                let field = (b.gate - b.bulk - flat) / (1e8 * self.toxp);
                let depth = smooth_min(
                    Dual::cst(xmax),
                    xmax * safe_exp(self.centroid_field * field),
                    1e-3 * self.toxe,
                );
                let cap_depl = cv.area * self.coxp / (1. + self.coxp / EPS_SI * depth);
                let offset = self.vt
                    * (1.
                        + raw * (raw + 2. * self.k1ox * self.phi.sqrt())
                            / (cv.moin * self.k1ox.powi(2) * self.vt).max(1e-20))
                    .ln();
                let shifted = raw - offset - 0.001;
                let over = 0.5 * (shifted + (shifted.sq() + 0.004 * raw).sqrt());
                (
                    over,
                    cap_depl * (drop - sub0),
                    cv.area * self.centroid_cap(raw),
                )
            }
        };
        if over.v <= 1e-25 {
            return [zero, -base, zero, base];
        }
        let sat = over / abulk;
        let vd = if cv.mode == 0 {
            if b.drain.v > sat.v { sat } else { b.drain }
        } else {
            smooth_min(sat, b.drain, 0.02)
        };
        partition(over, abulk, vd, cap, base, cv.xpart)
    }
}

// Ward-Dutton channel-charge integrals shared with BSIMSOI §4.2.
pub(crate) fn partition<const N: usize>(
    over: Dual<N>,
    abulk: Dual<N>,
    vd: Dual<N>,
    cap: Dual<N>,
    base: Dual<N>,
    xpart: f64,
) -> [Dual<N>; 4] {
    // Normalize by the overdrive before taking powers in the partition integral.
    let y = abulk * vd / over;
    let mean = 1. - 0.5 * y;
    let corr = y / (12. * mean);
    let inversion = -cap * over * (mean + y * corr);
    let bulk = base + cap * (1. - abulk) * vd * (0.5 - corr);
    let drain = if xpart == 0.5 {
        0.5 * inversion
    } else if xpart > 0.5 {
        -0.5 * cap * over * (1. - 1.5 * y + 3. * y * corr)
    } else {
        -0.5 * cap * over * (1. - (5. / 3.) * y + y.sq() * (1. - 0.2 * y)) / mean.sq()
    };
    [drain, -bulk - inversion, inversion - drain, bulk]
}

pub(crate) fn depletion<const N: usize>(arg: Dual<N>, k1: f64) -> Dual<N> {
    if arg.v < 0. {
        arg
    } else if k1 == 0. {
        Dual::cst(0.)
    } else {
        k1 * ((arg + 0.25 * k1 * k1).sqrt() - 0.5 * k1)
    }
}

// Bias-dependent lightly doped diffusion overlap (BSIMSOI §4.4).
pub(crate) fn ldd_overlap<const N: usize>(v: Dual<N>, kappa: f64) -> Dual<N> {
    let k = kappa.max(0.01);
    let sum = v + 0.02;
    let edge = 0.5 * (sum - (sum.sq() + 0.08).sqrt());
    v - edge - 0.5 * k * ((1. - 4. * edge / k).sqrt() - 1.)
}
