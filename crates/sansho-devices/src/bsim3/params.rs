//! Geometry and temperature specialization of the BSIM3 model card.
//! Provenance: UC Berkeley BSIM3v3.3 manual, Appendices A, B.1.9–11, B.2.1 and D.

use super::{EPS_OX, EPS_SI, Junction, Noise};
use crate::{CHARGE, ModelParams, MosBinding, REFTEMP, temperature::gap, thermal_voltage};

pub(super) struct Bins<'a> {
    pub card: &'a ModelParams,
    inv_l: f64,
    inv_w: f64,
}

impl Bins<'_> {
    fn optional(&self, name: &str, default: f64) -> Option<f64> {
        let c = self.card;
        let base = c.opt(&[name]);
        let l = c.opt(&[&format!("l{name}")]);
        let w = c.opt(&[&format!("w{name}")]);
        let p = c.opt(&[&format!("p{name}")]);
        if [base, l, w, p].iter().all(Option::is_none) {
            return None;
        }
        Some(
            base.unwrap_or(default)
                + self.inv_l * l.unwrap_or(0.)
                + self.inv_w * w.unwrap_or(0.)
                + self.inv_l * self.inv_w * p.unwrap_or(0.),
        )
    }
    pub fn get(&self, name: &str, default: f64) -> f64 {
        self.optional(name, default).unwrap_or(default)
    }
}

macro_rules! coefficients {
    ($($name:ident = $value:expr),+ $(,)?) => {
        #[derive(Debug)]
        pub(super) struct Coefficients { $(pub $name: f64),+ }
        impl Coefficients {
            fn bind(b: &Bins<'_>) -> Self {
                Self { $($name: b.get(stringify!($name), $value)),+ }
            }
        }
    };
}

coefficients! {
    k3=80., k3b=0., w0=2.5e-6, nlx=1.74e-7, dvt0=2.2, dvt1=0.53, dvt2=-0.032,
    dvt0w=0., dvt1w=5.3e6, dvt2w=-0.032, eta0=0.08, etab=-0.07,
    nfactor=1., cit=0., cdsc=2.4e-4, cdscb=0., cdscd=0., voff=-0.08,
    a0=1., ags=0., b0=0., b1=0., keta=-0.047, a1=0., a2=1.,
    prwb=0., prwg=0., dwg=0., dwb=0., pclm=1.3, pdiblc1=0.39,
    pdiblc2=0.0086, pdiblcb=0., drout=0.56, pscbe1=4.24e8, pscbe2=1e-5,
    pvag=0., delta=0.01, alpha0=0., alpha1=0., beta0=30., xj=1.5e-7,
    ngate=0., noff=1., voffcv=0., vfbcv=-1., clc=0.1e-6, cle=0.6, ckappa=0.6,
    acde=1., moin=15.
}

#[derive(Debug)]
pub struct Model {
    pub(super) p: Coefficients,
    pub(super) l: f64,
    pub(super) w: f64,
    pub(super) lcv: f64,
    pub(super) wcv: f64,
    pub(super) tox: f64,
    pub(super) cox: f64,
    pub(super) temp: f64,
    pub(super) vt: f64,
    pub(super) sign: f64,
    pub(super) mult: f64,
    pub(super) phi: f64,
    pub(super) xdep0: f64,
    pub(super) vbi: f64,
    pub(super) vbc: f64,
    pub(super) k1: f64,
    pub(super) k2: f64,
    pub(super) threshold_base: f64,
    pub(super) body_temp: f64,
    pub(super) flat: f64,
    pub(super) flat_cv: f64,
    pub(super) centroid_acde: f64,
    pub(super) debye: f64,
    pub(super) dsub: f64,
    pub(super) mobmod: u8,
    pub(super) capmod: u8,
    pub(super) xpart: f64,
    pub(super) u0: f64,
    pub(super) ua: f64,
    pub(super) ub: f64,
    pub(super) uc: f64,
    pub(super) vsat: f64,
    pub(super) rds0: f64,
    pub(super) overlap: [f64; 3],
    pub(super) ldd: [f64; 2],
    pub(super) junction: [Junction; 2],
    pub(super) resistance: [f64; 2],
    pub(super) noise: Noise,
}

fn dimension_correction(card: &ModelParams, axis: &str, l: f64, w: f64, cv: bool) -> f64 {
    let axis_l = l.powf(card.get(&format!("{axis}ln"), 1.));
    let axis_w = w.powf(card.get(&format!("{axis}wn"), 1.));
    let coeff = |suffix: &str| {
        let key = format!("{axis}{suffix}");
        let dc = card.get(&key, 0.);
        if cv {
            card.get(&format!("{key}c"), dc)
        } else {
            dc
        }
    };
    coeff("l") / axis_l + coeff("w") / axis_w + coeff("wl") / (axis_l * axis_w)
}

fn selector(p: &ModelParams, k: &str, default: u8, min: u8, max: u8) -> Result<u8, String> {
    let v = p.get(k, default.into());
    if v.fract() != 0. || !(f64::from(min)..=f64::from(max)).contains(&v) {
        return Err(format!("invalid {k}={v}"));
    }
    Ok(v as u8)
}

impl Model {
    pub(super) fn new(b: &MosBinding<'_>) -> Result<Self, String> {
        let c = b.model;
        let g = b.geometry;
        let sign = b.polarity;
        let mobmod = selector(c, "mobmod", 1, 1, 3)?;
        let capmod = selector(c, "capmod", 3, 0, 3)?;
        let noimod = selector(c, "noimod", 1, 1, 4)?;
        let lint = c.get("lint", 0.);
        let wint = c.get("wint", 0.);
        let l = g.l - 2. * (lint + dimension_correction(c, "l", g.l, g.w, false));
        let w = g.w - 2. * (wint + dimension_correction(c, "w", g.l, g.w, false));
        let scale = if c.get("binunit", 1.) == 1. { 1e-6 } else { 1. };
        let bin = Bins {
            card: c,
            inv_l: scale / l,
            inv_w: scale / w,
        };
        let dlc = bin.get("dlc", lint);
        let lcv = g.l - 2. * (dlc + dimension_correction(c, "l", g.l, g.w, true));
        let dwc = bin.get("dwc", wint);
        let wcv = g.w - 2. * (dwc + dimension_correction(c, "w", g.l, g.w, true));
        let tox = c.get("tox", 1.5e-8);
        let toxm = c.get("toxm", tox);
        let temp = b.temp + 273.15;
        let nominal = c.get("tnom", b.tnom) + 273.15;
        let mult = b.instance.opt(&["m", "mult"]).unwrap_or(1.);
        if [g.w, g.l, l, w, lcv, wcv, tox, toxm, temp, nominal, mult]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.)
        {
            return Err("dimensions, oxide thickness, absolute temperatures and multiplicity must be positive".into());
        }
        let p = Coefficients::bind(&bin);
        let vt = thermal_voltage(temp);
        let vt0 = thermal_voltage(nominal);
        let tr = temp / nominal - 1.;
        let ni = 1.45e10
            * (nominal / REFTEMP).powf(1.5)
            * (21.5565981 - gap(nominal) / (2. * vt0)).exp();
        let cox = EPS_OX / tox;
        let gamma1 = bin.optional("gamma1", (2. * CHARGE * EPS_SI * 1.7e23).sqrt() / cox);
        let nch = bin.get(
            "nch",
            gamma1.map_or(1.7e17, |x| {
                (x * cox).powi(2) / (2. * CHARGE * EPS_SI) * 1e-6
            }),
        );
        let nsub = bin.get("nsub", 6e16);
        let phi = 2. * vt0 * (nch / ni).ln();
        let gamma = |n: f64| (2. * CHARGE * EPS_SI * n * 1e6).sqrt() / cox;
        let gamma2 = bin.get("gamma2", gamma(nsub));
        let vbm = -bin.get("vbm", -3.).abs();
        let xt = bin.get("xt", 1.55e-7);
        let vbx = -bin
            .get("vbx", phi - CHARGE * nch * 1e6 * xt * xt / (2. * EPS_SI))
            .abs();
        let (k1, k2) = match (bin.optional("k1", 0.5), bin.optional("k2", 0.)) {
            (None, None) => {
                let k2 = (gamma1.unwrap_or_else(|| gamma(nch)) - gamma2)
                    * ((phi - vbx).sqrt() - phi.sqrt())
                    / (2. * ((phi * (phi - vbm)).sqrt() - phi) + vbm);
                (gamma2 - 2. * k2 * (phi - vbm).sqrt(), k2)
            }
            (a, b) => (a.unwrap_or(0.5), b.unwrap_or(0.)),
        };
        let vbc = if k2 < 0. {
            (0.9 * (phi - (k1 / (2. * k2)).powi(2)))
                .clamp(-30., -0.001)
                .min(vbm)
        } else {
            -30.
        };
        let vth = bin
            .optional("vth0", sign * (bin.get("vfb", -1.) + phi + k1 * phi.sqrt()))
            .map(|v| sign * v);
        let flat = bin.get("vfb", vth.map_or(-1., |v| v - phi - k1 * phi.sqrt()));
        let threshold_base = vth.unwrap_or(flat + phi + k1 * phi.sqrt()) - k1 * phi.sqrt()
            + (bin.get("kt1", -0.11) + bin.get("kt1l", 0.) / l) * tr;
        let u = bin.get("u0", if sign > 0. { 670. } else { 250. });
        let u0 = (if u > 1. { u * 1e-4 } else { u }) * (1. + tr).powf(bin.get("ute", -1.5));
        let vsat = bin.get("vsat", 8e4) - bin.get("at", 3.3e4) * tr;
        let rds0 =
            (bin.get("rdsw", 0.) + bin.get("prt", 0.) * tr) / (w * 1e6).powf(bin.get("wr", 1.));
        if [
            phi,
            nsub,
            u0,
            vsat,
            p.xj,
            p.delta,
            p.noff,
            p.moin,
            p.ckappa,
            w + p.w0,
            w + p.b1,
            l + p.nlx,
            p.a2,
        ]
        .iter()
        .any(|x| !x.is_finite() || *x <= 0.)
            || p.clc < 0.
            || !k1.is_finite()
            || !k2.is_finite()
        {
            return Err("invalid electrostatics, mobility, saturation or CV coefficients".into());
        }
        let cf = bin.get(
            "cf",
            2. * EPS_OX / std::f64::consts::PI * (4e-7 / tox).ln_1p(),
        );
        let ldd = [
            bin.get("cgdl", bin.get("cgd1", 0.)),
            bin.get("cgsl", bin.get("cgs1", 0.)),
        ];
        let overlap_sd = |name: &str, ldd: f64| {
            let base = if c.opt(&["dlc"]).is_some() && dlc > 0. {
                (dlc * cox - ldd).max(0.)
            } else {
                0.6 * p.xj * cox
            };
            wcv * (c.get(name, base) + cf)
        };
        let overlap = [
            overlap_sd("cgdo", ldd[0]),
            overlap_sd("cgso", ldd[1]),
            lcv * c.get("cgbo", 2. * dwc * cox),
        ];
        let resistance =
            [b.instance.get("nrd", 1.), b.instance.get("nrs", 1.)].map(|n| n * c.get("rsh", 0.));
        if resistance
            .iter()
            .chain([g.ad, g.as_, g.pd, g.ps].iter())
            .any(|&v| v < 0. || !v.is_finite())
        {
            return Err("invalid junction geometry or diffusion resistance".into());
        }
        let junction = Junction::pair(&bin, g, w, temp, nominal)?;
        let noise = Noise::new(c, noimod, sign, l)?;
        let dsub = bin.get("dsub", p.drout);
        let centroid_acde = p.acde * (nch / 2e16).powf(-0.25);
        let mut m = Self {
            p,
            l,
            w,
            lcv,
            wcv,
            tox,
            cox,
            temp,
            vt,
            sign,
            mult,
            phi,
            xdep0: (2. * EPS_SI * phi / (CHARGE * nch * 1e6)).sqrt(),
            vbi: vt0 * (1e20 * nch / ni.powi(2)).ln(),
            vbc,
            k1: k1 * tox / toxm,
            k2: k2 * tox / toxm,
            threshold_base,
            body_temp: bin.get("kt2", 0.022) * tr,
            flat,
            flat_cv: 0.,
            centroid_acde,
            debye: (EPS_SI * vt0 / (CHARGE * nch * 1e6)).sqrt(),
            dsub,
            mobmod,
            capmod,
            xpart: c.get("xpart", 0.),
            u0,
            ua: bin.get("ua", 2.25e-9) + bin.get("ua1", 4.31e-9) * tr,
            ub: bin.get("ub", 5.87e-19) + bin.get("ub1", -7.61e-18) * tr,
            uc: bin.get("uc", if mobmod == 3 { -0.046 } else { -4.65e-11 })
                + bin.get("uc1", if mobmod == 3 { -0.056 } else { -5.6e-11 }) * tr,
            vsat,
            rds0: rds0.max(0.),
            overlap,
            ldd: ldd.map(|x| x * wcv),
            junction,
            resistance,
            noise,
        };
        let zero = super::D::cst(0.);
        m.flat_cv = m.channel(zero, zero, zero).threshold.v - phi - m.k1 * phi.sqrt();
        if !m.flat_cv.is_finite() {
            return Err("nonfinite zero-bias threshold".into());
        }
        c.accept(&["level", "lmin", "lmax", "wmin", "wmax"]);
        Ok(m)
    }
}
