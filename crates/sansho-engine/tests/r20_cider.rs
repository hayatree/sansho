//! Numerical semiconductor checks through the netlist and analysis entry points.
//! Provenance: Shockley diffusion theory, abrupt-junction depletion theory and
//! ngspice 44 manual §§26.16–26.18. Original circuits.
#![allow(clippy::unwrap_used)]

use sansho_engine::{RunConfig, run_with};
use sansho_netlist::{parse, write};
use sansho_output::AnalysisResult;

fn run(deck: &str) -> AnalysisResult {
    let n = parse(deck).unwrap();
    let result = run_with(&n, &RunConfig { threads: 1 }).unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    result.results.analyses.into_iter().next().unwrap()
}

#[test]
fn long_diode_diffusion_current() {
    // Both neutral pieces exceed seven minority diffusion lengths.
    let deck = "long silicon diode\nVbias a 0 0.5\nD1 a 0 crystal\n.model crystal numd
+ options defa=1e-10
+ x.mesh loc=0 n=1
+ x.mesh loc=200 h.end=0.005 h.max=2 ratio=1.25
+ x.mesh width=200 h.start=0.005 h.max=2 ratio=1.25
+ domain num=1 material=1
+ material num=1 silicon tn=200n tp=200n
+ doping uniform n.type conc=1e16
+ doping uniform p.type conc=2e16 x.high=200
+ models srh
.op
.save i(vbias)";
    let current = -run(deck).real("i(vbias)").unwrap()[0];
    let vt = sansho_devices::thermal_voltage(300.15);
    let ni = 1.1376e10_f64;
    let is = sansho_devices::CHARGE * 1e-6 * ni.powi(2) / 1e16
        * ((1340. * vt / 200e-9).sqrt() + (500. * vt / 200e-9).sqrt());
    let predicted = is * (0.5 / vt).exp_m1();
    assert!(
        (current / predicted - 1.).abs() < 0.08,
        "{current:e}, {predicted:e}"
    );
}

#[test]
fn abrupt_junction_capacitance_converges() {
    let r = run(include_str!("../../../verify/probes/r20_mesh.cir"));
    let omega = std::f64::consts::TAU * 1e5;
    let [coarse, middle, fine] =
        ["i(v1)", "i(v2)", "i(v3)"].map(|k| -r.complex(k).unwrap()[0].1 / omega);
    let vbi = 2. * sansho_devices::thermal_voltage(300.15) * (1e16 / 1.1376e10_f64).ln();
    let expected = 1e-6
        * (sansho_devices::CHARGE * 11.7 * 8.8541878128e-14 * 1e16 / (4. * (vbi + 0.2))).sqrt();
    assert!((fine / expected - 1.).abs() < 0.04);
    assert!((fine - middle).abs() < 0.3 * (middle - coarse).abs());
}

#[test]
fn terminal_matrix_agrees_with_driven_circuit() {
    let deck=include_str!("../../../verify/decks/r20_nbjt_ac.cir").replace("i(vb) i(vc)","i(vb) i(vc) @qvertical[y12] @qvertical[y22] @qvertical[y32] @qvertical[c12] @qvertical[g12] @qvertical[ic] @qvertical[ib] @qvertical[ie]");
    let r = run(&deck);
    let signal = |k| r.complex(k).unwrap();
    for (i, &freq) in r.scale_values.iter().enumerate() {
        let [c, b, e] =
            ["@qvertical[y12]", "@qvertical[y22]", "@qvertical[y32]"].map(|k| signal(k)[i]);
        let close = |actual: f64, expected: f64| {
            assert!(
                (actual - expected).abs() < 1e-14 + 1e-6 * expected.abs(),
                "{actual:e} != {expected:e}"
            )
        };
        for (name, port) in [
            ("@qvertical[ic]", c),
            ("@qvertical[ib]", b),
            ("@qvertical[ie]", e),
        ] {
            let terminal = signal(name)[i];
            close(terminal.0, port.0);
            close(terminal.1, port.1);
        }
        close(c.0, -signal("i(vc)")[i].0);
        close(c.1, -signal("i(vc)")[i].1);
        close(b.0, -signal("i(vb)")[i].0);
        // Summing both carrier equations over the volume cancels large majority
        // currents. Bound the remaining complex current in amperes per volt.
        assert!((e.0 + c.0 + b.0).hypot(e.1 + c.1 + b.1) < 2e-12);
        close(c.0, signal("@qvertical[g12]")[i].0);
        close(
            c.1,
            signal("@qvertical[c12]")[i].0 * freq * std::f64::consts::TAU,
        );
    }
}

#[test]
fn gear_recovery_and_thread_determinism() {
    let deck = include_str!("../../../verify/decks/r20_diode_recovery.cir")
        .replace(".options itl1", ".options method=gear savecurrents itl1");
    let n = parse(&deck).unwrap();
    let simulate = |threads| run_with(&n, &RunConfig { threads }).unwrap().results;
    let first = simulate(1);
    let current = first.analyses[0].real("i(vdrive)").unwrap();
    let diode = first.analyses[0].real("@djunction[id]").unwrap();
    assert!(diode.iter().zip(current).all(|(d, i)| (d + i).abs() < 1e-9));
    assert!(current[0] < -1e-6 && current.iter().any(|&v| v > 1e-5));
    let bytes = |r| {
        let mut data = Vec::new();
        sansho_output::write_raw_binary(r, &mut data).unwrap();
        data
    };
    assert_eq!(bytes(&first), bytes(&simulate(2)));
}

#[test]
fn card_continuation_area_temperature_and_diagnostics() {
    let deck = "resistor\nV1 a 0 0.01\nD1 a 0 m area=2 temp=27 off\n.model m numd
+ options resistor defa=1e-10
+ x.mesh width=1 number=8
+ domain num=1 material=1
+ material num=1 silicon
+ doping uniform n.type
+ + conc=1e16
+ models concmob ^concmob
+ end
+ y.mesh width=2
.temp 125
.op
.save i(v1)";
    let n = parse(deck).unwrap();
    let r = run(&write(&n));
    let current = -r.real("i(v1)").unwrap()[0];
    let predicted = sansho_devices::CHARGE * 1e16 * 1340. * 2e-6 * 0.01 / 1e-4;
    assert!((current / predicted - 1.).abs() < 1e-6);
    for kind in ["numd level=3", "nbjt level=3"] {
        assert!(
            parse(&deck.replace(".model m numd", &format!(".model m {kind}")))
                .unwrap_err()
                .iter()
                .any(|d| d.code == "E-UNSUPPORTED")
        );
    }
    for (old, new, code) in [
        ("number=8", "number=99999999", "E-CIDER-MODEL"),
        ("conc=1e16", "conc=-1", "E-CIDER-MODEL"),
        (
            "models concmob ^concmob",
            "mobility material=1 fieldmodel=unknown",
            "E-UNSUPPORTED",
        ),
    ] {
        let n = parse(&deck.replace(old, new)).unwrap();
        assert!(
            run_with(&n, &RunConfig { threads: 1 })
                .unwrap_err()
                .diagnostics
                .iter()
                .any(|d| d.code == code)
        );
    }
}

#[test]
fn scoped_profiles_and_mobility_override() {
    let deck="locally bound silicon resistor
.subckt slab hi lo params: peak=1e16
Dinside hi lo crystal
.model crystal numd
+ options resistor defa=1e-10
+ x.mesh loc=0 n=1
+ x.mesh width=1 n=12
+ material num=1 silicon
+ domain num=1 mat=1 x.high=0.5
+ domain num=2 mat=1 x.low=0.5
+ doping uniform domains=(1,2) donor conc={peak}
+ mobility mat=1 electron majority concmodel=ct fieldmodel=sg mumax=800 mumin=100 ntref=1e16 ntexp=1 vsat=1e7
+ models concmob fieldmob
.ends
Va a 0 0.1
Vb b 0 0.1
Xone a 0 slab peak=1e16
Xtwo b 0 slab peak=2e16
.op
.save i(va) i(vb)";
    let r = run(deck);
    for (k, dop) in [("i(va)", 1e16), ("i(vb)", 2e16)] {
        let low = 100. + 700. / (1. + dop / 1e16);
        let field = 1000.;
        let mu = 2. * low / (1. + (1.0_f64 + 4. * (low * field / 1e7_f64).powi(2)).sqrt());
        let expected = 1e-6 * sansho_devices::CHARGE * dop * mu * field;
        assert!((-r.real(k).unwrap()[0] / expected - 1.).abs() < 1e-6);
    }
}

#[test]
fn asymmetric_nbjt_emitter_is_at_low_x() {
    let result = run(include_str!("../../../verify/decks/r20_nbjt_gummel.cir"));
    let ib = result.real("i(vb)").unwrap().last().unwrap();
    let ic = result.real("i(vc)").unwrap().last().unwrap();
    // The lightly doped emitter gives poor injection efficiency. Interchanging
    // the ends of this stack would give a high forward current gain.
    assert!(*ib < 0. && *ic < 0. && ic / ib < 1.);
}
