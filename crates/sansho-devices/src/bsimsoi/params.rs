//! SOI geometry, selector resolution and temperature-independent coefficients.
//! Provenance: BSIMSOIv4.4 Users' Manual, Appendices A, B and E, §§3.6, 5.2, 6.3 and 7.1.

use super::{D, EPS_OX, EPS_SI};
use crate::{CHARGE, MosBinding};

// Appendix E and the 44.2 card listing distinguish scalable coefficients
// from unscaled process constants. Leave inapplicable prefixes unconsumed.
fn binnable(k: &str) -> bool {
    matches!(
        k,
        "a0" | "a1"
            | "a2"
            | "abjtii"
            | "acde"
            | "aely"
            | "agidl"
            | "agisl"
            | "ags"
            | "ahli"
            | "ahlid"
            | "aigbcp2"
            | "aigc"
            | "aigsd"
            | "alpha0"
            | "alphagb1"
            | "alphagb2"
            | "at"
            | "b0"
            | "b1"
            | "beta0"
            | "beta1"
            | "beta2"
            | "betagb1"
            | "betagb2"
            | "bgidl"
            | "bgisl"
            | "bigbcp2"
            | "bigc"
            | "bigsd"
            | "cbjtii"
            | "cdsc"
            | "cdscb"
            | "cdscd"
            | "cgdl"
            | "cgidl"
            | "cgisl"
            | "cgsl"
            | "cigbcp2"
            | "cigc"
            | "cigsd"
            | "cit"
            | "ckappa"
            | "delta"
            | "delvt"
            | "drout"
            | "dsub"
            | "dvt0"
            | "dvt0w"
            | "dvt1"
            | "dvt1w"
            | "dvt2"
            | "dvt2w"
            | "dvtp0"
            | "dvtp1"
            | "dvtp2"
            | "dvtp3"
            | "dvtp4"
            | "dwb"
            | "dwg"
            | "ebjtii"
            | "egidl"
            | "egisl"
            | "esatii"
            | "eta0"
            | "etab"
            | "eu"
            | "fbjtii"
            | "fgidl"
            | "fgisl"
            | "fprout"
            | "idbjt"
            | "iddif"
            | "idrec"
            | "idtun"
            | "isbjt"
            | "isdif"
            | "isrec"
            | "istun"
            | "k1"
            | "k1w1"
            | "k1w2"
            | "k2"
            | "k3"
            | "k3b"
            | "kb1"
            | "keta"
            | "ketas"
            | "kgidl"
            | "kgisl"
            | "kt1"
            | "kt1l"
            | "kt2"
            | "ku0"
            | "kvth0"
            | "lbjt0"
            | "lii"
            | "lpe0"
            | "lpeb"
            | "mbjtii"
            | "minv"
            | "minvcv"
            | "moin"
            | "nbjt"
            | "nch"
            | "ndif"
            | "ndiode"
            | "ndioded"
            | "nfactor"
            | "ngate"
            | "ngidl"
            | "nigc"
            | "nlx"
            | "noff"
            | "nrecf0"
            | "nrecf0d"
            | "nrecr0"
            | "nrecr0d"
            | "nsd"
            | "nsub"
            | "ntrecf"
            | "ntrecr"
            | "ntun"
            | "ntund"
            | "pclm"
            | "pdiblc1"
            | "pdiblc2"
            | "pdiblcb"
            | "pdits"
            | "pditsd"
            | "pigcd"
            | "poxedge"
            | "prt"
            | "prwb"
            | "prwg"
            | "pvag"
            | "rdsw"
            | "rdw"
            | "rgidl"
            | "rgisl"
            | "rsw"
            | "sii0"
            | "sii1"
            | "sii2"
            | "siid"
            | "u0"
            | "ua"
            | "ua1"
            | "ub"
            | "ub1"
            | "uc"
            | "uc1"
            | "ucs"
            | "ucste"
            | "ud"
            | "ud1"
            | "ute"
            | "vabjt"
            | "vbci"
            | "vdsatii0"
            | "vfb"
            | "voff"
            | "voffcv"
            | "vrec0"
            | "vrec0d"
            | "vsat"
            | "vsdfb"
            | "vsdth"
            | "vth0"
            | "vtun0"
            | "vtun0d"
            | "w0"
            | "wr"
            | "xbjt"
            | "xdif"
            | "xdifd"
            | "xj"
            | "xrcrg1"
            | "xrcrg2"
            | "xrec"
            | "xrecd"
            | "xtun"
            | "xtund"
    )
}

macro_rules! coefficients {
    ($($key:ident = $default:expr),+ $(,)?) => {
        #[derive(Debug)]
        pub(super) struct Coeff { $(pub $key: f64,)+ }
        impl Coeff {
            fn new(get: &impl Fn(&str, f64) -> f64) -> Self {
                Self { $($key: get(stringify!($key), $default),)+ }
            }
        }
    };
}
coefficients! {
    a1=0.,a2=1.,dwg=0.,dwb=0.,ntrecf=0.,ntrecr=0.,acde=1.,ados=1.,bdos=1.,kb1=1.,cdsbs=0.,vsce=0.,minvcv=0.,voffcv=0.,
    dvtp0=0.,dvtp1=0.,dvtp2=0.,dvtp3=0.,dvtp4=0.,ud=0.,ud1=0.,eu=0.,ucs=0.,ucste=-0.004775,
    kt1=-0.11, kt1l=0., kt2=0.022, k2=0., k3=0., k3b=0., w0=2.5e-6,
    dvt0=2.2,dvt1=0.53,dvt2=-0.032,dvt0w=0.,dvt1w=5.3e6,dvt2w=-0.032,
    eta0=0.08,etab=-0.07,dsub=0.56,lpe0=1.74e-7,lpeb=0.,cdsc=2.4e-4,cdscb=0.,cdscd=0.,cit=0.,
    nfactor=1., voff=-0.08,minv=0., a0=1.,ags=0.,b0=0.,b1=0.,keta=-0.6,ketas=0.,
    ua=2.25e-9,ua1=4.31e-9,ub=5.87e-19,ub1=-7.61e-18,uc=-4.65e-11,uc1=-5.6e-11,
    ute=-1.5,vsat=8e4,at=3.3e4,rdsw=100.,prt=0.,prwg=0.,prwb=0.,wr=1.,delta=0.01,
    pdits=1e-20,pditsl=0.,pditsd=0.,fprout=0.,pclm=1.3,drout=0.56,pdiblc1=0.39,pdiblc2=0.0086,pdiblcb=0.,pvag=0.,
    vbsa=0.,k1b=1.,k2b=0.,dk2b=0.,dvbd0=0.,dvbd1=0.,nofffd=1.,vofffd=0.,moinfd=1000.,
    alpha0=0.,beta0=0.,beta1=0.,beta2=0.1,vdsatii0=0.9,lii=0.,tii=0.,sii0=0.5,sii1=0.1,sii2=0.,
    siid=0.,esatii=1e7,fbjtii=0.,abjtii=0.,cbjtii=0.,ebjtii=0.,mbjtii=0.4,vbci=0.,tvbci=0.,
    ln=2e-6,lbjt0=2e-7,nbjt=1.,vabjt=10.,aely=0.,tt=1e-12,ldif0=1.,ndif=-1.,
    fbody=1.,dlcb=0.,dlbg=0.,noff=1.,delvt=0.,moin=15.,clc=1e-8,cle=0.,
    xrcrg1=12.,xrcrg2=1.,ntox=1.,poxedge=1.,nigc=1.,pigcd=1.,
    vevb=0.075,vecb=0.026,ebg=1.2,alphagb1=0.35,betagb1=0.03,vgb1=300.,
    alphagb2=0.43,betagb2=0.05,vgb2=17.,voxh=5.,deltavox=0.005
}

#[derive(Debug)]
pub(super) struct Side {
    pub width: f64,
    pub density: [f64; 4],
    pub ideal: [f64; 4],
    pub xt: [f64; 4],
    pub vrec: f64,
    pub vtun: f64,
    pub ahli: f64,
    pub grading: f64,
    pub potential: f64,
    pub cj: f64,
    pub gidl: [f64; 7],
    pub edge: crate::bsim4::Barrier,
    pub overlap: f64,
    pub area: f64,
    pub fringe: f64,
    pub ldd: f64,
    pub kappa: f64,
    pub tcj: f64,
    pub tpb: f64,
}

// Fixed capacitance ratios from §6.3. The same divider determines the
// bias-independent mode decision and the biased film potential.
#[derive(Debug)]
pub(super) struct FilmCoupling {
    pub front: f64,
    pub zero_field_front: f64,
    back: f64,
    pub drain: f64,
    short: f64,
}

impl FilmCoupling {
    pub(super) fn offset(&self, band: D, drain: D, back: D) -> D {
        self.short * band + self.drain * drain + self.back * back
    }
}

#[derive(Debug)]
pub struct Model {
    pub(super) p: Coeff,
    pub(super) side: [Side; 2],
    pub(super) l: f64,
    pub(super) w: f64,
    pub(super) lcv: f64,
    pub(super) wcv: f64,
    pub(super) tox: f64,
    pub(super) tsi: f64,
    pub(super) cox: f64,
    pub(super) cbox: f64,
    pub(super) nch: f64,
    pub(super) phi: f64,
    pub(super) vbi: f64,
    pub(super) cmin: f64,
    pub(super) vsdfb: f64,
    pub(super) vsdth: f64,
    pub(super) asd: f64,
    pub(super) coupling: FilmCoupling,
    pub(super) vgstcv: u8,
    pub(super) nseg: f64,
    pub(super) toxp: f64,
    pub(super) ngate: f64,
    pub(super) nsd: f64,
    pub(super) vfbsdoff: f64,
    pub(super) dc_contact_area: f64,
    pub(super) opposite_area: f64,
    pub(super) opposite_gate: crate::bsim4::Barrier,
    pub(super) vfb: f64,
    pub(super) flat_back: f64,
    pub(super) same_substrate_type: bool,
    pub(super) vth: f64,
    pub(super) k1: f64,
    pub(super) k1ox: f64,
    pub(super) u0: f64,
    pub(super) sign: f64,
    pub(super) copies: f64,
    pub(super) nf: f64,
    pub(super) temp: f64,
    pub(super) nominal: f64,
    pub(super) mode: u8,
    pub(super) mobility: u8,
    pub(super) cap: u8,
    pub(super) rg: u8,
    pub(super) ii: u8,
    pub(super) gidl: u8,
    pub(super) igc: bool,
    pub(super) igb: bool,
    pub(super) bjt: bool,
    pub(super) rb: f64,
    pub(super) relectrode: f64,
    pub(super) junction_r: [f64; 2],
    pub(super) gbmin: f64,
    pub(super) extension: Option<[[f64; 2]; 2]>,
    pub(super) rsd: [f64; 2],
    pub(super) rth: f64,
    pub(super) cth: f64,
    pub(super) xpart: f64,
    pub(super) gate: crate::bsim4::Barrier,
    pub(super) toxqm: f64,
    pub(super) toxref: f64,
    pub(super) extra_area: [f64; 2],
    pub(super) cgeo: f64,
    pub(super) noise: super::noise::Noise,
    pub(super) dc_charges: bool,
    pub(super) vbsusr: Option<f64>,
}

impl Model {
    pub fn bind(b: &MosBinding<'_>) -> Result<Self, String> {
        let (card, i, g) = (b.model, b.instance, b.geometry);
        let select = |name: &str, v: f64, min: u8, max: u8| {
            if v.fract() == 0. && (f64::from(min)..=f64::from(max)).contains(&v) {
                Ok(v as u8)
            } else {
                Err(format!("invalid {name}={v}"))
            }
        };
        card.accept(&["level", "lmin", "lmax", "wmin", "wmax"]);
        if card.get("version", 4.4) != 4.4 {
            return Err("expected B4SOI version 4.4".into());
        }
        let nf = i.get("nf", 1.);
        let nseg = i.get("nseg", 1.);
        let copies = i.opt(&["m", "mult"]).unwrap_or(1.);
        let width = g.w / nf;
        let corr = |axis: &str, cv: bool| {
            let dl = g.l.powf(card.get(&format!("{axis}ln"), 1.));
            let dw = width.powf(card.get(&format!("{axis}wn"), 1.));
            let size = |suffix: &str| {
                let base = card.get(&format!("{axis}{suffix}"), 0.);
                if cv {
                    card.get(&format!("{axis}{suffix}c"), base)
                } else {
                    base
                }
            };
            size("l") / dl + size("w") / dw + size("wl") / (dl * dw)
        };
        let nbc = i.get("nbc", 0.);
        let l = g.l - 2. * (card.get("lint", 0.) + corr("l", false));
        let w = width
            - nbc * card.get("dwbc", 0.)
            - (2. - nbc) * (card.get("wint", 0.) + corr("w", false));
        let lcv = g.l - 2. * (card.get("dlc", card.get("lint", 0.)) + corr("l", true));
        let wcv = width
            - nbc * card.get("dwbc", 0.)
            - (2. - nbc) * (card.get("dwc", card.get("wint", 0.)) + corr("w", true));
        let unit = if card.get("binunit", 1.) == 1. {
            1e-6
        } else {
            1.
        };
        let get = |k: &str, d: f64| {
            if k == "lpe0" {
                let term = |prefix: &str, default: f64| {
                    card.opt(&[&format!("{prefix}nlx"), &format!("{prefix}lpe0")])
                        .unwrap_or(default)
                };
                return term("", d)
                    + unit / l * term("l", 0.)
                    + unit / w * term("w", 0.)
                    + unit / l * (unit / w) * term("p", 0.);
            }
            if !binnable(k) {
                return card.get(k, d);
            }
            card.get(k, d)
                + unit / l * card.get(&format!("l{k}"), 0.)
                + unit / w * card.get(&format!("w{k}"), 0.)
                + unit / l * (unit / w) * card.get(&format!("p{k}"), 0.)
        };
        let mut p = Coeff::new(&get);
        // Paired R17 thermal_potential / forward_film threshold measurements,
        // Appendix C2: a supplied K1 without K2 uses this second-order term.
        if card.opt(&["k1"]).is_some() && card.opt(&["k2"]).is_none() {
            p.k2 -= 0.0186;
        }
        let tox = card.get("tox", 1e-8);
        let toxm = card.get("toxm", tox);
        let tsi = card.get("tsi", 1e-7);
        let tbox = card.get("tbox", 3e-7);
        let nominal = card.get("tnom", b.tnom) + 273.15;
        let temp = b.temp + 273.15;
        let nch = get("nch", 1.7e17);
        if [
            l, w, lcv, wcv, tox, toxm, tsi, tbox, nf, nseg, copies, nominal, temp, nch, p.delta,
            p.ln, p.nofffd, p.moinfd,
        ]
        .into_iter()
        .any(|x| !x.is_finite() || x <= 0.)
        {
            return Err(
                "invalid SOI dimensions, doping, temperature or smoothing parameter".into(),
            );
        }
        let cox = EPS_OX / tox;
        let cbox = EPS_OX / tbox;
        let vt = super::VT_PER_K * nominal;
        let ni = 1.45e10
            * (nominal / 300.15).powf(1.5)
            * (21.5565981 - crate::temperature::gap(nominal) / (2. * vt)).exp();
        let opt = |k: &str, d: f64| i.get(k, card.get(k, d));
        let mut mode = select("soimod", opt("soimod", 0.), 0, 3)?;
        let csi = EPS_SI / tsi;
        let bend = CHARGE * nch * 1e6 * tsi * tsi / (2. * EPS_SI);
        let nsub = get("nsub", 6e16);
        let nsd = get("nsd", 1e20);
        let flat = |nch: f64| {
            if nsub > 0. {
                -vt * (nch / nsub).ln()
            } else {
                -vt * (nch * nsub.abs() / ni.powi(2)).ln()
            }
        };
        let decay = |k: f64| {
            let x = crate::junction::safe_exp(D::cst(-0.5 * k * l / (EPS_SI * tsi / cox).sqrt())).v;
            x + 2. * x * x
        };
        let fd = select("fdmod", card.get("fdmod", 0.), 0, 1)?;
        let sce = p.dvbd0 * decay(p.dvbd1);
        let coupling = if fd == 1 {
            let total = csi + cbox + p.cdsbs;
            FilmCoupling {
                front: csi / total,
                zero_field_front: csi / total,
                back: cbox / total,
                drain: p.cdsbs * sce / total,
                short: 0.,
            }
        } else {
            let front = csi / (csi + cbox);
            FilmCoupling {
                front,
                zero_field_front: 1.,
                back: (1. - front) * (p.k1b - p.k2b * decay(p.dk2b)),
                drain: 0.,
                short: front * sce,
            }
        };
        if mode == 3 {
            let phi0 = 2. * vt * (nch / ni).ln();
            let vbi = vt * (nsd * nch / ni.powi(2)).ln();
            let estimate = coupling.front * (phi0 - bend * (1. + p.lpe0 / l) + p.vbsa)
                + coupling
                    .offset(
                        D::cst(vbi - phi0),
                        D::cst(p.vsce),
                        D::cst(-b.polarity * flat(nch)),
                    )
                    .v;
            mode = if estimate > card.get("vbs0fd", 0.5) {
                2
            } else if estimate < card.get("vbs0pd", 0.) {
                0
            } else {
                1
            };
        }
        // Section 6.1, the v4.4 ideal-FD film band-bending check.
        let nch = if mode == 2 {
            // The 150 nm DEPLETION_CLIP log gives Nch=5.8623e16/cm³,
            // equivalent to the 1.12 eV silicon gap in the §6.1 limit.
            nch * (1.12_f64 - 0.1).min(bend) / bend
        } else {
            nch
        };
        let phi = 2. * vt * (nch / ni).ln();
        // Chapter 6 uses source-referenced, polarity-normalized voltages.
        // The complementary forward-IV pair resolves the sign of VFBb.
        let flat_back = b.polarity * flat(nch);
        let vbi = vt * (nsd * nch / ni.powi(2)).ln();
        let sign = b.polarity;
        // Appendix C2 and the three SPARSE lengths recover the BSIM
        // doping-derived body coefficients and the zero-bias flatband relation.
        let k1 = if card.opt(&["k1", "k2"]).is_some() {
            get("k1", 0.6)
        } else {
            let gamma1 = card.get("gamma1", crate::bsim_body::gamma(nch, cox));
            let gamma2 = card.get("gamma2", crate::bsim_body::gamma(nsub.abs(), cox));
            let vbm = -card.get("vbm", -3.).abs();
            let vbx = -card
                .get(
                    "vbx",
                    crate::bsim_body::vbx(phi, nch, card.get("xt", 1.55e-7)),
                )
                .abs();
            let denom = 2. * ((phi * (phi - vbm)).sqrt() - phi) + vbm;
            if denom == 0. {
                return Err("degenerate SOI VBM in body coefficient calculation".into());
            }
            p.k2 += (gamma1 - gamma2) * ((phi - vbx).sqrt() - phi.sqrt()) / denom;
            get("k1", gamma2 - 2. * p.k2 * (phi - vbm).sqrt())
        };
        let vth0 = get("vth0", sign * (get("vfb", -1.) + phi + k1 * phi.sqrt()));
        let k1 = k1 * (1. + get("k1w1", 0.) / (w + get("k1w2", 0.)).max(1e-12));
        let mut vth = sign * (vth0 + i.opt(&["delvto", "delvt0"]).unwrap_or(0.));
        let u = get("u0", if sign > 0. { 0.067 } else { 0.025 });
        let mut u0 = if u > 1. { u * 1e-4 } else { u };
        if let Some(stress) = crate::bsim4::layout_stress(b, nf, b.scale) {
            u0 *= stress.mobility;
            p.vsat *= stress.velocity;
            p.at *= stress.velocity;
            vth += sign * stress.threshold;
            p.k2 += stress.k2;
            p.eta0 += stress.eta;
        }
        p.k2 *= tox / toxm;
        let heat = select("shmod", card.get("shmod", 0.), 0, 1)? != 0;
        let thermal_width = (w + card.get("wth0", 0.)) * nf;
        let rth0 = opt("rth0", 0.);
        let cth0 = opt("cth0", 1e-5);
        let rbody = card.get("rbody", 0.) * w / (nseg * l);
        let rhalo = card.get("rhalo", 1e15) * w / (2. * nseg);
        let rb = opt("frbody", 1.)
            * (if rbody + rhalo > 0. {
                rbody * rhalo / (rbody + rhalo)
            } else {
                0.
            } + card.get("rbsh", 0.) * i.get("nrb", 1.));
        let ngcon = card.get("ngcon", 1.);
        let relectrode = card.get("rshg", 0.1) * (card.get("xgw", 0.) + w / (3. * ngcon * nseg))
            / (ngcon * (g.l - card.get("xgl", 0.)) * nf);
        let toxref = card.get("toxref", 2.5e-9);
        let toxqm = card.get("toxqm", tox);
        let barrier = |length: f64, thick: f64, coeff: [f64; 3]| {
            let (a, b) = if sign > 0. {
                (4.97232e-7, 7.45669e11)
            } else {
                (3.42537e-7, 1.16645e12)
            };
            crate::bsim4::Barrier {
                scale: w * nf * length * a * (toxref / thick).powf(p.ntox) / thick.powi(2),
                exponent: b * thick,
                coeff,
            }
        };
        let opposite_area = i.get("agbcp2", 0.) * b.scale.powi(2);
        let opposite_gate = barrier(
            opposite_area / (w * nf),
            toxqm,
            [
                get("aigbcp2", 0.043),
                get("bigbcp2", 0.0054),
                get("cigbcp2", 0.0075),
            ],
        );
        let gate = barrier(
            l,
            tox,
            [get("aigc", 0.43), get("bigc", 0.054), get("cigc", 0.075)],
        );
        // Eq. 4.18 uses the exposed perimeter of each diffusion.
        let fringe_coeff = card.get("cfrcoeff", 1.);
        if fringe_coeff <= 0. {
            return Err("CFRCOEFF must be positive".into());
        }
        let fringe = card.get("csdesw", 0.) * (fringe_coeff.min(2.) * (1. + tsi / tbox)).ln();
        let side = std::array::from_fn(|j| {
            let drain = j == 0;
            let pair = |s: &str, d: &str, def: f64| {
                let src = get(s, def);
                if drain { get(d, src) } else { src }
            };
            let sw = w / nseg + i.get(if drain { "pdbcp" } else { "psbcp" }, 0.) * b.scale;
            let area = if drain { g.ad } else { g.as_ };
            let perimeter = if drain { g.pd } else { g.ps };
            let ldd = get(if drain { "cgdl" } else { "cgsl" }, 0.);
            let dlc = card.get("dlc", 0.);
            let def_overlap = if dlc > 0. {
                (dlc * cox - ldd).max(0.)
            } else {
                0.6 * get("xj", tsi) * cox
            };
            let cf = card.get(
                "cf",
                2. * EPS_OX / std::f64::consts::PI * (4e-7 / tox).ln_1p(),
            );
            Side {
                width: sw * nf,
                density: [
                    pair("isdif", "iddif", 0.),
                    pair("isrec", "idrec", 1e-5),
                    pair("istun", "idtun", 0.),
                    pair("isbjt", "idbjt", 1e-6),
                ],
                ideal: [
                    pair("ndiode", "ndioded", 1.),
                    pair("nrecf0", "nrecf0d", 2.),
                    pair("nrecr0", "nrecr0d", 10.),
                    pair("ntun", "ntund", 10.),
                ],
                xt: [
                    pair("xdif", "xdifd", 1.),
                    pair("xrec", "xrecd", 1.),
                    pair("xtun", "xtund", 0.),
                    pair("xbjt", "xbjtd", 1.),
                ],
                vrec: pair("vrec0", "vrec0d", 0.),
                vtun: pair("vtun0", "vtun0d", 0.),
                ahli: pair("ahli", "ahlid", 0.),
                cj: pair("cjswg", "cjswgd", 1e-10) * sw * nf * (tsi / 1e-7),
                grading: pair("mjswg", "mjswgd", 0.5),
                potential: pair("pbswg", "pbswgd", 0.7),
                gidl: std::array::from_fn(|k| {
                    let prefix = ["a", "b", "c", "e", "r", "k", "f"][k];
                    let d = get(
                        &format!("{prefix}gidl"),
                        [0., 2.3e9, 0.5, 1.2, 1., 0., 0.][k],
                    );
                    if drain {
                        d
                    } else {
                        get(&format!("{prefix}gisl"), d)
                    }
                }),
                edge: barrier(
                    pair("dlcig", "dlcigd", card.get("lint", 0.)),
                    tox * p.poxedge,
                    std::array::from_fn(|k| {
                        let a = ["aig", "big", "cig"][k];
                        get(
                            &format!("{a}{}", if drain { "d" } else { "s" }),
                            get(&format!("{a}sd"), [0.43, 0.054, 0.075][k]),
                        )
                    }),
                ),
                overlap: wcv / nseg
                    * nf
                    * (card.get(if drain { "cgdo" } else { "cgso" }, def_overlap) + cf),
                area,
                fringe: fringe * (perimeter - w * nf).max(0.),
                ldd: ldd * wcv * nf / nseg,
                kappa: get("ckappa", 0.6),
                tcj: pair("tcjswg", "tcjswgd", 0.),
                tpb: pair("tpbswg", "tpbswgd", 0.),
            }
        });
        let phi_sd = 2. * vt * (nsub.abs() / ni).ln();
        let vsdfb = get(
            "vsdfb",
            if nsub > 0. {
                // Eq. 4.17 inverted at both diffusions in KB1 gives +0.3 V.
                -vt * (1e20 * nsub / ni.powi(2)).ln() + 0.3
            } else {
                -vt * (1e20 / nsub.abs()).ln() + 0.3
            },
        );
        let gamma = crate::bsim_body::gamma(nsub.abs(), cbox);
        let vsdth = get(
            "vsdth",
            vsdfb + nsub.signum() * (phi_sd + gamma * phi_sd.sqrt()),
        );
        let asd = card.get("asd", 0.3);
        let toxp = card.get("toxp", tox);
        if nf.fract() != 0.
            || nf < 1.
            || nseg < 1.
            || !(0.0..=2.0).contains(&nbc)
            || g.ad < 0.
            || g.as_ < 0.
            || p.cdsbs < 0.
            || p.vsat <= 0.
            || p.pdits < 0.
            || p.pditsl < 0.
            || p.clc < 0.
            || !vbi.is_finite()
            || !flat_back.is_finite()
            || !u0.is_finite()
            || !(0.0..1.0).contains(&asd)
            || asd == 0.
            || toxp <= 0.
            || nsub == 0.
            || p.a2 <= 0.
            || p.lbjt0 <= 0.
            || l + p.lpe0 <= 0.
            || l + p.lpeb <= 0.
            || w + p.b1 <= 0.
            || w + p.w0 <= 0.
            || p.deltavox <= 0.
            || p.vgb1 <= 0.
            || p.vgb2 <= 0.
            || p.vevb <= 0.
            || p.vecb <= 0.
            || p.noff <= 0.
            || p.moin <= 0.
            || !phi.is_finite()
            || phi <= 0.
            || u0 <= 0.
            || rth0 < 0.
            || cth0 < 0.
            || thermal_width <= 0.
            || rb < 0.
            || !relectrode.is_finite()
            || relectrode < 0.
            || toxqm <= 0.
            || p.poxedge <= 0.
            || p.nigc <= 0.
            || side
                .iter()
                .any(|s| s.potential <= 0. || s.ideal.iter().any(|n| *n <= 0.))
        {
            return Err("invalid SOI transport, junction or resistance parameters".into());
        }
        let body_network = select("rbodymod", opt("rbodymod", 0.), 0, 1)? != 0;
        let resistances = ["rbdb", "rbsb"].map(|k| opt(k, 50.));
        let junction_r = if body_network { resistances } else { [0.; 2] };
        let external_r =
            [["rdw", "rdwmin"], ["rsw", "rswmin"]].map(|[r, min]| [get(r, 50.), card.get(min, 0.)]);
        let extension =
            (select("rdsmod", card.get("rdsmod", 0.), 0, 1)? != 0).then_some(external_r);
        let rsd = ["nrd", "nrs"].map(|k| i.get(k, 1.) * card.get("rsh", 0.));
        if rsd.iter().chain(&junction_r).any(|r| *r < 0.)
            || extension.iter().flatten().flatten().any(|r| *r < 0.)
        {
            return Err("negative SOI lead resistance".into());
        }
        Ok(Self {
            mode,
            l,
            w,
            lcv,
            wcv,
            tox,
            tsi,
            cox,
            cbox,
            nch: nch * 1e6,
            phi,
            vbi,
            flat_back,
            same_substrate_type: nsub > 0.,
            vsdfb,
            vsdth,
            asd,
            toxp,
            nseg,
            ngate: get("ngate", 0.),
            nsd,
            vfbsdoff: get("vfbsdoff", 0.),
            opposite_area,
            opposite_gate,
            dc_contact_area: i.get("agbcpd", 0.) * b.scale.powi(2),
            vfb: get("vfb", vth - phi - k1 * phi.sqrt()),
            cmin: card.get(
                "csdmin",
                1. / (1. / cbox
                    + (2. * EPS_SI * phi_sd / (CHARGE * nsub.abs() * 1e6)).sqrt() / EPS_SI),
            ),
            coupling,
            vgstcv: select("vgstcvmod", card.get("vgstcvmod", 1.), 0, 2)?,
            vth,
            k1,
            k1ox: k1 * tox / toxm,
            u0,
            sign,
            copies,
            nf,
            temp,
            nominal,
            mobility: select("mobmod", card.get("mobmod", 1.), 1, 4)?,
            cap: select("capmod", card.get("capmod", 2.), 2, 3)?,
            rg: select("rgatemod", opt("rgatemod", 0.), 0, 3)?,
            ii: select("iiimod", card.get("iiimod", 0.), 0, 1)?,
            gidl: select("gidlmod", card.get("gidlmod", 0.), 0, 1)?,
            igc: select("igcmod", card.get("igcmod", 0.), 0, 1)? != 0,
            igb: select("igbmod", card.opt(&["igmod", "igbmod"]).unwrap_or(0.), 0, 1)? != 0,
            bjt: i.get("bjtoff", 0.) == 0.,
            rb,
            relectrode,
            rsd,
            junction_r,
            extension,
            gbmin: card.get("gbmin", 1e-12),
            rth: if heat { rth0 / thermal_width } else { 0. },
            cth: cth0 * thermal_width,
            xpart: card.get("xpart", 0.),
            gate,
            toxqm,
            toxref,
            extra_area: ["agbcp", "aebcp"].map(|k| i.get(k, 0.) * b.scale.powi(2)),
            cgeo: card.get("cgeo", 0.) * l * nf,
            noise: super::noise::Noise::bind(card, sign)?,
            dc_charges: i.get("debug", 0.) == -1.,
            vbsusr: i.opt(&["vbsusr"]),
            p,
            side,
        })
    }
}
