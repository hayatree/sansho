//! Positive-drain silicon channel and substrate current.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, equations 1.7, 2.40–45, 3.4–8,
//! 3.21–22, 5.1–9, 5.17, 5.23–58 and 6.1–8.

use super::{D, EPS_SI, Size, floor, smooth_min, softplus, theta};
use crate::junction::safe_exp;

pub(super) struct Bias {
    pub beta: D,
    pub id_per_volt: D,
    pub over: D,
    pub vd: D,
    pub gate: D,
    pub bulk: D,
    pub drain: D,
    pub threshold: D,
    pub swing: D,
    pub abulk0: D,
}

impl Size {
    pub(super) fn gate_voltage(&self, vgs: D) -> D {
        let flat = self.vfb + self.phi;
        if vgs.v <= flat || self.poly == 0. {
            return vgs;
        }
        let over = vgs - flat;
        let oxide = 2. * over / (1. + (1. + 2. * over / self.poly).sqrt());
        let poly_drop = 0.5 * oxide.sq() / self.poly;
        // Strong poly inversion bounds the surface potential near the silicon band gap.
        vgs - smooth_min(D::cst(1.12), poly_drop, 0.05)
    }

    pub(super) fn centroid_cap(&self, over: D) -> D {
        // The empirical electric field is expressed in MV/cm.
        let field = (over + 4. * (self.vth0 - self.vfb - self.phi).max(0.)) / (2e8 * self.toxp);
        let depth = self.ados * 1.9e-9 / (1. + floor(field, 1e-30).powf(0.7 * self.bdos));
        self.coxp / (1. + self.coxp / EPS_SI * depth)
    }

    pub(super) fn channel(&self, gate: D, drain: D, body: D) -> (D, D, Bias) {
        let p = &self.p;
        let gate = self.gate_voltage(gate);
        let low = body - self.vbc - 0.001;
        let bulk = self.vbc + 0.5 * (low + (low.sq() - 0.004 * self.vbc).sqrt());
        let bulk = smooth_min(D::cst(0.95 * self.phi), bulk, 0.001);
        let sqrt_phi = self.phi.sqrt();
        let surface = (self.phi - bulk).sqrt();
        let xdep = self.xdep0 * surface / sqrt_phi;
        let length_scale = (EPS_SI / self.coxe * xdep).sqrt();
        let zero_scale = (EPS_SI / self.coxe * self.xdep0).sqrt();
        let short = theta(p.dvt1 * self.l / (length_scale * floor(1. + p.dvt2 * bulk, 0.05)));
        let narrow =
            theta(p.dvt1w * self.l * self.w / (length_scale * floor(1. + p.dvt2w * bulk, 0.05)));
        let swing = floor(
            1. + (p.nfactor * EPS_SI / xdep
                + p.cit
                + (p.cdsc + p.cdscb * bulk + p.cdscd * drain) * short)
                / self.coxe,
            0.1,
        );
        let pocket = (1. + p.lpeb / self.l).sqrt();
        let width_shift = self.toxe * self.phi / (self.w + p.w0);
        let mut threshold = self.vth0 + (self.k1ox * surface - self.k1 * sqrt_phi) * pocket
            - self.k2ox * bulk
            + self.k1ox * ((1. + p.lpe0 / self.l).sqrt() - 1.) * sqrt_phi
            + (p.k3 + p.k3b * bulk) * width_shift
            - (p.dvt0 * short + p.dvt0w * narrow) * (self.vbi - self.phi)
            - theta(D::cst(self.dsub * self.l / zero_scale)) * (p.eta0 + p.etab * bulk) * drain
            + self.vth_temp
            + self.body_temp * bulk;
        if p.dvtp0 > 0. {
            threshold += swing
                * self.vt_pocket
                * (1. + p.dvtp0 / self.l * (1. + safe_exp(-p.dvtp1 * drain))).ln();
        }
        if p.dvtp4 != 0. {
            let e = safe_exp(-2. * p.dvtp4 * drain);
            threshold += -(p.dvtp5 + p.dvtp2 / self.l.powf(p.dvtp3)) * (1. - e) / (1. + e);
        }
        let nvt = swing * self.vt;
        let mstar = 0.5 + p.minv.atan() / std::f64::consts::PI;
        let vgst = gate - threshold;
        let over = nvt * softplus(mstar * vgst / nvt)
            / (mstar
                + swing
                    * (self.coxe * self.xdep0 / EPS_SI)
                    * safe_exp((p.voff - (1. - mstar) * vgst) / nvt));
        let weff = floor(
            self.w - 2. * (p.dwg * over + p.dwb * (surface - sqrt_phi)),
            1e-10,
        );
        let body_factor = pocket * self.k1ox / (2. * surface) + self.k2ox - p.k3b * width_shift;
        let ratio = self.l / (self.l + 2. * (p.xj * xdep).sqrt());
        let body_den = floor(1. + p.keta * bulk, 0.05);
        let abulk0 = (1. + body_factor * (p.a0 * ratio + p.b0 / (self.w + p.b1))) / body_den;
        let abulk = floor(
            abulk0 - body_factor * p.a0 * p.ags * ratio * ratio.sq() * over / body_den,
            0.05,
        );
        let coef = if self.sign > 0. { 2. } else { 2.5 };
        let built_in = coef * (self.vth0 - self.vfb - self.phi);
        let scattering = if self.mobmod >= 4 {
            D::cst(built_in)
        } else {
            threshold
        };
        let field = if self.mobmod >= 4 {
            (over + built_in) / self.toxe
        } else {
            (over + 2. * threshold) / self.toxe
        };
        let coulomb = if self.mobmod == 3 {
            let n0 = 1.
                + (p.nfactor * EPS_SI / self.xdep0
                    + p.cit
                    + p.cdsc * theta(D::cst(p.dvt1 * self.l / zero_scale)).v)
                    / self.coxe;
            let v0 = n0 * self.vt * std::f64::consts::LN_2
                / (mstar + n0 * self.coxe * self.xdep0 / EPS_SI * (p.voff / (n0 * self.vt)).exp());
            2. * p.ud / (1. + over / v0).powf(self.ucs)
        } else {
            p.ud * (scattering * self.toxe / (over + 2. * (scattering.sq() + 0.0001).sqrt())).sq()
        };
        let degradation = match self.mobmod {
            1 | 5 => (self.ua * field + self.ub * field.sq()) * (1. + self.uc * bulk),
            2 | 3 | 6 => {
                let tox = if self.mobmod == 3 {
                    6. * self.toxe
                } else {
                    self.toxe
                };
                (self.ua + self.uc * bulk) * floor((over + built_in) / tox, 1e-20).powf(self.eu)
            }
            _ => (self.ua + self.uc * bulk) * field + self.ub * field.sq(),
        };
        let mobility = self.u0 / floor(1. + degradation + coulomb, 0.05);
        let esat_l = 2. * self.vsat * self.l / mobility;
        let rds = (self.rmin
            + p.rdsw
                * floor(
                    p.prwb * (surface - sqrt_phi) + 1. / floor(1. + p.prwg * over, 0.05),
                    0.,
                ))
            / (1e6 * self.wj).powf(p.wr);
        let drive = over + 2. * self.vt;
        let lambda = floor(p.a1 * over + p.a2, 0.01);
        let coxeff = self.centroid_cap(over);
        let load = weff * self.vsat * self.coxe * rds;
        let a = abulk * (abulk * load + 1. / lambda - 1.);
        let b = drive * (2. / lambda - 1.) + abulk * esat_l + 3. * abulk * drive * load;
        let c = drive * (esat_l + 2. * drive * load);
        let sat = 2. * c / (b + floor(b.sq() - 4. * a * c, 1e-30).sqrt());
        let vd = smooth_min(sat, drain, p.delta);
        let excess = drain - vd;
        let g0 = mobility * coxeff * (weff / self.l) * over * (1. - 0.5 * abulk * vd / drive)
            / (1. + vd / esat_l);
        let resistance_factor = 1. + rds * g0;
        let mut conductance = g0 / resistance_factor * self.nf;
        let litl = (EPS_SI / self.coxe * p.xj).sqrt();
        let pocket_factor = 1. / (1. + p.fprout.max(0.) * self.l.sqrt() / drive);
        let gate_factor = floor(1. + p.pvag * over / esat_l, 0.05);
        if p.pclm > 0. {
            let cclm =
                pocket_factor * gate_factor * resistance_factor * (1. + sat / esat_l) * self.l
                    / (p.pclm * litl);
            let va = (esat_l + sat + 2. * load * over * (1. - 0.5 * abulk * sat / drive))
                / (load * abulk - 1. + 2. / lambda);
            conductance = conductance * (1. + (1. + cclm * excess / va).ln() / cclm);
        }
        let rout = p.pdiblc1 * theta(D::cst(p.drout * self.l / zero_scale)) + p.pdiblc2;
        if rout.v > 0. {
            let inv_va =
                rout * (1. + p.pdiblcb * bulk) * (drive + abulk * sat) / (drive.sq() * gate_factor);
            conductance = conductance * (1. + excess * inv_va);
        }
        if p.pdits > 0. {
            let va = pocket_factor
                * (1. + (1. + self.pditsl * self.l) * safe_exp(p.pditsd * drain))
                / p.pdits;
            conductance = conductance * (1. + excess / va);
        }
        let mut substrate = D::cst(0.);
        if excess.v > 0. {
            if p.alpha0 + p.alpha1 * self.l > 0. && p.beta0 >= 0. {
                substrate = vd
                    * conductance
                    * (p.alpha0 / self.l + p.alpha1)
                    * excess
                    * (-p.beta0 / excess).exp();
            }
            if p.pscbe2 > 0. && p.pscbe1 > 0. {
                conductance = conductance
                    * (1. + excess * (p.pscbe2 / self.l) * (-p.pscbe1 * litl / excess).exp());
            }
        }
        if self.overshoot > 0. {
            let field = 1. + excess * self.l / (esat_l * litl);
            let factor =
                1. + self.overshoot / (self.l * mobility) * (field.sq() - 1.) / (field.sq() + 1.);
            conductance = conductance * (1. + vd / esat_l) / (1. + vd / (esat_l * factor));
        }
        if self.ballistic_speed > 0. && over.v > 1e-25 {
            let speed = vd * conductance / (self.nf * weff * coxeff * over);
            conductance = conductance / (1. + (speed / self.ballistic_speed).sq().sq()).powf(0.25);
        }
        (
            vd * conductance,
            substrate,
            Bias {
                beta: self.nf * mobility * coxeff * weff / self.l,
                id_per_volt: conductance,
                over,
                vd,
                gate,
                bulk,
                drain,
                threshold,
                swing,
                abulk0,
            },
        )
    }

    pub(super) fn gidl(&self, side: usize, drain: D, gate: D, reverse_body: D) -> D {
        let [a, b, c, e] = self.leakage[side];
        if let Some(shape) = self.gidl_shape {
            let [r, k, f] = shape[side];
            let field = (drain - r * gate - e) / (3. * self.toxe);
            if field.v <= 0. || a <= 0. || b <= 0. {
                return D::cst(0.);
            }
            let body = -floor(reverse_body + f, -self.gidl_clamp);
            return a * self.wj * self.nf * field * (-b / field).exp() * safe_exp(k / body);
        }
        let field = (drain - gate - e) / (3. * self.toxe);
        if field.v <= 0. || reverse_body.v <= 0. || a <= 0. || b <= 0. || c <= 0. {
            return D::cst(0.);
        }
        let body = reverse_body * reverse_body.sq();
        a * self.wj * self.nf * field * (-b / field).exp() * body / (c + body)
    }
}
