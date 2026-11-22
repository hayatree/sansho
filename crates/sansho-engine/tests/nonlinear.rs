//! Nonlinear devices, measurements and determinism.
//!
//! Provenance: original test circuits; expected values are derived from the model equations
//! documented in the device modules.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use sansho_engine::{RunConfig, run, run_batch, run_with};
use sansho_netlist::parse;
use sansho_output::SimResults;

fn simulate(src: &str) -> SimResults {
    let nl = parse(src).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    match run(&nl) {
        Ok(out) => out.results,
        Err(e) => panic!("run failed: {:#?}", e.diagnostics),
    }
}

fn op(r: &SimResults, name: &str) -> f64 {
    r.analyses[0]
        .real(name)
        .unwrap_or_else(|| panic!("no {name}"))[0]
}

fn meas(r: &SimResults, name: &str) -> f64 {
    let m = r.measures.iter().find(|m| m.name == name).unwrap();
    m.value.unwrap_or_else(|| panic!("{name}: {:?}", m.error))
}

const VT: f64 = 1.380_649e-23 * 300.15 / 1.602_176_634e-19;

#[test]
fn diode_resistor_operating_point() {
    let r = simulate("d\nV1 a 0 5\nR1 a k 1k\nD1 k 0 dm\n.model dm d is=1e-14 n=1.5\n.op\n");
    let vd = op(&r, "v(k)");
    let i = (5.0 - vd) / 1e3;
    // KCL with the exact diode equation (plus gmin = 1e-12 S). The residual reflects the
    // SPICE stopping criterion (|dv| <= reltol*|v| + vntol), i.e. a voltage error ~1e-7 V.
    let id = 1e-14 * ((vd / (1.5 * VT)).exp() - 1.0) + 1e-12 * vd;
    assert!((i - id).abs() < 1e-5 * i, "i={i} id={id}");
}

#[test]
fn diode_tbv_parameters_warn_and_leave_breakdown_unchanged() {
    let probe = include_str!("../../../verify/probes/r8_diode_tbv_temp.cir");
    let nl = parse(&probe.replace(".temp 20", ".temp 100")).unwrap();
    let out = run(&nl).unwrap();
    assert_eq!(op(&out.results, "v(test)"), op(&out.results, "v(control)"));
    for name in ["tbv1", "tbv2"] {
        assert!(
            out.warnings
                .iter()
                .any(|w| w.code == "W-MODEL-IGNORED" && w.message.contains(name))
        );
    }
}

#[test]
fn vdmos_thermal_parameter_names() {
    let deck = "idle thermal pins\nM1 0 0 0 tj tc p thermal\nVcase tc 0 10\n.model p vdmos(rthjc=5 rthca=1000 cthj=100u cthjc=1)\n.op\n";
    let out = run(&parse(deck).unwrap()).unwrap();
    let warnings: Vec<_> = out
        .warnings
        .iter()
        .filter(|w| w.code == "W-MODEL-IGNORED")
        .collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.ends_with("ignored: cthjc"));
    assert!((op(&out.results, "v(tj)") - 10.135).abs() < 1e-8);
    assert!((op(&out.results, "i(vcase)") - 0.017).abs() < 1e-8);
}

#[test]
fn vdmos_open_case_starts_near_ambient() {
    let deck = include_str!("../../../verify/probes/r9_vdmos_implicit_case.cir")
        .replace(".param probe=10", ".param probe=1000");
    let out = simulate(&deck);
    let jf = out.analyses[0].real("v(jf)").unwrap();
    let jc = out.analyses[0].real("v(jc)").unwrap();
    assert!((jf[0] - 27.13514).abs() < 1e-5);
    assert!((jc[0] - 0.135001).abs() < 1e-5);
}

#[test]
fn vdmos_hot_drain_access_at_fixed_junction_rise() {
    let out = simulate(include_str!(
        "../../../verify/probes/r9_vdmos_hot_channel.cir"
    ));
    let access = out.analyses[0].real("i(vaccess)").unwrap();
    assert!((access.last().unwrap() + 1.902550529).abs() < 1e-6);
}

#[test]
fn bjt_forward_active_gain() {
    let r = simulate(
        "q\nVCE c 0 5\nIB 0 b 10u\nQ1 c b 0 qm\n.model qm npn is=1e-15 bf=120 br=2\n.op\n",
    );
    let ic = -op(&r, "i(vce)");
    // Ic = BF·Ib minus the reverse-junction terms (negligible), up to gmin currents.
    assert!((ic / 10e-6 - 120.0).abs() < 1e-4, "beta = {}", ic / 10e-6);
}

#[test]
fn nmos_square_law_saturation() {
    let r = simulate(
        "m\nVDS d 0 3\nVGS g 0 1.5\nM1 d g 0 0 nm l=1u w=10u\n\
         .model nm nmos vto=0.7 kp=100u lambda=0.02\n.op\n",
    );
    let id = -op(&r, "i(vds)");
    let expect = 0.5 * 100e-6 * 10.0 * 0.8f64.powi(2) * (1.0 + 0.02 * 3.0);
    // Up to the drain junction leakage and gmin (a few pA).
    assert!(
        (id - expect).abs() < 1e-9 * expect + 1e-11,
        "{id} vs {expect}"
    );
}

#[test]
fn cmos_inverter_transfer_curve() {
    let r = simulate(
        "inv\nVDD vdd 0 1.8\nVIN in 0 0\nMP out in vdd vdd pm l=1u w=2u\nMN out in 0 0 nm l=1u w=1u\n\
         .model nm nmos vto=0.5 kp=100u\n.model pm pmos vto=-0.5 kp=50u\n.dc VIN 0 1.8 0.01\n",
    );
    let v = r.analyses[0].real("v(out)").unwrap();
    assert!((v[0] - 1.8).abs() < 1e-6 && v.last().unwrap().abs() < 1e-6);
    assert!(v.windows(2).all(|w| w[1] <= w[0] + 1e-9), "not monotonic");
    // Symmetric inverter (equal kp·W/L and |vto|): the output crosses VDD/2 at VIN = VDD/2.
    let k = v.iter().position(|&x| x < 0.9).unwrap();
    let vin = r.analyses[0].scale_values[k];
    assert!((vin - 0.9).abs() <= 0.011, "switching threshold {vin}");
}

#[test]
fn rc_time_constant_and_statistics_via_meas() {
    let src = "\
rc
V1 in 0 PULSE(0 1 0 1p 1p 1 2)
R1 in out 1k
C1 out 0 1n
V2 s 0 SIN(0 2 1meg)
R2 s 0 1k
.tran 1n 5u
.meas tran tau TRIG AT=0 TARG v(out) VAL=0.6321205588 RISE=1
.meas tran vavg AVG v(s) FROM=0 TO=4u
.meas tran vrms RMS v(s) FROM=0 TO=4u
.meas tran area INTEG v(in) FROM=1u TO=3u
.meas tran vpp PP v(s)
.meas tran half WHEN v(out)=0.5
.meas tran v1u FIND v(out) AT=1u
";
    let r = simulate(src);
    assert!(
        (meas(&r, "tau") - 1e-6).abs() < 2e-3 * 1e-6,
        "{}",
        meas(&r, "tau")
    );
    assert!(meas(&r, "vavg").abs() < 1e-3);
    assert!((meas(&r, "vrms") - 2.0 / 2f64.sqrt()).abs() < 2e-3);
    assert!((meas(&r, "area") - 2e-6).abs() < 1e-12);
    assert!((meas(&r, "vpp") - 4.0).abs() < 1e-3);
    assert!((meas(&r, "half") - 1e-6 * 2f64.ln()).abs() < 2e-3 * 1e-6);
    assert!((meas(&r, "v1u") - (1.0 - (-1.0f64).exp())).abs() < 1e-3);
}

#[test]
fn failed_measurement_is_reported_not_fatal() {
    let nl = parse("t\nV1 a 0 1\nR1 a 0 1\n.tran 1n 10n\n.meas tran x WHEN v(a)=5\n").unwrap();
    let out = run(&nl).unwrap();
    assert_eq!(out.results.measures[0].value, None);
    assert!(out.warnings.iter().any(|w| w.code == "W-MEAS-FAILED"));
}

#[test]
fn gear_integration_rc_step() {
    let r = simulate(
        "rc\n.options method=gear\nV1 in 0 PULSE(0 1 0 1p 1p 1 2)\nR1 in out 1k\nC1 out 0 1n\n.tran 10n 5u\n",
    );
    let a = &r.analyses[0];
    let v = a.real("v(out)").unwrap();
    let err = a
        .scale_values
        .iter()
        .zip(v)
        .map(|(&t, &v)| (v - (1.0 - (-t / 1e-6).exp())).abs())
        .fold(0.0f64, f64::max);
    assert!(err < 2e-3, "max error {err}");
}

const MIXED: &str = "\
mixed
VDD vdd 0 3.3
VIN in 0 DC 1 AC 1 PULSE(0 3.3 1n 0.2n 0.2n 5n 10n)
MP out in vdd vdd pch l=1u w=4u
MN out in 0 0 nch l=1u w=2u
CL out 0 50f
D1 out 0 dm
.model dm d is=1e-15 cjo=10f
.model nch nmos vto=0.7 kp=110u tox=9n lambda=0.04
.model pch pmos vto=-0.8 kp=50u tox=9n lambda=0.05
.op
.dc VIN 0 3.3 0.05
.ac dec 5 1k 1g
.tran 0.05n 20n
";

#[test]
fn results_do_not_depend_on_thread_count() {
    let nl = parse(MIXED).unwrap();
    let a = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    let b = run_with(&nl, &RunConfig { threads: 4 }).unwrap().results;
    assert_eq!(a, b);
    let batch = run_batch(&[nl.clone(), nl], &RunConfig { threads: 3 });
    for r in batch {
        assert_eq!(r.unwrap().results, a);
    }
}

/// A ring oscillator large enough to enable parallel device evaluation and assembly (L1/L2).
fn big_ring(stages: usize) -> String {
    let mut s = String::from(
        "big ring\nvdd vdd 0 1.8\n\
         .model nm nmos vto=0.5 kp=100u lambda=0.02 cgso=1n cgdo=1n cj=1e-4 tox=10n\n\
         .model pm pmos vto=-0.5 kp=40u lambda=0.02 cgso=1n cgdo=1n cj=1e-4 tox=10n\n\
         ik 0 n0 pulse(0 1m 0 0.1n 0.1n 0.5n 1)\n",
    );
    for i in 0..stages {
        let (a, b) = (i, (i + 1) % stages);
        s += &format!("mp{i} n{b} n{a} vdd vdd pm w=2u l=0.5u ad=2p as=2p\n");
        s += &format!("mn{i} n{b} n{a} 0 0 nm w=1u l=0.5u ad=1p as=1p\n");
        s += &format!("c{i} n{b} 0 5f\n");
    }
    s + ".tran 0.1n 3n\n.end\n"
}

#[test]
fn parallel_assembly_is_bit_identical() {
    let nl = parse(&big_ring(301)).unwrap();
    let a = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    for threads in [2, 5, 8] {
        let b = run_with(&nl, &RunConfig { threads }).unwrap().results;
        assert_eq!(a, b, "threads = {threads}");
    }
}

#[test]
fn model_diagnostics() {
    let e = parse("t\nD1 a 0 nosuch\nV1 a 0 1\n.op\n").err().unwrap();
    assert!(e.iter().any(|d| d.code == "E-PARSE-MODEL"));
    let nl = parse("t\nD1 a 0 dm\nV1 a 0 0.5\n.model dm d is=1e-14 foo=3\n.op\n").unwrap();
    let out = run(&nl).unwrap();
    assert!(out.warnings.iter().any(|w| w.code == "W-MODEL-IGNORED"));
    let nl = parse("t\nM1 d g 0 0 m4\nV1 d 0 1\nV2 g 0 1\n.model m4 nmos level=99\n.op\n").unwrap();
    let err = run(&nl).err().unwrap();
    assert!(err.diagnostics.iter().any(|d| d.code == "E-ELAB-MODEL"));
}

#[test]
fn convergence_failure_names_culprits() {
    let nl = parse(
        "t\nI1 0 a 1\nR1 a b 1\nD1 b 0 dd\nD2 0 b dd\n.model dd d is=1e-30 n=0.01\n\
         .options gminsteps=0 srcsteps=0 itl1=3\n.op\n",
    )
    .unwrap();
    let err = run(&nl).expect_err("should not converge");
    let d = &err.diagnostics[0];
    assert_eq!(d.code, "E-CONV-OP");
    assert_eq!(d.attempts, ["newton"]);
    assert_eq!(d.culprits[0].name, "b");
    assert!(d.culprits.iter().any(|c| c.name == "d1"));
    assert!(d.suggestions.iter().any(|s| s.action.contains("gminsteps")));
    assert!(d.info().is_some());
}

#[test]
#[ignore = "single-worker LU profile with SANSHO_PROFILE_DEVICES=1"]
fn profile_big_ring() {
    let deck = parse(&big_ring(301)).unwrap();
    run_with(&deck, &RunConfig { threads: 1 }).unwrap();
}
