//! SOI conservation, differential stamps and thermal equilibrium tests.
//! Provenance: BSIMSOIv4.4 Users' Manual §§4.1 and 5.2, original test biases.
use super::*;
use crate::{ModelParams, MosBinding, mos1::Mos1Geometry};

fn model(params: &[(&str, f64)], sign: f64) -> Model {
    let params = ModelParams::new(
        &params
            .iter()
            .map(|&(k, v)| (k.to_string(), v))
            .collect::<Vec<_>>(),
    );
    let inst = ModelParams::default();
    Model::bind(&MosBinding {
        model: &params,
        instance: &inst,
        polarity: sign,
        geometry: Mos1Geometry {
            l: 0.45e-6,
            w: 1.8e-6,
            ..Default::default()
        },
        scale: 1.,
        temp: 27.,
        tnom: 27.,
    })
    .unwrap()
}

#[test]
fn jacobian_and_conservation() {
    for sign in [-1., 1.] {
        for mode in [0., 1., 2., 3.] {
            let m = model(
                &[
                    ("soimod", mode),
                    ("shmod", 1.),
                    ("rth0", 0.02),
                    ("tsi", 3e-8),
                    ("alpha0", 1e-5),
                    ("k1", 0.4),
                    ("rgatemod", 3.),
                    ("rdsmod", 1.),
                    ("rbodymod", 1.),
                    ("fdmod", 1.),
                    ("vgstcvmod", 2.),
                    ("cdsbs", 0.001),
                ],
                sign,
            );
            for vd in [-0.25, 0.7] {
                let v = [
                    vd * sign,
                    sign,
                    0.,
                    0.12 * sign,
                    -0.3 * sign,
                    8.,
                    1.01 * sign,
                    0.14 * sign,
                    0.1 * sign,
                    (vd + 0.01) * sign,
                    -0.005 * sign,
                    0.,
                ];
                let a = m.evaluate(v, 1e-12);
                for row in [&a.i, &a.q] {
                    let sum = row
                        .iter()
                        .enumerate()
                        .filter(|(k, _)| *k != 5)
                        .map(|(_, x)| x.v)
                        .sum::<f64>();
                    assert!(
                        sum.abs() < 1e-12 * row.iter().map(|x| x.v.abs()).sum::<f64>().max(1e-26)
                    );
                }
                for c in 0..N {
                    let h = 1e-6;
                    let mut hi = v;
                    hi[c] += h;
                    let mut lo = v;
                    lo[c] -= h;
                    let (hi, lo) = (m.evaluate(hi, 1e-12), m.evaluate(lo, 1e-12));
                    for r in 0..N {
                        for (x, y, z, floor) in [
                            (a.i[r], hi.i[r], lo.i[r], 1e-11),
                            (a.q[r], hi.q[r], lo.q[r], 1e-21),
                        ] {
                            let fd = (y.v - z.v) / (2. * h);
                            assert!(
                                (fd - x.d[c]).abs() < floor + 1e-4 * fd.abs(),
                                "{mode} {r} {c} {fd} {}",
                                x.d[c]
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn depletion_modes_and_thermal_power() {
    let mut m = model(
        &[("shmod", 1.), ("rth0", 0.05), ("soimod", 3.), ("tsi", 1e-8)],
        1.,
    );
    assert_eq!(m.mode, 2);
    let v = [1., 1., 0., 0.1, 0., 0., 1., 0., 0., 1., 0., 0.];
    let cold = m.evaluate(v, 0.);
    assert_eq!(cold.i[3].v, 0.);
    assert!(cold.bias.ids.v > 0. && cold.bias.ids.d[4] > 0.);
    assert_eq!(cold.i[5].v, -cold.bias.ids.v);
    m.mode = 0;
    let pd = m.evaluate(v, 0.);
    assert_eq!(pd.bias.ids.d[4], 0.);
    assert!(pd.q[4].d[4] > 0.);
}

#[test]
fn contact_pins_keep_ground_distinct_from_omission() {
    let mut group = Sois::default();
    for (name, extra, thermal) in [
        ("floating", vec![], false),
        ("tied", vec![None], false),
        ("thermal", vec![Some(1)], true),
        ("both", vec![None, Some(1)], true),
        ("exposed", vec![None, Some(0), Some(1)], true),
    ] {
        group
            .push(
                name,
                [None; 4],
                &extra,
                thermal,
                false,
                model(&[("shmod", 1.), ("rth0", 0.02)], 1.),
            )
            .unwrap();
    }
    let mut unknowns = Unknowns::with_nodes(["film", "thermal"]);
    group.alloc_unknowns(&mut unknowns);
    let devices = &group.instances;
    assert!(devices[0].nodes[3].is_some() && devices[0].nodes[5].is_some());
    assert_eq!(devices[1].nodes[3], None);
    assert!(devices[2].nodes[3].is_some());
    assert_eq!(devices[2].nodes[5], Some(1));
    assert_eq!((devices[3].nodes[3], devices[3].nodes[5]), (None, Some(1)));
    assert_eq!(
        (devices[4].nodes[3], devices[4].nodes[5]),
        (Some(0), Some(1))
    );
    assert!(devices[4].body_short.is_some());
}

#[test]
fn film_width_and_temperature_set_diode_current() {
    let m = model(
        &[("isbjt", 0.), ("isrec", 0.), ("isdif", 0.02), ("tsi", 8e-8)],
        1.,
    );
    let mut v = [0.; N];
    v[3] = 0.18;
    let at = m.evaluate(v, 0.);
    let expected = m.nf * m.w * m.tsi * 0.02 * (v[3] / (super::VT_PER_K * m.temp)).exp_m1();
    assert!((at.junction[1].current.v / expected - 1.).abs() < 1e-12);
    assert_eq!(at.bjt.v, 0.);
    assert_eq!(at.bias.ids.v, 0.);
}

#[test]
fn binning_uses_effective_dimensions_and_rejects_spurious_prefixes() {
    let m = model(
        &[
            ("vth0", 0.4),
            ("lvth0", 0.1),
            ("wvth0", 0.02),
            ("pvth0", 0.03),
        ],
        1.,
    );
    assert!((m.vth - (0.4 + 0.1 / 0.45 + 0.02 / 1.8 + 0.03 / (1.8 * 0.45))).abs() < 1e-12);
    let p = ModelParams::new(&[("ltox".to_owned(), 0.01), ("lwth0".into(), 0.02)]);
    let instance = ModelParams::default();
    Model::bind(&MosBinding {
        model: &p,
        instance: &instance,
        scale: 1.,
        polarity: 1.,
        geometry: Default::default(),
        temp: 27.,
        tnom: 27.,
    })
    .unwrap();
    assert_eq!(p.unused(), ["ltox", "lwth0"]);
}
