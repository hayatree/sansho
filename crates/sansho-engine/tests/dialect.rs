//! Netlist dialect features end to end: parameters, subcircuits, libraries, binning,
//! `.ic`, `.nodeset` and UIC.
//!
//! Provenance: original test circuits; expected values follow from circuit theory.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use sansho_engine::run;
use sansho_netlist::{SourceLoader, parse, parse_with};
use sansho_output::SimResults;

fn simulate(src: &str) -> SimResults {
    let nl = parse(src).unwrap_or_else(|d| panic!("parse failed: {d:#?}"));
    run(&nl)
        .unwrap_or_else(|e| panic!("{:#?}", e.diagnostics))
        .results
}

fn op(r: &SimResults, name: &str) -> f64 {
    r.analyses[0].real(name).unwrap()[0]
}

#[test]
fn parameterized_subcircuit_divider() {
    let r = simulate(
        "div\n.param rtop=3k gain='1/4'\n\
         .subckt div in out params: r1=1k r2={r1*gain/(1-gain)}\nR1 in out {r1}\nR2 out 0 {r2}\n.ends\n\
         V1 a 0 4\nX1 a b div r1=rtop\nX2 a c div\n.op\n",
    );
    assert!((op(&r, "v(b)") - 1.0).abs() < 1e-12);
    assert!((op(&r, "v(c)") - 1.0).abs() < 1e-12);
}

#[test]
fn internal_subcircuit_nodes_and_branches_are_named_hierarchically() {
    let r = simulate(
        "h\n.subckt cell a b\nR1 a m 1k\nR2 m b 1k\nVs m 0 0.5\n.ends\nV1 x 0 1\nX1 x 0 cell\n.op\n",
    );
    assert!((op(&r, "v(x1.m)") - 0.5).abs() < 1e-12);
    assert!((op(&r, "i(v.x1.vs)") - 0.0).abs() < 1e-6);
}

#[test]
fn binned_models_are_selected_by_length() {
    let src = "bins\nVD d 0 1\nVG g 0 1\nMS d g 0 0 n l=0.5u w=1u\nML d g 0 0 n l=2u w=1u\n\
               .model n.1 nmos lmin=0.1u lmax=1u vto=0.2 kp=100u\n\
               .model n.2 nmos lmin=1u lmax=10u vto=0.6 kp=100u\n.op\n";
    let nl = parse(src).unwrap();
    let models: Vec<&str> = nl.elements[2..]
        .iter()
        .map(|e| e.kind.model().unwrap())
        .collect();
    assert_eq!(models, ["n.1", "n.2"]);
    let bad = parse(&src.replace("l=2u", "l=20u")).unwrap_err();
    assert!(bad.iter().any(|d| d.code == "E-PARSE-MODEL"));
}

#[test]
fn nodeset_selects_the_latch_state() {
    let latch = "latch\nVDD vdd 0 1.8\nMP1 q qb vdd vdd p l=1u w=2u\nMN1 q qb 0 0 n l=1u w=1u\n\
                 MP2 qb q vdd vdd p l=1u w=2u\nMN2 qb q 0 0 n l=1u w=1u\n\
                 .model n nmos vto=0.5 kp=100u\n.model p pmos vto=-0.5 kp=40u\n.op\n";
    let hi = simulate(&format!("{latch}.nodeset v(q)=1.8 v(qb)=0\n"));
    assert!(op(&hi, "v(q)") > 1.79 && op(&hi, "v(qb)") < 0.01);
    let lo = simulate(&format!("{latch}.nodeset v(q)=0 v(qb)=1.8\n"));
    assert!(op(&lo, "v(q)") < 0.01 && op(&lo, "v(qb)") > 1.79);
}

#[test]
fn initial_conditions_with_and_without_uic() {
    // RC discharge from 1 V: with UIC the capacitor starts at its .ic value.
    let r = simulate("rc\nR1 a 0 1k\nC1 a 0 1n\n.ic v(a)=1\n.tran 10n 3u uic\n");
    let a = &r.analyses[0];
    let (t, v) = (&a.scale_values, a.real("v(a)").unwrap());
    let k = t.iter().position(|&x| x >= 1e-6).unwrap();
    assert!(
        (v[k] - (-t[k] / 1e-6).exp()).abs() < 5e-3,
        "{} at {}",
        v[k],
        t[k]
    );
    // Without UIC, .ic holds the node during the operating point.
    let r = simulate("rc\nV1 s 0 0\nR1 s a 1k\nC1 a 0 1n\n.ic v(a)=1\n.tran 10n 3u\n");
    assert!((r.analyses[0].real("v(a)").unwrap()[0] - 1.0).abs() < 1e-6);
}

struct Mem;

impl SourceLoader for Mem {
    fn load(&self, path: &str, _from: Option<&str>) -> Result<(String, String), String> {
        match path {
            "lib.spi" => Ok((
                path.into(),
                ".lib slow\n.param r=2k\n.endl\n.lib fast\n.param r=500\n.endl\n".into(),
            )),
            "cell.inc" => Ok((path.into(), "R2 out 0 {r}\n".into())),
            _ => Err(format!("no such file `{path}`")),
        }
    }
}

#[test]
fn libraries_and_includes_through_a_loader() {
    let nl = parse_with(
        "t\n.lib lib.spi fast\nV1 in 0 1\nR1 in out 500\n.include cell.inc\n.op\n",
        &Mem,
    )
    .unwrap();
    assert_eq!(nl.sources, ["", "lib.spi", "cell.inc"]);
    let r = run(&nl).unwrap().results;
    assert!((op(&r, "v(out)") - 0.5).abs() < 1e-12);
    let e = parse_with("t\n.lib lib.spi typ\n", &Mem).unwrap_err();
    assert_eq!(e[0].code, "E-PARSE-INCLUDE");
}

#[test]
fn coupled_decay_has_two_time_constants_in_both_integrators() {
    for method in ["trap", "gear"] {
        let r = simulate(&format!(
            "two windings\nK1 La Lb -0.5\nLa a 0 1m ic=1m\nLb b 0 1m\nRa a 0 10\nRb b 0 10\n.options method={method}\n.tran 1u 200u uic\n"
        ));
        let a = &r.analyses[0];
        let (ia, ib) = (a.real("i(la)").unwrap(), a.real("i(lb)").unwrap());
        for (i, &t) in a.scale_values.iter().enumerate() {
            assert!((ia[i] + ib[i] - 1e-3 * (-t / 50e-6).exp()).abs() < 2e-6);
            assert!((ia[i] - ib[i] - 1e-3 * (-t / 150e-6).exp()).abs() < 2e-6);
        }
    }
}

#[test]
fn several_couplings_induce_signed_open_circuit_voltages() {
    let r = simulate(
        "sense\nI1 0 a AC 1\nLa a 0 4m\nLb b 0 1m\nLc c 0 1m\nKab La Lb 0.5\nKac La Lc -0.25\n.ac lin 1 1k 1k\n",
    );
    for (signal, m) in [("v(b)", 1e-3), ("v(c)", -0.5e-3)] {
        let (re, im) = r.analyses[0].complex(signal).unwrap()[0];
        assert!(re.abs() < 1e-12 && (im - std::f64::consts::TAU * 1e3 * m).abs() < 1e-12);
    }
}

#[test]
fn behavioral_sources_bind_lexical_functions_and_linearize_at_bias() {
    let src = "probes\n.param gain=3\n.func sq(x) {x*x}\n.subckt cell p out\nVsense p local 0\nRin local 0 1k\nBprod out 0 V={gain*sq(v(local,0))+100*i(Vsense)}\n.ends\nVa 01 0 dc 2 ac 1\nX1 01 out cell\nEclip copy 0 table={v(01)} (0,0) (3,6)\nG1 0 feedback value={v(copy)*1m}\nR1 feedback 0 1k\n.op\n.ac lin 1 1 1\n";
    let nl = parse(src).unwrap();
    let r = simulate(&sansho_netlist::write(&nl));
    assert!((op(&r, "v(out)") - 12.2).abs() < 1e-12);
    assert!((r.analyses[1].complex("v(out)").unwrap()[0].0 - 12.1).abs() < 1e-12);
    assert!((r.analyses[1].complex("v(feedback)").unwrap()[0].0 - 2.).abs() < 1e-12);
}

#[test]
fn switches_remember_their_state_across_a_sweep() {
    let r = simulate(
        "switch\nVb src 0 1\nVc gate 0 0\nRc gate 0 1k\nS1 src a gate 0 sm on\nRa a 0 1k\nW1 src b Vc wm on\nRb b 0 1k\n.model sm SW(ron=10 roff=100k vt=1 vh=0.4)\n.model wm CSW(ron=10 roff=100k it=-1m ih=0.4m)\n.dc Vc 1.2 0.2 -0.2\n",
    );
    let a = &r.analyses[0];
    let out = a.real("v(a)").unwrap();
    assert!(out[0] > 0.99 && out[2] > 0.99 && out[5] < 0.01);
    let b = a.real("v(b)").unwrap();
    assert!(b.iter().all(|v| *v > 0.99));
}

#[test]
fn capacitor_ic_is_differential_and_only_applies_under_uic() {
    let src = "charge\nC1 a b 1n ic=-0.75\nRa a 0 1k\nRb b 0 1k\n.tran 10n 2u";
    let r = simulate(&format!("{src} uic\n"));
    let a = &r.analyses[0];
    let vp = a.real("v(a)").unwrap();
    let vn = a.real("v(b)").unwrap();
    for (i, t) in a.scale_values.iter().enumerate() {
        assert!((vp[i] - vn[i] + 0.75 * (-t / 2e-6).exp()).abs() < 1e-3);
    }
    assert_eq!(op(&simulate(src), "v(a)"), 0.);
}

#[test]
fn temperature_sweep_uses_nominal_and_instance_temperatures() {
    let r = simulate(
        "temperature\nV1 in 0 1\nR1 in 0 rmod 1k tc2=100u\n.model rmod R(tc1=0.01 tnom=10)\nV2 keep 0 1\nR2 keep 0 1k temp=10 tc1=0.01\n.options tnom=10\nB1 t 0 V={temper}\n.dc temp 10 30 10\n",
    );
    let a = &r.analyses[0];
    assert_eq!(a.scale_values, vec![10., 20., 30.]);
    for (i, &t) in a.scale_values.iter().enumerate() {
        let d = t - 10.;
        assert!((a.real("i(v1)").unwrap()[i] + 1e-3 / (1. + d * (0.01 + 1e-4 * d))).abs() < 1e-14);
        assert!((a.real("i(v2)").unwrap()[i] + 1e-3).abs() < 1e-14);
        assert!((a.real("v(t)").unwrap()[i] - t).abs() < 1e-12);
    }
}

#[test]
fn semiconductor_temperature_sweeps_match_junction_laws() {
    let r = simulate(
        "temperature\nVd d 0 0.5\nD1 d 0 dd\n.model dd D(is=1e-14 n=1.2 eg=1.1 xti=2 tnom=20)\nIb 0 b 1u\nVc c 0 2\nQ1 c b 0 q\n.model q npn(bf=100 xtb=2 tnom=20)\n.dc temp 20 80 20\n",
    );
    let a = &r.analyses[0];
    for (i, &c) in a.scale_values.iter().enumerate() {
        let t = c + 273.15;
        let ratio = t / 293.15;
        let vt = sansho_devices::thermal_voltage(t);
        let is = 1e-14 * ((2. * ratio.ln() + 1.1 * (ratio - 1.) / vt) / 1.2).exp();
        let expected = is * (0.5 / (1.2 * vt)).exp_m1() + 0.5e-12;
        assert!((a.real("i(vd)").unwrap()[i] + expected).abs() < 1e-9 * expected);
        assert!((a.real("i(vc)").unwrap()[i] + 100e-6 * ratio.powi(2)).abs() < 1e-9);
    }
}

#[test]
fn scaled_mos_matches_si_dimensions_in_dc_and_ac() {
    let deck = include_str!("../../../verify/decks/mos_geometry_scale.cir");
    let scaled = simulate(deck);
    let si = simulate(&deck.replace(".options scale=1u", "").replace(
        "L=2 W=6 AD=6 AS=6 PD=14 PS=14",
        "L=2u W=6u AD=6p AS=6p PD=14u PS=14u",
    ));
    for (x, y) in scaled.analyses[0]
        .complex("v(d)")
        .unwrap()
        .iter()
        .zip(si.analyses[0].complex("v(d)").unwrap())
    {
        assert!((x.0 - y.0).abs() < 1e-12 && (x.1 - y.1).abs() < 1e-12);
    }
}

#[test]
fn thermal_junction_capacitance_and_mos_mobility() {
    // At zero bias the diode's imaginary admittance is omega times Cj0(T).
    let src = "junction\nVd d 0 dc 0 ac 1\nD1 d 0 dd\n.model dd D(cjo=20p vj=0.8 m=0.5 tnom=27)\n.ac lin 1 1meg 1meg\n";
    let capacitance = |temp| {
        -simulate(&format!("{src}.temp {temp}\n")).analyses[0]
            .complex("i(vd)")
            .unwrap()[0]
            .1
            / (1e6 * std::f64::consts::TAU)
    };
    assert!((capacitance(27.) - 20e-12).abs() < 1e-25);
    let hot = capacitance(77.);
    let t = 350.15;
    let nom = 300.15;
    let gap = |k: f64| 1.16 - 7.02e-4 * k * k / (k + 1108.);
    let ratio = t / nom;
    let potential = ratio * 0.8 + gap(t)
        - ratio * gap(nom)
        - 3. * sansho_devices::thermal_voltage(t) * ratio.ln();
    assert!((hot - 20e-12 * (1. + 0.5 * (4e-4 * 50. - (potential / 0.8 - 1.)))).abs() < 1e-24);
    let r = simulate(include_str!("../../../verify/decks/mos_temperature.cir"));
    let a = &r.analyses[0];
    let vt = sansho_devices::thermal_voltage;
    for (i, &temp) in a.scale_values.iter().enumerate() {
        let (t, nom) = (temp + 273.15, 303.15);
        let ratio = t / nom;
        let phi = ratio * 0.65 + gap(t) - ratio * gap(nom) - 3. * vt(t) * ratio.ln();
        let threshold = 0.6
            + 0.5 * (gap(nom) - gap(t) + phi - 0.65)
            + 0.4 * ((phi + 0.3).sqrt() - 0.65_f64.sqrt());
        let id = 0.5 * 35e-6 / ratio.powf(1.5) * 4. * (1.2 - threshold).powi(2);
        assert!((a.real("i(vd)").unwrap()[i] + id).abs() < 1e-10);
    }
}

#[test]
fn transient_switch_crossings_follow_time_and_preserve_hysteresis() {
    let r = simulate(
        "crossings\nVd rail 0 1\nBcontrol ctl 0 V={time<1m ? time*1000 : 2-time*1000}\nS1 rail out ctl 0 sw\nR1 out 0 1k\n.model sw sw(vt=0.5 vh=0.1 ron=1 roff=1meg)\n.tran 87u 2m\n",
    );
    let a = &r.analyses[0];
    let out = a.real("v(out)").unwrap();
    let mut transitions = vec![];
    for (i, pair) in out.windows(2).enumerate() {
        if (pair[0] > 0.5) != (pair[1] > 0.5) {
            transitions.push(a.scale_values[i + 1]);
        }
    }
    assert_eq!(transitions.len(), 2);
    for (time, expected) in transitions.into_iter().zip([0.6e-3, 1.6e-3]) {
        assert!((time - expected).abs() < 1e-7, "{time}");
    }
}

#[test]
fn bsim_temperature_sweep_rebinds_the_model_at_each_bias() {
    let deck =
        include_str!("../../../verify/bsim4-decks/b4_n_temp.cir").replace("vg g 0 0", "vg g 0 1");
    let sweep = simulate(&deck.replace(".dc vg -1.2 1.2 10m", ".dc temp 10 110 50"));
    let id = sweep.analyses[0].real("i(vd)").unwrap();
    assert!((id[0] - id[2]).abs() > 1e-6);
    for (i, t) in [10, 60, 110].into_iter().enumerate() {
        let fixed = simulate(
            &deck
                .replace(".temp 110", &format!(".temp {t}"))
                .replace(".dc vg -1.2 1.2 10m", ".op"),
        );
        assert!((id[i] - op(&fixed, "i(vd)")).abs() < 1e-12);
    }
}
