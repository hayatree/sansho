//! Positive-drain silicon channel and substrate current.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, equations 1.7, 2.40–45, 3.4–8,
//! 3.21–22, 5.1–9, 5.17, 5.23–58 and 6.1–8.

use super::{EPS_SI, Size, floor, smooth_min, softplus, theta};
use crate::dual::Dual;
use crate::junction::safe_exp;

pub(super) struct Bias<const N: usize> {
    pub sat: f64,
    pub mobility: f64,
    pub width: f64,
    pub abulk: f64,
    pub rds: f64,
    pub beta: Dual<N>,
    pub id_per_volt: Dual<N>,
    pub over: Dual<N>,
    pub vd: Dual<N>,
    pub cv: CvBias<N>,
}

#[derive(Clone, Copy)]
pub(super) struct CvBias<const N: usize> {
    pub gate: Dual<N>,
    pub bulk: Dual<N>,
    pub drain: Dual<N>,
    pub threshold: Dual<N>,
    pub swing: Dual<N>,
    pub abulk0: Dual<N>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Body<const N: usize> {
    bulk: Dual<N>,
    surface: Dual<N>,
    xdep: Dual<N>,
    short: Dual<N>,
    narrow: Dual<N>,
    abulk0: Dual<N>,
    abulk_slope: Dual<N>,
}
impl Body<1> {
    fn at_zero<const N: usize>(&self, body: Dual<N>) -> Body<N> {
        let lift = |x: Dual<1>| Dual {
            v: x.v,
            d: body.d.map(|p| p * x.d[0]),
        };
        Body {
            bulk: lift(self.bulk),
            surface: lift(self.surface),
            xdep: lift(self.xdep),
            short: lift(self.short),
            narrow: lift(self.narrow),
            abulk0: lift(self.abulk0),
            abulk_slope: lift(self.abulk_slope),
        }
    }
}

impl Size {
    pub(super) fn gate_voltage<const N: usize>(&self, vgs: Dual<N>) -> Dual<N> {
        let flat = self.vfb + self.phi;
        if vgs.v <= flat || self.poly == 0. {
            return vgs;
        }
        let over = vgs - flat;
        let oxide = 2. * over / (1. + (1. + 2. * over / self.poly).sqrt());
        let poly_drop = 0.5 * oxide.sq() / self.poly;
        // Strong poly inversion bounds the surface potential near the silicon band gap.
        vgs - smooth_min(Dual::cst(1.12), poly_drop, 0.05)
    }

    pub(super) fn centroid_cap<const N: usize>(&self, over: Dual<N>) -> Dual<N> {
        // The empirical electric field is expressed in MV/cm.
        let field = (over + 4. * (self.vth0 - self.vfb - self.phi).max(0.)) / (2e8 * self.toxp);
        let depth = self.ados * 1.9e-9 / (1. + floor(field, 1e-30).powf(0.7 * self.bdos));
        self.coxp / (1. + self.coxp / EPS_SI * depth)
    }

    pub(super) fn body_bias<const N: usize>(&self, body: Dual<N>) -> Body<N> {
        let p = &self.p;
        let low = body - self.vbc - 0.001;
        let bulk = self.vbc + 0.5 * (low + (low.sq() - 0.004 * self.vbc).sqrt());
        let bulk = smooth_min(Dual::cst(0.95 * self.phi), bulk, 0.001);
        let sqrt_phi = self.sqrt_phi;
        let surface = (self.phi - bulk).sqrt();
        let xdep = self.xdep0 * surface / sqrt_phi;
        let length_scale = (EPS_SI / self.coxe * xdep).sqrt();
        let short = theta(p.dvt1 * self.l / (length_scale * floor(1. + p.dvt2 * bulk, 0.05)));
        let narrow = if p.dvt0w == 0. {
            Dual::cst(0.)
        } else {
            theta(p.dvt1w * self.l * self.w / (length_scale * floor(1. + p.dvt2w * bulk, 0.05)))
        };
        let body_factor =
            self.pocket * self.k1ox / (2. * surface) + self.k2ox - p.k3b * self.width_shift;
        let ratio = self.l / (self.l + 2. * (p.xj * xdep).sqrt());
        let body_den = floor(1. + p.keta * bulk, 0.05);
        let abulk0 = (1. + body_factor * (p.a0 * ratio + p.b0 / (self.w + p.b1))) / body_den;
        Body {
            bulk,
            surface,
            xdep,
            short,
            narrow,
            abulk0,
            abulk_slope: body_factor * p.a0 * p.ags * ratio * ratio.sq() / body_den,
        }
    }

    pub(super) fn electrostatics<const N: usize>(
        &self,
        gate: Dual<N>,
        drain: Dual<N>,
        body: Dual<N>,
    ) -> (CvBias<N>, Dual<N>, Dual<N>) {
        let p = &self.p;
        let gate = self.gate_voltage(gate);
        let Body {
            bulk,
            surface,
            xdep,
            short,
            narrow,
            abulk0,
            abulk_slope,
        } = if let Some(zero) = self.zero_body.as_ref().filter(|_| body.v == 0.) {
            zero.at_zero(body)
        } else {
            self.body_bias(body)
        };
        let sqrt_phi = self.sqrt_phi;
        let swing = floor(
            1. + (p.nfactor * EPS_SI / xdep
                + p.cit
                + (p.cdsc + p.cdscb * bulk + p.cdscd * drain) * short)
                / self.coxe,
            0.1,
        );
        let pocket = self.pocket;
        let width_shift = self.width_shift;
        let mut threshold = self.vth0 + (self.k1ox * surface - self.k1 * sqrt_phi) * pocket
            - self.k2ox * bulk
            + self.pocket_shift
            + (p.k3 + p.k3b * bulk) * width_shift
            - (p.dvt0 * short + p.dvt0w * narrow) * (self.vbi - self.phi)
            - self.dibl_factor * (p.eta0 + p.etab * bulk) * drain
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
        (
            CvBias {
                gate,
                bulk,
                drain,
                threshold,
                swing,
                abulk0,
            },
            surface,
            abulk_slope,
        )
    }

    pub(super) fn channel<const N: usize>(
        &self,
        gate: Dual<N>,
        drain: Dual<N>,
        body: Dual<N>,
    ) -> (Dual<N>, Dual<N>, Bias<N>) {
        if self.basic_transport {
            self.transport::<N, true>(gate, drain, body)
        } else {
            self.transport::<N, false>(gate, drain, body)
        }
    }

    fn transport<const N: usize, const BASIC: bool>(
        &self,
        gate: Dual<N>,
        drain: Dual<N>,
        body: Dual<N>,
    ) -> (Dual<N>, Dual<N>, Bias<N>) {
        let p = &self.p;
        let (cv, surface, abulk_slope) = self.electrostatics(gate, drain, body);
        let CvBias {
            gate,
            bulk,
            threshold,
            swing,
            abulk0,
            ..
        } = cv;
        let sqrt_phi = self.sqrt_phi;
        let nvt = swing * self.vt;
        let mstar = self.mstar;
        let vgst = gate - threshold;
        let over = nvt * softplus(mstar * vgst / nvt)
            / (mstar
                + swing
                    * (self.coxe * self.xdep0 / EPS_SI)
                    * safe_exp((p.voff - (1. - mstar) * vgst) / nvt));
        let weff = if BASIC {
            Dual::cst(self.w.max(1e-10))
        } else {
            floor(
                self.w - 2. * (p.dwg * over + p.dwb * (surface - sqrt_phi)),
                1e-10,
            )
        };
        let abulk = floor(abulk0 - abulk_slope * over, 0.05);
        let coef = if self.sign > 0. { 2. } else { 2.5 };
        let built_in = coef * (self.vth0 - self.vfb - self.phi);
        let scattering = if !BASIC && self.mobmod >= 4 {
            Dual::cst(built_in)
        } else {
            threshold
        };
        let field = if !BASIC && self.mobmod >= 4 {
            (over + built_in) / self.toxe
        } else {
            (over + 2. * threshold) / self.toxe
        };
        let coulomb = if BASIC || p.ud == 0. {
            Dual::cst(0.)
        } else if self.mobmod == 3 {
            let n0 = 1.
                + (p.nfactor * EPS_SI / self.xdep0 + p.cit + p.cdsc * self.short_zero) / self.coxe;
            let v0 = n0 * self.vt * std::f64::consts::LN_2
                / (mstar + n0 * self.coxe * self.xdep0 / EPS_SI * (p.voff / (n0 * self.vt)).exp());
            2. * p.ud / (1. + over / v0).powf(self.ucs)
        } else {
            p.ud * (scattering * self.toxe / (over + 2. * (scattering.sq() + 0.0001).sqrt())).sq()
        };
        let degradation = match if BASIC { 0 } else { self.mobmod } {
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
            / self.resistance_width;
        let drive = over + 2. * self.vt;
        let lambda = if BASIC {
            Dual::cst(1.)
        } else {
            floor(p.a1 * over + p.a2, 0.01)
        };
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
        let litl = self.litl;
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
        let rout = self.rout_factor;
        if rout > 0. {
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
        let mut substrate = Dual::cst(0.);
        if excess.v > 0. {
            if p.alpha0 + p.alpha1 * self.l > 0. && p.beta0 >= 0. && p.beta0 <= 746. * excess.v {
                substrate = vd
                    * conductance
                    * (p.alpha0 / self.l + p.alpha1)
                    * excess
                    * (-p.beta0 / excess).exp();
            }
            if p.pscbe2 > 0. && p.pscbe1 > 0. && p.pscbe1 * litl <= 746. * excess.v {
                conductance = conductance
                    * (1. + excess * (p.pscbe2 / self.l) * (-p.pscbe1 * litl / excess).exp());
            }
        }
        if !BASIC && self.overshoot > 0. {
            let field = 1. + excess * self.l / (esat_l * litl);
            let factor =
                1. + self.overshoot / (self.l * mobility) * (field.sq() - 1.) / (field.sq() + 1.);
            conductance = conductance * (1. + vd / esat_l) / (1. + vd / (esat_l * factor));
        }
        if !BASIC && self.ballistic_speed > 0. && over.v > 1e-25 {
            let speed = vd * conductance / (self.nf * weff * coxeff * over);
            conductance = conductance / (1. + (speed / self.ballistic_speed).sq().sq()).powf(0.25);
        }
        (
            vd * conductance,
            substrate,
            Bias {
                sat: sat.v,
                mobility: mobility.v,
                width: weff.v,
                abulk: abulk.v,
                rds: rds.v,
                beta: self.nf * mobility * coxeff * weff / self.l,
                id_per_volt: conductance,
                over,
                vd,
                cv,
            },
        )
    }

    pub(super) fn gidl<const N: usize>(
        &self,
        side: usize,
        drain: Dual<N>,
        gate: Dual<N>,
        reverse_body: Dual<N>,
    ) -> Dual<N> {
        let [a, b, c, e] = self.leakage[side];
        if let Some(shape) = self.gidl_shape {
            let [r, k, f] = shape[side];
            let field = (drain - r * gate - e) / (3. * self.toxe);
            if field.v <= 0. || a <= 0. || b <= 0. {
                return Dual::cst(0.);
            }
            let body = -floor(reverse_body + f, -self.gidl_clamp);
            return a * self.wj * self.nf * field * (-b / field).exp() * safe_exp(k / body);
        }
        let field = (drain - gate - e) / (3. * self.toxe);
        if field.v <= 0. || reverse_body.v <= 0. || a <= 0. || b <= 0. || c <= 0. {
            return Dual::cst(0.);
        }
        let body = reverse_body * reverse_body.sq();
        a * self.wj * self.nf * field * (-b / field).exp() * body / (c + body)
    }
}
