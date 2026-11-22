//! Layout-dependent shifts and substrate, gate and diffusion resistances.
//! Provenance: Berkeley BSIM4 4.8.2 technical manual, §§5.3, 9.2–3, 12.2, 14.1 and 15.1.

use super::{Binding, Size};
use crate::{ModelParams, mos1::Mos1Geometry};

pub(super) fn stress_active(i: &ModelParams, nf: f64) -> bool {
    i.get("sa", 0.) > 0. && i.get("sb", 0.) > 0. && (nf == 1. || i.get("sd", 0.) > 0.)
}
/// Common LOD geometry and shifts from BSIM4 §14 and BSIMSOI §9.
pub(crate) struct Stress {
    pub mobility: f64,
    pub velocity: f64,
    pub threshold: f64,
    pub k2: f64,
    pub eta: f64,
}

pub(crate) fn layout_stress(b: &Binding<'_>, nf: f64, scale: f64) -> Option<Stress> {
    let (p, i) = (b.model, b.instance);
    let Mos1Geometry { l, w, .. } = b.geometry;
    let sa = i.get("sa", 0.) * scale;
    let sb = i.get("sb", 0.) * scale;
    let sd = i.get("sd", 0.) * scale;
    if !stress_active(i, nf) {
        return None;
    }
    let width = w / nf + p.get("xw", 0.) + p.get("wlod", 0.);
    let length = l + p.get("xl", 0.);
    let factor = |base: &str, exp: &str| {
        let l = length.powf(p.get(&format!("llod{exp}"), 0.));
        let w = width.powf(p.get(&format!("wlod{exp}"), 0.));
        1. + p.get(&format!("l{base}"), 0.) / l
            + p.get(&format!("w{base}"), 0.) / w
            + p.get(&format!("p{base}"), 0.) / (l * w)
    };
    let tnom = p.get("tnom", b.tnom) + 273.15;
    let ku = p.get("ku0", 0.)
        / (factor("ku0", "ku0") * (1. + p.get("tku0", 0.) * ((b.temp + 273.15) / tnom - 1.)));
    let kv = factor("kvth0", "vth");
    let reference = 1. / (p.get("saref", 1e-6) + 0.5 * l) + 1. / (p.get("sbref", 1e-6) + 0.5 * l);
    // Euler-Maclaurin tail of the equally spaced reciprocal sum for large NF.
    let inverse = |distance: f64| {
        let first = distance + 0.5 * l;
        let pitch = sd + l;
        let count = nf.min(128.) as usize;
        let mut sum = (0..count)
            .map(|k| 1. / (first + k as f64 * pitch))
            .sum::<f64>();
        if nf > 128. {
            let a = first + 128. * pitch;
            let z = first + nf * pitch;
            sum += (z / a).ln() / pitch
                + 0.5 * (1. / a - 1. / z)
                + pitch / 12. * (1. / a.powi(2) - 1. / z.powi(2));
        }
        sum / nf
    };
    let actual = inverse(sa) + inverse(sb);
    let kvsat = p.get("kvsat", 0.);
    let diff = actual - reference;
    Some(Stress {
        mobility: (1. + ku * actual) / (1. + ku * reference),
        velocity: (1. + kvsat * ku * actual) / (1. + kvsat * ku * reference),
        threshold: p.get("kvth0", 0.) / kv * diff,
        k2: p.get("stk2", 0.) / kv.powf(p.get("lodk2", 1.)) * diff,
        eta: p.get("steta0", 0.) / kv.powf(p.get("lodeta0", 1.)) * diff,
    })
}

pub(super) fn stress(m: &mut Size, b: &Binding<'_>, scale: f64) -> Result<(), String> {
    let (p, i) = (b.model, b.instance);
    let bin = super::params::Bin::new(p, m.l, m.w);
    if let Some(stress) = layout_stress(b, m.nf, scale) {
        // The 44.2 BSIM4 KU0-only sweep leaves mobility unchanged.
        m.vsat *= stress.velocity;
        m.vth0 += m.sign * stress.threshold;
        m.k2ox += m.toxe / p.get("toxm", m.toxe) * stress.k2;
        m.p.eta0 += stress.eta;
    }
    let sc = i.get("sca", 0.)
        + p.get("web", 0.) * i.get("scb", 0.)
        + p.get("wec", 0.) * i.get("scc", 0.);
    if p.get("wpemod", 0.) == 1. {
        m.vth0 += m.sign * bin.get("kvth0we", 0.) * sc;
        m.k2ox += m.toxe / p.get("toxm", m.toxe) * bin.get("k2we", 0.) * sc;
        m.u0 *= 1. + bin.get("ku0we", 0.) * sc;
    }
    if !m.u0.is_finite() || !m.vsat.is_finite() || m.u0 <= 0. || m.vsat <= 0. {
        return Err(
            "temperature- and layout-adjusted mobility and velocity must be positive".into(),
        );
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct Network {
    pub body: Option<[f64; 5]>,
    pub body_noise: [f64; 5],
    pub gate: f64,
    pub gate_mode: u8,
    pub diffusion: [f64; 2],
    pub external: bool,
    pub extension: [[f64; 2]; 2],
    pub vflat: f64,
    pub xrcrg1: f64,
    pub xrcrg2: f64,
}
impl Network {
    pub(super) fn new(b: &Binding<'_>, m: &mut Size) -> Result<Self, String> {
        let p = b.model;
        let bin = super::params::Bin::new(p, m.l, m.w);
        let i = b.instance;
        let gate_mode = i.get("rgatemod", p.get("rgatemod", 0.));
        let body_mode = i.get("rbodymod", p.get("rbodymod", 0.));
        let rds = p.get("rdsmod", 0.);
        if ![0., 1., 2., 3.].contains(&gate_mode)
            || ![0., 1., 2.].contains(&body_mode)
            || ![0., 1.].contains(&rds)
        {
            return Err("invalid resistance selector".into());
        }
        let external = rds == 1.;
        let diffusion =
            std::array::from_fn(|k| i.get(["nrd", "nrs"][k], m.squares[k]) * p.get("rsh", 0.));
        let ngcon = i.get("ngcon", p.get("ngcon", 1.));
        let gate = if gate_mode > 0. {
            p.get("rshg", 0.1)
                * (i.opt(&["xgw"]).map_or(p.get("xgw", 0.), |v| v * b.scale) + m.wj / (3. * ngcon))
                / (ngcon * (b.geometry.l + p.get("xl", 0.) - p.get("xgl", 0.)) * m.nf)
        } else {
            0.
        };
        let mut body_noise = [0.; 5];
        let body = if body_mode == 0. {
            None
        } else {
            let gmin = p.get("gbmin", 1e-12);
            let mut res = ["rbpb", "rbpd", "rbps", "rbdb", "rbsb"].map(|n| i.get(n, p.get(n, 50.)));
            if body_mode == 2. {
                let scaled = |base: &str, exp: &str, default: f64| {
                    p.get(&format!("{base}0"), default)
                        * (m.l * 1e6).powf(p.get(&format!("{exp}l"), 0.))
                        * (m.w * 1e6).powf(p.get(&format!("{exp}w"), 0.))
                        * m.nf.powf(p.get(&format!("{exp}nf"), 0.))
                };
                let parallel = |x: f64, y: f64| 1. / (1. / x + 1. / y);
                res[0] = parallel(
                    scaled("rbpbx", "rbpbx", 100.),
                    scaled("rbpby", "rbpby", 100.),
                );
                let one = p.opt(&["rbps0"]).is_none() || p.opt(&["rbpd0"]).is_none();
                for (slot, n) in ["rbpd", "rbps"].into_iter().enumerate() {
                    res[slot + 1] = if one { 0. } else { scaled(n, n, 50.) };
                }
                let three = ["rbdb", "rbsb"]
                    .iter()
                    .any(|n| p.opt(&[&format!("{n}x0"), &format!("{n}y0")]).is_none());
                for (slot, n) in ["rbdb", "rbsb"].into_iter().enumerate() {
                    res[slot + 3] = if one || three {
                        f64::INFINITY
                    } else {
                        parallel(
                            scaled(&format!("{n}x"), "rbsdbx", 100.),
                            scaled(&format!("{n}y"), "rbsdby", 100.),
                        )
                    };
                }
            }
            // The 1-R and 3-R reductions retain numerical ties and GBMIN in
            // the matrix. Only the physical resistances generate thermal noise.
            body_noise = res.map(|r| if r > 0. { 1. / r.max(1e-3) } else { 0. });
            Some(res.map(|r| gmin + 1. / r.max(1e-3)))
        };
        let mut extension = [[0.; 2]; 2];
        if external {
            let tempmod = p.get("tempmod", 0.);
            let tn = p.get("tnom", b.tnom);
            let dt = b.temp - tn;
            let prt = bin.get("prt", 0.);
            let adj = |v: f64| {
                if tempmod == 0. {
                    (v + prt * dt / (tn + 273.15)).max(0.)
                } else {
                    (v * (1. + prt * dt)).max(0.)
                }
            };
            for (side, n) in ["rdw", "rsw"].into_iter().enumerate() {
                let norm = (m.wj * 1e6).powf(m.p.wr) * m.nf;
                extension[side] = [
                    adj(p.get(&format!("{n}min"), 0.)) / norm,
                    adj(bin.get(n, 100.)) / norm,
                ];
            }
            m.p.rdsw = 0.;
            m.rmin = 0.;
        }
        Ok(Self {
            body,
            body_noise,
            gate,
            gate_mode: gate_mode as u8,
            diffusion,
            external,
            extension,
            vflat: m.flat_sd,
            xrcrg1: bin.get("xrcrg1", 12.),
            xrcrg2: bin.get("xrcrg2", 1.),
        })
    }
}

/// Diffusion area, isolation-edge perimeter and approximate contact squares by side.
pub(super) fn diffusions(
    p: &ModelParams,
    i: &ModelParams,
    nf: f64,
    width: f64,
) -> Result<[[f64; 3]; 2], String> {
    let geo = i.get("geomod", p.get("geomod", 0.));
    let rgeo = i.get("rgeomod", p.get("rgeomod", 0.));
    if !(0. ..=10.).contains(&geo)
        || geo.fract() != 0.
        || !(0. ..=8.).contains(&rgeo)
        || rgeo.fract() != 0.
        || (geo >= 9. && nf % 2. != 0.)
        || nf < 1.
        || nf.fract() != 0.
    {
        return Err("invalid finger count or diffusion geometry selector".into());
    }
    let gap = p.get("dmcgt", 0.);
    let cg = (p.get("dmcg", 0.) - gap).max(0.);
    let ci = p.get("dmci", p.get("dmcg", 0.)).max(0.);
    let dg = (p.get("dmdg", 0.) - gap).max(0.);
    // End types in drain, source order. All interior diffusions are shared.
    let ends = match geo as u8 {
        0 => [0, 0],
        1 => [1, 0],
        2 => [0, 1],
        3 => [1, 1],
        4 => [2, 0],
        5 => [2, 1],
        6 => [0, 2],
        7 => [1, 2],
        8 => [2, 2],
        9 => [1, 0],
        _ => [0, 1],
    };
    let min_source = i.get("min", 0.) != 0.;
    let n_end = if nf % 2. != 0. {
        [1., 1.]
    } else if geo == 9. {
        [0., 2.]
    } else if geo == 10. || min_source {
        [2., 0.]
    } else {
        [0., 2.]
    };
    Ok(std::array::from_fn(|k| {
        let end = n_end[k];
        let shared = (nf - end) * 0.5;
        let (depth, outer) = match ends[k] {
            0 => (cg + ci, width),
            1 => (cg, 0.),
            _ => (dg, 0.),
        };
        // In GEO 9/10 one of the outer diffusions combines an isolated and a shared end.
        let mixed = if geo >= 9. && end == 2. { 1. } else { 0. };
        let total = (end - mixed) * depth + (2. * shared + mixed) * cg;
        let squares = if rgeo == 0. || cg == 0. {
            0.
        } else {
            let point = matches!((k, rgeo as u8), (0, 2 | 4 | 8) | (1, 3 | 4 | 6));
            let end_squares = if point && depth > 0. {
                width / (3. * depth)
            } else {
                cg / width
            };
            1. / (2. * shared * width / cg + end / end_squares)
        };
        [width * total, 2. * total + (end - mixed) * outer, squares]
    }))
}
