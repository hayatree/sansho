//! Oriented channel current and bias quantities shared by charge and noise.
//! Provenance: UC Berkeley BSIM3v3.3 manual, chapters 2–3 and Appendix B.1.

use super::{D, EPS_SI, Model, floor, smooth_min};
use crate::CHARGE;

pub(super) struct Bias {
    pub gate: D,
    pub drain: D,
    pub root: D,
    pub threshold: D,
    pub swing: D,
    pub overdrive: D,
    pub bulk0: D,
    pub bulk: D,
    pub mu: D,
    pub width: D,
    pub rds: D,
    pub sat: D,
    pub effective_drain: D,
    pub ids: D,
    pub substrate: D,
}

pub(super) fn softplus(x: D) -> D {
    if x.v > 0. {
        x + (-x).exp().ln_1p()
    } else {
        x.exp().ln_1p()
    }
}

fn attenuation(argument: D) -> D {
    let e = (-argument * 0.5).exp();
    e + 2. * e.sq()
}

impl Model {
    pub(super) fn channel(&self, vg: D, vd: D, vb: D) -> Bias {
        let p = &self.p;
        let gate_boundary = self.flat + self.phi;
        let gate = if p.ngate > 0. && vg.v > gate_boundary {
            let voltage = vg - gate_boundary;
            let scale = CHARGE * EPS_SI * p.ngate * 1e6 / self.cox.powi(2);
            gate_boundary + 2. * voltage / (1. + (1. + 2. * voltage / scale).sqrt())
        } else {
            vg
        };
        let diff = vb - self.vbc - 0.001;
        let body = self.vbc + 0.5 * (diff + (diff.sq() - 0.004 * self.vbc).sqrt());
        // Positive forward bias can exceed phi during a Newton iterate.
        let potential = if body.v <= 0.8 * self.phi {
            self.phi - body
        } else {
            (0.2 * self.phi).powi(2) / (body - 0.6 * self.phi)
        };
        let root = potential.sqrt();
        let ref_root = self.phi.sqrt();
        let xdep = self.xdep0 * root / ref_root;
        let lt = (EPS_SI / self.cox * xdep).sqrt();
        let lt0 = (EPS_SI / self.cox * self.xdep0).sqrt();
        let short = attenuation(p.dvt1 * self.l / (lt * floor(1. + p.dvt2 * body, 0.05)));
        let narrow =
            attenuation(p.dvt1w * self.w * self.l / (lt * floor(1. + p.dvt2w * body, 0.05)));
        let dibl = attenuation(D::cst(self.dsub * self.l / lt0));
        let threshold = self.threshold_base
            + self.k1 * root
            + (self.body_temp - self.k2) * body
            + self.k1 * ref_root * ((1. + p.nlx / self.l).sqrt() - 1.)
            + (p.k3 + p.k3b * body) * self.tox * self.phi / (self.w + p.w0)
            - (p.dvt0 * short + p.dvt0w * narrow) * (self.vbi - self.phi)
            - dibl * (p.eta0 + p.etab * body) * vd;
        let swing = floor(
            1. + (p.nfactor * EPS_SI / xdep
                + p.cit
                + (p.cdsc + p.cdscd * vd + p.cdscb * body) * short)
                / self.cox,
            0.1,
        );
        let slope = 2. * self.vt * swing;
        let vgth = gate - threshold;
        let numerator = slope * softplus(vgth / slope);
        let depletion_ratio = 2. * swing * self.cox * self.xdep0 / EPS_SI;
        let exponent = (2. * p.voff - vgth) / slope;
        let overdrive = if exponent.v > 0. {
            let inverse = (-exponent).exp();
            numerator * inverse / (inverse + depletion_ratio)
        } else {
            numerator / (1. + depletion_ratio * exponent.exp())
        };
        let width = floor(
            self.w - 2. * (p.dwg * overdrive + p.dwb * (root - ref_root)),
            1e-12,
        );
        let field = if self.mobmod == 2 {
            overdrive
        } else {
            overdrive + 2. * threshold
        } / self.tox;
        let scattering = if self.mobmod == 3 {
            field * (self.ua + self.ub * field) * (1. + self.uc * body)
        } else {
            field * (self.ua + self.uc * body + self.ub * field)
        };
        let mu = self.u0 / floor(1. + scattering, 0.05);
        let length_ratio = self.l / (self.l + 2. * (p.xj * xdep).sqrt());
        let body_factor = self.k1 / (2. * root);
        let keta = floor(1. + p.keta * body, 0.05);
        let bulk0 = (1. + body_factor * (p.a0 * length_ratio + p.b0 / (self.w + p.b1))) / keta;
        let bulk = floor(
            bulk0 - body_factor * p.a0 * p.ags * length_ratio.powf(3.) * overdrive / keta,
            0.01,
        );
        let rds = self.rds0 * floor(1. + p.prwg * overdrive + p.prwb * (root - ref_root), 0.);
        let esat = 2. * self.vsat / mu;
        let e_length = esat * self.l;
        let drive = overdrive + 2. * self.vt;
        let inv_lambda = 1. / floor(p.a1 * overdrive + p.a2, 0.01);
        let r = width * self.vsat * self.cox * rds;
        let a = bulk * (bulk * r + inv_lambda - 1.);
        let minus_b = drive * (2. * inv_lambda - 1.) + bulk * e_length + 3. * bulk * drive * r;
        let c = drive * (e_length + 2. * drive * r);
        let sat = 2. * c / (minus_b + floor(minus_b.sq() - 4. * a * c, 1e-30).sqrt());
        let eff = smooth_min(sat, vd, p.delta);
        let excess = vd - eff;
        let g0 = width * mu * self.cox / self.l * overdrive * (1. - bulk * eff / (2. * drive))
            / (1. + eff / e_length);
        let linear = g0 * eff / (1. + g0 * rds);
        let vasat = (e_length + sat + 2. * r * overdrive * (1. - bulk * sat / (2. * drive)))
            / (2. * inv_lambda - 1. + r * bulk);
        let litl = (EPS_SI / self.cox * p.xj).sqrt();
        let rout = p.pdiblc1 * attenuation(D::cst(p.drout * self.l / lt0)) + p.pdiblc2;
        let inv_dibl = floor(
            rout * (1. + p.pdiblcb * body) * (bulk * sat + drive) / drive.sq(),
            0.,
        );
        let early_gate = floor(1. + p.pvag * overdrive / e_length, 0.01);
        let clm = if p.pclm > 0. {
            let va = (bulk * e_length + overdrive) * excess / (p.pclm * bulk * esat * litl);
            excess / (vasat + early_gate * va / (1. + va * inv_dibl))
        } else {
            excess * inv_dibl / (vasat * inv_dibl + early_gate)
        };
        let base = linear * (1. + clm);
        let field_factor = |barrier: f64| {
            if excess.v > 0. {
                excess * (-barrier / excess).exp()
            } else {
                D::cst(0.)
            }
        };
        let ids = base * (1. + p.pscbe2.max(0.) / self.l * field_factor(p.pscbe1 * litl));
        let substrate = base * (p.alpha0 / self.l + p.alpha1).max(0.) * field_factor(p.beta0);
        Bias {
            gate,
            drain: vd,
            root,
            threshold,
            swing,
            overdrive,
            bulk0,
            bulk,
            mu,
            width,
            rds,
            sat,
            effective_drain: eff,
            ids,
            substrate,
        }
    }
}
