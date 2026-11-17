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
    assert!(first.notes.iter().any(|n| n.contains('b')));
    assert!(err.diagnostics.iter().any(|d| d.code == "W-ELAB-NODC"));
}

#[test]
fn rshunt_fixes_floating_node() {
    let r = simulate("floating\nV1 a 0 1\nC1 a b 1n\nC2 b 0 1n\n.options rshunt=1e12\n.op\n");
    assert!(r.analyses[0].real("v(b)").unwrap()[0].abs() < 1e-9);
}
