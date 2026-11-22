//! Oriented SOI electrostatics, channel transport and intrinsic terminal charge.
//! Provenance: BSIMSOIv4.4 Users' Manual, chapters 2, 4, 6 and Appendix C.
use super::{D, EPS_SI, Model};
use crate::{
    CHARGE,
    bsim4::{depletion, floor, partition, smooth_min, softplus},
    junction::safe_exp,
};

pub(super) struct Bias {
    pub phi: D,
    pub flat_back: D,
    pub vb: D,
    pub vt: D,
    pub temp: D,
    pub vg: D,
    pub vd: D,
    pub threshold: D,
    pub over: D,
    pub bulk: D,
    pub bulk0: D,
    pub sat: D,
    pub eff: D,
    pub ids: D,
    pub conductance: D,
    pub beta: D,
    pub mu: D,
    pub rds: D,
    pub esat: D,
    pub swing: D,
}

fn attenuation(x: D) -> D {
    let e = safe_exp(-0.5 * x);
    e + 2. * e.sq()
}

struct Surface {
    vb: D,
    high: D,
    root: D,
    xdep: D,
    lt0: D,
    threshold: D,
    swing: D,
}

impl Model {
    fn surface(&self, vb: D, vd: D, vt: D, dt: D, phi_s: D, vbi: D) -> Surface {
        let p = &self.p;
        // Eqs. 2.1–2.5, including tangent continuation above 0.95 phi.
        let delta = 0.001;
        let low = vb + 5. - delta;
        let limited = -5. + 0.5 * (low + (low.sq() + 20. * delta).sqrt());
        let ceiling = |v: D, c: D| {
            // The paired film/threshold probes resolve the upper joins of
            // Eqs. 2.2 and 2.3 at 2 mV, preserving zero applied film bias.
            let delta = 0.002;
            let d = c - v - delta;
            c - 0.5 * (d + (d.sq() + 4. * delta * c).sqrt())
        };
        let high = ceiling(limited, D::cst(1.5));
        let vb = ceiling(high, 0.95 * phi_s);
        let root = (phi_s - vb).sqrt();
        let ext = root - (high - vb) / (2. * (0.05 * phi_s).sqrt());
        let xdep = (2. * EPS_SI / (CHARGE * self.nch)).sqrt() * root;
        let lt = (EPS_SI / self.cox * xdep).sqrt();
        let lt0 = (EPS_SI / self.cox * (2. * EPS_SI * phi_s / (CHARGE * self.nch)).sqrt()).sqrt();
        let short = attenuation(p.dvt1 * self.l / (lt * floor(1. + p.dvt2 * vb, 0.05)));
        let narrow = attenuation(p.dvt1w * self.w * self.l / (lt * floor(1. + p.dvt2w * vb, 0.05)));
        let threshold = self.vth
            + (self.k1ox * ext - self.k1 * phi_s.sqrt()) * (1. + p.lpeb / self.l).sqrt()
            - p.k2 * vb
            + self.k1ox * phi_s.sqrt() * ((1. + p.lpe0 / self.l).sqrt() - 1.)
            + (p.k3 + p.k3b * vb) * self.tox * phi_s / (self.w + p.w0)
            - (p.dvt0 * short + p.dvt0w * narrow) * (vbi - phi_s)
            - attenuation(p.dsub * self.l / lt0) * (p.eta0 + p.etab * vb) * vd
            + (p.kt1 + p.kt1l / self.l + p.kt2 * vb) * dt;
        let swing = floor(
            1. + (p.nfactor * EPS_SI / xdep
                + p.cit
                + (p.cdsc + p.cdscb * vb + p.cdscd * vd) * short)
                / self.cox,
            0.1,
        );
        let nv = swing * vt;
        let pocket = self.l + p.dvtp0 * (1. + safe_exp(-p.dvtp1 * vd));
        let th_arg = safe_exp(-2. * p.dvtp4 * vd);
        let threshold = threshold
            - nv * (self.l / floor(pocket, 1e-12)).ln()
            - p.dvtp2 / self.l.powf(p.dvtp3) * (1. - th_arg) / (1. + th_arg);
        Surface {
            vb,
            high,
            root,
            xdep,
            lt0,
            threshold,
            swing,
        }
    }

    fn body(&self, vg: D, vd: D, ve: D, vb: D, vt: D, [phi_s, vbi, flat_back]: [D; 3]) -> D {
        if self.mode == 0 {
            return vb;
        }
        // Chapter 6.3 and Appendix C1, front/back capacitive divider.
        let p = &self.p;
        let csi = EPS_SI / self.tsi;
        let bend = CHARGE * self.nch * self.tsi.powi(2) / (2. * EPS_SI);
        let a = self.coupling.front;
        // Eqs. 6.1–6.4 and the Appendix C pocket-length correction.
        let offset = a * (p.vbsa - bend * (1. + p.lpe0 / self.l))
            + self
                .coupling
                .offset(vbi - phi_s, vd + p.vsce, ve - flat_back);
        // Section 6.3: use the same zero-field construction for the
        // threshold reference (phi=2 Phi_B) and the operating bias.
        let pocket = 1. + p.lpe0 / self.l;
        let zero_field = |phi: D, body: D| {
            // Negative back-bias K1B/TBOX sweeps resolve a 5 mV join.
            // CDSBS sweeps retain the front and drain weights of Eq. 6.4
            // in the zero-back-field reference.
            let top = self.coupling.zero_field_front * (phi + p.vbsa - bend * pocket)
                + self.coupling.drain * (vd + p.vsce);
            let d = top - body - 0.005;
            let deficit = 0.5 * (d + (d.sq() + 0.005_f64.powi(2)).sqrt());
            body - csi * deficit.sq() / (2. * CHARGE * self.nch * self.tsi * pocket)
        };
        let ref_body = zero_field(phi_s, a * phi_s + offset);
        let dt = vt / (super::VT_PER_K * self.nominal) - 1.;
        let vthfd = self
            .surface(smooth_min(phi_s, ref_body, 0.01), vd, vt, dt, phi_s, vbi)
            .threshold;
        let slope = p.nofffd * vt;
        let ov = slope * softplus((vg - vthfd - p.vofffd) / slope);
        let on = phi_s
            + vt * (ov * (ov + 2. * self.k1 * phi_s.sqrt())
                / floor(p.moinfd * self.k1 * vt.sq(), 1e-20))
            .ln_1p();
        let phi = on
            - self.cox / (self.cox + a * self.cbox)
                * slope
                * softplus((vthfd - vg - p.vofffd) / slope);
        // The three gate biases in the K1B and TBOX probes give a common
        // 20 mV addition to the ideal-FD interface potential.
        let lower = a * phi + offset + 0.02;
        let body = if self.mode == 2 {
            lower
        } else {
            // R17 unified_floor isolates the joined interface voltage.
            let d = vb - lower - 0.01;
            lower + 0.5 * (d + (d.sq() + 0.01_f64.powi(2)).sqrt())
        };
        zero_field(phi, body)
    }

    pub(super) fn channel(&self, vg: D, vd: D, ve: D, vb: D, temp: D) -> Bias {
        let p = &self.p;
        let vt = super::VT_PER_K * temp;
        let ratio = temp / self.nominal;
        let dt = ratio - 1.;
        // §5.2. Recompute the silicon potentials from the local temperature.
        // Differentiating this translation also supplies the thermal column for AC.
        let gap = 1.16 - 7.02e-4 * temp.sq() / (temp + 1108.);
        let shift = gap - ratio * crate::temperature::gap(self.nominal) - 3. * vt * ratio.ln();
        let phi_s = floor(ratio * self.phi + shift, 0.01);
        let vbi = ratio * self.vbi + shift;
        let flat_back = ratio * self.flat_back
            - if self.same_substrate_type {
                D::cst(0.)
            } else {
                self.sign * shift
            };
        let boundary = self.vfb + phi_s;
        let vg = if self.ngate > 0. && vg.v > boundary.v {
            let scale = CHARGE * EPS_SI * self.ngate * 1e6 / self.cox.powi(2);
            boundary + 2. * (vg - boundary) / (1. + (1. + 2. * (vg - boundary) / scale).sqrt())
        } else {
            vg
        };
        let vb = self.body(vg, vd, ve, vb, vt, [phi_s, vbi, flat_back]);
        let Surface {
            vb,
            high,
            root,
            xdep,
            lt0,
            threshold,
            swing,
        } = self.surface(vb, vd, vt, dt, phi_s, vbi);
        let nv = swing * vt;
        let star = 0.5 + p.minv.atan() / std::f64::consts::PI;
        let vgst = vg - threshold;
        let over = nv * softplus(star * vgst / nv)
            / (star
                + swing
                    * self.cox
                    * (2. * phi_s / (CHARGE * EPS_SI * self.nch)).sqrt()
                    * safe_exp((p.voff - (1. - star) * vgst) / nv));
        // Eq. 2.6 and Appendix C2 mobility and saturation.
        let len = self.l / (self.l + 2. * (self.tsi * xdep).sqrt());
        let potential = floor(phi_s + p.ketas, 0.01);
        let surface = potential - high / floor(1. + p.keta * high, 0.05);
        // Eq. 2.6. Inverting the KETA sweeps gives the normalized
        // inverse root up to 5, followed by a tangent-matched rational
        // continuation to 6 as the film bias increases.
        let normalized = surface / potential;
        let inverse_root = if normalized.v >= 0.04 {
            1. / surface.sqrt()
        } else {
            (6. - 1. / (1. + (0.04 - normalized) / (2. * 0.04_f64.powf(1.5)))) / potential.sqrt()
        };
        let factor = 0.5 * self.k1ox * (1. + p.lpeb / self.l).sqrt() * inverse_root;
        let bulk0 = floor(1. + factor * (p.a0 * len + p.b0 / (self.w + p.b1)), 0.01);
        let bulk = floor(bulk0 - factor * p.a0 * p.ags * len.powf(3.) * over, 0.01);
        let field = if self.mobility == 2 {
            over
        } else {
            over + 2. * threshold
        } / self.tox;
        let ua = p.ua + p.ua1 * dt;
        let ub = p.ub + p.ub1 * dt;
        let uc = p.uc + p.uc1 * dt;
        let scatter = if self.mobility == 4 {
            let field = floor((over + 2. * (self.vth - self.vfb - phi_s)) / self.tox, 1e-9);
            let exp = p.ucs * ratio.powf(p.ucste);
            (ua + uc * vb) * field.powf(p.eu)
                + (p.ud + p.ud1 * dt) * safe_exp(-exp * (1. + over / vt).ln())
        } else if self.mobility == 3 {
            field * (ua + ub * field) * (1. + uc * vb)
        } else {
            field * (ua + ub * field + uc * vb)
        };
        let mu = self.u0 * ratio.powf(p.ute) / floor(1. + scatter, 0.05);
        let vsat = floor(p.vsat - p.at * dt, 1.);
        let esat = 2. * vsat / mu;
        let el = esat * self.l;
        let rds = if self.extension.is_some() {
            D::cst(0.)
        } else {
            floor(
                (p.rdsw + p.prt * dt) / (1e6 * self.w).powf(p.wr)
                    * (1. + p.prwg * over + p.prwb * (root - phi_s.sqrt())),
                0.,
            )
        };
        let width = floor(
            self.w - 2. * (p.dwg * over + p.dwb * (root - phi_s.sqrt())),
            1e-12,
        );
        let r = width * vsat * self.cox * rds;
        let drive = over + 2. * vt;
        let inv_lambda = 1. / floor(p.a1 * over + p.a2, 0.01);
        let b = drive * (2. * inv_lambda - 1.) + bulk * el + 3. * bulk * drive * r;
        let c = drive * (el + 2. * drive * r);
        let sat = 2. * c
            / (b + floor(b.sq() - 4. * bulk * (bulk * r + inv_lambda - 1.) * c, 1e-24).sqrt());
        let eff = smooth_min(sat, vd, p.delta);
        let beta = width * mu * self.cox / self.l;
        let g0 = beta * over * (1. - bulk * eff / (2. * drive)) / (1. + eff / el);
        let excess = vd - eff;
        let vasat = (el + sat + 2. * r * over * (1. - bulk * sat / (2. * drive)))
            / (2. * inv_lambda - 1. + r * bulk);
        let rout = p.pdiblc1 * attenuation(p.drout * self.l / lt0) + p.pdiblc2;
        let inv_dibl = floor(
            rout * (1. + p.pdiblcb * vb) * (bulk * sat + drive) / drive.sq(),
            0.,
        );
        let gate = floor(1. + p.pvag * over / el, 0.01);
        let litl = (EPS_SI / self.cox * self.tsi).sqrt();
        // Eq. 2.7 and Appendix C2 combine inverse Early voltages
        // before adding VAsat. The PDITS sweep isolates the pocket term.
        let inv_dits = p.pdits * (1. + p.fprout.max(0.) * self.l.sqrt() / drive)
            / (1. + (1. + p.pditsl * self.l) * safe_exp(p.pditsd * vd));
        let inv_dibl = inv_dibl + inv_dits;
        let clm = if p.pclm > 0. {
            let va = (bulk * el + over) * excess / (p.pclm * bulk * esat * litl);
            excess / (vasat + gate * va / (1. + va * inv_dibl))
        } else {
            excess * inv_dibl / (vasat * inv_dibl + gate)
        };
        let conductance = self.nf / self.nseg * g0 / (1. + g0 * rds) * (1. + clm);
        Bias {
            phi: phi_s,
            flat_back,
            vb,
            vt,
            temp,
            vg,
            vd,
            threshold,
            over,
            bulk,
            bulk0,
            sat,
            eff,
            ids: conductance * eff,
            conductance,
            beta: beta * self.nf,
            mu,
            rds: rds / self.nf,
            esat,
            swing,
        }
    }

    pub(super) fn charges(&self, a: &Bias) -> ([D; 4], f64) {
        let p = &self.p;
        let phi_s = a.phi;
        let area = self.wcv * self.lcv * self.nf / self.nseg + self.extra_area[0];
        let slope = p.noff * a.swing * a.vt;
        let vgst = a.vg - a.threshold - p.delvt;
        let raw = if self.vgstcv == 2 {
            let nv = a.vt * a.swing;
            let star = 0.5 + p.minvcv.atan() / std::f64::consts::PI;
            nv * softplus(star * vgst / nv)
                / (star
                    + a.swing
                        * self.cox
                        * (2. * phi_s / (CHARGE * EPS_SI * self.nch)).sqrt()
                        * safe_exp((p.voffcv - (1. - star) * vgst) / nv))
        } else {
            slope * softplus(vgst / slope)
        };
        let flat = if self.cap == 3 {
            self.surface(
                D::cst(0.),
                D::cst(0.),
                a.vt,
                a.temp / self.nominal - 1.,
                phi_s,
                D::cst(self.vbi),
            )
            .threshold
                - phi_s
                - self.k1 * phi_s.sqrt()
        } else {
            a.threshold - phi_s - self.k1 * (phi_s - a.vb).sqrt()
        };
        // Appendix C, front-gate accumulation. The FBODY and body-drive
        // slopes resolve delta=80 mV for the classical charge law.
        let delta = if self.cap == 2 { 0.08 } else { 0.02 };
        let d = flat - a.vg + a.vb - delta;
        let drop =
            0.5 * (d + (d.sq() + 4. * delta * if flat.v < 0. { -flat } else { flat }).sqrt());
        let cdep = if self.cap == 3 {
            let xmax = (EPS_SI * a.vt / (CHARGE * self.nch)).sqrt() / 3.;
            let field = (a.vg - a.vb - flat + drop) / (1e8 * self.toxp);
            let depth = smooth_min(
                xmax,
                xmax * safe_exp(p.acde * (self.nch / 2e22).powf(-0.25) * field),
                1e-3 * self.tox,
            );
            EPS_SI / (3. * self.toxp + depth)
        } else {
            D::cst(self.cox)
        };
        let base = cdep * area * (drop - depletion(a.vg - flat + drop - raw - a.vb, self.k1));
        let (over, cap) = if self.cap == 3 {
            // Eqs. 4.25–4.27, finite inversion-layer thickness.
            let centroid = p.ados * 1.9e-9
                / (1.
                    + floor(
                        (raw + 4. * (self.vth - flat - phi_s)) / (2e8 * self.toxp),
                        1e-15,
                    )
                    .powf(0.7 * p.bdos));
            let shift = a.vt
                * (raw * (raw + 2. * self.k1 * phi_s.sqrt())
                    / floor(p.moin * self.k1.powi(2) * a.vt, 1e-20))
                .ln_1p();
            let d = raw - shift - 0.001;
            (
                0.5 * (d + (d.sq() + 0.004 * raw).sqrt()),
                area * EPS_SI / (3. * self.toxp + centroid),
            )
        } else {
            (raw, D::cst(area * self.cox))
        };
        let bulk = a.bulk0 * (1. + (p.clc / self.lcv).powf(p.cle));
        let mut q = if over.v > 1e-25 {
            partition(
                over,
                bulk,
                smooth_min(over / bulk, a.vd, 0.02),
                cap,
                base,
                self.xpart,
            )
        } else {
            [D::cst(0.), -base, D::cst(0.), base]
        };
        // Eq. 4.1. The FBODY sweep resolves the allocation of the
        // drain-induced depletion charge: with CAPMOD=2 its countercharge
        // appears at the drain, with CAPMOD=3 at the gate.
        let film_scale = if self.mode == 2 {
            0.
        } else {
            p.fbody * (1. - p.dlcb / self.lcv)
        };
        let inversion = q[0].v + q[2].v;
        let drain_depletion = film_scale * (q[3] - base);
        q[3] = film_scale * base + drain_depletion;
        if self.cap == 2 {
            if self.xpart == 0.5 {
                q[0] = q[0] - 0.5 * drain_depletion;
                q[2] = q[2] - 0.5 * drain_depletion;
            } else {
                q[0] = q[0] - drain_depletion;
            }
        }
        q[1] = -q[0] - q[2] - q[3];
        (q, inversion)
    }
}
