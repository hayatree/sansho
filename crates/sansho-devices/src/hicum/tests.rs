//! Differential and conservation checks of the native HICUM equations.
//! Provenance: GICCR balance and Kirchhoff laws, independently evaluated.

use super::*;
use crate::ModelParams;

#[test]
fn nonlinear_hole_charge_closes_the_integral_balance() {
    let fields = [
        ("tef0", 2e-11),
        ("thcs", 4e-11),
        ("hfe", 2.),
        ("hfc", 3.),
        ("fthc", 1.),
        ("tr", 2e-12),
    ];
    let p = ModelParams::new(&fields.map(|(n, v)| (n.into(), v)));
    let m = Model::from_params(&p, 1., None).unwrap();
    let tau = D::cst(8e-12);
    for jf in [1e-17, 1e-6, 0.01, 10.] {
        let jf = D::var(jf, 0);
        let jr = D::cst(1e-9);
        let storage = transport(
            &m,
            base_charge(D::cst(1.2), D::cst(1.)),
            jf,
            jr,
            tau,
            D::cst(0.001),
            D::cst(1.),
        );
        let q0 = tau * storage.forward;
        let emitter = storage.qf - q0 - storage.excess;
        let base = 0.05 + positive_part(D::cst(1.15), D::cst(0.1 * std::f64::consts::LN_2));
        let width = base + (q0 + m.hfe * emitter + m.hfc * storage.excess + storage.qr) / m.qp0;
        let check = storage.forward * width;
        assert!((check.v / jf.v - 1.).abs() < 1e-12);
        assert!((check.d[0] - 1.).abs() < 1e-12);
        assert!((storage.reverse.v * width.v / jr.v - 1.).abs() < 1e-12);
    }
}

#[test]
fn coupled_charge_and_current_jacobians_match_perturbations() {
    let fields = [
        ("flnqs", 1.),
        ("flsh", 2.),
        ("rth", 200.),
        ("cth", 2e-8),
        ("t0", 5e-12),
        ("tef0", 8e-12),
        ("thcs", 20e-12),
        ("rbi0", 30.),
        ("rbx", 6.),
        ("re", 2.),
        ("rcx", 4.),
        ("rsu", 50.),
        ("csu", 4e-12),
        ("itss", 1e-16),
        ("cjei0", 5e-15),
        ("cjci0", 3e-15),
        ("favl", 0.05),
        ("qavl", 1e-15),
        ("fdqr0", 0.4),
        ("fcrbi", 1.),
    ];
    let p = ModelParams::new(&fields.map(|(n, v)| (n.into(), v)));
    let m = Model::from_params(&p, 1., None).unwrap();
    let mut group = Hicums::default();
    group.push(
        "q1",
        [None; 4],
        Some(Some(0)),
        Arc::new(m),
        Temperature {
            kelvin: 310.,
            nominal: 300.15,
        },
        1.5,
        2.,
        false,
    );
    let device = &group.0[0];
    let x = [
        1.1, 0.80, 0., -0.1, 1.04, 0.78, 0.77, 0.005, -0.09, 4., 0.002, 0.6, 0.0001,
    ];
    let eval = |x| device.evaluate(x, 1e-12, false, |_, _, _, _| {});
    let point = eval(x);
    for a in [&point.f, &point.q] {
        let total = a[..TJ].iter().map(|x| x.v).sum::<f64>();
        assert!(total.abs() <= 1e-14 * a[..TJ].iter().map(|x| x.v.abs()).sum::<f64>());
    }
    for k in 0..N {
        let h = if k == TJ { 1e-4 } else { 1e-6 };
        let mut hi = x;
        let mut lo = x;
        hi[k] += h;
        lo[k] -= h;
        let (hi, lo) = (eval(hi), eval(lo));
        for (a, b, c, tol) in [
            (&point.f, &hi.f, &lo.f, 1e-10),
            (&point.q, &hi.q, &lo.q, 1e-20),
        ] {
            for row in 0..N {
                let slope = (b[row].v - c[row].v) / (2. * h);
                let exact = a[row].d[k];
                assert!(
                    (exact - slope).abs() < tol + 2e-6 * exact.abs(),
                    "row {row} column {k}: {exact} vs {slope}"
                );
            }
        }
    }
}

#[test]
fn cap_continuation_is_finite_for_shallow_grading_and_has_the_rated_slope() {
    for grading in [0.001, 0.4, 1.] {
        let (q, c) = depletion(D::var(1., 0), D::cst(0.7), grading, 2.5, 1., D::cst(0.026));
        assert!(q.v.is_finite() && (1. ..=2.5).contains(&c.v) && (q.d[0] - c.v).abs() < 1e-14);
        let (q, c) = depletion(D::var(-2., 0), D::cst(0.7), grading, 2.5, 1., D::cst(0.026));
        assert!((q.d[0] - c.v).abs() < 1e-15);
    }
}
