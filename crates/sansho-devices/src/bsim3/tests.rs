//! Regression tests for terminal laws, derivatives and card binding.
//! Provenance: original tests of UC Berkeley BSIM3v3.3 manual, Appendices B and D.

use super::*;
use crate::{ModelParams, mos1::Mos1Geometry};

fn model(params: &[(&str, f64)], sign: f64) -> Model {
    let params: Vec<_> = params.iter().map(|&(k, v)| (k.to_string(), v)).collect();
    Model::new(&MosBinding {
        model: &ModelParams::new(&params),
        instance: &ModelParams::default(),
        geometry: Mos1Geometry {
            w: 2e-6,
            l: 0.45e-6,
            ad: 1e-12,
            as_: 1e-12,
            pd: 3e-6,
            ps: 3e-6,
        },
        temp: 43.,
        tnom: 27.,
        scale: 1.,
        polarity: sign,
    })
    .unwrap()
}

fn close(a: f64, b: f64, rtol: f64, atol: f64) {
    assert!(
        (a - b).abs() <= atol + rtol * a.abs().max(b.abs()),
        "{a:e} vs {b:e}"
    );
}

#[test]
fn terminal_jacobians_and_conservation() {
    for sign in [-1., 1.] {
        for cap in 0..=3 {
            for mob in 1..=3 {
                for part in [0., 0.5, 1.] {
                    let m = model(
                        &[
                            ("capmod", cap as f64),
                            ("mobmod", mob as f64),
                            ("xpart", part),
                            ("vth0", 0.55 * sign),
                            ("ngate", 1e20),
                            ("alpha0", 1e-7),
                            ("beta0", 1.),
                            ("rdsw", 110.),
                            ("a2", 0.92),
                            ("ags", 0.2),
                        ],
                        sign,
                    );
                    for (d, g) in [(0.04, -1.7), (0.08, 0.4), (0.6, 1.3), (-0.25, 1.2)] {
                        let bias = [d, g, 0., -0.3].map(|v| v * sign);
                        let (i, q) = m.terminals(bias, 1e-12);
                        for (rows, atol) in [(&i, 1e-18), (&q, 1e-28)] {
                            close(rows.iter().map(|r| r.v).sum(), 0., 0., atol);
                            for k in 0..4 {
                                close(rows.iter().map(|r| r.d[k]).sum(), 0., 0., atol);
                                close(rows[k].d.iter().sum(), 0., 0., atol);
                            }
                        }
                        for k in 0..4 {
                            let mut hi = bias;
                            let mut lo = bias;
                            let h = 1e-6;
                            hi[k] += h;
                            lo[k] -= h;
                            let (ip, qp) = m.terminals(hi, 1e-12);
                            let (im, qm) = m.terminals(lo, 1e-12);
                            for r in 0..4 {
                                close(i[r].d[k], (ip[r].v - im[r].v) / (2. * h), 3e-5, 2e-12);
                                close(q[r].d[k], (qp[r].v - qm[r].v) / (2. * h), 3e-5, 5e-22);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn exchange_and_zero_drain_bias() {
    for cap in 0..=3 {
        let m = model(&[("capmod", cap as f64)], 1.);
        let (i, q) = m.terminals([0., 1.2, 0., -0.1], 0.);
        close(i[0].v, i[2].v, 0., 1e-24);
        close(q[0].v, q[2].v, 0., 1e-27);
        let c = m.channel(D::cst(1.2), D::var(0., 0), D::cst(-0.1));
        assert_eq!(c.ids.v, 0.);
        assert!(c.ids.d[0] > 0.);
        for dv in [1e-7, 0.01, 0.9] {
            let (i, q) = m.terminals([dv, 1.2, 0., -0.1], 1e-12);
            let (ri, rq) = m.terminals([0., 1.2, dv, -0.1], 1e-12);
            for (r, k) in [2, 1, 0, 3].into_iter().enumerate() {
                close(i[r].v, ri[k].v, 1e-12, 1e-24);
                close(q[r].v, rq[k].v, 1e-12, 1e-28);
            }
        }
    }
}

#[test]
fn values_remain_continuous_at_region_boundaries() {
    for cap in 0..=3 {
        let m = model(&[("capmod", cap as f64), ("eta0", 0.), ("etab", 0.)], 1.);
        let v = |g, d| m.channel(D::cst(g), D::cst(d), D::cst(-0.2));
        let b = v(1.4, 0.);
        let threshold = m.p.vfbcv + m.phi + m.k1 * b.root.v;
        for (d, g, terminal) in [
            (0.05, m.p.vfbcv - 0.2, 1),
            (0.05, threshold, 1),
            (0.05, b.threshold.v, 1),
            (b.sat.v, 1.4, 0),
            (
                (1.4 - threshold) / (b.bulk0.v * (1. + (m.p.clc / m.l).powf(m.p.cle))),
                1.4,
                0,
            ),
        ] {
            let mut hi = [d, g, 0., -0.2];
            let mut lo = hi;
            hi[terminal] += 1e-8;
            lo[terminal] -= 1e-8;
            let (ip, qp) = m.terminals(hi, 1e-12);
            let (im, qm) = m.terminals(lo, 1e-12);
            for j in 0..4 {
                close(ip[j].v, im[j].v, 0., 1e-10);
                close(qp[j].v, qm[j].v, 0., 5e-22);
            }
        }
    }
}

#[test]
fn bins_use_iv_geometry_and_chosen_units() {
    let m = model(
        &[
            ("lint", 25e-9),
            ("wint", 0.1e-6),
            ("vsat", 1e5),
            ("lvsat", 1e4),
            ("wvsat", 2e4),
            ("pvsat", 3e4),
            ("at", 0.),
            ("dlc", 1e-8),
            ("ldlc", 1e-9),
        ],
        1.,
    );
    close(
        m.vsat,
        1e5 + 1e4 / 0.4 + 2e4 / 1.8 + 3e4 / (0.4 * 1.8),
        1e-13,
        0.,
    );
    close(m.lcv, 0.45e-6 - 2. * (1e-8 + 1e-9 / 0.4), 1e-13, 0.);
    let m_si = model(
        &[
            ("binunit", 0.),
            ("lint", 25e-9),
            ("wint", 0.1e-6),
            ("vsat", 1e5),
            ("lvsat", 0.01),
            ("wvsat", 0.02),
            ("pvsat", 3e-8),
            ("at", 0.),
        ],
        1.,
    );
    close(m.vsat, m_si.vsat, 1e-14, 0.);
}

#[test]
fn noise_selectors_choose_independent_channel_and_flicker_laws() {
    for vg in [0.1, 1.5] {
        let powers = (1..=4)
            .map(|mode| {
                let m = model(&[("noimod", mode as f64), ("kf", 1e-25)], 1.);
                m.noise.powers(&m, vg, 0.7, -0.2)
            })
            .collect::<Vec<_>>();
        assert!(powers.iter().flatten().all(|p| p.is_finite() && *p > 0.));
        assert_eq!(powers[0][0], powers[2][0]);
        assert_eq!(powers[1][0], powers[3][0]);
        assert_eq!(powers[0][1], powers[3][1]);
        assert_eq!(powers[1][1], powers[2][1]);
        assert_ne!(powers[0], powers[1]);
    }
}

#[test]
fn strong_inversion_still_depends_on_the_weak_noise_branch() {
    let m = model(&[("noimod", 2.), ("noia", 0.), ("noib", 3e4)], 1.);
    let [thermal, flicker] = m.noise.powers(&m, 1.5, 1.7, 0.);
    assert!(thermal > 0.);
    assert_eq!(flicker, 0.);
}

#[test]
fn junction_capacitance_and_current_limiting() {
    let m = model(
        &[
            ("nj", 1.1),
            ("js", 0.01),
            ("ijth", 1e-3),
            ("cj", 1e-3),
            ("cjsw", 0.),
            ("cjswg", 0.),
            ("mj", 1.),
            ("pb", 0.8),
        ],
        1.,
    );
    let j = &m.junction[0];
    for v in [-0.4, 0., 0.2] {
        let (_, q) = j.evaluate(D::var(v, 0), 0.);
        let c0 = 1e-15;
        let expected = if v < 0. {
            c0 / (1. - v / 0.8)
        } else {
            c0 * (1. + v / 0.8)
        };
        close(q.d[0], expected, 1e-14, 0.);
    }
    let isat = -j.evaluate(D::cst(-3.), 0.).0.v;
    let knee = j.nvt * (0.001 / isat).ln_1p();
    for v in [knee, knee + 0.15] {
        let (i, _) = j.evaluate(D::var(v, 0), 0.);
        let slope = (isat + 0.001) / j.nvt;
        close(i.v, 0.001 + slope * (v - knee), 1e-13, 0.);
        close(i.d[0], slope, 1e-13, 0.);
    }
}

#[test]
fn strong_inversion_partition_at_saturation() {
    for (xpart, fraction) in [(0., 0.4), (0.5, 0.5), (1., 0.)] {
        let m = model(&[("capmod", 0.), ("xpart", xpart)], 1.);
        let b = m.channel(D::cst(1.8), D::cst(3.), D::cst(0.));
        let [d, _, s, _] = m.charges(&b);
        let expected =
            -2. / 3. * m.cox * m.wcv * m.lcv * (1.8 - m.p.vfbcv - m.phi - m.k1 * b.root.v);
        close(d.v + s.v, expected, 1e-13, 0.);
        close(d.v, expected * fraction, 1e-13, 1e-29);
    }
}

#[test]
fn unlimited_junction_retains_exponential_above_eighty_thermal_voltages() {
    let m = model(&[("ijth", 0.), ("js", 1e-6)], 1.);
    let j = &m.junction[0];
    let isat = -j.evaluate(D::cst(-3.), 0.).0.v;
    let (i, _) = j.evaluate(D::var(85. * j.nvt, 0), 0.);
    close(i.v, isat * 85f64.exp_m1(), 1e-13, 0.);
    close(i.d[0], i.v / j.nvt, 1e-13, 0.);
}

#[test]
fn gate_bulk_overlap_uses_cv_width_correction_unless_explicit() {
    for (params, dwc) in [
        (vec![("wint", 15e-9)], 15e-9),
        (vec![("wint", 15e-9), ("dwc", 7e-9)], 7e-9),
    ] {
        let m = model(&params, 1.);
        close(m.overlap[2], m.lcv * 2. * dwc * m.cox, 1e-14, 0.);
        let mut explicit = params;
        explicit.push(("cgbo", 0.));
        assert_eq!(model(&explicit, 1.).overlap[2], 0.);
    }
}

#[test]
fn accumulation_centroid_uses_channel_doping() {
    let m = model(&[("nch", 3.2e17), ("nsub", 2e16), ("acde", 0.8)], 1.);
    // (NCH / 2e16)^(-1/4) = 1/2 for this channel.
    close(m.centroid_acde, 0.4, 1e-14, 0.);
}

#[test]
fn perimeter_contributes_junction_capacitance_with_area_only_current() {
    let m = model(
        &[
            ("js", 0.003),
            ("jssw", 1e-8),
            ("cj", 0.),
            ("cjsw", 4e-10),
            ("cjswg", 7e-10),
            ("tnom", 43.),
        ],
        1.,
    );
    let j = &m.junction[0];
    let (reverse, _) = j.evaluate(D::cst(-3.), 0.);
    close(reverse.v, -0.003 * 1e-12, 1e-14, 0.);
    let (_, q) = j.evaluate(D::var(0., 0), 0.);
    close(q.d[0], 1e-6 * 4e-10 + 2e-6 * 7e-10, 1e-14, 0.);
}
