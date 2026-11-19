//! Geometry, process and temperature specialization of the silicon channel.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, chapters 1–3, 5, 13–15 and Appendix A.

use super::{EPS_SI, EPS0, Junction};
use crate::{CHARGE, ModelParams, mos1::Mos1Geometry, thermal_voltage};

/// Inverse effective dimensions in the units used by the bin coefficients.
pub(super) struct Bin<'a> {
    params: &'a ModelParams,
    length: f64,
    width: f64,
}

impl<'a> Bin<'a> {
    pub(super) fn new(params: &'a ModelParams, l: f64, w: f64) -> Self {
        let unit = if params.get("binunit", 1.) == 1. {
            1e-6
        } else {
            1.
        };
        Self {
            params,
            length: unit / l,
            width: unit / w,
        }
    }

    pub(super) fn get(&self, name: &str, default: f64) -> f64 {
        let p = self.params;
        if name == "vth0" {
            let term = |a: &str, b: &str| p.opt(&[a, b]).unwrap_or(0.);
            return p.opt(&["vth0", "vtho"]).unwrap_or(default)
                + self.length * term("lvth0", "lvtho")
                + self.width * term("wvth0", "wvtho")
                + self.length * self.width * term("pvth0", "pvtho");
        }
        let nominal = p.get(name, default);
        if super::catalog::parameter(name).is_some_and(|spec| !spec.2) {
            return nominal;
        }
        nominal
            + self.length * p.get(&format!("l{name}"), 0.0)
            + self.width * p.get(&format!("w{name}"), 0.0)
            + self.length * self.width * p.get(&format!("p{name}"), 0.0)
    }
}

// Each entry names a binned quantity in the model card and supplies its nominal default.
macro_rules! channel_parameters {
    ($($field:ident = $default:expr),* $(,)?) => {
        #[derive(Debug)]
        pub(super) struct Channel { $(pub(super) $field: f64,)* }
        impl Channel {
            fn new(bin: &Bin<'_>) -> Self {
                Self { $($field: bin.get(stringify!($field), $default),)* }
            }
        }
    };
}

channel_parameters! {
    dvt0=2.2, dvt1=0.53, dvt2=-0.032, dvt0w=0., dvt1w=5.3e6, dvt2w=-0.032,
    eta0=0.08, etab=-0.07, k3=80., k3b=0., w0=2.5e-6, lpe0=1.74e-7, lpeb=0.,
    dvtp0=0., dvtp1=0., dvtp2=0., dvtp3=0., dvtp4=0., dvtp5=0.,
    nfactor=1., cit=0., cdsc=2.4e-4, cdscb=0., cdscd=0., minv=0., voff=-0.08,
    a0=1., ags=0., b0=0., b1=0., keta=-0.047, xj=1.5e-7,
    dwg=0., dwb=0., ud=0., a1=0., a2=1., delta=0.01,
    rdsw=200., prwg=1., prwb=0., wr=1., pclm=1.3, pvag=0.,
    pdiblc1=0.39, pdiblc2=0.0086, pdiblcb=0., drout=0.56, fprout=0.,
    pdits=0., pditsd=0., pscbe1=4.24e8, pscbe2=1e-5,
    alpha0=0., alpha1=0., beta0=0.,
}

#[derive(Debug)]
pub(super) struct Size {
    pub(super) p: Channel,
    pub(super) zero_body: Option<super::channel::Body<1>>,
    pub(super) sqrt_phi: f64,
    pub(super) pocket: f64,
    pub(super) pocket_shift: f64,
    pub(super) width_shift: f64,
    pub(super) litl: f64,
    pub(super) short_zero: f64,
    pub(super) sign: f64,
    pub(super) l: f64,
    pub(super) w: f64,
    pub(super) wj: f64,
    pub(super) nf: f64,
    pub(super) mult: f64,
    pub(super) coxe: f64,
    pub(super) coxp: f64,
    pub(super) toxe: f64,
    pub(super) toxp: f64,
    pub(super) ados: f64,
    pub(super) bdos: f64,
    pub(super) vth0: f64,
    pub(super) vfb: f64,
    pub(super) k1: f64,
    pub(super) k1ox: f64,
    pub(super) k2ox: f64,
    pub(super) vbc: f64,
    pub(super) phi: f64,
    pub(super) vbi: f64,
    pub(super) xdep0: f64,
    pub(super) poly: f64,
    pub(super) dibl_factor: f64,
    pub(super) rout_factor: f64,
    pub(super) resistance_width: f64,
    pub(super) mstar: f64,
    pub(super) centroid_field: f64,
    pub(super) vt: f64,
    pub(super) vt_pocket: f64,
    pub(super) vth_temp: f64,
    pub(super) body_temp: f64,
    pub(super) mobmod: u8,
    pub(super) basic_transport: bool,
    pub(super) ua: f64,
    pub(super) ub: f64,
    pub(super) uc: f64,
    pub(super) eu: f64,
    pub(super) ucs: f64,
    pub(super) u0: f64,
    pub(super) vsat: f64,
    pub(super) rmin: f64,
    pub(super) pditsl: f64,
    pub(super) overshoot: f64,
    pub(super) ballistic_speed: f64,
    pub(super) squares: [f64; 2],
    pub(super) junction: [Junction; 2],
    pub(super) overlap: [f64; 3],
    pub(super) leakage: [[f64; 4]; 2],
    pub(super) gidl_shape: Option<[[f64; 3]; 2]>,
    pub(super) gidl_clamp: f64,
    pub(super) flat_sd: f64,
    pub(super) gate: super::gate::Gate,
    pub(super) noise: super::noise::Parameters,
    pub(super) cv: super::ChargeModel,
}

pub(super) fn bandgap(t: f64) -> f64 {
    1.16 - 7.02e-4 * t * t / (t + 1108.)
}

impl Size {
    pub(super) fn new(
        p: &ModelParams,
        i: &ModelParams,
        geom: Mos1Geometry,
        sign: f64,
        temp: f64,
        tnom: f64,
    ) -> Result<Self, String> {
        let nf = i.get("nf", 1.);
        let mult = i.opt(&["m", "mult"]).unwrap_or(1.);
        let drawn_l = geom.l + p.get("xl", 0.);
        let drawn_w = geom.w / nf + p.get("xw", 0.);
        let tnom = p.get("tnom", tnom) + 273.15;
        let temp = temp + 273.15;
        if [nf, mult, drawn_l, drawn_w, temp, tnom]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.)
        {
            return Err("length, width, multiplicity, finger count and absolute temperature must be positive".into());
        }
        let correction = |axis: &str, offset: f64, cv: bool| {
            let l = drawn_l.powf(p.get(&format!("{axis}ln"), 1.));
            let w = drawn_w.powf(p.get(&format!("{axis}wn"), 1.));
            let coeff = |suffix: &str| {
                let name = format!("{axis}{suffix}");
                let dc = p.get(&name, 0.);
                if cv {
                    p.get(&format!("{name}c"), dc)
                } else {
                    dc
                }
            };
            offset + coeff("l") / l + coeff("w") / w + coeff("wl") / (l * w)
        };
        let lint = p.get("lint", 0.);
        let wint = p.get("wint", 0.);
        let l = drawn_l - 2. * correction("l", lint, false);
        let w = drawn_w - 2. * correction("w", wint, false);
        let dwc = p.get("dwc", wint);
        let dlc = p.get("dlc", lint);
        let wj = drawn_w - 2. * correction("w", p.get("dwj", dwc), true);
        let wcv = drawn_w - 2. * correction("w", dwc, true);
        let lcv = drawn_l - 2. * correction("l", dlc, true);
        let bin = Bin::new(p, l, w);
        let mut ch = Channel::new(&bin);
        let toxe = p.get("toxe", p.get("toxp", 3e-9) + p.get("dtox", 0.));
        let toxp = p.get("toxp", toxe - p.get("dtox", 0.));
        let toxm = p.get("toxm", toxe);
        let eps = p.get("epsrox", 3.9) * EPS0;
        if [l, w, wj, lcv, wcv, toxe, toxp, toxm, eps, ch.delta]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.)
        {
            return Err("effective dimensions, dielectric thickness, permittivity and DELTA must be positive".into());
        }
        if ch.pdits < 0. || p.get("pditsl", 0.) < 0. {
            return Err("PDITS and PDITSL must be nonnegative".into());
        }
        let coxe = eps / toxe;
        let vt = thermal_voltage(temp);
        let vtn = thermal_voltage(tnom);
        let ni = 1.45e10
            * (tnom / 300.15).powf(1.5)
            * (bandgap(300.15) / (2. * thermal_voltage(300.15)) - bandgap(tnom) / (2. * vtn)).exp();
        let ndep_default = p.opt(&["gamma1"]).map_or(1.7e17, |g| {
            (g * coxe).powi(2) / (2. * CHARGE * EPS_SI * 1e6)
        });
        let ndep = bin.get("ndep", ndep_default);
        let nsd = bin.get("nsd", 1e20);
        let phi = 0.4 + vtn * (ndep / ni).ln() + bin.get("phin", 0.);
        if !(phi > 0. && ndep > 0. && nsd > 0.) {
            return Err("invalid surface potential or doping concentration".into());
        }
        let xdep0 = (2. * EPS_SI * phi / (CHARGE * ndep * 1e6)).sqrt();
        let vbi = vtn * (ndep * nsd / ni.powi(2)).ln();
        let vbm = bin.get("vbm", -3.).min(0.);
        let (k1, k2) = if p.opt(&["k1", "k2"]).is_some() {
            (bin.get("k1", 0.5), bin.get("k2", 0.))
        } else {
            let gamma = |n: f64| (2. * CHARGE * EPS_SI * n * 1e6).sqrt() / coxe;
            let g1 = bin.get("gamma1", gamma(ndep));
            let g2 = bin.get("gamma2", gamma(bin.get("nsub", 6e16)));
            let vbx = -bin
                .get(
                    "vbx",
                    phi - CHARGE * ndep * 1e6 * bin.get("xt", 1.55e-7).powi(2) / (2. * EPS_SI),
                )
                .abs();
            let k2 = (g1 - g2) * ((phi - vbx).sqrt() - phi.sqrt())
                / (2. * phi.sqrt() * ((phi - vbm).sqrt() - phi.sqrt()) + vbm);
            (g2 - 2. * k2 * (phi - vbm).sqrt(), k2)
        };
        let vbc = if k2 < 0. {
            (0.9 * (phi - (k1 / (2. * k2)).powi(2)))
                .clamp(-30., -3.)
                .min(vbm)
        } else {
            -30.
        };
        let vth0 = sign * bin.get("vth0", sign * (bin.get("vfb", -1.) + phi + k1 * phi.sqrt()));
        let vfb = bin.get("vfb", vth0 - phi - k1 * phi.sqrt());
        let tempmod = p.get("tempmod", 0.);
        let mob = p.get("mobmod", 0.);
        if ![0., 1., 2., 3.].contains(&tempmod) || ![0., 1., 2., 3., 4., 5., 6.].contains(&mob) {
            return Err(format!("unsupported tempMod={tempmod} or mobMod={mob}"));
        }
        let dt = temp - tnom;
        let ratio = temp / tnom;
        let rel = dt / tnom;
        let degrade = |base: f64, coeff: f64| match tempmod as u8 {
            0 => base + coeff * rel,
            3 => base * ratio.powf(coeff),
            _ => base * (1. + coeff * dt),
        };
        let ua = degrade(
            bin.get("ua", if matches!(mob, 2. | 6.) { 1e-15 } else { 1e-9 }),
            bin.get("ua1", 1e-9),
        );
        let ub = degrade(bin.get("ub", 1e-19), bin.get("ub1", -1e-18));
        let uc = degrade(
            bin.get(
                "uc",
                if matches!(mob, 1. | 5.) {
                    -0.0465
                } else {
                    -0.0465e-9
                },
            ),
            bin.get(
                "uc1",
                if matches!(mob, 1. | 5.) {
                    -0.056
                } else {
                    -0.056e-9
                },
            ),
        );
        ch.ud = degrade(ch.ud, bin.get("ud1", 0.));
        let mut u0 = bin.get("u0", if sign > 0. { 0.067 } else { 0.025 });
        if u0 > 1. {
            u0 *= 1e-4;
        }
        u0 *= ratio.powf(bin.get("ute", -1.5))
            * (1. - bin.get("up", 0.) * (-l / bin.get("lp", 1e-8)).exp())
            * i.get("mulu0", 1.);
        let vsat = bin.get("vsat", 8e4);
        let at = bin.get("at", 3.3e4);
        let vsat = if tempmod == 0. {
            vsat - at * rel
        } else {
            vsat * (1. - at * dt)
        };
        let prt = bin.get("prt", 0.);
        let resist = |v: f64| {
            if tempmod == 0. {
                (v + prt * rel).max(0.)
            } else {
                (v * (1. + prt * dt)).max(0.)
            }
        };
        ch.rdsw = resist(ch.rdsw);
        let rmin = resist(p.get("rdswmin", 0.));
        ch.nfactor += bin.get("tnfactor", 0.) * rel;
        ch.eta0 += bin.get("teta0", 0.) * rel;
        ch.voff = (ch.voff + p.get("voffl", 0.) / l) * (1. + bin.get("tvoff", 0.) * dt);
        let ngate = bin.get("ngate", 0.);
        let poly = if (1e18..1e25).contains(&ngate) {
            CHARGE * EPS_SI * ngate * 1e6 / coxe.powi(2)
        } else {
            0.
        };
        let dsub = bin.get("dsub", ch.drout);
        let edge = wj * nf;
        let perimeter = |length: f64| {
            if p.get("permod", 1.) == 1. {
                length - edge
            } else {
                length
            }
        };
        let diff = super::layout::diffusions(p, i, nf, wj)?;
        let area = std::array::from_fn(|k| {
            if i.opt(&[["ad", "as"][k]]).is_some() || [geom.ad, geom.as_][k] > 0. {
                [geom.ad, geom.as_][k]
            } else {
                diff[k][0]
            }
        });
        let perim = std::array::from_fn(|k| {
            if i.opt(&[["pd", "ps"][k]]).is_some() {
                perimeter([geom.pd, geom.ps][k])
            } else {
                diff[k][1]
            }
        });
        let junction = Junction::pair(p, area, perim, wj, nf, temp, tnom);
        if junction
            .iter()
            .any(|j| !j.nvt.is_finite() || j.nvt <= 0. || !j.isat.is_finite() || j.isat < 0.)
        {
            return Err("invalid junction emission coefficient or saturation current".into());
        }
        let cf = bin.get(
            "cf",
            2. * eps / std::f64::consts::PI * (1. + 0.4e-6 / toxe).ln(),
        );
        let cov = |side: &str| {
            let edge_cap = bin.get(&format!("cg{side}l"), 0.);
            let default = if dlc > 0. && p.opt(&["dlc"]).is_some() {
                dlc * coxe - edge_cap
            } else {
                0.6 * ch.xj * coxe
            };
            (p.get(&format!("cg{side}o"), default.max(0.)) + cf) * wcv * nf
        };
        let overlap = [
            cov("d"),
            cov("s"),
            p.get("cgbo", 2. * dwc * coxe) * lcv * nf,
        ];
        let gidl = ["agidl", "bgidl", "cgidl", "egidl"];
        let drain = std::array::from_fn(|k| bin.get(gidl[k], [0., 2.3e9, 0.5, 0.8][k]));
        let source = std::array::from_fn(|k| bin.get(&gidl[k].replace("idl", "isl"), drain[k]));
        let gdshape = ["rgidl", "kgidl", "fgidl"];
        let gidlmod = p.get("gidlmod", 0.);
        if ![0., 1.].contains(&gidlmod) {
            return Err("invalid gidlMod".into());
        }
        let gidl_shape = (gidlmod == 1.).then(|| {
            let d = std::array::from_fn(|k| bin.get(gdshape[k], [1., 0., 1.][k]));
            [
                d,
                std::array::from_fn(|k| bin.get(&gdshape[k].replace("idl", "isl"), d[k])),
            ]
        });
        let vt_pocket = if tempmod < 2. { vt } else { vtn };
        let flat_sd = if ngate > 0. {
            vtn * (ngate / nsd).ln()
                + bin.get("vfbsdoff", 0.) * (1. + bin.get("tvfbsdoff", 0.) * dt)
        } else {
            0.
        };
        let gate = super::gate::Gate::new(p, &bin, sign, toxe, l, w * nf, vt_pocket, flat_sd)?;
        let mut cv = super::ChargeModel::new(p, &bin, lcv, wcv, nf);
        cv.voff = (cv.voff + bin.get("voffcvl", 0.) / l) * (1. + bin.get("tvoffcv", 0.) * dt);
        let zero_scale = (EPS_SI / coxe * xdep0).sqrt();
        let theta0 = |exponent| super::theta(super::D::cst(exponent * l / zero_scale)).v;
        let density = 2. * EPS_SI * phi / (crate::CHARGE * xdep0.powi(2)) / 1e6;
        Ok(Self {
            zero_body: None,
            sqrt_phi: phi.sqrt(),
            pocket: (1. + ch.lpeb / l).sqrt(),
            pocket_shift: k1 * toxe / toxm * ((1. + ch.lpe0 / l).sqrt() - 1.) * phi.sqrt(),
            width_shift: toxe * phi / (w + ch.w0),
            litl: (EPS_SI / coxe * ch.xj).sqrt(),
            short_zero: theta0(ch.dvt1),
            basic_transport: false,
            dibl_factor: theta0(dsub),
            rout_factor: ch.pdiblc1 * theta0(ch.drout) + ch.pdiblc2,
            resistance_width: (1e6 * wj).powf(ch.wr),
            mstar: 0.5 + ch.minv.atan() / std::f64::consts::PI,
            centroid_field: cv.acde * (density / 2e16).powf(-0.25),
            p: ch,
            sign,
            l,
            w,
            wj,
            nf,
            mult,
            toxe,
            toxp,
            coxe,
            coxp: eps / toxp,
            ados: p.get("ados", 1.),
            bdos: p.get("bdos", 1.),
            vth0: vth0 + sign * i.opt(&["delvto", "delvt0"]).unwrap_or(0.),
            vfb,
            k1,
            k1ox: k1 * toxe / toxm,
            k2ox: k2 * toxe / toxm,
            phi,
            vbi,
            vbc,
            xdep0,
            poly,
            vt,
            vt_pocket: if tempmod < 2. { vt } else { vtn },
            vth_temp: (bin.get("kt1", -0.11) + bin.get("kt1l", 0.) / l) * rel,
            body_temp: bin.get("kt2", 0.022) * rel,
            mobmod: mob as u8,
            ua,
            ub,
            uc,
            eu: bin.get("eu", if sign > 0. { 1.67 } else { 1. }),
            u0,
            vsat,
            rmin,
            overshoot: bin.get("lambda", 0.),
            ballistic_speed: {
                let back = l / (bin.get("xn", 3.).max(3.) * l + p.get("lc", 0.).max(0.));
                bin.get("vtl", 0.) * (1. - back) / (1. + back)
            },
            pditsl: p.get("pditsl", 0.),
            ucs: bin.get("ucs", if sign > 0. { 1.67 } else { 1. })
                * ratio.powf(if tempmod == 3. {
                    bin.get("ucste", -4.775e-3)
                } else {
                    0.
                }),
            noise: super::noise::Parameters::new(p, sign),
            gidl_shape,
            gidl_clamp: p.get("gidlclamp", -1e-5).min(-1e-12),
            squares: diff.map(|d| d[2]),
            junction,
            overlap,
            flat_sd,
            gate,
            leakage: [drain, source],
            cv,
        })
    }
}
