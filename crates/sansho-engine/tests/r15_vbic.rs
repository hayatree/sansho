//! Native VBIC integration, topology, terminal outputs and replay tests.
//! Provenance: original circuits, ngspice 44 manual §§7.3.4 and 27.5.2.
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
.model n npn(level=4 selft=1 rth=200 is=1e-16 ibei=1e-18 rbx=3 rbi=8 rci=2 rcx=1 re=1)
.model p pnp(level=4 selft=1 rth=200 is=1e-16 ibei=1e-18 rbx=3 rbi=8 rci=2 rcx=1 re=1)
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
fn transport_phase_has_the_declared_low_frequency_delay() {
    let deck = sansho_netlist::parse(
        "matched transport
Vc c 0 2
Vd d 0 2
Vb b 0 .69 ac 1
Q1 c b 0 ref
Q2 d b 0 delay
.model ref npn(level=4 rbi=0 rci=0)
.model delay npn(level=4 rbi=0 rci=0 td=1n)
.ac lin 2 10meg 100meg
.save i(Vc) i(Vd)
",
    )
    .unwrap();
    let run = sansho_engine::run(&deck).unwrap();
    let plot = &run.results.analyses[0];
    let iref = plot.complex("i(vc)").unwrap();
    let delayed = plot.complex("i(vd)").unwrap();
    for ((&(re, im), &(r, i)), &freq) in delayed.iter().zip(iref).zip(&plot.scale_values) {
        let w = std::f64::consts::TAU * freq * 1e-9;
        let a = 1. - w * w / 3.;
        assert!((re * a - im * w - r).abs() < 1e-10);
        assert!((im * a + re * w - i).abs() < 1e-10);
    }
}

#[test]
fn noise_levels_follow_shot_and_flicker_laws() {
    let deck = sansho_netlist::parse(
        "base current readout
Vc c 0 1
Vb b 0 .69 ac 1
Hread out 0 Vb 1k
Rload out 0 1k
Q1 c b 0 n area=2 m=2
.model n npn(level=4 ibei=1e-18 ibci=0 rbi=10 rci=0 kfn=1e-15 afn=1.2 bfn=.8)
.op
.save @q1[vbe]
.noise v(out) Vb lin 2 1 100
",
    )
    .unwrap();
    let run = sansho_engine::run(&deck).unwrap();
    let vbe = run.results.analyses[0].real("@q1[vbe]").unwrap()[0];
    let plot = &run.results.analyses[1];
    let vt = 300.15 * 8.617_347_89e-5;
    let diode = 4e-18 * (vbe / vt).exp_m1();
    let ib = diode + 1e-12 * vbe;
    let pocket = 1e-12 * (0.69_f64 - 1.).abs();
    let r = 10. / 4.;
    let g = (diode + 4e-18) / vt;
    let transfer = 1. / (1. + r * g);
    let johnson = 4. * sansho_devices::BOLTZMANN * 300.15 * r * g * g;
    for (&f, &read) in plot
        .scale_values
        .iter()
        .zip(plot.real("onoise_spectrum").unwrap())
    {
        let noise = |i: f64| {
            2. * sansho_devices::CHARGE * i + 2. * 1e-15 * (i / 2.).powf(1.2) / f.powf(0.8)
        };
        // The pocket source reaches the ideal base drive directly. The
        // intrinsic base source and RBI's series voltage noise see feedback.
        let expected = 1e6 * (noise(pocket) + transfer * transfer * (noise(ib) + johnson));
        assert!(
            (read * read / expected - 1.).abs() < 1e-7,
            "{read} vs {expected}"
        );
    }
}

#[test]
fn explicit_breakdown_offset_shifts_the_zero_bias_base_current() {
    let deck = sansho_netlist::parse(
        "breakdown offset
Vc c 0 2
Vb b 0 0
Vr r 0 0
Q1 c b 0 dev
Qr c r 0 ref
.model dev npn(level=4 vbbe=4 ibbe=1u nbbe=1.5 ebbe=1e-6)
.model ref npn(level=4 vbbe=4 ibbe=1u nbbe=1.5 ebbe=0)
.op
.save i(Vb) i(Vr)
",
    )
    .unwrap();
    let run = sansho_engine::run(&deck).unwrap();
    assert!(run.warnings.is_empty());
    let plot = &run.results.analyses[0];
    let shift = plot.real("i(vb)").unwrap()[0] - plot.real("i(vr)").unwrap()[0];
    assert!((shift + 1e-12).abs() < 1e-17, "{shift}");
}

#[test]
fn unsupported_card_inputs_warn_and_disto_reports_analysis_error() {
    let text = "unsupported derivatives\nVc c 0 2\nVb b 0 .65 distof1 .001\nQ1 c b 0 dev\n.model dev npn(level=4 foo=2)\n.disto dec 2 1k 10k\n";
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
            .any(|d| d.code == "E-ANALYSIS" && d.message.contains("vbic"))
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
        let deck = sansho_netlist::parse_file(&root.join(format!("r15_vbic_{name}.cir"))).unwrap();
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
            let tj = a.real("v(tj)").unwrap();
            let peak = tj.iter().copied().fold(0., f64::max);
            assert!(peak > 0.1);
            let fraction = (tj[tj.len() - 1] - tj[0]) / (peak - tj[0]);
            let expected = (-43.8e-6_f64 / (200. * 60e-9)).exp();
            assert!((fraction - expected).abs() < 0.001);
        }
    }
}

#[test]
fn only_the_exported_temperature_pin_heats_the_transistor() {
    let deck = sansho_netlist::parse(
        "thermal pin export
.subckt package c b heat
Qcell c b 0 0 heat n area=2 icvbe=.61 icvce=1.1
.ends
Vc1 c1 0 2
Vc2 c2 0 2
Vb b 0 .72
X1 c1 b rise package
Q2 c2 b 0 n 2
.model n npn(level=4 selft=1 rth=200 cth=8n re=2)
.op
.save i(Vc1) i(Vc2) v(rise)
",
    )
    .unwrap();
    let cell = deck.elements.iter().find(|e| !e.ic.is_empty()).unwrap();
    assert_eq!(cell.ic, [0.61, 1.1]);
    let again = sansho_netlist::parse(&sansho_netlist::write(&deck)).unwrap();
    let run = sansho_engine::run(&again).unwrap();
    assert!(run.warnings.is_empty(), "{:?}", run.warnings);
    let read = |name| run.results.analyses[0].real(name).unwrap()[0];
    assert!(read("v(rise)") > 0.);
    assert!(read("i(vc1)") < read("i(vc2)"));
}

#[test]
fn current_probes_include_transient_charge_behind_series_resistances() {
    let deck=sansho_netlist::parse("terminal charge readout
Vc c 0 1.8
Vb b 0 pulse(.48 .7 1n 2n 2n 4n 20n)
Vs s 0 -.2
Q1 c b 0 s n
.model n npn(level=4 rci=5 rcx=1 rbi=20 rbx=5 re=1 rs=2 rbp=8 cje=3p cjc=2p cjep=1p cjcp=.5p cbeo=.3p cbco=.2p tf=.3n wbe=.6)
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
fn parallel_batches_retain_the_same_floating_point_sum() {
    use std::fmt::Write;
    let mut text = String::from(
        "parallel Q groups\nVb b 0 .71\nVc c 0 2\n.model n npn(level=4 is=2e-16 vef=50)\n",
    );
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
