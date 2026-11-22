//! Native HICUM/L2 integration, topology, terminal outputs and replay tests.
//! Provenance: original circuits, ngspice 44 manual §§7.3.5 and 11.5.3.2.
#![allow(clippy::unwrap_used)]

use sansho_engine::{RunConfig, run_with};
use std::path::Path;

#[test]
fn mirrored_scaled_currents_and_thermal_power_agree_with_terminal_readouts() {
    let deck = sansho_netlist::parse(
        "complementary pair
Vc c 0 2
Vb b 0 .7
Vcp cp 0 -2
Vbp bp 0 -.7
Qn c b 0 0 tj n
Qp cp bp 0 0 0 p 2 m=1.5
.model n npn(level=8 version=2.4.0 flsh=2 rth=200 c10=2e-30 ibeis=1e-18 rbx=3 rbi0=8 rcx=1 re=1)
.model p pnp(level=8 version=2.4.0 flsh=2 rth=200 c10=2e-30 ibeis=1e-18 rbx=3 rbi0=8 rcx=1 re=1)
Vclamp tj 0 0
.options gmin=1e-18
.op
.save @qn[ic] @qn[ib] @qn[ie] @qn[is] @qn[p] @qp[ic] @qp[gm] @qn[gm] @qn[cpi] i(Vb) i(Vc) i(Vclamp)
",
    )
    .unwrap();
    let result = sansho_engine::run(&deck).unwrap();
    let v = |s| result.results.analyses[0].real(s).unwrap()[0];
    assert!((v("@qn[ic]") + v("i(vc)")).abs() < 1e-12);
    assert!((v("@qn[ib]") + v("i(vb)")).abs() < 1e-12);
    assert!((v("@qn[ie]") + v("@qn[ic]") + v("@qn[ib]") + v("@qn[is]")).abs() < 1e-15);
    assert!((v("@qp[ic]") + 3. * v("@qn[ic]")).abs() < 1e-12);
    assert!(v("@qn[gm]") > 0. && (v("@qp[gm]") - 3. * v("@qn[gm]")).abs() < 1e-9);
    assert!((v("@qn[p]") - v("i(vclamp)")).abs() < 1e-12);
    assert!((v("@qn[p]") + 2. * v("i(vc)") + 0.7 * v("i(vb)")).abs() < 2e-11);
}

#[test]
fn isolated_bc_diode_currents_reverse_on_the_entire_mirrored_sweep() {
    let template = include_str!("../../../verify/probes/r16b_pnp_bc.cir");
    let deck =
        sansho_netlist::parse(&template.replace(".param probe=0", ".param probe=1e-16")).unwrap();
    let result = sansho_engine::run(&deck).unwrap();
    let sweep = &result.results.analyses[0];
    for (pnp, npn) in [("i(vbp)", "i(vbn)"), ("i(vc)", "i(vcn)")] {
        let p = sweep.real(pnp).unwrap();
        let n = sweep.real(npn).unwrap();
        assert_eq!(p.len(), 51);
        assert_eq!(n.len(), p.len());
        for (&p, &n) in p.iter().zip(n) {
            assert!((p + n).abs() < 1e-24, "{pnp}={p}, {npn}={n}");
        }
    }
}

#[test]
fn unsupported_card_inputs_warn_and_disto_reports_analysis_error() {
    let text = "unsupported derivatives\nVc c 0 2\nVb b 0 .65 distof1 .001\nQ1 c b 0 dev\n.model dev npn(level=8 foo=2)\n.disto dec 2 1k 10k\n";
    let nl = sansho_netlist::parse(text).unwrap();
    let check = sansho_engine::check(&nl).unwrap();
    assert!(
        check
            .warnings
            .iter()
            .any(|d| d.code == "W-MODEL-IGNORED" && d.message.contains("foo"))
    );
    assert!(
        sansho_engine::run(&nl)
            .unwrap_err()
            .diagnostics
            .iter()
            .any(|d| d.code == "E-ANALYSIS" && d.message.contains("hicum"))
    );
    let bad = sansho_netlist::parse(&text.replace("foo=2", "rcx=-1")).unwrap();
    assert!(
        sansho_engine::check(&bad)
            .unwrap_err()
            .iter()
            .any(|d| d.code == "E-ELAB-PARAM")
    );
}

#[test]
fn acceptance_decks_replay_all_samples_and_complex_components() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../verify/decks");
    for name in [
        "gummel",
        "output",
        "ft",
        "thermal",
        "substrate",
        "pnp",
        "temperature",
    ] {
        let deck = sansho_netlist::parse_file(&root.join(format!("r16_hicum_{name}.cir"))).unwrap();
        let run = |threads| run_with(&deck, &RunConfig { threads }).unwrap();
        let first = run(1);
        assert!(first.warnings.is_empty(), "{name}: {:?}", first.warnings);
        let second = run(2);
        let (a, b) = (&first.results.analyses[0], &second.results.analyses[0]);
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert!(a.scale_values.len() > 6);
        assert_eq!(bits(&a.scale_values), bits(&b.scale_values));
        for var in &a.variables {
            if let Some(v) = a.real(&var.name) {
                assert!(v.iter().all(|x| x.is_finite()));
                assert_eq!(bits(v), bits(b.real(&var.name).unwrap()));
            } else {
                for (&(re, im), &(r, i)) in a
                    .complex(&var.name)
                    .unwrap()
                    .iter()
                    .zip(b.complex(&var.name).unwrap())
                {
                    assert!(re.is_finite() && im.is_finite());
                    assert_eq!((re.to_bits(), im.to_bits()), (r.to_bits(), i.to_bits()));
                }
            }
        }
        if name == "thermal" {
            let rise = a.real("v(tj)").unwrap();
            let peak = rise.iter().copied().fold(0., f64::max);
            assert!(peak > 0.1 && rise[rise.len() - 1] < 0.1 * peak);
        }
    }
}

#[test]
fn parallel_batches_retain_the_same_floating_point_sum() {
    use std::fmt::Write;
    let mut text =
        String::from("parallel Q groups\nVb b 0 .71\nVc c 0 2\n.model n npn(level=8 t0=2p)\n");
    for i in 0..257 {
        writeln!(text, "Q{i} c b 0 n area={}", 1. + (i % 67) as f64 / 10.).unwrap();
    }
    text.push_str(".op\n.save i(Vc) i(Vb)\n");
    let deck = sansho_netlist::parse(&text).unwrap();
    let run = |threads| run_with(&deck, &RunConfig { threads }).unwrap();
    let (a, b) = (run(1), run(2));
    for probe in ["i(vc)", "i(vb)"] {
        let i = a.results.analyses[0].real(probe).unwrap()[0];
        assert!(i < 0.);
        assert_eq!(
            i.to_bits(),
            b.results.analyses[0].real(probe).unwrap()[0].to_bits()
        );
    }
}
#[test]
fn current_probes_include_transient_charge_behind_series_resistances() {
    let deck=sansho_netlist::parse("terminal charge readout
Vc c 0 1.8
Vb b 0 pulse(.48 .7 1n 2n 2n 4n 20n)
Vs s 0 -.2
Q1 c b 0 s n
.model n npn(level=8 qp0=1p rcx=1 rbi0=20 rbx=5 re=1 rsu=2 cjei0=3p cjci0=2p cjep0=1p cjs0=.5p cbepar=.3p cbcpar=.2p t0=.3n)
.tran 50p 12n
.save i(Vc) i(Vb) i(Vs) @q1[ic] @q1[ib] @q1[is] @q1[ie]
").unwrap();
    let run = sansho_engine::run(&deck).unwrap();
    let a = &run.results.analyses[0];
    for (s, p) in [
        ("i(vc)", "@q1[ic]"),
        ("i(vb)", "@q1[ib]"),
        ("i(vs)", "@q1[is]"),
    ] {
        let read = a.real(p).unwrap();
        let src = a.real(s).unwrap();
        let peak = src.iter().copied().map(f64::abs).fold(0., f64::max);
        assert!(peak > 1e-9);
        let error = src
            .iter()
            .zip(read)
            .map(|(x, y)| (x + y).abs())
            .fold(0., f64::max);
        assert!(error < 1e-10 + peak * 1e-3, "{s}: {error} vs {peak}");
    }
}

#[test]
fn default_card_reproduces_the_supplied_forward_bias_current_scale() {
    let nl=sansho_netlist::parse("nominal\nVc c 0 1\nVb b 0 .8\nQ1 c b 0 n\n.model n npn level=8\n.op\n.save @q1[ic] @q1[ib] @q1[ick] @q1[gmi] @q1[cpi]\n").unwrap();
    let run = sansho_engine::run(&nl).unwrap();
    let a = &run.results.analyses[0];
    // The listing gives one rounded sample, not an accuracy bound over bias.
    for (name, observed) in [
        ("ic", 0.00270488),
        ("ib", 2.7083e-5),
        ("ick", 0.00296642),
        ("gmi", 0.102413),
        ("cpi", 2.49397e-20),
    ] {
        let actual = a.real(&format!("@q1[{name}]")).unwrap()[0];
        assert!((actual / observed - 1.).abs() < 3e-5, "{name}: {actual}");
    }
}

#[test]
fn soa_checks_accepted_external_voltages_before_save_projection() {
    let nl=sansho_netlist::parse("safe area\nVb b 0 .6\nVc c 0 -1\nQ1 c b 0 0 n\n.model n pnp level=8 vce_max=.7 vcs_max=.9\n.options warn=1 maxwarns=1\n.dc Vc -1 -2 -.2\n.save i(Vc)\n").unwrap();
    let run = sansho_engine::run(&nl).unwrap();
    let warnings: Vec<_> = run
        .warnings
        .iter()
        .filter(|d| d.code == "W-HICUM-SOA")
        .collect();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings.iter().any(|d| d.message.contains("vce_max"))
            && run
                .warnings
                .iter()
                .any(|d| d.code == "W-MODEL-IGNORED" && d.message.contains("vcs_max"))
    );
}

#[test]
fn shot_noise_spectrum_at_an_ideal_base_current_readout() {
    let nl=sansho_netlist::parse("noisy input\nVc c 0 1\nVb b 0 .72 ac 1\nHread out 0 Vb 1k\nQ1 c b 0 n m=2\n.model n npn level=8 ibeis=2e-18 ibcis=0 kf=1e-13 af=1.5\n.noise v(out) Vb dec 3 1 1meg\n").unwrap();
    let run = sansho_engine::run(&nl).unwrap();
    let a = &run.results.analyses[0];
    let i = 2e-18 * (0.72 / sansho_devices::thermal_voltage(300.15)).exp_m1();
    for (&freq, &amplitude) in a
        .scale_values
        .iter()
        .zip(a.real("onoise_spectrum").unwrap())
    {
        // Two unscaled BC shunts contribute shot noise from their reverse currents.
        let bc_shunts = 2. * (1. - 0.72) * 1e-12;
        let psd =
            1e6 * (2. * sansho_devices::CHARGE * (2. * i + bc_shunts) + 2e-13 * i.powf(1.5) / freq);
        assert!((amplitude.powi(2) / psd - 1.).abs() < 1e-8);
    }
}

#[test]
fn nqs_current_poles_match_the_analytic_transfer_function() {
    let nl = sansho_netlist::parse(
        "two current readouts
Vb b 0 .68 ac 1
Vc c 0 1
Vd d 0 1
Q1 c b 0 ref
Q2 d b 0 delayed
.model ref npn level=8 t0=100p alit=0 alqf=0 ibcis=0 cjci0=0 cjcx0=0
.model delayed npn level=8 t0=100p ibcis=0 cjci0=0 cjcx0=0 flnqs=1 alit=.5
.option gmin=1e-18
.ac dec 3 10meg 10G
.save i(Vc) i(Vd)
",
    )
    .unwrap();
    let run = sansho_engine::run(&nl).unwrap();
    let a = &run.results.analyses[0];
    for ((&(re, im), &(r, i)), &freq) in a
        .complex("i(vd)")
        .unwrap()
        .iter()
        .zip(a.complex("i(vc)").unwrap())
        .zip(&a.scale_values)
    {
        let wt = std::f64::consts::TAU * freq * 50e-12;
        let real = 1. - wt * wt / 3.;
        assert!((real * re - im * wt - r).abs() < 1e-11 && (real * im + re * wt - i).abs() < 1e-11);
    }
}

#[test]
fn thermal_node_survives_subcircuit_expansion_and_omission_allocates_internal_heat_balance() {
    let nl = sansho_netlist::parse(
        "packaged transistor
.subckt cell c b tj
Qdie c b 0 0 tj heat
.ends
Vc c 0 1.3
Vd d 0 1.3
Vb b 0 .76
X1 c b tj cell
Qref d b 0 0 heat
.model heat npn level=8 version=2.4.0 flsh=2 rth=300
.op
.save v(tj) i(Vc) i(Vd)
",
    )
    .unwrap();
    let run = sansho_engine::run(&nl).unwrap();
    let a = &run.results.analyses[0];
    assert!(a.real("v(tj)").unwrap()[0] > 0.01);
    assert!((a.real("i(vc)").unwrap()[0] - a.real("i(vd)").unwrap()[0]).abs() < 1e-11);
}
