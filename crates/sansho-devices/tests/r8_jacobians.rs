//! Voltage perturbations of fully assembled device currents and charges.
//! Provenance: central differences applied to the Statz (1987), Kull (1985),
//! VDMOS, distributed RC and junction models. All bias fixtures are original.
#![allow(clippy::unwrap_used)]

use sansho_devices::{
    DeviceGroup, EvalCtx, ModelParams, Slots, StampBuilder, StateRef, Unknowns,
    bjt::{BjtModel, Bjts},
    diode::{DiodeModel, Diodes},
    jfet::Jfet,
    temperature::Temperature,
    urc::Urc,
    vdmos::Vdmos,
};

fn params(entries: &[(&str, f64)]) -> ModelParams {
    ModelParams::new(
        &entries
            .iter()
            .map(|&(k, v)| (k.into(), v))
            .collect::<Vec<_>>(),
    )
}
const HOT: Temperature = Temperature {
    kelvin: 327.15,
    nominal: 300.15,
};

fn check(mut dev: impl DeviceGroup, external: &[&str], bias: &[f64]) {
    let mut u = Unknowns::with_nodes(external.iter().copied());
    dev.alloc_unknowns(&mut u);
    assert_eq!(u.len(), bias.len(), "{:?}", u.names());
    let mut b = StampBuilder::new(&u);
    dev.setup(&mut b).unwrap();
    let l = b.finish();
    let eval = |x: &[f64]| {
        let mut out = Slots::for_layout(&l);
        let state = vec![0.; dev.state_len()];
        dev.eval(
            &EvalCtx {
                limit: false,
                ..EvalCtx::dc()
            },
            x,
            StateRef {
                accepted: &state,
                current: &mut state.clone(),
            },
            &mut out,
        );
        out
    };
    let nominal = eval(bias);
    for col in 0..bias.len() {
        let mut x = bias.to_vec();
        let h = 1e-6;
        x[col] += h;
        let plus = eval(&x);
        x[col] -= 2. * h;
        let minus = eval(&x);
        for (rows, pattern, values, upper, lower, floor) in [
            (&l.f, &l.g, &nominal.g, &plus.f, &minus.f, 1e-11),
            (&l.q, &l.c, &nominal.c, &plus.q, &minus.q, 1e-20),
        ] {
            for row in 0..bias.len() {
                let analytic: f64 = pattern
                    .iter()
                    .zip(values)
                    .filter_map(|(&(r, c), &v)| (r == Some(row) && c == Some(col)).then_some(v))
                    .sum();
                let measured: f64 = rows
                    .iter()
                    .zip(upper.iter().zip(lower))
                    .filter_map(|(&r, (&a, &b))| (r == Some(row)).then_some((a - b) / (2. * h)))
                    .sum();
                assert!(
                    (analytic - measured).abs() < floor + 2e-6 * analytic.abs().max(measured.abs()),
                    "{} [{row},{col}]: {analytic:e} vs {measured:e}",
                    dev.type_name()
                );
            }
        }
    }
}

#[test]
fn fet_current_and_charge_jacobians_at_hot_forward_and_reverse_bias() {
    for sign in [1., -1.] {
        for drain in [-0.5, 0.25, 2.] {
            let p = params(&[
                ("vto", -1.),
                ("beta", 0.01),
                ("b", 0.4),
                ("alpha", 2.),
                ("cgs", 3e-12),
                ("cgd", 2e-13),
            ]);
            let mes = Jfet::mesfet(
                "z1",
                [Some(0), Some(1), None],
                &p,
                sign,
                HOT,
                1.5,
                2.,
                false,
            )
            .unwrap();
            check(mes, &["d", "g"], &[drain * sign, -0.3 * sign]);
            let p = params(&[
                ("pchan", f64::from(sign < 0.)),
                ("vto", 0.7 * sign),
                ("kp", 0.5),
                ("theta", 0.1),
                ("lambda", 0.05),
                ("cgs", 2e-10),
                ("cgdmin", 5e-11),
                ("cgdmax", 3e-10),
                ("cjo", 2e-11),
                ("tt", 1e-8),
                ("bv", 6.),
            ]);
            let mos =
                Vdmos::new("m1", [Some(0), Some(1), None], &p, &params(&[]), HOT, false).unwrap();
            check(mos, &["d", "g"], &[drain * sign, 1.4 * sign]);
        }
    }
}

#[test]
fn temperature_adjusted_avalanche_and_series_jacobian() {
    let mut m = DiodeModel::from_params(&params(&[
        ("rs", 3.),
        ("bv", 6.),
        ("tcv", 0.001),
        ("trs1", 0.005),
        ("trs2", 1e-5),
        ("tt", 8e-9),
        ("ttt1", 0.01),
        ("tm2", 1e-5),
        ("cjo", 2e-12),
    ]))
    .0;
    m.at_temperature(HOT);
    assert!((m.rs - 3. * (1. + 27. * (0.005 + 27e-5))).abs() < 1e-14);
    assert_eq!(m.bv, 5.973);
    assert!((m.tt - 8e-9 * 1.27).abs() < 1e-22);
    let bias = -m.bv - 0.02;
    let mut d = Diodes::default();
    d.push("d1", Some(0), None, m, 1., false);
    check(d, &["anode"], &[bias - 0.01, bias]);
}

#[test]
fn bjt_saturated_epitaxial_and_partitioned_junction_derivatives() {
    let p = params(&[
        ("rco", 4.),
        ("vo", 0.5),
        ("gamma", 2e-10),
        ("qco", 4e-12),
        ("xcjc", 0.6),
        ("cjc", 2e-12),
        ("cjs", 3e-12),
        ("iss", 1e-15),
        ("rc", 0.5),
    ]);
    let mut m = BjtModel::from_params(&p, 1.).0;
    m.at_temperature(HOT);
    m.junction_areas(1., 2., 3.);
    let mut q = Bjts::default();
    q.push("q", [Some(0), Some(1), None, None], m, 1., false);
    check(q, &["c", "b"], &[0.2, 0.78, 0.18, 0.09]);
}

#[test]
fn urc_automatic_count_follows_measured_fmax_transitions() {
    let instance = params(&[("l", 2e-3)]);
    for (fmax, count) in [(1e8, 3), (8.5e8, 4), (1e9, 5), (1.5e9, 5), (1.7e9, 6)] {
        let model = params(&[("k", 1.6), ("rperl", 1e5), ("cperl", 30e-9), ("fmax", fmax)]);
        let mut nodes = Unknowns::with_nodes(["near", "far"]);
        let mut line = Urc::new("u", [Some(0), Some(1), None], &model, &instance, HOT).unwrap();
        line.alloc_unknowns(&mut nodes);
        // The capacitive ladder adds one tap between adjacent series resistors.
        assert_eq!(nodes.len() - 2, 2 * count - 1, "FMAX={fmax}");
    }
}

#[test]
fn urc_distributed_junction_and_end_resistance_derivatives() {
    let p = params(&[
        ("k", 1.7),
        ("rperl", 120.),
        ("cperl", 1e-10),
        ("isperl", 2e-12),
        ("rsperl", 1.),
    ]);
    let u = Urc::new(
        "u1",
        [Some(0), Some(1), None],
        &p,
        &params(&[("l", 1.), ("n", 2.)]),
        HOT,
    )
    .unwrap();
    check(
        u,
        &["near", "far"],
        &[0.1, 0.2, 0.12, 0.119, 0.15, 0.149, 0.18, 0.179],
    );
}

#[test]
fn lossy_line_trial_jacobian_includes_the_current_interval() {
    use sansho_devices::{
        Mode,
        ltra::{LineConstants, Ltra},
    };
    for (r, l, g, c) in [
        (50., 0., 0., 1e-10),
        (10., 2.5e-7, 0., 1e-10),
        (30., 0., 1e-4, 0.),
    ] {
        let constants = LineConstants::new(&params(&[
            ("len", 1.),
            ("r", r),
            ("l", l),
            ("g", g),
            ("c", c),
        ]))
        .unwrap();
        let mut line = Ltra::new("o1", [Some(0), None, Some(1), None], constants, false);
        let mut u = Unknowns::with_nodes(["near", "far"]);
        line.alloc_unknowns(&mut u);
        let mut b = StampBuilder::new(&u);
        line.setup(&mut b).unwrap();
        let layout = b.finish();
        let hist = [
            (0., [0.4, 0.01, -0.1, -0.01]),
            (1e-9, [0.5, 0.012, -0.2, -0.011]),
        ];
        let ctx = EvalCtx {
            time: 12e-9,
            mode: Mode::Tran,
            ..EvalCtx::dc()
        };
        let eval = |x: &[f64]| {
            let mut slots = Slots::for_layout(&layout);
            line.trial(ctx.time, &hist, &mut slots);
            line.eval(
                &ctx,
                x,
                StateRef {
                    accepted: &[],
                    current: &mut [],
                },
                &mut slots,
            );
            slots
        };
        let bias = [0.25, -0.12, 0.012, -0.01];
        let nominal = eval(&bias);
        for col in 0..4 {
            let mut x = bias;
            x[col] += 1e-6;
            let plus = eval(&x);
            x[col] -= 2e-6;
            let minus = eval(&x);
            for row in 0..4 {
                let expected: f64 = nominal
                    .g
                    .iter()
                    .zip(&layout.g)
                    .filter_map(|(&v, &rc)| (rc == (Some(row), Some(col))).then_some(v))
                    .sum();
                let measured: f64 = layout
                    .f
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &r)| {
                        (r == Some(row)).then_some((plus.f[i] - minus.f[i]) / 2e-6)
                    })
                    .sum();
                assert!((measured - expected).abs() < 1e-10);
            }
        }
        // A trial between accepted samples is reproducible after another trial.
        let mut slots = Slots::for_layout(&layout);
        line.trial(ctx.time * 0.75, &hist, &mut slots);
        line.trial(ctx.time, &hist, &mut slots);
        line.eval(
            &ctx,
            &bias,
            StateRef {
                accepted: &[],
                current: &mut [],
            },
            &mut slots,
        );
        assert_eq!(slots.b, nominal.b);
    }
}

#[test]
fn thermal_vdmos_coupled_jacobian_with_drift_resistance() {
    use sansho_devices::vdmos::ThermalVdmos;
    let p = params(&[
        ("kp", 0.7),
        ("vto", 1.1),
        ("rq", 1.2),
        ("vq", 0.8),
        ("rd", 0.1),
        ("rs", 0.2),
        ("subshift", 0.03),
        ("ksubthres", 0.07),
        ("tcvth", 0.001),
    ]);
    let dev = Vdmos::new("m1", [Some(0), Some(1), None], &p, &params(&[]), HOT, false).unwrap();
    let thermal = ThermalVdmos::new(dev, [Some(2), Some(3)], 2., 90., 1e-5).unwrap();
    check(
        thermal,
        &["d", "g", "tj", "tc"],
        &[1.2, 1.4, 61., 45., 0.9, 0.05, 0.4],
    );
}

#[test]
fn analog_port_residuals_and_dynamic_charges() {
    let build = |card: &str, ports: &str| {
        let nl = sansho_netlist::parse(&format!(
            "finite differences\nAtest {ports} m\n.model m {card}\n"
        ))
        .unwrap();
        let sansho_netlist::ElementKind::CodeModel { ports, .. } = &nl.elements[0].kind else {
            unreachable!()
        };
        sansho_devices::xspice::Analog::new("atest", ports, &nl.models[0], |n| {
            ["a", "b", "out"].iter().position(|&k| k == n)
        })
        .unwrap()
    };
    for (card, ports, bias) in [
        (
            "mult(in_gain=[2 3])",
            "%vd[a b b 0] out",
            vec![0.45, 0.2, 0.1, 0.002],
        ),
        (
            "limit(out_lower_limit=-1 out_upper_limit=1 limit_range=0.1)",
            "%vd(a b) out",
            vec![1.22, 0.2, 0.1, 0.002],
        ),
        (
            "pwl(x_array=[-1 0 2] y_array=[-2 0 1] input_domain=0.2 fraction=false)",
            "%vd(a b) out",
            vec![0.25, 0.2, 0.1, 0.002],
        ),
        (
            "aswitch(cntl_off=-1 cntl_on=1 r_off=1k r_on=10 log=true)",
            "a %hd(out b)",
            vec![0.4, 0.2, 0.1, 0.002],
        ),
        (
            "d_dt(gain=0.5 out_lower_limit=-1 out_upper_limit=1)",
            "%id(a b) %i(out)",
            vec![0.4, 0.2, 0.1, 0.003, 0.002, 0.7],
        ),
        (
            "s_xfer(den_coeff=[2 3 4] num_coeff=[3 1] denormalized_freq=1000)",
            "%vd(a b) out",
            vec![0.4, 0.2, 0.1, 0.002, 0.3, 0.5],
        ),
    ] {
        check(build(card, ports), &["a", "b", "out"], &bias);
    }
}
