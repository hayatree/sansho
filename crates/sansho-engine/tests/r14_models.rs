//! Classical transistor acceptance through netlist binding and batch analyses.
//! Provenance: original R14 circuits, ngspice 44 manual §§7.4, 7.6 and 7.7.
#![allow(clippy::unwrap_used)]

use sansho_engine::{RunConfig, run_with};
use std::path::Path;

#[test]
fn acceptance_decks_have_finite_distinct_traces_and_replay_exactly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../verify/decks");
    for name in [
        "jfet2_output",
        "jfet2_transfer",
        "jfet2_amplifier",
        "mos9_output",
        "bsim1_output",
        "bsim1_transfer",
        "bsim1_inverter",
        "bsim2_output",
        "bsim2_transfer",
        "bsim2_inverter",
        "vdmos_thermal",
    ] {
        let deck = sansho_netlist::parse_file(&root.join(format!("r14_{name}.cir"))).unwrap();
        let a = run_with(&deck, &RunConfig { threads: 1 }).unwrap();
        assert!(a.warnings.is_empty(), "{name}: {:?}", a.warnings);
        let b = run_with(&deck, &RunConfig { threads: 2 }).unwrap();
        let plot = &a.results.analyses[0];
        let other = &b.results.analyses[0];
        let bits = |x: &[f64]| x.iter().copied().map(f64::to_bits).collect::<Vec<_>>();
        assert_eq!(bits(&plot.scale_values), bits(&other.scale_values));
        for var in &plot.variables {
            let x = plot.real(&var.name).unwrap();
            assert!(x.iter().all(|v| v.is_finite()), "{name}");
            assert_eq!(bits(x), bits(other.real(&var.name).unwrap()), "{name}");
        }
        let signal = if name.ends_with("inverter") {
            "v(out)"
        } else if name.ends_with("amplifier") {
            "v(d)"
        } else if name == "vdmos_thermal" {
            "v(tj)"
        } else {
            "i(vd)"
        };
        let x = plot.real(signal).unwrap();
        let low = x.iter().copied().fold(f64::INFINITY, f64::min);
        let high = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(high - low > 1e-5, "{name}: {low}..{high}");
        if name.ends_with("inverter") {
            assert!(
                low < 0.3 && high > 2.7 && x[0] > 2.7,
                "{name}: {low}..{high}"
            );
        }
    }
}

#[test]
fn mos9_geometry_matches_mos3_at_adjusted_dimensions() {
    let nl=sansho_netlist::parse("paired geometry\nVd d 0 2\nVg g 0 1.6\nVx x 0 2\nM1 d g 0 0 a w=4u l=1u\nM2 x g 0 0 b w=3.5u l=1.1u\n.model a nmos(level=9 xl=.1u xw=-.5u delvt0=.1 vto=.5 theta=.2 wd=.1u)\n.model b nmos(level=3 vto=.6 theta=.2 wd=.1u)\n.op\n.save @m1[gm] @m2[gm] @m1[id] @m2[id] @m1[vth]\n").unwrap();
    let run = sansho_engine::run(&nl).unwrap();
    assert!(run.warnings.is_empty(), "{:?}", run.warnings);
    let v = |k| run.results.analyses[0].real(k).unwrap()[0];
    assert!((v("@m1[gm]") - v("@m2[gm]")).abs() < 1e-15);
    assert!((v("@m1[id]") - v("@m2[id]")).abs() < 1e-15);
    assert!((v("@m1[vth]") - 0.6).abs() < 1e-14);
}

#[test]
fn fet_noise_and_unsupported_parameters() {
    for (inst, card) in [
        ("J1 d g 0 dev", "njf(level=2 beta=.4m vto=-1)"),
        ("M1 d g 0 0 dev", "nmos(level=9 vto=.3 kp=.1m)"),
        (
            "M1 d g 0 0 dev",
            "nmos(level=4 phi=.7 tox=.03 vfb=-.5 muz=600)",
        ),
        (
            "M1 d g 0 0 dev",
            "nmos(level=5 phi=.7 tox=.03 vfb=-.5 mu0=600)",
        ),
    ] {
        let text = format!(
            "noise\n{inst}\n.model dev {card}\nVg g 0 0.4 ac 1\nVd d 0 2\nHread out 0 Vd 1k\n.noise v(out) Vg lin 2 10 100\n"
        );
        let nl = sansho_netlist::parse(&text).unwrap();
        let o = sansho_engine::run(&nl).unwrap();
        let spectrum = o.results.analyses[0].real("onoise_spectrum").unwrap();
        assert!(spectrum[0] > 0. && spectrum[0].is_finite());
        assert_eq!(spectrum[0], spectrum[1]);
        let bad = text.replace(card, &card.replace(')', " unknownfit=2)"));
        let nl = sansho_netlist::parse(&bad).unwrap();
        assert!(
            sansho_engine::check(&nl)
                .unwrap()
                .warnings
                .iter()
                .any(|d| d.code == "W-MODEL-IGNORED" && d.message.contains("unknownfit"))
        );
    }
}

#[test]
fn mos9_sensitivity_flags_and_scalar_initial_conditions() {
    let nl=sansho_netlist::parse("geometry sensitivity\nVd d 0 1\nVg g 0 2\nM1 d g 0 0 n w=3u l=1u sens_l sens_w icvgs=.4\n.model n nmos(level=9 kp=1e-4 vto=.4)\n.sens i(Vd)\n").unwrap();
    assert_eq!(nl.elements[2].ic, [0., 0.4, 0.]);
    let run = sansho_engine::run(&nl).unwrap();
    assert!(run.warnings.is_empty(), "{:?}", run.warnings);
    let a = &run.results.analyses[0];
    let dw = a.real("m1:w").unwrap()[0];
    let dl = a.real("m1:l").unwrap()[0];
    assert!(dw < 0. && (dl + 3. * dw).abs() < 1e-5);
}

#[test]
fn early_bsim_charge_vectors_are_intrinsic_and_conservative() {
    for (level, mu) in [(4, "muz"), (5, "mu0")] {
        let text = format!(
            "charges\nVd d 0 2\nVg g 0 1.7\nM1 d g 0 0 n w=3u l=1u\n.model n nmos(level={level} tox=.03 phi=.7 vfb=-.5 {mu}=600 cgdo=1n xpart=0)\n.op\n.save @m1[qd] @m1[qs] @m1[qg] @m1[qb] @m1[cgd] @m1[cgs] @m1[cgg] @m1[cgb]\n"
        );
        let run = sansho_engine::run(&sansho_netlist::parse(&text).unwrap()).unwrap();
        let q = |name: &str| {
            run.results.analyses[0]
                .real(&format!("@m1[{name}]"))
                .unwrap()[0]
        };
        assert!((q("qd") + q("qs") + q("qg") + q("qb")).abs() < 1e-27);
        assert!((q("cgd") + q("cgs") + q("cgg") + q("cgb")).abs() < 1e-27);
        assert!(q("qg") > 0. && q("cgg") > 0.);
        let fraction = if level == 4 { 0. } else { 0.4 };
        assert!((q("qd") / (q("qd") + q("qs")) - fraction).abs() < 1e-12);
    }
}

#[test]
fn r14_oracle_card_diagnostics() {
    let run_jfet = |card| {
        let text = format!(
            "input recognition\nVd d 0 1\nVg g 0 -1\nJ1 d g 0 j\n.model j njf(level=2 {card})\n.op\n.save i(Vd)\n"
        );
        sansho_engine::run(&sansho_netlist::parse(&text).unwrap()).unwrap()
    };
    let invalid = sansho_netlist::parse(include_str!("../../../verify/probes/r14b_jfet2_id.cir"));
    assert!(
        invalid
            .unwrap_err()
            .iter()
            .any(|d| { d.code == "E-PARSE-EXPR" && d.message.contains("pf1") })
    );
    let clean = run_jfet("");
    let ignored = run_jfet("afac=3 nfing=3 cdss=1n ld=2n lg=1n ls=2n rg=10");
    for name in ["afac", "nfing", "cdss", "ld", "lg", "ls", "rg"] {
        assert!(
            ignored
                .warnings
                .iter()
                .any(|w| w.code == "W-MODEL-IGNORED" && w.message.contains(name))
        );
    }
    assert_eq!(
        clean.results.analyses[0].real("i(vd)"),
        ignored.results.analyses[0].real("i(vd)")
    );
    for key in ["alpha", "xd", "input_delta"] {
        let text =
            format!("invalid card\nVd d 0 1\nM1 d d 0 0 n\n.model n nmos(level=9 {key}=0)\n.op\n");
        let err = sansho_engine::check(&sansho_netlist::parse(&text).unwrap()).unwrap_err();
        assert!(
            err.iter()
                .any(|d| d.code == "E-ELAB-MODEL" && d.message.contains(key))
        );
    }
    assert!(sansho_netlist::parse("invalid option\n.options numdgt=4\n.end\n").is_err());
}

#[test]
fn early_bsim_gate_and_drain_excitation_match_measured_charge_derivatives() {
    let gate = include_str!("../../../verify/probes/r14b_bsim2_gate_charge.cir");
    let drain = include_str!("../../../verify/probes/r14b_bsim2_triode_charge.cir");
    let first = include_str!("../../../verify/probes/r14b_bsim1_gate_charge.cir");
    for (deck, vg, expected_ff) in [
        (first, ".6", [1.516361, 0., 0., -1.516361]),
        (first, ".8", [4.742555, 0., -4.419840, -0.322715]),
        (gate, "0", [6.629760, 0., 0., -6.629760]),
        (gate, ".2", [0.289121, 0., 0., -0.289121]),
        (gate, ".4", [4.419840, -1.767936, -2.651904, 0.]),
        (drain, "1.4", [-6.026948, 4.181604, 1.818825, 0.026519]),
    ] {
        let text = deck.replace(".param probe=0", &format!(".param probe={vg}"));
        let run = sansho_engine::run(&sansho_netlist::parse(&text).unwrap()).unwrap();
        let ac = &run.results.analyses[0];
        let omega = std::f64::consts::TAU * ac.scale_values[0];
        for (port, expected) in ["i(vg)", "i(vd)", "i(vs)", "i(vb)"]
            .into_iter()
            .zip(expected_ff)
        {
            let measured = -ac.complex(port).unwrap()[0].1 * 1e15 / omega;
            assert!(
                (measured - expected).abs() < 1e-6,
                "{port} at Vg={vg}: {measured} fF"
            );
        }
    }
}
