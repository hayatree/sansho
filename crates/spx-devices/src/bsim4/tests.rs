//! Geometry, charge conservation, reciprocity and Jacobian checks for the silicon model.
//! Provenance: original numerical tests of the Berkeley BSIM4 4.8.2 manual, chapters 2–7 and 13.

use super::*;
use crate::mos1::Mos1Geometry;

fn params(entries: &[(&str, f64)]) -> ModelParams {
    ModelParams::new(
        &entries
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect::<Vec<_>>(),
    )
}

fn transistor(entries: &[(&str, f64)], sign: f64, t: f64) -> Size {
    let mut group = Bsim4s::default();
    let model = params(entries);
    group
        .push(
            "m1",
            [None; 4],
            false,
            Binding {
                model: &model,
                instance: &ModelParams::default(),
                geometry: Mos1Geometry {
                    l: 0.4e-6,
                    w: 2e-6,
                    ..Default::default()
                },
                polarity: sign,
                temp: t,
                tnom: 27.,
                scale: 1.,
            },
        )
        .unwrap();
    group.instances.remove(0).size
}

#[test]
fn dimensions_binning_and_temperature() {
    let nominal = transistor(&[], 1., 27.);
    assert_eq!(nominal.p.dvt0, 2.2);
    assert_eq!(nominal.toxe, 3e-9);
    for (unit, scale) in [(1., 1.), (2., 1e-6)] {
        let m = transistor(
            &[
                ("binunit", unit),
                ("u0", 0.05),
                ("lu0", 0.001 * scale),
                ("wu0", 0.002 * scale),
                ("pu0", 0.001 * scale * scale),
                ("ute", -1.),
            ],
            1.,
            57.,
        );
        let expected = (0.05 + 0.001 / 0.4 + 0.002 / 2. + 0.001 / 0.8) * 300.15 / 330.15;
        assert!((m.u0 - expected).abs() < 1e-14);
    }
}

#[test]
fn reciprocity_and_channel_boundaries() {
    for sign in [-1., 1.] {
        let m = transistor(&[("vth0", 0.45 * sign)], sign, 27.);
        for gate in [-0.3, 0.3, 0.45, 0.55, 1.2] {
            let (f, q, _) = m.terminals([0.7 * sign, gate * sign, 0., -0.1 * sign]);
            let (rev, qr, _) = m.terminals([0., gate * sign, 0.7 * sign, -0.1 * sign]);
            for (j, k) in [2, 1, 0, 3].into_iter().enumerate() {
                assert_eq!(f[j].v, rev[k].v);
                assert_eq!(q[j].v, qr[k].v);
            }
            let (left, _, _) = m.terminals([-1e-9, gate * sign, 0., 0.]);
            let (right, _, _) = m.terminals([1e-9, gate * sign, 0., 0.]);
            assert!((left[0].d[0] - right[0].d[0]).abs() < 1e-9);
        }
    }
}

#[test]
fn terminal_jacobians() {
    let mut bits = 3456789_u64;
    let mut next = || {
        bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
        (bits >> 11) as f64 / ((1_u64 << 53) as f64)
    };
    for mob in 0..=6 {
        for cap in 0..=2 {
            for igc in 0..=2 {
                for igb in 0..=1 {
                    for gidl in 0..=1 {
                        let m = transistor(
                            &[
                                ("mobmod", mob as f64),
                                ("capmod", cap as f64),
                                ("cvchargemod", ((igc + gidl) % 2) as f64),
                                ("xpart", [0., 0.5, 1.][(mob + igc) % 3]),
                                ("lambda", 1e-9),
                                ("vtl", 2e5),
                                ("tempmod", (mob % 4) as f64),
                                ("at", 0.),
                                ("igcmod", igc as f64),
                                ("igbmod", igb as f64),
                                ("gidlmod", gidl as f64),
                                ("agidl", 1e-9),
                                ("alpha0", 1e-7),
                                ("vth0", 0.47),
                                ("k1", 0.45),
                                ("k2", 0.01),
                            ],
                            1.,
                            27.,
                        );
                        for _ in 0..4 {
                            let v = [
                                next() * 1.5 - 0.4,
                                next() * 2. - 0.4,
                                next() * 0.2,
                                -next() * 0.4,
                            ];
                            let (f, q, _) = m.terminals(v);
                            for col in 0..4 {
                                let mut lo = v;
                                let mut hi = v;
                                let h = 1e-6;
                                lo[col] -= h;
                                hi[col] += h;
                                let (fl, ql, _) = m.terminals(lo);
                                let (fh, qh, _) = m.terminals(hi);
                                for (a, b, c, atol) in
                                    [(&f, &fl, &fh, 2e-12), (&q, &ql, &qh, 2e-23)]
                                {
                                    let total = a.iter().map(|x| x.v).sum::<f64>();
                                    assert!(total.abs() < atol);
                                    for row in 0..4 {
                                        let fd = (c[row].v - b[row].v) / (2. * h);
                                        let ad = a[row].d[col];
                                        assert!(
                                            (fd - ad).abs() < atol + 2e-4 * fd.abs().max(ad.abs()),
                                            "{mob}/{cap}/{igc}/{igb}/{gidl} row {row} col {col}: {fd} vs {ad}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "release-mode intrinsic device timing"]
fn evaluation_cost() {
    let m = transistor(&[("vth0", 0.45), ("k1", 0.5)], 1., 27.);
    let start = std::time::Instant::now();
    let n = 500_000;
    for i in 0..n {
        std::hint::black_box(m.terminals(std::hint::black_box([
            1.,
            (i % 1024) as f64 / 1024.,
            0.,
            0.,
        ])));
    }
    println!(
        "current, charge and both terminal Jacobians: {:.1} ns/eval",
        start.elapsed().as_nanos() as f64 / n as f64
    );
}

#[test]
fn reverse_trap_current_and_charge_slopes() {
    let p = params(&[("jtss", 0.05), ("jss", 0.), ("jtsd", 0.1)]);
    for (side, j) in Junction::pair(&p, [1e-12; 2], [0.; 2], 1e-6, 1., 300.15, 300.15)
        .iter()
        .enumerate()
    {
        for v in [-5., -1., -0.15, 0., 0.2, 0.75] {
            let (i, q) = j.eval(Dual::<1>::var(v, 0), 0.);
            let eval = |v| j.eval(Dual::<1>::cst(v), 0.);
            let (ilo, qlo) = eval(v - 1e-6);
            let (ihi, qhi) = eval(v + 1e-6);
            assert!((i.d[0] - (ihi.v - ilo.v) / 2e-6).abs() < 1e-17);
            assert!((q.d[0] - (qhi.v - qlo.v) / 2e-6).abs() < 1e-21);
            let expected = -[0.1, 0.05][side]
                * 1e-12
                * ((-v / (20. * crate::thermal_voltage(300.15)) * 10. / (10. - v)).exp() - 1.);
            assert!((i.v - expected).abs() < 1e-23);
        }
    }
}

#[test]
fn selector_diagnostics_and_layout_shift() {
    let instance = params(&[
        ("sa", 0.3e-6),
        ("sb", 0.6e-6),
        ("nf", 2.),
        ("sd", 0.5e-6),
        ("sca", 0.02),
    ]);
    let geometry = Mos1Geometry {
        l: 0.4e-6,
        w: 2e-6,
        ..Default::default()
    };
    let bind = |pairs: &[(&str, f64)]| {
        let mut group = Bsim4s::default();
        group
            .push(
                "m",
                [None; 4],
                false,
                Binding {
                    model: &params(pairs),
                    instance: &instance,
                    geometry,
                    polarity: 1.,
                    temp: 27.,
                    tnom: 27.,
                    scale: 1.,
                },
            )
            .map(|w| (group.instances.remove(0), w))
    };
    assert_eq!(bind(&[("trnqsmod", 1.)]).unwrap_err().code, "E-UNSUPPORTED");
    assert_eq!(bind(&[("capmod", 1.5)]).unwrap_err().code, "E-ELAB-MODEL");
    let (original, _) = bind(&[]).unwrap();
    let (shifted, _) = bind(&[("wpemod", 1.), ("kvth0we", 0.1), ("ku0", 2e-8)]).unwrap();
    assert!((shifted.size.vth0 - original.size.vth0 - 0.002).abs() < 1e-12);
    let inv = (1. / 0.5e-6 + 1. / 1.4e-6 + 1. / 0.8e-6 + 1. / 1.7e-6) / 2.;
    let ratio = (1. + 2e-8 * inv) / (1. + 2e-8 * 2. / 1.2e-6);
    assert!((shifted.size.u0 / original.size.u0 - ratio).abs() < 1e-12);
}

#[test]
fn resistance_and_junction_network_jacobians() {
    for gate in 0..=3 {
        for body in 0..=2 {
            for rds in 0..=1 {
                for diode in 0..=2 {
                    let mut group = Bsim4s::default();
                    let model = params(&[
                        ("rgatemod", gate as f64),
                        ("rbodymod", body as f64),
                        ("rdsmod", rds as f64),
                        ("diomod", diode as f64),
                        ("rbps0", 80.),
                        ("rbpd0", 120.),
                        ("rbsbx0", 100.),
                        ("rbdbx0", 100.),
                        ("vth0", 0.45),
                        ("rshg", 10.),
                        ("cjs", 1e-3),
                        ("xpart", 0.5),
                    ]);
                    let geom = Mos1Geometry {
                        l: 0.4e-6,
                        w: 1e-6,
                        ad: 0.3e-12,
                        as_: 0.3e-12,
                        ..Default::default()
                    };
                    group
                        .push(
                            "m",
                            [0, 1, 2, 3].map(Some),
                            false,
                            Binding {
                                model: &model,
                                instance: &ModelParams::default(),
                                geometry: geom,
                                polarity: 1.,
                                temp: 27.,
                                tnom: 27.,
                                scale: 1.,
                            },
                        )
                        .unwrap();
                    let mut u = Unknowns::with_nodes(["d", "g", "s", "b"]);
                    group.alloc_unknowns(&mut u);
                    let mut builder = StampBuilder::new(&u);
                    group.setup(&mut builder).unwrap();
                    let layout = builder.finish();
                    let n = u.len();
                    let eval = |v: &[f64]| {
                        let mut slots = Slots {
                            g: vec![0.; layout.g.len()],
                            c: vec![0.; layout.c.len()],
                            f: vec![0.; layout.f.len()],
                            q: vec![0.; layout.q.len()],
                            ..Default::default()
                        };
                        group.eval(
                            &EvalCtx {
                                limit: false,
                                ..EvalCtx::dc()
                            },
                            v,
                            StateRef {
                                current: &mut [0.; 5],
                                accepted: &[0.; 5],
                            },
                            &mut slots,
                        );
                        let matrix = |keys: &[(Node, Node)], data: &[f64]| {
                            let mut mat = vec![0.; n * n];
                            for (&(row, col), &val) in keys.iter().zip(data) {
                                if let (Some(i), Some(j)) = (row, col) {
                                    mat[i * n + j] += val;
                                }
                            }
                            mat
                        };
                        let vector = |keys: &[Node], data: &[f64]| {
                            let mut out = vec![0.; n];
                            for (&node, &val) in keys.iter().zip(data) {
                                if let Some(i) = node {
                                    out[i] += val;
                                }
                            }
                            out
                        };
                        [
                            (vector(&layout.f, &slots.f), matrix(&layout.g, &slots.g)),
                            (vector(&layout.q, &slots.q), matrix(&layout.c, &slots.c)),
                        ]
                    };
                    for phase in [0.72_f64, 1.34, 2.76] {
                        let v = (0..n)
                            .map(|k| 0.4 + 0.55 * (phase * (k + 1) as f64).sin())
                            .collect::<Vec<_>>();
                        let a = eval(&v);
                        for col in 0..n {
                            let mut lo = v.clone();
                            let mut hi = v.clone();
                            lo[col] -= 1e-6;
                            hi[col] += 1e-6;
                            let left = eval(&lo);
                            let right = eval(&hi);
                            for k in 0..2 {
                                for row in 0..n {
                                    let fd = (right[k].0[row] - left[k].0[row]) / 2e-6;
                                    let ad = a[k].1[row * n + col];
                                    let roundoff = 2e-9 * a[k].0[row].abs();
                                    assert!(
                                        (fd - ad).abs()
                                            < [2e-11, 2e-23][k]
                                                + roundoff
                                                + 2e-4 * ad.abs().max(fd.abs()),
                                        "{gate}/{body}/{rds}/{diode} row {row} col {col} {fd} vs {ad}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
