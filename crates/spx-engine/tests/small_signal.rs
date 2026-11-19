//! Release-prep analysis checks against independent circuit and Fourier solutions.
//! Provenance: original networks using Johnson-Nyquist, shot-noise, MNA and DFT identities.
#![allow(clippy::unwrap_used, clippy::panic)]
use spx_engine::{RunConfig, run_with};
use spx_output::{AnalysisResult, SimResults};

fn simulate(deck: &str) -> SimResults {
    let nl = spx_netlist::parse(deck).unwrap();
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
    let noise = (4. * spx_devices::BOLTZMANN * 300.15 * 2000. / 3.).sqrt();
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
    let noise = (4. * spx_devices::BOLTZMANN * 323.15 / 1e3).sqrt();
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
    let shot = 2. * spx_devices::CHARGE * 1e-3;
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
        -spx_devices::thermal_voltage(300.15) / 1e-15,
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
        let nl = spx_netlist::parse(&format!("bad\nV1 a 0 1\nR1 a 0 1k\n{card}\n")).unwrap();
        let err = run_with(&nl, &RunConfig { threads: 1 }).unwrap_err();
        assert_eq!(err.diagnostics[0].code, "E-ANALYSIS");
    }
}

#[test]
fn complex_sensitivity_of_rc_lowpass() {
    let r = simulate(include_str!("../../../verify/decks/sens_rc_ac.cir"));
    let a = &r.analyses[0];
    for (i, &f) in a.scale_values.iter().enumerate() {
        let s = spx_sparse::c64::new(0., std::f64::consts::TAU * f);
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
    let nl = spx_netlist::parse(deck).unwrap();
    let written = spx_netlist::write(&nl);
    let nl = spx_netlist::parse(&written).unwrap();
    assert_eq!(spx_netlist::write(&nl), written);
    let run = run_with(&nl, &RunConfig { threads: 2 }).unwrap();
    assert_eq!(run.noise_summary_intervals, [2]);
    let mut raw = vec![];
    spx_output::write_raw_ascii(&run.results, &mut raw).unwrap();
    let decoded = spx_output::read_raw_ascii(std::str::from_utf8(&raw).unwrap()).unwrap();
    assert_eq!(decoded.analyses.len(), 6);
    for (before, after) in run.results.analyses.iter().zip(&decoded.analyses) {
        assert_eq!(
            (before.kind, &before.variables, &before.scale, before.len()),
            (after.kind, &after.variables, &after.scale, after.len())
        );
    }
}
