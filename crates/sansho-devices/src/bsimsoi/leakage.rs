//! Film junctions, lateral transistor, impact generation and oxide currents.
//! Provenance: BSIMSOIv4.4 Users' Manual, equations 3.1–3.25, 4.15–4.16 and 5.1.
use super::{D, Model, channel::Bias};
use crate::{
    bsim4::{floor, smooth_min, softplus},
    junction::{depletion_charge, safe_exp},
};

#[derive(Clone, Copy)]
pub(super) struct Junction {
    pub current: D,
    pub charge: D,
    pub emission: D,
    pub scale: D,
    pub high: D,
}

impl Model {
    pub(super) fn junction(&self, side: usize, v: D, a: &Bias, alpha: f64) -> Junction {
        let s = &self.side[side];
        let p = &self.p;
        let dt = a.temp / self.nominal - 1.;
        // Eq. 5.1: the XBJT=2/1 current ratio gives Eg(300 K)=1.115 eV.
        let gap = 1.115 * dt / a.vt;
        let [nd, nf, nr, nt] = s.ideal;
        let [diff, rec, tun, bjt] = std::array::from_fn(|i| {
            s.width
                * self.tsi
                * s.density[i]
                * safe_exp(if i == 2 {
                    s.xt[i] * dt
                } else {
                    s.xt[i] * gap / if i == 1 { nf } else { nd }
                })
        });
        let inj = safe_exp(v / (nd * a.vt)) - 1.;
        let high = floor(s.ahli * inj, 0.);
        let length = p.lbjt0 * (1. / self.l + 1. / p.ln);
        let injection = bjt * length.powf(p.nbjt) * inj;
        let reverse = |scale: f64, n: D| {
            if scale == 0. {
                D::cst(1.)
            } else {
                safe_exp(-v / (0.026 * n) * scale / floor(scale - v, 1e-3))
            }
        };
        let current = diff * inj
            + rec
                * (safe_exp(v / floor(0.026 * nf * (1. + p.ntrecf * dt), 1e-4))
                    - reverse(s.vrec, floor(nr * (1. + p.ntrecr * dt), 0.01)))
            + tun * (1. - reverse(s.vtun, D::cst(nt)))
            + (1. - alpha) * injection / (1. + high).sqrt();
        // Rescale the normalized depletion integral so its temperature derivative
        // includes both junction potential and zero-bias capacitance.
        let delta = a.temp - self.nominal;
        let potential = floor(s.potential - s.tpb * delta, 0.01);
        // Appendix C junction integral. The forward AC plateau in
        // r17b_junction_cap measures Cj0 / (1 - 0.9)^M.
        let normalized = v / potential;
        let beyond = floor(normalized - 0.9, 0.);
        let depletion = depletion_charge(normalized - beyond, 1., 1., s.grading, 0.9)
            + beyond * 0.1_f64.powf(-s.grading);
        let charge = potential * s.cj * floor(1. + s.tcj * delta, 0.) * depletion
            + p.tt * bjt * (1. + p.ldif0 * length.powf(p.ndif)) * inj / (1. + high).sqrt();
        Junction {
            current,
            charge,
            emission: inj,
            scale: bjt * length.powf(p.nbjt),
            high,
        }
    }
    pub(super) fn impact(&self, a: &Bias, bjt: D, vbd: D) -> D {
        let p = &self.p;
        let gate = p.esatii * self.l / (1. + p.esatii * self.l)
            * (1. / floor(1. + p.sii1 * a.over, 0.01) + p.sii2)
            * p.sii0
            * (a.vg - a.threshold)
            / floor(1. + p.siid * a.vd, 0.01);
        let dt = a.temp / self.nominal - 1.;
        let diff = a.vd - gate - p.vdsatii0 * (1. + p.tii * dt) + p.lii / self.l;
        let den = p.beta2 + p.beta1 * diff + p.beta0 * diff.sq();
        let gain = if p.alpha0 <= 0. || den.v.abs() < 1e-15 {
            D::cst(0.)
        } else {
            -floor(-p.alpha0 * safe_exp(diff / den), -10.)
        };
        if self.ii == 0 {
            gain * (a.ids + p.fbjtii * bjt)
        } else {
            let field = floor(p.vbci * (1. + p.tvbci * dt) - vbd, 1e-15);
            gain * a.ids
                + (p.cbjtii / self.l + p.ebjtii)
                    * bjt
                    * field
                    * safe_exp(-p.abjtii * field.powf(p.mbjtii - 1.))
        }
    }
    pub(super) fn induced(&self, j: usize, end: D, gate: D, rev_body: D) -> D {
        let [a, b, c, e, r, k, f] = self.side[j].gidl;
        let field = (end - (if self.gidl == 0 { 1. } else { r }) * gate - e) / (3. * self.tox);
        if a <= 0. || field.v <= 0. {
            return D::cst(0.);
        }
        let shape = if self.gidl == 1 {
            if k == 0. {
                D::cst(1.)
            } else {
                safe_exp(-k / floor(rev_body + f, 1e-3))
            }
        } else if rev_body.v > 0. && c > 0. {
            let cube = rev_body * rev_body.sq();
            cube / (c + cube)
        } else {
            D::cst(0.)
        };
        a * self.side[j].width * field * (-b / field).exp() * shape
    }
    pub(super) fn tunneling(&self, a: &Bias, vgb: D, vgd: D, vgs: D, reverse: bool) -> [D; 5] {
        let p = &self.p;
        let mut current = [D::cst(0.); 5];
        // Eqs. 3.12–3.15 use the zero-bias flatband and the
        // smoothed oxide drop (Appendix C, accumulation charge).
        let flat = self.vth - a.phi - self.k1 * a.phi.sqrt();
        let d = flat - vgb - 0.02;
        let accumulation =
            0.5 * (d + (d.sq() + 0.08 * if flat.v < 0. { -flat } else { flat }).sqrt());
        let ox = a.over - accumulation + vgb - (a.vg - a.vb)
            + crate::bsim4::depletion(a.vg - flat + accumulation - a.vb - a.over, self.k1);
        if self.igc {
            for (j, v) in [vgd, vgs].into_iter().enumerate() {
                let flat_sd = if self.ngate > 0. {
                    a.vt * (self.ngate / self.nsd).ln() + self.vfbsdoff
                } else {
                    D::cst(0.)
                };
                let aux = ((v - flat_sd).sq() + 0.0001).sqrt();
                current[j] = self.side[j].edge.current(v, aux, aux);
            }
            // 3.15–3.25 use the common BSIM4 exponential barrier and partition.
            let slope = p.nigc * a.vt;
            let ig = self
                .gate
                .current(a.vg, slope * softplus((a.vg - self.vth) / slope), ox);
            let y = p.pigcd * a.vd;
            let exp = safe_exp(-y);
            current[2] = ig * (1. - (1. + y) * exp + 1e-4) / (y.sq() + 2e-4);
            current[3] = ig * (y + exp - 1. + 1e-4) / (y.sq() + 2e-4);
            if reverse {
                current.swap(2, 3)
            }
        }
        if self.igb && self.mode != 2 {
            let (field, aux, alpha, beta, limit, scale, exponent) = if vgb.v >= 0. {
                let field = smooth_min(D::cst(p.voxh), floor(ox, 0.), p.deltavox);
                (
                    field,
                    p.vevb * softplus((field - p.ebg) / p.vevb),
                    p.alphagb1,
                    p.betagb1,
                    p.vgb1,
                    3.7622e-7,
                    3.1051e10,
                )
            } else {
                let field = smooth_min(D::cst(p.voxh), accumulation, p.deltavox);
                (
                    field,
                    p.vecb * softplus((flat - vgb) / p.vecb),
                    p.alphagb2,
                    p.betagb2,
                    p.vgb2,
                    4.9758e-7,
                    2.357e10,
                )
            };
            current[4] =
                (self.l * self.w * self.nf / self.nseg + self.dc_contact_area) * scale * vgb * aux
                    / self.tox.powi(2)
                    * (self.toxref / self.toxqm).powf(p.ntox)
                    * safe_exp(
                        -exponent * self.toxqm * (alpha - beta * field)
                            / floor(1. - field / limit, 0.01),
                    );
        }
        current
    }
}
