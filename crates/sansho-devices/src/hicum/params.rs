//! Nominal parameters of the native HICUM/L2 card.
//! Provenance: parameter spellings and nominal values from a black-box 44.2
//! showmod measurement. The grouping and validation are original to Sansho.

use crate::ModelParams;
use sansho_diag::Diagnostic;

pub const VERSION: &str = "2.4.0";

// Keep each physical input and its binder together. Unimplemented controls
// stay unconsumed so that the engine can diagnose them.
macro_rules! card {
    ($($name:ident = $default:expr),+ $(,)?) => {
        #[derive(Debug)]
        pub struct Model {
            pub tnom: Option<f64>,
            pub(super) polarity: f64,
            $(pub(super) $name: f64,)+
        }

        impl Model {
            pub fn from_params(
                p: &ModelParams,
                polarity: f64,
                version: Option<&str>,
            ) -> Result<Self, Diagnostic> {
                if version.is_some_and(|v| v != VERSION)
                    || p.opt(&["version"]).is_some_and(|v| v != 2.4)
                {
                    return Err(Diagnostic::error(
                        "E-ELAB-PARAM",
                        format!("supported HICUM/L2 version is {VERSION}"),
                    ));
                }
                p.accept(&["level", "flcomp", "flcono", "zetacx"]);
                for key in ["vbe_max", "vbc_max", "vce_max"] {
                    let limit = p.get(key, 1e99);
                    if !limit.is_finite() || limit <= 0. {
                        return Err(Diagnostic::error(
                            "E-ELAB-PARAM",
                            format!("invalid HICUM {key} limit"),
                        ));
                    }
                }
                let model = Self {
                    polarity,
                    tnom: p.opt(&["tnom"]),
                    $($name: p.get(stringify!($name), $default),)+
                };
                if ![$(model.$name,)+].iter().all(|v| v.is_finite()) {
                    return Err(Diagnostic::error("E-ELAB-PARAM", "nonfinite HICUM parameter"));
                }
                model.validate()?;
                Ok(model)
            }
        }
    }
}
card! {
    c10=2e-30, qp0=2e-14, ich=0., hf0=1., hfe=1., hfc=1., hjei=1.,
    ahjei=0., rhjei=1., hjci=1.,
    ibeis=1e-18, mbei=1., ireis=0., mrei=2., ibeps=0., mbep=1., ireps=0., mrep=2.,
    mcf=1., tbhrec=0., ibcis=1e-16, mbci=1., ibcxs=0., mbcx=1.,
    ibets=0., abet=40., tunode=0.,
    favl=0., qavl=0., kavl=0., alfav=0., alqav=0., alkav=0.,
    rbi0=0., rbx=0., fgeo=0.6557, fdqr0=0., fcrbi=0., fqi=1., re=0., rcx=0.,
    itss=0., msf=1., iscs=0., msc=1., tsf=0., rsu=0., csu=0.,
    cjei0=1e-20, vdei=0.9, zei=0.5, ajei=2.5,
    cjep0=1e-20, vdep=0.9, zep=0.5, ajep=2.5,
    cjci0=1e-20, vdci=0.7, zci=0.4, vptci=100.,
    cjcx0=1e-20, vdcx=0.7, zcx=0.4, vptcx=100.,
    fbcpar=0., fbepar=1., cjs0=0., vds=0.6, zs=0.5, vpts=100.,
    cscp0=0., vdsp=0.6, zsp=0.5, vptsp=100.,
    t0=0., dt0h=0., tbvl=0., tef0=0., gtfe=1., thcs=0., ahc=0.1,
    fthc=0., rci0=150., vlim=0.5, vces=0.1, vpt=100., aick=0.001, delck=2.,
    tr=0., vcbar=0., icbar=0., acbar=0.01, cbepar=0., cbcpar=0.,
    alqf=0.167, alit=0.333, flnqs=0., kf=0., af=2., cfbe=0., kfre=0., afre=2.,
    latb=0., latl=0., vgb=1.17, alt0=0., kt0=0., zetaci=0., alvs=0., alces=0.,
    zetarbi=0., zetarbx=0., zetarcx=0., zetare=0.,
    vge=1.17, vgc=1.17, vgs=1.17, f1vg=-0.000102377, f2vg=0.00043215,
    zetact=3., zetabet=3.5, alb=0., dvgbe=0., zetahjei=1., zetavgbe=1.,
    flsh=0., rth=0., zetarth=0., alrth=0., cth=0.,
}

impl Model {
    fn validate(&self) -> Result<(), Diagnostic> {
        let m = self;
        if [
            m.qp0, m.mbei, m.mrei, m.mbep, m.mrep, m.mcf, m.mbci, m.mbcx, m.msc, m.msf, m.vdei,
            m.vdep, m.vdci, m.vdcx, m.vds, m.vdsp, m.vlim, m.delck, m.rhjei,
        ]
        .iter()
        .any(|x| *x <= 0.)
            || [
                m.c10, m.ich, m.ibeis, m.ireis, m.ibeps, m.ireps, m.ibcis, m.ibcxs, m.ibets,
                m.itss, m.iscs, m.rbi0, m.rbx, m.re, m.rcx, m.rsu, m.csu, m.rci0, m.rth, m.cth,
                m.t0, m.tef0, m.thcs, m.tr, m.tsf, m.tbhrec, m.cjei0, m.cjep0, m.cjci0, m.cjcx0,
                m.cjs0, m.cscp0, m.cbepar, m.cbcpar, m.kf, m.kfre, m.alit, m.alqf, m.hf0, m.hfe,
                m.hfc, m.ahc, m.gtfe, m.favl, m.qavl, m.kavl, m.abet, m.fcrbi, m.fgeo, m.fdqr0,
                m.af, m.afre, m.latb, m.latl, m.zei, m.zep, m.zci, m.zcx, m.zs, m.zsp,
            ]
            .iter()
            .any(|x| *x < 0.)
            || [m.fbepar, m.fbcpar, m.fqi, m.fthc]
                .iter()
                .any(|x| !(0.0..=1.).contains(x))
            || !matches!(m.flsh, 0. | 1. | 2.)
            || !matches!(m.flnqs, 0. | 1.)
            || !matches!(m.tunode, 0. | 1.)
        {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                "invalid HICUM charge, junction, resistance, transit, noise or selector parameter",
            ));
        }
        Ok(())
    }
}
