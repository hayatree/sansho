//! End-to-end tests against analytic solutions.
//!
//! Provenance: original test circuits; expected values derived by hand from circuit theory.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use spx_engine::run;
use spx_netlist::parse;
use spx_output::{AnalysisKind, SimResults};

fn simulate(src: &str) -> SimResults {
    let nl = parse(src).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    match run(&nl) {
        Ok(out) => out.results,
        Err(e) => panic!("run failed: {:#?}", e.diagnostics),
    }
}

fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a - b).abs() <= rel * b.abs() + abs
}

#[test]
fn op_divider_and_branch_current() {
    let r = simulate("divider\nV1 in 0 DC 10\nR1 in out 1k\nR2 out 0 3k\n.op\n.end\n");
    let a = &r.analyses[0];
    assert_eq!(a.kind, AnalysisKind::Op);
    assert!(close(a.real("v(out)").unwrap()[0], 7.5, 1e-12, 0.0));
    // SPICE convention: current flows into the + terminal, so a sourcing V1 reports -2.5 mA.
    assert!(close(a.real("i(v1)").unwrap()[0], -2.5e-3, 1e-12, 0.0));
}

#[test]
fn op_controlled_sources() {
    let src = "\
controlled sources
V1 a 0 1
R1 a 0 1k
E1 e 0 a 0 3
Re e 0 1k
G1 0 g a 0 2m
Rg g 0 1k
Vs a s 0
Rs s 0 500
F1 0 f vs 4
Rf f 0 100
H1 h 0 vs 1k
Rh h 0 1k
.op
";
    let r = simulate(src);
    let a = &r.analyses[0];
    let v = |n: &str| a.real(n).unwrap()[0];
    assert!(close(v("v(e)"), 3.0, 1e-12, 0.0));
    // G1: 2 mA from node 0 through the source into g → v(g) = 2 V.
    assert!(close(v("v(g)"), 2.0, 1e-12, 0.0));
    // i(vs) = 1 V / 500 Ω = 2 mA; F1 drives 8 mA into f → 0.8 V; H1 → 2 V.
    assert!(close(v("i(vs)"), 2e-3, 1e-12, 0.0));
    assert!(close(v("v(f)"), 0.8, 1e-12, 0.0));
    assert!(close(v("v(h)"), 2.0, 1e-12, 0.0));
}

#[test]
fn dc_sweep_linear() {
    let r = simulate("sweep\nV1 in 0 0\nR1 in out 1k\nR2 out 0 1k\n.dc V1 0 2 0.5\n");
    let a = &r.analyses[0];
    assert_eq!(a.scale_values, vec![0.0, 0.5, 1.0, 1.5, 2.0]);
    let out = a.real("v(out)").unwrap();
    for (s, v) in a.scale_values.iter().zip(out) {
        assert!(close(*v, s / 2.0, 1e-12, 1e-15));
    }
}

#[test]
fn ac_rc_lowpass() {
    let r = 1e3;
    let c = 1e-6;
    let src = format!("rc\nV1 in 0 DC 0 AC 1\nR1 in out {r}\nC1 out 0 {c}\n.ac dec 20 1 1meg\n");
    let res = simulate(&src);
    let a = &res.analyses[0];
    let out = a.complex("v(out)").unwrap();
    for (f, &(re, im)) in a.scale_values.iter().zip(out) {
        let wrc = 2.0 * std::f64::consts::PI * f * r * c;
        // H = 1 / (1 + j w R C)
        let den = 1.0 + wrc * wrc;
        assert!(close(re, 1.0 / den, 1e-10, 1e-15), "f={f}");
        assert!(close(im, -wrc / den, 1e-10, 1e-15), "f={f}");
    }
}

#[test]
fn ac_rlc_series_resonance() {
    // Series RLC driven by 1 V: at resonance the full voltage appears across R.
    let src = "rlc\nV1 in 0 AC 1\nL1 in a 1m\nC1 a out 1u\nR1 out 0 10\n.ac lin 1 5032.921210448704 5032.921210448704\n";
    let res = simulate(src);
    let (re, im) = res.analyses[0].complex("v(out)").unwrap()[0];
    assert!(close(re, 1.0, 1e-9, 0.0), "re={re}");
    assert!(im.abs() < 1e-8, "im={im}");
}

#[test]
fn tran_rc_step_response() {
    // RC = 1 us; PULSE edge of 1 ns at t = 0 is effectively a step for this tolerance.
    let src = "rc step\nV1 in 0 PULSE(0 1 0 1n 1n 1 2)\nR1 in out 1k\nC1 out 0 1n\n.tran 10n 5u\n";
    let res = simulate(src);
    let a = &res.analyses[0];
    let tau = 1e-6;
    let out = a.real("v(out)").unwrap();
    let mut max_err = 0.0f64;
    for (t, v) in a.scale_values.iter().zip(out) {
        // Response to a 1 ns ramp followed by a constant level.
        let tr = 1e-9;
        let exact = if *t <= tr {
            (t - tau * (1.0 - (-t / tau).exp())) / tr
        } else {
            1.0 - (tau / tr) * ((tr / tau).exp() - 1.0) * (-t / tau).exp()
        };
        max_err = max_err.max((v - exact).abs());
    }
    assert!(max_err < 1e-3, "max error {max_err}");
    assert!((a.scale_values.last().copied().unwrap() - 5e-6).abs() < 1e-15);
}

#[test]
fn tran_lc_oscillation_energy() {
    // Undamped LC tank: the trapezoidal rule preserves the amplitude.
    let src = "lc\nI1 0 a PULSE(1m 0 0 1n 1n 1 2)\nL1 a 0 1u\nC1 a 0 1n\n.tran 1n 2u\n";
    let res = simulate(src);
    let a = &res.analyses[0];
    let v = a.real("v(a)").unwrap();
    let late: f64 = v
        .iter()
        .zip(&a.scale_values)
        .filter(|(_, t)| **t > 1e-6)
        .map(|(v, _)| v.abs())
        .fold(0.0, f64::max);
    // Initial inductor current 1 mA → peak voltage I·sqrt(L/C) = 1e-3·sqrt(1000) ≈ 31.6 mV.
    let expected = 1e-3 * (1e-6f64 / 1e-9).sqrt();
    assert!(
        close(late, expected, 0.02, 0.0),
        "peak {late}, expected {expected}"
    );
}

#[test]
fn tran_sin_through_divider() {
    let src = "sin\nV1 in 0 SIN(0 1 1meg)\nR1 in out 1k\nR2 out 0 1k\n.tran 10n 2u\n";
    let res = simulate(src);
    let a = &res.analyses[0];
    let out = a.real("v(out)").unwrap();
    for (t, v) in a.scale_values.iter().zip(out) {
        let exact = 0.5 * (2.0 * std::f64::consts::PI * 1e6 * t).sin();
        assert!(close(*v, exact, 1e-9, 1e-12), "t={t}");
    }
}

#[test]
fn singular_matrix_is_diagnosed() {
    let nl = parse("floating\nV1 a 0 1\nC1 a b 1n\nC2 b 0 1n\n.op\n").unwrap();
    let err = run(&nl).expect_err("should fail");
    let first = &err.diagnostics[0];
    assert_eq!(first.code, "E-SOLVE-SINGULAR");
    assert!(first.culprits.iter().any(|c| c.name == "b"));
    assert!(
        first
            .suggestions
            .iter()
            .any(|s| s.action.contains("rshunt"))
    );
    assert!(err.diagnostics.iter().any(|d| d.code == "W-ELAB-NODC"));
}

#[test]
fn rshunt_fixes_floating_node() {
    let r = simulate("floating\nV1 a 0 1\nC1 a b 1n\nC2 b 0 1n\n.options rshunt=1e12\n.op\n");
    assert!(r.analyses[0].real("v(b)").unwrap()[0].abs() < 1e-9);
}

#[test]
fn nested_source_and_temperature_axes() {
    let deck = include_str!("../../../verify/decks/dc_nested.cir");
    let result = simulate(&deck.replace(".dc temp 20 40 10 Vdrive 1 2 1", ".dc Vdrive 2 0 -1 Iload 0 2m 1m\n.dc temp 20 40 10 Vdrive 1 2 1\n.dc Iload 0 2m 1m temp 10 30 10"));
    for (index, (row, rows)) in [(3, 3), (3, 2), (3, 3)].into_iter().enumerate() {
        let plot = &result.analyses[index];
        assert_eq!(plot.len(), row * rows);
        for (k, &v) in plot.real("v(out)").unwrap().iter().enumerate() {
            let i = (k % row) as f64;
            let j = (k / row) as f64;
            let (drive, load, temp, scale) = match index {
                0 => (2. - i, j * 0.001, 27., 2. - i),
                1 => (1. + j, 0., 20. + 10. * i, 20. + 10. * i),
                _ => (0., i * 0.001, 10. + 10. * j, i * 0.001),
            };
            let r = 1000. * (1. + 0.002 * (temp - 27.));
            assert_eq!(plot.scale_values[k], scale);
            assert!(close(
                v,
                (drive / 1000. + load) / (1. / 1000. + 1. / r),
                1e-11,
                1e-14
            ));
        }
    }
}

#[test]
fn modulated_source_limits_the_step_after_its_delay() {
    for (source, frequency) in [
        ("V1 a 0 SFFM(1 1 10k 2 1k 80m 30 90)", 12e3),
        ("I1 0 a AM(1 2 3 1k 10k 80m 30 90)", 11e3),
    ] {
        let r = simulate(&format!(
            "carrier sampling\n{source}\nR1 a 0 1\n.tran 10m 81m 0 10m\n"
        ));
        let plot = &r.analyses[0];
        let onset = plot.scale_values.iter().position(|&t| t == 0.08).unwrap();
        assert!(onset < 30);
        assert_eq!(plot.real("v(a)").unwrap()[onset], 0.);
        for pair in plot.scale_values[onset..].windows(2) {
            assert!(pair[1] - pair[0] <= 1.000001 / (64. * frequency));
        }
    }
}

#[test]
fn measurement_events_windows_and_prior_names() {
    let result = simulate(include_str!("../../../verify/decks/meas_events.cir"));
    let expected = [
        1000.,
        -1000.,
        0.00225,
        0.5,
        0.25,
        1.,
        0.,
        1.,
        0.5,
        1. / 3_f64.sqrt(),
        0.001,
        1000.,
    ];
    assert_eq!(result.measures.len(), expected.len());
    for (m, v) in result.measures.iter().zip(expected) {
        assert!(
            close(m.value.unwrap(), v, 5e-5, 1e-14),
            "{}: {:?}",
            m.name,
            m
        );
    }
    let descending = simulate(
        "descending\nV1 a 0\nR1 a 0 1k\n.dc v1 1 -1 -0.25\n.meas dc x deriv v(a) when v(a)=-0.6 fall=1\n.meas dc y avg v(a) from=-0.75 to=-0.25\n.meas dc z param='later+1'\n",
    );
    assert_eq!(descending.measures[0].value, Some(1.));
    assert_eq!(descending.measures[1].value, Some(-0.5));
    assert!(
        descending.measures[2]
            .error
            .as_ref()
            .unwrap()
            .contains("later")
    );
}

#[test]
fn bjt_forward_transport_acquires_exact_excess_phase() {
    let run = |angle| {
        simulate(&format!(
            "phase\nVb b 0 0.7 AC 1\nVc c 0 2\nQ1 c b 0 qn\n.model qn npn(is=1e-16 tf=1n ptf={angle})\n.ac lin 1 250meg 250meg\n"
        ))
    };
    let zero = run(0.);
    let phase = run(45.);
    let (re, im) = phase.analyses[0].complex("i(vc)").unwrap()[0];
    let gm = zero.analyses[0].complex("i(vc)").unwrap()[0].0;
    let angle = -std::f64::consts::TAU * 250e6 * 1e-9 * 45f64.to_radians();
    assert!(close(re / gm, angle.cos(), 1e-8, 1e-12));
    assert!(close(im / gm, angle.sin(), 1e-8, 1e-12));
}

#[test]
fn bjt_external_collector_charge_bypasses_base_resistance() {
    let plot = simulate(
        "partition\nV1 b 0 0 AC 1\nVc c 0 0\nQ1 c b 0 qn\n.model qn npn(rb=1k cjc=2p xcjc=0.25)\n.ac lin 1 1g 1g\n",
    );
    let (re, im) = plot.analyses[0].complex("i(v1)").unwrap()[0];
    let w = std::f64::consts::TAU * 1e9;
    let b = w * 0.5e-12;
    let denom = 1. + (1000. * b).powi(2);
    assert!(close(-re, 1000. * b * b / denom, 1e-7, 1e-12));
    assert!(close(-im, w * 1.5e-12 + b / denom, 1e-7, 1e-12));
}

#[test]
fn diode_breakdown_slope_and_perimeter_capacitance() {
    let r = simulate(
        "avalanche\nV1 d 0\nD1 d 0 dd area=2\n.model dd d(bv=3 ibv=2m nbv=1.4 isr=1u)\n.dc v1 -3.05 -3 0.05\n",
    );
    let current = r.analyses[0].real("i(v1)").unwrap();
    let vt = spx_devices::thermal_voltage(spx_devices::REFTEMP);
    assert!(close(current[1], 0.004, 1e-8, 0.));
    assert!(close(
        current[0] / current[1],
        (0.05 / (1.4 * vt)).exp(),
        1e-8,
        0.
    ));
    let r = simulate(
        "junction perimeter\nV1 d 0 0 AC 1\nD1 d 0 dd area=2 pj=5u m=3\n.model dd d(cjo=1p cjp=1u)\n.ac lin 1 1meg 1meg\n",
    );
    let (_, b) = r.analyses[0].complex("i(v1)").unwrap()[0];
    assert!(close(-b, std::f64::consts::TAU * 1e6 * 21e-12, 1e-10, 0.));
}

#[test]
fn diode_recombination_and_high_injection() {
    let r = simulate(
        "forward\nV1 d 0 0.7\nD1 d 0 dd\n.model dd d(is=1e-14 ikf=2m isr=20p nr=2 vj=1 m=0)\n.op\n",
    );
    let vt = spx_devices::thermal_voltage(spx_devices::REFTEMP);
    let i = 1e-14 * (0.7 / vt).exp_m1();
    let recombination = 20e-12 * (0.7 / (2. * vt)).exp_m1();
    let forward = i + recombination;
    let expected = forward / (1. + (forward / 0.002).sqrt()) + 0.7e-12;
    assert!(close(
        -r.analyses[0].real("i(v1)").unwrap()[0],
        expected,
        1e-10,
        0.
    ));
}

#[test]
fn lateral_substrate_junction_returns_current_to_base() {
    let r = simulate(
        "substrate\nVc c 0 0\nVb b 0 0\nVs s 0 -0.4\nQ1 c b 0 s qq\n.model qq npn(subs=-1 iss=1p ns=1.5)\n.op\n",
    );
    let i = 1e-12 * (0.4 / (1.5 * spx_devices::thermal_voltage(spx_devices::REFTEMP))).exp_m1();
    assert!(close(r.analyses[0].real("i(vs)").unwrap()[0], i, 1e-10, 0.));
    assert!(close(
        r.analyses[0].real("i(vb)").unwrap()[0],
        -i,
        1e-10,
        0.
    ));
    assert!(r.analyses[0].real("i(vc)").unwrap()[0].abs() < 1e-14);
}

#[test]
fn bjt_transport_transient_step_response() {
    // PTF in degrees gives a 1 ns delay. The forward-current step is
    // smoothed over 0.5 ps, and sampled one delay after its leading edge.
    let r = simulate(
        "transport step\nVb b 0 DC 0.7 PULSE(0.7 0.701 0.5n 0.5p 1p 10n 20n)\nVc c 0 2\nQ1 c b 0 qq\n.model qq npn(is=1e-16 tf=1n ptf=57.29577951308232)\n.tran 4p 4n 0 4p\n.meas tran initial FIND i(vc) AT=0.25n\n.meas tran delayed FIND i(vc) AT=1.5n\n",
    );
    let vt = spx_devices::thermal_voltage(spx_devices::REFTEMP);
    let step = 1e-16 * (0.7 / vt).exp() * (0.001 / vt).exp_m1();
    let actual = (r.measures[0].value.unwrap() - r.measures[1].value.unwrap()) / step;
    let root = 3_f64.sqrt();
    let expected = 1. - (-1.5_f64).exp() * ((root / 2.).cos() + root * (root / 2.).sin());
    assert!(close(actual, expected, 0.005, 0.));
}
