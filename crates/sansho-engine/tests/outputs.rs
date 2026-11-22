//! End-to-end netlist output, sweep, UIC and stochastic-source checks.
//! Provenance: original circuits and analytic expectations from Ohm's law,
//! charge continuity, small-signal linearization and SPICE element conventions.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sansho_engine::{RunConfig, run, run_with};
use sansho_netlist::{parse, write as write_netlist};
use sansho_output::{Data, SimResults};

fn simulate(text: &str) -> SimResults {
    run(&parse(text).unwrap()).unwrap().results
}
fn near(x: f64, y: f64, tol: f64) {
    assert!((x - y).abs() <= tol, "{x} vs {y}, tolerance {tol}");
}

#[test]
fn behavioral_source_columns_match_the_black_box_captures() {
    let cases = [
        (
            include_str!("../../../verify/probes/r22c_order_families.cir"),
            "v(vin) v(h1) v(b1) v(l1) v(e1) v(vs) v(e2) v(l2) v(b2) v(h2) i(blate) i(bearly) i(vlate) i(hlate) i(vearly) i(hearly) i(llate) i(learly) i(elate) i(eearly)",
        ),
        (
            include_str!("../../../verify/probes/r22b_order_terminals.cir"),
            "v(p) v(n) v(a) v(z) v(01) v(middle) i(vz) i(va) i(vone)",
        ),
        (
            include_str!("../../../verify/probes/r22b_order_branch.cir"),
            "v(out) v(z) v(a) v(middle) v(sense) i(vsense)",
        ),
        (
            include_str!("../../../verify/probes/r22b_order_func.cir"),
            "v(first) v(out) v(body) v(arg) v(middle) v(unused) i(vb) i(va)",
        ),
        (
            include_str!("../../../verify/probes/r22b_order_eg_value.cir"),
            "v(eo) v(en) v(efirst_int1) v(a) v(z) v(go) v(gn) v(gnext_int1) v(y) v(middle) i(bgnext) i(befirst) i(efirst) i(vz) i(va) i(vy)",
        ),
        (
            include_str!("../../../verify/probes/r22b_order_subckt.cir"),
            "v(one) v(ctl) v(x1.local) v(x1.middle) v(two) v(x2.local) v(x2.middle) i(vc) i(v.x2.vl) i(v.x1.vl)",
        ),
        (
            include_str!("../../../verify/probes/r22b_order_expr_only.cir"),
            "v(out) v(ghost) v(middle)",
        ),
    ];
    for (text, expected) in cases {
        let plot = simulate(text).analyses.remove(0);
        let names: Vec<_> = plot.variables.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, expected.split_whitespace().collect::<Vec<_>>());
        if let Some(ghost) = plot.real("v(ghost)") {
            assert_eq!(ghost, [0.]);
        }
        if let Some(driver) = plot.real("v(efirst_int1)") {
            near(driver[0], 2., 1e-14);
            near(plot.real("v(gnext_int1)").unwrap()[0], -0.003, 1e-14);
            near(plot.real("v(eo)").unwrap()[0], 1., 1e-14);
            near(plot.real("v(go)").unwrap()[0], 3., 1e-14);
            near(plot.real("i(efirst)").unwrap()[0], -0.001, 1e-14);
            near(plot.real("i(befirst)").unwrap()[0], 0., 1e-14);
            near(plot.real("i(bgnext)").unwrap()[0], 0., 1e-14);
        }
    }
}

#[test]
fn batch_raw_plot_and_mixed_save_order() {
    let r = simulate(include_str!("../../../verify/probes/r22c_order_save.cir"));
    let mut raw = Vec::new();
    sansho_output::write_raw_ascii(&r, &mut raw).unwrap();
    let decoded = sansho_output::read_raw_ascii(std::str::from_utf8(&raw).unwrap()).unwrap();
    assert_eq!(decoded.analyses[0].kind, sansho_output::AnalysisKind::Ac);
    assert_eq!(decoded.analyses[1].kind, sansho_output::AnalysisKind::Op);
    for plot in &decoded.analyses {
        let names: Vec<_> = plot.variables.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            ["i(vfirst)", "v(beta)", "v(alpha)", "i(vlast)", "@rfirst[i]"]
        );
    }
    assert_eq!(decoded.analyses[0].complex("v(beta)").unwrap(), [(4., 0.)]);
    near(
        decoded.analyses[1].real("@rfirst[i]").unwrap()[0],
        0.001,
        1e-16,
    );
}

#[test]
fn both_raw_writers_match_the_materialized_batch_projection() {
    for deck in [
        include_str!("../../../verify/decks/r22d_order_noise_summary.cir"),
        include_str!("../../../verify/decks/r22d_order_save_bias.cir"),
        include_str!("../../../verify/decks/r22d_order_plots_forward.cir"),
    ] {
        let full = simulate(deck);
        let projected = full.clone().into_raw_output();
        type Writer = fn(&SimResults, &mut Vec<u8>) -> std::io::Result<()>;
        let writers: [Writer; 2] = [
            sansho_output::write_raw_ascii,
            sansho_output::write_raw_binary,
        ];
        for write in writers {
            let mut direct = vec![];
            let mut materialized = vec![];
            write(&full, &mut direct).unwrap();
            write(&projected, &mut materialized).unwrap();
            assert_eq!(direct, materialized);
        }
    }
}

#[test]
fn explicit_save_keeps_the_solved_diode_access_voltage() {
    let circuit = "diode access\nI1 0 n 1m\nD1 n 0 dj\n.model dj d(is=1e-14 rs=50)\n.op\n";
    let r = simulate(circuit);
    let a = &r.analyses[0];
    let junction = a.real("v(d1#a)").unwrap()[0];
    near(a.real("v(n)").unwrap()[0] - junction, 0.05, 1e-10);
    assert!(r.into_raw_output().analyses[0].real("v(d1#a)").is_none());
    let saved = simulate(&format!("{circuit}.save v(d1#a)\n")).into_raw_output();
    assert_eq!(saved.analyses[0].real("v(d1#a)").unwrap(), [junction]);
}

#[test]
fn parameter_voltage_probes_fail_at_the_declaration() {
    for text in [
        include_str!("../../../verify/probes/r22b_order_param_unused.cir"),
        include_str!("../../../verify/probes/r22b_order_param_used.cir"),
    ] {
        let errors = parse(text).unwrap_err();
        assert!(errors.iter().any(|d| d.code == "E-PARSE-EXPR"
            && d.message.contains("node voltages")
            && d.span.is_some()));
    }
}

#[test]
fn printed_and_measured_vectors_precede_projection() {
    let src = "test\n.title Source and load\nV1 n 0 2\nR1 n 0 2k\n.dc V1 1 2 0.5\n.print dc v(n) @r1[i] @v1[p]\n.meas dc power find @r1[p] at=2\n.save @r1[i]\n.end";
    let nl = parse(src).unwrap();
    let res = run(&nl).unwrap().results;
    assert_eq!(res.title, "Source and load");
    assert_eq!(res.prints[0].columns[2], [0.0005, 0.00075, 0.001]);
    near(res.prints[0].columns[3][2], -0.002, 1e-15);
    near(res.measures[0].value.unwrap(), 0.002, 1e-15);
    assert_eq!(res.measures[0].unit, "W");
    assert_eq!(res.analyses[0].variables.len(), 1);
    assert_eq!(simulate(&write_netlist(&nl)), res);
    assert!(
        run(&parse(&src.replace(".save @r1[i]", ".save @r1[gm]")).unwrap())
            .unwrap_err()
            .diagnostics
            .iter()
            .any(|d| d.code == "E-OUTPUT-VECTOR")
    );
}

#[test]
fn noise_spectrum_measure_and_print() {
    let r = simulate(
        "spectrum\nV1 a 0 ac 1\nR1 a b 1k\nR2 b 0 1k\n.noise v(b) V1 dec 3 10 1k\n.meas noise density find onoise_spectrum at=100\n.print noise inoise_spectrum\n.end",
    );
    let a = &r.analyses[0];
    near(
        r.measures[0].value.unwrap(),
        a.real("onoise_spectrum").unwrap()[0],
        1e-20,
    );
    assert_eq!(r.measures[0].unit, "V/√Hz");
    assert_eq!(r.prints[0].columns[1], a.real("inoise_spectrum").unwrap());
}

#[test]
fn differential_device_currents_and_ac_resistance() {
    let r = simulate(
        "AC sense\nV1 p 0 dc 3 ac 2 90\nR1 p 0 1k ac=2k\nC1 p 0 1n\n.options savecurrents\n.ac lin 1 1k 1k\n.op\n.end",
    );
    let ac = &r.analyses[0];
    near(ac.complex("@r1[i]").unwrap()[0].1, 0.001, 1e-16);
    near(
        ac.complex("@c1[i]").unwrap()[0].0,
        -2e-6 * std::f64::consts::TAU,
        1e-16,
    );
    assert_eq!(ac.complex("i(v1)"), ac.complex("@v1[i]"));
    near(r.analyses[1].real("@r1[i]").unwrap()[0], 0.003, 1e-15);
    let r = simulate(
        "current\nV1 n 0 pwl(0 0 1u 1)\nC1 n 0 2n\n.tran 20n 1u\n.save @c1[i] i(v1)\n.end",
    );
    for (&a, &b) in r.analyses[0]
        .real("@c1[i]")
        .unwrap()
        .iter()
        .zip(r.analyses[0].real("i(v1)").unwrap())
        .skip(1)
    {
        near(a, -b, 1e-10);
    }
}

#[test]
fn transistor_operating_quantities() {
    for level in [1, 2, 3, 49, 54] {
        let src = format!(
            "MOS outputs\nVd d 0 1.2\nVg g 0 1.4\nM1 d g 0 0 mn w=10u l=1u\n.model mn nmos(level={level})\n.op\n.save @m1[id] @m1[gm] @m1[gds] @m1[gmbs] @m1[vth] @m1[vdsat] @m1[cgg] @m1[cgd] @m1[cgs]\n.end"
        );
        let a = simulate(&src).analyses.remove(0);
        assert_eq!(a.variables.len(), 9);
        assert!(a.real("@m1[gm]").unwrap()[0] > 0.);
        let dx = 1e-5;
        let current = |vg: f64| {
            simulate(&src.replace("Vg g 0 1.4", &format!("Vg g 0 {vg}"))).analyses[0]
                .real("@m1[id]")
                .unwrap()[0]
        };
        let fd = (current(1.4 + dx) - current(1.4 - dx)) / (2. * dx);
        near(a.real("@m1[gm]").unwrap()[0], fd, 1e-5 * fd.abs() + 1e-13);
        if level >= 49 {
            let s = src.replace("@m1[id]", "@m1[cgb] @m1[qg] @m1[qd] @m1[qs] @m1[qb]");
            let a = simulate(&s).analyses.remove(0);
            let val = |p| a.real(&format!("@m1[{p}]")).unwrap()[0];
            near(val("cgg") + val("cgs") + val("cgd") + val("cgb"), 0., 1e-25);
            near(val("qg") + val("qd") + val("qs") + val("qb"), 0., 1e-25);
        }
    }
}

#[test]
fn device_initial_conditions() {
    let ic = include_str!("../../../verify/decks/r7_device_uic.cir");
    let diode = simulate(include_str!("../../../verify/decks/r7_diode_uic.cir"));
    let junction = diode.analyses[0].real("v(a)").unwrap();
    near(junction[0], 0.2, 1e-14);
    assert!(junction[1] > 0. && junction[1] < junction[0]);
    let r = simulate(ic);
    for (node, v) in [("b", 0.2), ("c", 0.4), ("jg", -0.1), ("near", 0.2)] {
        near(
            r.analyses[0].real(&format!("v({node})")).unwrap()[0],
            v,
            1e-14,
        );
    }
}

#[test]
fn mos_overlap_uic_resolves_both_decay_modes() {
    let mos = simulate(include_str!("../../../verify/decks/r7_mos_uic.cir"));
    let tran = &mos.analyses[0];
    let drain = tran.real("v(d)").unwrap();
    let gate = tran.real("v(g)").unwrap();
    near(drain[0], 0.1, 1e-14);
    near(gate[0], 0.2, 1e-14);

    // In drain/gate order C is 1.6 fF times [[1,-1],[-1,2]].
    // Its eigenvectors [1, 1-lambda] decay with tau = 10 kΩ * 1.6 fF * lambda.
    let fast = (3. - 5_f64.sqrt()) / 2.;
    let slow = (3. + 5_f64.sqrt()) / 2.;
    let fast_amplitude = (0.2 - (1. - slow) * 0.1) / (slow - fast);
    for ((&t, &d), &g) in tran.scale_values.iter().zip(drain).zip(gate) {
        let f = fast_amplitude * (-t / (16e-12 * fast)).exp();
        let s = (0.1 - fast_amplitude) * (-t / (16e-12 * slow)).exp();
        near(d, f + s, 3e-4);
        near(g, (1. - fast) * f + (1. - slow) * s, 2e-4);
    }
}

#[test]
fn stochastic_repeatability() {
    let deck = "random\nV1 a 0 trnoise(0.1 1u 1 0.03 0.5 2u 5u)\nI1 0 b trrandom(2 2u 3u 0.1 1)\nR1 b 0 1\n.tran 0.2u 30u\n.tran 0.3u 15u\n.options seed=730\n.end";
    let nl = parse(deck).unwrap();
    let run_n = |n| run_with(&nl, &RunConfig { threads: n }).unwrap().results;
    assert_eq!(run_n(1), run_n(2));
    assert_eq!(run_n(1), simulate(&write_netlist(&nl)));
    assert_ne!(
        run_n(1).analyses,
        simulate(&deck.replace("seed=730", "seed=732")).analyses
    );
}

#[test]
fn behavioral_storage_and_nested_resistance() {
    let ramp =
        "ramp\nV1 n 0 pwl(0 0 1u 1)\nC1 n 0 C='1n*(1+v(n))'\n.tran 20n 1u\n.save i(v1)\n.end";
    let a = simulate(ramp).analyses.remove(0);
    for (&t, &i) in a.scale_values.iter().zip(a.real("i(v1)").unwrap()).skip(2) {
        near(i, -1e-3 * (1. + t * 1e6), 1e-8);
    }
    let r = simulate(include_str!("../../../verify/decks/r7_resistor_sweep.cir"));
    assert_eq!(
        r.analyses[0].scale_values,
        [500., 1000., 1500., 500., 1000., 1500.]
    );
    assert!(matches!(r.analyses[0].data, Data::Real { .. }));
    near(r.prints[0].columns[1][0], 4. / 3., 1e-12);
    near(r.prints[0].columns[1][3], 2. * 1044. / 1544., 1e-12);
}

#[test]
fn nested_sweep_keeps_resistor_temperature_and_parallel_instances() {
    let r = simulate(
        "resistor\nV1 n 0 1\nR1 n 0 1k m=2 tc1=0.01\n.dc temp 27 37 10 R1 1k 2k 1k\n.save @r1[i]\n.end",
    );
    for (&v, i) in
        r.analyses[0]
            .real("@r1[i]")
            .unwrap()
            .iter()
            .zip([0.002, 2. / 1100., 0.001, 1. / 1100.])
    {
        near(v, i, 1e-16);
    }
}

#[test]
fn typed_device_currents_and_derivatives() {
    let r = simulate(
        "bias\nV1 d 0 1\nV2 g 0 0.5\nD1 g 0 dd\n.model dd d(is=1e-14 cjo=1p)\nQ1 d g 0 np\n.model np npn(bf=80 cje=2p cjc=1p)\nJ1 d 0 0 nj\n.model nj njf(vto=-1 beta=2m lambda=0.02)\nG1 d 0 g 0 1m\nE1 e 0 d 0 0.1\nRe e 0 10\nB1 b 0 I={v(g)*2m}\nRb b 0 1k\n.op\n.options savecurrents\n.save all @d1[gd] @d1[cd] @q1[gm] @q1[gpi] @q1[gmu] @q1[go] @q1[cpi] @q1[cmu] @j1[gm] @j1[gds]\n.end",
    );
    let v = |s| r.analyses[0].real(s).unwrap()[0];
    near(v("@e1[i]"), -0.01, 1e-15);
    near(v("@g1[i]"), 0.0005, 1e-15);
    near(v("@b1[i]"), 0.001, 1e-15);
    near(
        v("@q1[ic]") + v("@q1[ib]") + v("@q1[ie]") + v("@q1[is]"),
        0.,
        1e-16,
    );
    for key in [
        "@d1[gd]", "@d1[cd]", "@q1[gpi]", "@q1[cpi]", "@q1[cmu]", "@j1[gm]", "@j1[gds]",
    ] {
        assert!(v(key) > 0., "{key}");
    }
}

#[test]
fn integration_options_and_cosmetic_cards() {
    let nl=parse("flags\nV1 n 0 sin(0 1 1k)\nR1 n o 1k\nC1 o 0 1n\n.tran 1u 50u\n.plot tran v(o)\n.width out=100\n.options method=trapezoidal itl5=0 interp unsupportedflag\n.end").unwrap();
    let checked = sansho_engine::check(&nl).unwrap();
    assert_eq!(checked.options.method, sansho_engine::Method::Trap);
    for code in ["W-PLOT-IGNORED", "W-OPT-INTERP", "W-OPT-IGNORED"] {
        assert!(checked.warnings.iter().any(|d| d.code == code));
    }
    let gear = write_netlist(&nl).replace("method=trapezoidal", "method=gear");
    let one = simulate(&gear.replace("method=gear", "method=gear maxord=1"));
    let two = simulate(&gear.replace("method=gear", "method=gear maxord=2"));
    assert_ne!(one.analyses[0].scale_values, two.analyses[0].scale_values);
}

#[test]
fn junction_ic_charge_is_independent_for_parallel_devices() {
    let r = simulate(
        "charge sharing\nD1 n 0 dd ic=-1\nD2 n 0 dd ic=-0.2\nR1 n 0 1g\n.model dd d(is=1e-16 cjo=1n m=0)\n.tran 10p 1n uic\n.save v(n) @d1[id] @d2[id]\n.end",
    );
    let a = &r.analyses[0];
    // Equal constant depletion capacitances redistribute their stored charge.
    near(a.real("v(n)").unwrap()[1], -0.6, 1e-9);
    let current = |s| a.real(s).unwrap()[1];
    near(
        current("@d1[id]") + current("@d2[id]") + current("v(n)") / 1e9,
        0.,
        1e-8,
    );
}

#[test]
fn behavioral_control_current_time_temperature_and_ac_override() {
    let src = "controlled resistor\nVs x 0 dc 1 ac 1\nRref x 0 1k\nVload y 0 dc 1 ac 1\nRload y 0 R='1000*(1-i(Vs)+time*1meg+temper/100)' m=2 tc1=0.01 ac=4k\n.temp 37\n.op\n.ac lin 1 1k 1k\n.tran 1u 5u\n.save @rload[i] @rload[p]\n.end";
    let nl = parse(src).unwrap();
    let r = run(&nl).unwrap().results;
    let conductance = |t| 2. / (1000. * (1. + 0.001 + t * 1e6 + 0.37) * 1.1);
    near(
        r.analyses[0].real("@rload[i]").unwrap()[0],
        conductance(0.),
        1e-14,
    );
    near(
        r.analyses[1].complex("@rload[i]").unwrap()[0].0,
        0.0005,
        1e-14,
    );
    near(
        r.analyses[1].complex("@rload[p]").unwrap()[0].0,
        conductance(0.),
        1e-14,
    );
    let a = &r.analyses[2];
    for (&t, &i) in a.scale_values.iter().zip(a.real("@rload[i]").unwrap()) {
        near(i, conductance(t), 1e-14);
    }
    assert_eq!(simulate(&write_netlist(&nl)), r);
}

#[test]
fn controlled_inductance_uses_differential_flux_law() {
    let r = simulate(
        "coil\nI1 0 n pwl(0 0 10u 1m)\nL1 n 0 L='1m*(1+time*1meg)'\n.tran 100n 10u\n.save v(n) @l1[i]\n.end",
    );
    let a = &r.analyses[0];
    for (&t, &v) in a.scale_values.iter().zip(a.real("v(n)").unwrap()).skip(2) {
        near(v, 0.1 * (1. + t * 1e6), 1e-7);
    }
}

#[test]
fn raw_output_preserves_internal_names_and_quantity() {
    let r = simulate(include_str!("../../../verify/decks/r7_mos_observables.cir"));
    let mut bytes = Vec::new();
    sansho_output::write_raw_ascii(&r, &mut bytes).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\t@m.x1.m1[gm]\tadmittance"));
    let parsed = sansho_output::read_raw_ascii(&text).unwrap();
    let a = &parsed.analyses[0];
    let b = &r.analyses[0];
    assert_eq!(a.variables, b.variables);
    for var in &a.variables {
        for (&x, &y) in a
            .real(&var.name)
            .unwrap()
            .iter()
            .zip(b.real(&var.name).unwrap())
        {
            near(x, y, 1e-15 * y.abs());
        }
    }
}
