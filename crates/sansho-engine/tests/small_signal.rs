//! Release-prep analysis checks against independent circuit and Fourier solutions.
//! Provenance: original networks using Johnson-Nyquist, shot-noise, MNA and DFT identities.
#![allow(clippy::unwrap_used, clippy::panic)]
use sansho_engine::{RunConfig, run_with};
use sansho_output::{AnalysisResult, SimResults};

fn simulate(deck: &str) -> SimResults {
    let nl = sansho_netlist::parse(deck).unwrap();
    let one = run_with(&nl, &RunConfig { threads: 1 }).unwrap().results;
    let many = run_with(&nl, &RunConfig { threads: 4 }).unwrap().results;
    assert_eq!(one, many);
    one
}
fn check(x: f64, want: f64, rtol: f64) {
    assert!(
        (x - want).abs() <= rtol * want.abs().max(1e-30),
        "{x:e} vs {want:e}"
    );
}
fn first(a: &AnalysisResult, key: &str) -> f64 {
    a.real(key).unwrap()[0]
}

#[test]
fn divider_gain_impedances_noise_and_integral() {
    let r = simulate(
        "divider\nV1 a 0 1\nR1 a b 1k\nR2 b 0 2k\n.tf v(a,b) V1\n.noise v(b) V1 lin 7 10 1000\n.sens v(b)\n",
    );
    let a = &r.analyses;
    check(first(&a[0], "transfer_function"), 1. / 3., 1e-12);
    check(first(&a[0], "v1#input_impedance"), 3000., 1e-12);
    check(
        first(&a[0], "output_impedance_at_v(a,b)"),
        2000. / 3.,
        1e-12,
    );
    let noise = (4. * sansho_devices::BOLTZMANN * 300.15 * 2000. / 3.).sqrt();
    check(first(&a[1], "onoise_spectrum"), noise, 1e-12);
    check(first(&a[1], "inoise_spectrum"), 1.5 * noise, 1e-12);
    check(first(&a[2], "onoise_total"), noise * 990f64.sqrt(), 1e-12);
    check(first(&a[3], "r1"), -2000. / 9e6, 1e-8);
    check(first(&a[3], "r2"), 1000. / 9e6, 1e-8);
    check(first(&a[3], "v1"), 2. / 3., 1e-8);
}
#[test]
fn adjoint_current_source_with_frequency_dependent_gain() {
    let r = simulate(
        "amp\nI1 0 a ac 1\nRin a 0 1k temp=50\nGamp 0 b a 0 0.02\nRout b 0 100 noisy=0\nC1 b 0 1u\n.noise v(b) I1 dec 1 100 10k\n.tf v(b) I1\n",
    );
    let a = &r.analyses;
    let noise = (4. * sansho_devices::BOLTZMANN * 323.15 / 1e3).sqrt();
    for (i, f) in a[0].scale_values.iter().enumerate() {
        let gain = 2000. / (1. + (std::f64::consts::TAU * f * 1e-4).powi(2)).sqrt();
        check(a[0].real("inoise_spectrum").unwrap()[i], noise, 1e-13);
        check(
            a[0].real("onoise_spectrum").unwrap()[i],
            gain * noise,
            1e-13,
        );
    }
    check(first(&a[2], "i1#input_impedance"), 1000., 1e-13);
}
#[test]
fn diode_shot_and_flicker_integrate_independently() {
    let r = simulate(
        "diode\nI1 0 a 1m ac 1\nD1 a 0 m\n.model m d(is=1e-15 kf=1e-16 af=1)\n.options gmin=0 reltol=1e-10 vntol=1e-12 abstol=1e-16\n.noise v(a) I1 dec 2 10 1000\n.sens v(a)\n",
    );
    let shot = 2. * sansho_devices::CHARGE * 1e-3;
    let pink = 1e-19;
    let a = &r.analyses;
    check(
        first(&a[0], "inoise_spectrum"),
        (shot + pink / 10.).sqrt(),
        1e-8,
    );
    check(
        first(&a[1], "inoise_total"),
        (shot * 990. + pink * 100f64.ln()).sqrt(),
        1e-8,
    );
    check(
        first(&a[2], "m:is"),
        -sansho_devices::thermal_voltage(300.15) / 1e-15,
        1e-7,
    );
}
#[test]
fn lead_lag_and_complex_poles() {
    let r = simulate(include_str!("../../../verify/decks/pz_lead_lag.cir"));
    let a = &r.analyses[0];
    check(a.complex("pole(1)").unwrap()[0].0, -1e6, 1e-12);
    check(a.complex("zero(1)").unwrap()[0].0, -5e5, 1e-12);
    let r = simulate("parallel RLC\nR1 a 0 1k\nL1 a 0 1m\nC1 a 0 1u\n.pz a 0 a 0 cur pol\n");
    for col in ["pole(1)", "pole(2)"] {
        let (re, im) = r.analyses[0].complex(col).unwrap()[0];
        check(re, -500., 1e-12);
        check(im.abs(), (1e9f64 - 250000.).sqrt(), 1e-12);
    }
}
#[test]
fn interpolated_fourier_and_card_order() {
    let r = simulate(
        "two sines\nV1 a 0 sin(0.25 2 1k)\nV2 b a sin(0 0.5 3k)\nR1 b 0 1k\n.four 1k v(b) v(b,a)\n.tran 1.4u 3m\n",
    );
    let a = &r.analyses[0];
    let m = a.real("magnitude(v(b))").unwrap();
    check(m[0], 0.25, 1e-5);
    check(m[1], 2., 1e-4);
    check(m[3], 0.5, 1e-4);
    check(first(a, "thd(v(b))"), 25., 2e-4);
    check(a.real("magnitude(v(b,a))").unwrap()[3], 0.5, 1e-4);
}
#[test]
fn diagnostics_for_invalid_requests() {
    for card in [
        ".noise v(x) V1 lin 2 1 2",
        ".tf v(a) missing",
        ".four 1k v(a)",
        ".noise v(a,a) V1 lin 2 1 2",
    ] {
        let nl = sansho_netlist::parse(&format!("bad\nV1 a 0 1\nR1 a 0 1k\n{card}\n")).unwrap();
        let err = run_with(&nl, &RunConfig { threads: 1 }).unwrap_err();
        assert_eq!(err.diagnostics[0].code, "E-ANALYSIS");
    }
}

#[test]
fn complex_sensitivity_of_rc_lowpass() {
    let r = simulate(include_str!("../../../verify/decks/sens_rc_ac.cir"));
    let a = &r.analyses[0];
    for (i, &f) in a.scale_values.iter().enumerate() {
        let s = sansho_sparse::c64::new(0., std::f64::consts::TAU * f);
        let h = 1. / (1. + s * 1e-5);
        for (name, tau_derivative) in [("r1", 1e-8), ("c1", 1e3)] {
            let (re, im) = a.complex(name).unwrap()[i];
            let want = -s * tau_derivative * h * h;
            check(re, want.re, 1e-8);
            check(im, want.im, 1e-8);
        }
    }
}
#[test]
fn bsim3_weak_inversion_noise_spectrum() {
    // The 44.2 Vgs = 0.35 V probe isolates flicker at 1 Hz and the
    // inversion-charge thermal floor at 100 MHz through a 1 kΩ readout.
    let r = simulate(
        "weak-inversion channel noise
Vgate g 0 DC 0.35 AC 1
Vdrain d 0 1.7
Hread out 0 Vdrain 1k
M1 d g 0 0 nch L=0.65u W=2.8u
.model nch nmos level=8 version=3.3 tox=8n xj=0.13u nch=2.1e17
+ vth0=0.54 k1=0.52 k2=-0.023 u0=460 vsat=92000 rdsw=85 pclm=1.15
+ lint=9n wint=15n mobmod=2 a0=1.15 pdiblc2=0.013 tnom=27
+ noimod=2 noia=8e19 noib=3e4 noic=2e-13 em=4.1e7
.noise v(out) Vgate lin 2 1 100meg
.end",
    );
    let density = r.analyses[0].real("onoise_spectrum").unwrap();
    check(density[0], 2.88131446e-11, 2e-3);
    check(density[1], 3.66985173e-12, 2e-3);
}

#[test]
fn bsim4_thermal_and_flicker_selectors() {
    let base = include_str!("../../../verify/decks/noise_bsim4_cs.cir");
    let mut spectra = vec![];
    for thermal in ["0", "1"] {
        for flicker in ["0", "1"] {
            let deck = base
                .replace("tnoimod=1", &format!("tnoimod={thermal}"))
                .replace("fnoimod=1", &format!("fnoimod={flicker} kf=1e-27"));
            let result = simulate(&deck);
            let density = result.analyses[0].real("onoise.mamp").unwrap().to_vec();
            assert!(density.iter().all(|v| v.is_finite() && *v > 0.));
            spectra.push(density);
        }
    }
    assert_ne!(spectra[0], spectra[1]);
    assert_ne!(spectra[1], spectra[3]);
}

#[test]
fn bsim4_partition_survives_zero_diffusion_resistance() {
    // The terminal-voltage generator must approach the same limit from
    // finite R, including capacitive feedthrough and reversed drain bias.
    let base = include_str!("../../../verify/decks/noise_bsim4_cs.cir")
        .replace("fnoimod=1", "fnoimod=0")
        .replace("dec 5 10 10meg", "dec 1 10 10gig");
    for device in ["Mamp out g 0 0", "Mamp 0 g out 0"] {
        let deck = base.replace("Mamp out g 0 0", device);
        let explicit = deck
            .replace("l=0.5u", "l=0.5u nrd=1 nrs=1")
            .replace("tnoimod=1", "rsh=1m tnoimod=1");
        let shorted = simulate(&deck);
        let finite = simulate(&explicit);
        for (a, b) in shorted.analyses[0]
            .real("onoise.mamp")
            .unwrap()
            .iter()
            .zip(finite.analyses[0].real("onoise.mamp").unwrap())
        {
            check(*a, *b, 1e-5);
        }
    }
}

#[test]
fn parallel_mos1_noise_matches_explicit_multiplicity() {
    let base = include_str!("../../../verify/decks/noise_mos1_cs.cir").replace(
        ".end",
        ".options gmin=0 reltol=1e-10 vntol=1e-12
.end",
    );
    let device = "Mamp out g 0 0 amp w=12u l=2u";
    let pair = simulate(&base.replace(
        device,
        &format!("{device}\n{}", device.replace("Mamp", "Mcopy")),
    ));
    let scaled = simulate(&base.replace(device, &format!("{device} m=2")));
    for (x, y) in scaled.analyses[0]
        .real("onoise_spectrum")
        .unwrap()
        .iter()
        .zip(pair.analyses[0].real("onoise_spectrum").unwrap())
    {
        check(*x, *y, 1e-10);
    }
}

#[test]
fn canonical_cards_and_raw_plots_round_trip() {
    let deck = "ports\nV1 a 0 1 ac 1 sin(1 1 1k)\nR1 a v 2k\nR2 v 0 1k\nC1 v 0 1n\n.noise v(a,v) v1 oct 3 10 80 2\n.tf i(v1) v1\n.sens v(a,v) ac lin 2 1 2\n.four {sqrt(1e6)} v(a,v) i(v1)\n.tran 2u 2m\n";
    let nl = sansho_netlist::parse(deck).unwrap();
    let written = sansho_netlist::write(&nl);
    let nl = sansho_netlist::parse(&written).unwrap();
    assert_eq!(sansho_netlist::write(&nl), written);
    let run = run_with(&nl, &RunConfig { threads: 2 }).unwrap();
    assert_eq!(run.noise_summary_intervals, [2]);
    let mut raw = vec![];
    sansho_output::write_raw_ascii(&run.results, &mut raw).unwrap();
    let decoded = sansho_output::read_raw_ascii(std::str::from_utf8(&raw).unwrap()).unwrap();
    assert_eq!(decoded.analyses.len(), 6);
    for (before, after) in run.results.analyses.iter().zip(&decoded.analyses) {
        assert_eq!(
            (before.kind, &before.variables, &before.scale, before.len()),
            (after.kind, &after.variables, &after.scale, after.len())
        );
    }
}

#[test]
fn distributed_line_phase_dc_and_sensitivity() {
    let r = simulate(
        "isolated ports\nVin a 0 2 ac 1\nR1 a b 50\nT1 b 0 c ref z0=50 f=25meg\nVref ref 0 7\nR2 c ref 50\n.op\n.ac lin 1 25meg 25meg\n.sens v(c,ref) ac lin 1 25meg 25meg\n.noise v(c,ref) Vin lin 1 25meg 25meg\n",
    );
    check(first(&r.analyses[0], "v(c)"), 8., 1e-12);
    let (re, im) = r.analyses[1].complex("v(c)").unwrap()[0];
    assert!(re.abs() < 1e-15);
    check(im, -0.5, 1e-13);
    let (re, im) = r.analyses[2].complex("t1:td").unwrap()[0];
    check(re, -std::f64::consts::PI * 25e6, 1e-8);
    assert!(im.abs() < 0.01);
    check(
        first(&r.analyses[3], "onoise_spectrum"),
        (4. * sansho_devices::BOLTZMANN * 300.15 * 25.).sqrt(),
        1e-12,
    );
}

#[test]
fn incident_and_reflected_ramps_land_on_delayed_corners() {
    let r = simulate(include_str!("../../../verify/decks/line_reflections.cir"));
    let tran = &r.analyses[1];
    let at = |ns: f64| {
        tran.scale_values
            .iter()
            .position(|&t| (t - ns * 1e-9).abs() < 1e-20)
            .unwrap()
    };
    for (ns, b, d) in [(7., 0., 0.), (7.5, 0.5, 1.), (17.5, 0.5, 5. / 6.)] {
        let k = at(ns);
        assert!((tran.real("v(b)").unwrap()[k] - b).abs() < 1e-10);
        assert!((tran.real("v(d)").unwrap()[k] - d).abs() < 1e-10);
    }
}

#[test]
fn all_poly_source_types_resolve_nested_controls_and_stamp_derivatives() {
    let r = simulate(
        "polynomials\n.subckt pol a b e g f h\nVs a sense 0\nRin sense 0 1\nEp e 0 poly(2) a 0 b 0 0 0 0 0 1\nGp 0 g poly(2) a 0 b 0 0 0 0 0 1\nFp 0 f poly(1) Vs 1 0 0 1\nHp h 0 poly(1) Vs 1 0 0 1\n.ends\nVa a 0 -2 ac 1\nVb b 0 3\nX1 a b e g f h pol\nRg g 0 1\nRf f 0 1\n.op\n.ac lin 1 1 1\n",
    );
    for (node, dc, gain) in [
        ("v(e)", -6., 3.),
        ("v(g)", -6., 3.),
        ("v(f)", -7., 12.),
        ("v(h)", -7., 12.),
    ] {
        check(first(&r.analyses[0], node), dc, 1e-13);
        check(r.analyses[1].complex(node).unwrap()[0].0, gain, 1e-13);
    }
}

#[test]
fn jfet_channel_polarity_noise_and_gate_junction_charge() {
    for (model, sign) in [("njf", 1.), ("pjf", -1.)] {
        let r = simulate(&format!(
            "JFET\nVd d 0 {}\nVg g 0 {} ac 1\nJ1 d g 0 m\n.model m {model}(vto=-2 beta=1m cgs=8p cgd=2p pb=1)\n.options gmin=0\n.op\n.ac lin 1 1meg 1meg\n.sens i(Vd)\n",
            3. * sign,
            -sign
        ));
        check(first(&r.analyses[0], "i(vd)"), -sign * 1e-3, 1e-9);
        check(r.analyses[1].complex("i(vd)").unwrap()[0].0, -2e-3, 1e-10);
        check(first(&r.analyses[2], "m:beta"), -sign, 1e-8);
        let c = 8e-12 / 2f64.sqrt() + 2e-12 / 5f64.sqrt();
        check(
            r.analyses[1].complex("i(vg)").unwrap()[0].1,
            -std::f64::consts::TAU * 1e6 * c,
            1e-11,
        );
    }
    let r = simulate(
        "noise\nVd supply 0 6\nRd supply d 1k noisy=0\nVg g 0 -1\nJ1 d g 0 m\n.model m njf(beta=1m kf=1e-16)\n.noise v(d) Vg lin 1 1000 1000\n",
    );
    check(
        first(&r.analyses[0], "onoise.j1"),
        1e3 * (4. * sansho_devices::BOLTZMANN * 300.15 * (2. / 3.) * 2e-3 + 1e-16 * 1e-3 / 1e3)
            .sqrt(),
        1e-6,
    );
}

#[test]
fn r3_diagnostics_and_export() {
    for (card, code) in [
        ("T1 a 0 b 0 z0=0 td=1n", "E-LINE-PARAM"),
        ("T1 a 0 b 0 z0=50 nl=.2", "E-LINE-PARAM"),
        ("E1 a 0 poly(99999999) a 0 1", "E-POLY-ARITY"),
        ("J1 a b 0 wrong\n.model wrong npn", "E-PARSE-MODEL"),
    ] {
        assert!(
            sansho_netlist::parse(&format!("bad\n{card}\n"))
                .unwrap_err()
                .iter()
                .any(|d| d.code == code && d.span.is_some())
        );
    }
    for (card, code) in [
        ("T1 a 0 b 0 z0=50 td=1n\n.pz a 0 b 0 cur pol", "E-ANALYSIS"),
        ("Hbad b 0 poly(1) missing 1\n.op", "E-BSOURCE-EXPR"),
        ("Jbad b a 0 m\n.model m njf pb=0\n.op", "E-ELAB-PARAM"),
    ] {
        let nl = sansho_netlist::parse(&format!("bad\nV1 a 0 0\nR1 b 0 1k\n{card}\n")).unwrap();
        assert!(
            sansho_engine::run(&nl)
                .unwrap_err()
                .diagnostics
                .iter()
                .any(|d| d.code == code)
        );
    }
    for src in [
        include_str!("../../../verify/decks/line_reflections.cir"),
        include_str!("../../../verify/decks/jfet_amplifier.cir"),
        include_str!("../../../verify/decks/poly_sources.cir"),
    ] {
        let written = sansho_netlist::write(&sansho_netlist::parse(src).unwrap());
        assert_eq!(
            written,
            sansho_netlist::write(&sansho_netlist::parse(&written).unwrap())
        );
    }
}

#[test]
fn poly_dimension_expression_and_linear_node_named_poly() {
    let r = simulate(
        "poly node\n.param n=2\n.subckt gain a b z\nE1 z 0 poly({n}) a 0 b 0 0 0 0 0 1\n.ends\nVa poly 0 2\nVb b 0 3\nElinear y 0 poly 0 4\nX1 poly b z gain\n.op\n",
    );
    check(first(&r.analyses[0], "v(y)"), 8., 1e-13);
    check(first(&r.analyses[0], "v(z)"), 6., 1e-13);
}

#[test]
fn mixed_classical_devices_are_bit_identical_across_worker_counts() {
    use std::fmt::Write;
    let mut deck = String::from(
        "mixed devices\nVg g 0 dc 0.8 ac 1 pulse(0.8 1.2 1n 2n 2n 5n 15n)\nVdd vdd 0 3\nRload vdd d 5\nT1 d 0 out 0 z0=50 td=1.5n\nRterm out 0 50\n.model nj njf(vto=-1 is=0 cgs=0.1p)\n.model n2 nmos(level=2 kp=1u vto=0.5 gamma=0.3)\n.model n3 nmos(level=3 kp=1u vto=0.4 gamma=0.4 theta=0.1)\n",
    );
    for i in 0..65 {
        writeln!(deck,"J{i} d 0 0 nj\nMa{i} d g 0 0 n2 w=2u l=1u\nMb{i} d g 0 0 n3 w=2u l=1u\nG{i} d 0 poly(1) g 0 0 0 1u").unwrap();
    }
    deck.push_str(".ac lin 3 1meg 100meg\n.tran 0.5n 5n\n.end\n");
    let nl = sansho_netlist::parse(&deck).unwrap();
    let run = |threads| {
        let r = run_with(&nl, &RunConfig { threads }).unwrap().results;
        let mut bits = vec![];
        for plot in r.analyses {
            bits.extend(plot.scale_values.into_iter().map(f64::to_bits));
            match plot.data {
                sansho_output::Data::Real { columns } => {
                    bits.extend(columns.into_iter().flatten().map(f64::to_bits))
                }
                sansho_output::Data::Complex { columns } => bits.extend(
                    columns
                        .into_iter()
                        .flatten()
                        .flat_map(|(a, b)| [a.to_bits(), b.to_bits()]),
                ),
            }
        }
        bits
    };
    let serial = run(1);
    for threads in [2, 5] {
        assert_eq!(serial, run(threads));
    }
}

#[test]
fn classical_mos_temperature_noise_sensitivity_and_poles_agree_in_square_law_limit() {
    let output = |level| {
        simulate(&format!("classical long channel\nVdd vdd 0 5\nVin g 0 dc 1 ac 1\nRd vdd d 1k\nM1 d g 0 0 n l=5u w=20u ad=4p\n.model n nmos(level={level} kp=20u vto=0.4 gamma=0 tox=30n cj=1m cgdo=200p kf=1e-25)\n.dc temp 0 80 20\n.noise v(d) Vin dec 2 10 1meg\n.sens v(d)\n.pz d 0 d 0 cur pol\n")).analyses
    };
    let reference = output(1);
    for level in [2, 3] {
        let plots = output(level);
        for (index, signal) in [
            (0, "v(d)"),
            (1, "onoise.m1"),
            (2, "onoise_total"),
            (3, "n:kp"),
        ] {
            let a = plots[index].real(signal).unwrap();
            let b = reference[index].real(signal).unwrap();
            for (&a, &b) in a.iter().zip(b) {
                check(a, b, 1e-8);
            }
        }
        check(
            plots[4].complex("pole(1)").unwrap()[0].0,
            reference[4].complex("pole(1)").unwrap()[0].0,
            1e-10,
        );
    }
}

#[test]
fn bsim3_common_source_tf_sens_and_pole() {
    let deck = "BSIM3 loaded common source\n.model chip nmos level=49 version=3.3.0 vth0=0.52 k1=0.5 k2=0\nVd rail 0 1.8\nVg g 0 dc 1.15 ac 1\nRd rail out 4k\nM1 out g 0 0 chip l=0.5u w=2u\nCl out 0 1n\n.tf v(out) Vg\n.sens v(out)\n.pz out 0 out 0 cur pol\n.end";
    let r = simulate(deck);
    let gain = first(&r.analyses[0], "transfer_function");
    assert!(gain < -0.1);
    check(first(&r.analyses[1], "vg"), gain, 1e-5);
    assert!(first(&r.analyses[1], "chip:vth0") > 0.);
    let resistance = first(&r.analyses[0], "output_impedance_at_v(out)");
    check(
        r.analyses[2].complex("pole(1)").unwrap()[0].0,
        -1. / (resistance * 1e-9),
        1e-4,
    );
}

#[test]
fn bsim3_versions_selectors_and_invalid_dimensions() {
    let deck = |level: u8, opts: &str| {
        format!(
            "BSIM3 binding\n.model n nmos level={level} {opts}\nM1 d g 0 0 n w=2u l=0.5u\nVd d 0 1\nVg g 0 1\n.op\n"
        )
    };
    let nominal = simulate(&deck(8, ""));
    for level in [8, 49] {
        for v in ["3.3", "3.3.0", "3.30"] {
            let r = simulate(&deck(level, &format!("version={v}")));
            assert_eq!(nominal.analyses, r.analyses);
        }
    }
    for (opts, code) in [
        ("version=3.2", "E-UNSUPPORTED"),
        ("version=3.3.1", "E-UNSUPPORTED"),
        ("nqsmod=1", "E-UNSUPPORTED"),
        ("acnqsmod=1", "E-UNSUPPORTED"),
        ("lint=1u", "E-ELAB-MODEL"),
        ("capmod=3.5", "E-ELAB-MODEL"),
    ] {
        let nl = sansho_netlist::parse(&deck(49, opts)).unwrap();
        let err = sansho_engine::run(&nl).unwrap_err();
        assert!(err.diagnostics.iter().any(|d| d.code == code));
    }
}

#[test]
fn bsim3_parallel_chunks_are_bit_deterministic() {
    use std::fmt::Write;
    let mut deck =
        String::from("MOS array\nVg g 0 1\nVd d 0 1\n.model m nmos level=8 vth0=0.5 k1=0.45\n");
    for k in 0..257 {
        writeln!(deck, "M{k} d g 0 0 m w={}u l=0.55u", 1 + k % 5).unwrap();
    }
    deck.push_str(".dc Vg 0 1.5 0.25\n.end");
    let nl = sansho_netlist::parse(&deck).unwrap();
    let raw = |threads| {
        let out = run_with(&nl, &RunConfig { threads }).unwrap();
        let mut raw = Vec::new();
        sansho_output::write_raw_binary(&out.results, &mut raw).unwrap();
        raw
    };
    assert_eq!(raw(1), raw(2));
}

#[test]
fn lossy_line_matches_diffusion_and_wavefront_solutions() {
    let tau = 1e-8;
    // Open far end, unit step at the near end. At t=RC the higher
    // diffusion eigenmodes have already decayed by more than nine decades.
    let r = simulate(
        "RC diffusion\nVin in 0 pulse(0 1 0 1p 1p 100n 200n)\nO1 in 0 far 0 wire\n.model wire ltra(r=100 c=100p len=1)\n.tran 0.025n 10n\n.ac lin 1 1meg 1meg\n.save v(far)\n",
    );
    let tr = &r.analyses[0];
    let t = *tr.scale_values.last().unwrap();
    let pi = std::f64::consts::PI;
    let reference = 1. - 4. / pi * (-pi * pi * t / (4. * tau)).exp();
    check(*tr.real("v(far)").unwrap().last().unwrap(), reference, 1e-4);
    // A matched source launches half a volt. An open end doubles the
    // wavefront, whose attenuation is exp(-R/(2 Z0)).
    let r = simulate(
        "wave\nVin in 0 pulse(0 1 1n 5p 5p 50n 100n)\nRs in n 50\nO1 n 0 far 0 wire\n.model wire ltra(r=10 l=250n c=100p len=1)\n.tran 0.05n 6.01n\n.save v(far)\n",
    );
    let tr = &r.analyses[0];
    let v = tr.real("v(far)").unwrap();
    for (&t, &v) in tr.scale_values.iter().zip(v) {
        if t < 5.999e-9 {
            assert!(v.abs() < 1e-12);
        }
    }
    check(*v.last().unwrap(), (-0.1f64).exp(), 0.001);
}

#[test]
fn rc_and_rg_ac_two_ports_match_open_circuit_solution() {
    let r = simulate(
        "open line\nVin in 0 dc 1 ac 1\nO1 in 0 out 0 a\nO2 in 0 rg 0 b\n.model a ltra(r=200 c=50p len=1)\n.model b ltra(r=250 g=1m len=2)\n.ac lin 1 100meg 100meg\n",
    );
    let (re, im) = r.analyses[0].complex("v(out)").unwrap()[0];
    let phase = std::f64::consts::PI.sqrt();
    let (a, b) = (phase.cosh() * phase.cos(), phase.sinh() * phase.sin());
    check(re, a / (a * a + b * b), 1e-10);
    check(im, -b / (a * a + b * b), 1e-10);
    check(
        r.analyses[0].complex("v(rg)").unwrap()[0].0,
        1. / 1f64.cosh(),
        1e-12,
    );
}

#[test]
fn loaded_rlc_line_has_phase_delay_in_ac() {
    let r = simulate(include_str!("../../../verify/decks/r8_ltra_ac.cir"));
    let ac = &r.analyses[0];
    let k = ac.scale_values.partition_point(|&f| f < 50e6);
    // Loaded ABCD solution at 10^7.7 Hz, with Z and Y for the full length.
    // A=D=cosh(sqrt(ZY)); (B,C)=(Z,Y)*sinh(sqrt(ZY))/sqrt(ZY).
    // Vf = 1 / (A+B/Rload+Rs*(C+D/Rload)), Iline = Vf*(C+D/Rload).
    let (re, im) = ac.complex("v(far)").unwrap()[k];
    check(re, -0.334616965715733, 1e-11);
    check(im, 0.024240328286045, 1e-11);
    // The source supplies the static RG port in parallel with the RLC port.
    let (re, im) = ac.complex("i(vin)").unwrap()[k];
    check(re, -0.0106624590849174, 1e-11);
    check(im, -0.000708175245429, 1e-11);
}

#[test]
fn single_urc_lump_reduces_to_a_center_shunt() {
    let r = simulate(
        "one lump\nVin in 0 dc 1 ac 1\nU1 in out 0 sheet l=1 n=1\n.model sheet urc(rperl=200 cperl=1n)\nRload out 0 200\n.op\n.ac lin 1 1meg 1meg\n.sens v(out)\n.save all @u1[i1]\n",
    );
    check(first(&r.analyses[0], "@u1[i1]"), 0.0025, 1e-12);
    check(first(&r.analyses[0], "v(out)"), 0.5, 1e-12);
    let h = sansho_sparse::c64::from(0.5)
        / sansho_sparse::c64::new(1., std::f64::consts::TAU * 1e6 * 75e-9);
    let (re, im) = r.analyses[1].complex("v(out)").unwrap()[0];
    check(re, h.re, 1e-12);
    check(im, h.im, 1e-12);
    check(
        first(&r.analyses[2], "sheet:rperl"),
        -200. / 400f64.powi(2),
        1e-7,
    );
}

#[test]
fn statz_polarity_noise_sensitivity_and_power_mos_vectors() {
    for (model, sign) in [("nmf", 1.), ("pmf", -1.)] {
        let r = simulate(&format!(
            "Schottky gate\nVd d 0 {}\nVg g 0 {} ac 1\nZ1 d g 0 ga\n.model ga {model}(vto=-2 beta=2m alpha=2 b=0.25 is=0)\n.options gmin=0\n.op\n.ac lin 1 1meg 1meg\n.sens i(vd)\n.save all @z1[id] @z1[gm]\n",
            3. * sign,
            -sign
        ));
        check(first(&r.analyses[0], "@z1[id]"), sign * 0.002 / 1.25, 1e-12);
        check(
            r.analyses[1].complex("i(vd)").unwrap()[0].0,
            -0.002 * 2.25 / 1.25f64.powi(2),
            1e-12,
        );
        check(first(&r.analyses[2], "ga:beta"), -sign / 1.25, 1e-8);
    }
    let r = simulate(
        "noise\nVg g 0 -1 ac 1\nVdd rail 0 4\nRd rail d 10 noisy=0\nZ1 d g 0 ga\n.model ga nmf(vto=-2 beta=1m b=0 lambda=0 is=0)\n.noise v(d) Vg lin 1 1000 1000\n",
    );
    check(
        first(&r.analyses[0], "onoise.z1"),
        10. * (4. * sansho_devices::BOLTZMANN * 300.15 * (2. / 3.) * 0.002).sqrt(),
        1e-8,
    );
    let r = simulate(
        "p-channel\n.subckt switch d g s\nMcore d g s pw\n.model pw vdmos(pchan vto=-1 kp=2 cgs=2n cgdmin=10p cgdmax=30p)\n.ends\nX1 d g 0 switch\nVg g 0 -2 ac 1\nVd d 0 -3\n.op\n.ac lin 1 1meg 1meg\n.sens i(vd)\n.options savecurrents\n.save all @m.x1.mcore[gm]\n",
    );
    let drive = 1. + 0.1 * (-10_f64).exp().ln_1p();
    let gm = 2. * drive / (1. + (-10_f64).exp());
    check(
        first(&r.analyses[0], "@m.x1.mcore[id]"),
        -drive * drive,
        1e-8,
    );
    check(first(&r.analyses[0], "@m.x1.mcore[gm]"), gm, 1e-12);
    check(r.analyses[1].complex("i(vd)").unwrap()[0].0, -gm, 1e-10);
}

#[test]
fn vdmos_and_mesfet_local_startup_and_parameterized_model_flags() {
    let r = simulate(
        "intrinsic UIC\n.param cap=2n\nM1 d g 0 pf ic=-0.1,-0.5\nRg g 0 10k\nRd d 0 100\n.model pf vdmos(pchan vto=-2 kp=1 cgs={cap} cgdmin=0 cgdmax=0 cjo=0)\nZ1 zd zg 0 ga ic=0.1,-0.3\nRz zg 0 10k\nRzd zd 0 100\n.model ga nmf(vto=-2 cgs=3p cgd=0.4p)\n.tran 1n 100n uic\n.save v(g) v(zg) @m1[cgs] @z1[qgs]\n",
    );
    let a = &r.analyses[0];
    check(first(a, "v(g)"), -0.5, 1e-12);
    check(first(a, "v(zg)"), -0.3, 1e-12);
    check(first(a, "@m1[cgs]"), 2e-9, 1e-12);
    assert_ne!(first(a, "@z1[qgs]"), 0.);
    check(
        *a.real("v(g)").unwrap().last().unwrap(),
        -0.5 * (-100e-9f64 / 20e-6).exp(),
        1e-5,
    );
}

#[test]
fn bjt_area_factors_scale_independent_ac_junction_currents() {
    // At zero DC bias and unit collector AC voltage, the base and substrate
    // drive currents give independent measurements of the two depletion caps.
    let r = simulate(
        "junction areas\nVc c 0 dc 0 ac 1\nVb b 0 0\nVs sub 0 0\nQ1 c b 0 sub sw area=4 areab=2 areac=3 m=2\n.model sw npn(cjc=3p cjs=5p cje=0)\n.ac lin 1 1meg 1meg\n.sens i(vb) ac lin 1 1meg 1meg\n",
    );
    let omega = std::f64::consts::TAU * 1e6;
    check(
        r.analyses[0].complex("i(vb)").unwrap()[0].1,
        omega * 12e-12,
        1e-12,
    );
    check(
        r.analyses[0].complex("i(vs)").unwrap()[0].1,
        omega * 30e-12,
        1e-12,
    );
    check(
        r.analyses[1].complex("q1:areab").unwrap()[0].1,
        omega * 6e-12,
        1e-8,
    );
}
