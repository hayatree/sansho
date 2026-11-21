//! Distributed line checks through parsing, elaboration and transient/AC assembly.
//! Provenance: original circuits; uniform telegrapher equations and Paul’s modal decomposition.
#![allow(clippy::unwrap_used)]

use sansho_engine::{RunConfig, run_with};
use sansho_output::SimResults;

fn simulate(deck: &str) -> SimResults {
    let nl = sansho_netlist::parse(deck).unwrap();
    let run = |threads| run_with(&nl, &RunConfig { threads }).unwrap().results;
    let raw = |result: &SimResults| {
        let mut bytes = Vec::new();
        sansho_output::write_raw_binary(result, &mut bytes).unwrap();
        bytes
    };
    let result = run(1);
    assert_eq!(raw(&result), raw(&run(2)));
    result
}

#[test]
fn lossless_limit_and_distortionless_wavefront() {
    let r = simulate(include_str!("../../../verify/decks/r18_txl_lossless.cir"));
    let tr = &r.analyses[0];
    for (a, b) in [("v(near)", "v(other)"), ("v(far)", "v(control)")] {
        for (y, z) in tr.real(a).unwrap().iter().zip(tr.real(b).unwrap()) {
            assert!((y - z).abs() < 2e-12);
        }
    }
    let r = simulate(include_str!("../../../verify/decks/r18_txl_matched.cir"));
    let tr = &r.analyses[0];
    for (&t, &v) in tr.scale_values.iter().zip(tr.real("v(far)").unwrap()) {
        let expected = 0.5 * (-0.2f64).exp() * ((t - 6e-9) / 1e-10).clamp(0., 1.);
        assert!((v - expected).abs() < 2e-11);
    }
    for corner in [6., 6.1, 11., 11.1] {
        assert!(
            tr.scale_values
                .iter()
                .any(|&t| (t - corner * 1e-9).abs() < 1e-20)
        );
    }
}

#[test]
fn noncommuting_ac_matches_loaded_matrix_exponential() {
    let r = simulate(include_str!("../../../verify/decks/r18_cpl_ac.cir"));
    let ac = &r.analyses[0];
    let k = ac
        .scale_values
        .iter()
        .position(|&f| (f - 100e6).abs() < 0.1)
        .unwrap();
    // Independent ABCD series in V/I coordinates, fixed 64-section cascade,
    // solved with 50-ohm source resistors and 75/80-ohm loads.
    for (key, re, im) in [
        ("v(x)", -0.4733674385370629, 0.2595373425575745),
        ("v(y)", 0.012216774834917345, 0.0920836165713617),
    ] {
        let (a, b) = ac.complex(key).unwrap()[k];
        assert!((a - re).hypot(b - im) < 2e-13);
    }
}

#[test]
fn dc_length_and_stationary_prehistory() {
    let r = simulate(
        "biased pair\nVin in 0 1\nY1 in 0 a 0 wire\nY2 in 0 b 0 wire len=2\n.model wire txl r=20 l=250n c=100p length=1\nRa a 0 80\nRb b 0 80\n.op\n.tran .3n 20n\n",
    );
    for plot in &r.analyses {
        for (key, want) in [("v(a)", 0.8), ("v(b)", 2. / 3.)] {
            assert!(
                plot.real(key)
                    .unwrap()
                    .iter()
                    .all(|v| (v - want).abs() < 1e-12)
            );
        }
    }
}

#[test]
fn coupled_common_and_differential_modes_match_scalar_controls() {
    let r = simulate(
        "balanced pair on split reference nodes
V1 src 0 pulse(0 1 1n .15n .15n 12n 40n)
Vref ref 0 .7
R1 src a 50
R2 b 0 50
P1 a b 0 x y ref pair
.model pair cpl length=1 r=12 12 g=0 0 l=300n 50n 300n c=120p -10p 120p
Rx x ref 75
Ry y ref 75
Re src e 50
Ro src o 50
Ye e 0 even 0 common
Yo o 0 odd 0 diff
.model common txl length=1 r=12 l=350n c=110p
.model diff txl length=1 r=12 l=250n c=130p
R3 even 0 75
R4 odd 0 75
.tran .06n 25n
.end",
    );
    let tr = &r.analyses[0];
    let v = |name| tr.real(name).unwrap();
    for k in 0..tr.scale_values.len() {
        let near = 0.5 * (v("v(e)")[k] - v("v(o)")[k]);
        let far = 0.5 * (v("v(even)")[k] - v("v(odd)")[k]);
        assert!((v("v(b)")[k] - near).abs() < 1e-11);
        assert!((v("v(y)")[k] - 0.7 - far).abs() < 1e-11);
    }
}

#[test]
fn matrices_require_consistent_dimensions_and_passivity() {
    for (l, r, c, g) in [
        ("1 0", "1 1", "1 0 1", "0 0"),
        ("1 2 1", "1 1", "1 0 1", "0 0"),
        ("1 0 1", "1 2 1", "1 0 1", "0 0"),
        ("1 0 1", "1 1", "1 1 1", "0 0"),
        ("1 0 1", "0 0", "1 0 1", "0 1 0"),
    ] {
        let net = format!(
            "invalid\nP1 a b 0 x y 0 m\n.model m cpl length=1 r={r} l={l} g={g} c={c}\n.op\n"
        );
        let nl = sansho_netlist::parse(&net).unwrap();
        let error = sansho_engine::run(&nl).unwrap_err();
        assert!(error.diagnostics.iter().any(|d| d.code == "E-LINE-MATRIX"));
    }
}

#[test]
fn local_cpl_model_expressions_survive_netlist_round_trip() {
    let text = "scoped matrix\n.subckt pair a b x y params: mutual=0\nP1 a b 0 x y 0 wire len=2\n.model wire cpl length=1 r=1 2 l=4 {mutual} 5 g=0 0 c=3 -1 6\n.ends\nX1 a b x y pair mutual=1\nX2 c d u v pair mutual=2\n.end";
    let nl = sansho_netlist::parse(text).unwrap();
    let serialized = sansho_netlist::write(&nl);
    let round = sansho_netlist::parse(&serialized).unwrap();
    for (i, card) in round.models.iter().enumerate() {
        assert_eq!(card.arrays, nl.models[i].arrays);
        assert!(
            card.arrays
                .iter()
                .any(|(key, v)| key == "l" && v == &[4., (i + 1) as f64, 5.])
        );
    }
    assert_eq!(round.models.len(), 2);
}

#[test]
fn uic_preserves_the_first_wave_and_zero_interior() {
    let r = simulate(
        "UIC wave\nRs a 0 50\nY1 a 0 far 0 wire\n.model wire txl length=1 r=10 l=250n g=4m c=100p\nRl far 0 50\n.ic v(a)=1\n.tran .04n 6n uic",
    );
    let tr = &r.analyses[0];
    let far = tr.real("v(far)").unwrap();
    let front = tr
        .scale_values
        .iter()
        .position(|&t| (t - 5e-9).abs() < 1e-20)
        .unwrap();
    assert!(far[..front].iter().all(|&v| v == 0.));
    assert!((far[front] - 0.5 * (-0.2f64).exp()).abs() < 1e-12);
}

#[test]
fn instance_len_overrides_only_the_length_of_its_own_line() {
    let r = simulate(
        "length shadowing\n.param len=4\nVin off 0 1\nY1 off 0 a 0 wire len=2\nY2 off 0 b 0 wire\n.model wire txl r={10*len} l=250n c=100p length={len}\nRa a 0 80\nRb b 0 80\n.op",
    );
    let op = &r.analyses[0];
    assert!((op.real("v(a)").unwrap()[0] - 0.5).abs() < 1e-12);
    assert!((op.real("v(b)").unwrap()[0] - 1. / 3.).abs() < 1e-12);
}

#[test]
fn asymmetric_loss_settles_to_independent_dc_dividers() {
    let template = include_str!("../../../verify/probes/r18b_cpl_settling.cir");
    for (resistance, tolerance) in [(12, 3e-7), (24, 1e-9), (60, 1e-9)] {
        let deck = template.replace(".param probe=12", &format!(".param probe={resistance}"));
        let r = simulate(&deck);
        for (name, expected) in [
            ("v(a)", 87. / 137.),
            ("v(b)", 0.),
            ("v(x)", 75. / 137.),
            ("v(y)", 0.),
        ] {
            let last = r.analyses[0].real(name).unwrap().last().unwrap();
            assert!(
                (last - expected).abs() < tolerance,
                "{resistance} {name}: {last}"
            );
        }
    }
}

#[test]
fn pure_modes_arrive_with_their_own_attenuation_and_tail() {
    let deck = include_str!("../../../verify/probes/r18b_cpl_modes.cir")
        .replace(".param probe=0.01", ".param probe=4");
    let r = simulate(&deck);
    let tr = &r.analyses[0];
    for (positive, other, scalar, polarity, l, c) in [
        ("v(x)", "v(y)", "v(even)", 1., 365e-9_f64, 102e-12_f64),
        ("v(w)", "v(z)", "v(odd)", -1., 235e-9, 128e-12),
    ] {
        let delay = (l * c).sqrt();
        let z = (l / c).sqrt();
        let front = 0.5 * (-0.5 * (48. / z + 1.2e-3 * z)).exp();
        let output = tr.real(positive).unwrap();
        let companion = tr.real(other).unwrap();
        let control = tr.real(scalar).unwrap();
        for (k, &t) in tr.scale_values.iter().enumerate() {
            assert!((output[k] - polarity * companion[k]).abs() < 1e-12);
            assert!((output[k] - control[k]).abs() < 1e-12);
            if t < 1e-9 + delay {
                assert!(output[k].abs() < 1e-12);
            }
        }
        // The continuous tail has already begun during the 160 ps source ramp.
        let end = tr
            .scale_values
            .iter()
            .position(|t| (t - (1.16e-9 + delay)).abs() < 1e-18)
            .unwrap();
        assert!((output[end] - front).abs() < 1e-3);
    }
}

#[test]
fn coincident_flights_retain_different_modal_attenuations() {
    let deck = include_str!("../../../verify/probes/r18b_cpl_coincident.cir")
        .replace(".param probe=-19p", ".param probe=-20p");
    let r = simulate(&deck);
    let tr = &r.analyses[0];
    let ramp_end = 1.1e-9 + 24_f64.sqrt() * 1e-9;
    let k = tr
        .scale_values
        .iter()
        .position(|t| (t - ramp_end).abs() < 1e-18)
        .unwrap();
    let front = |z: f64| 2. * 50. * z / (50. + z).powi(2) * (-10. / (2. * z)).exp();
    let even = front((300e-9_f64 / 80e-12).sqrt());
    let odd = front((200e-9_f64 / 120e-12).sqrt());
    // The first 100 ps ramp contains a small contribution from the tail.
    for (name, expected) in [("v(x)", 0.5 * (even + odd)), ("v(y)", 0.5 * (even - odd))] {
        assert!((tr.real(name).unwrap()[k] - expected).abs() < 6e-5);
    }
}

#[test]
fn loaded_txl_operating_point_includes_shunt_leakage() {
    let template = include_str!("../../../verify/probes/r18b_txl_ac_ports.cir");
    let gamma = (24_f64 * 0.0012).sqrt();
    let sinhc = gamma.sinh() / gamma;
    for rs in [25, 50, 100] {
        let deck = template.replace(".param probe=25", &format!(".param probe={rs}"));
        let result = simulate(&deck);
        let far = 80.
            / ((80. + f64::from(rs)) * gamma.cosh() + (24. + f64::from(rs) * 80. * 0.0012) * sinhc);
        let near = far * (gamma.cosh() + 24. * sinhc / 80.);
        let op = &result.analyses[0];
        for (signal, expected) in [
            ("v(x)", far),
            ("v(a)", near),
            ("i(vin)", (near - 1.) / f64::from(rs)),
        ] {
            assert!((op.real(signal).unwrap()[0] - expected).abs() < 2e-13);
        }
    }
}

#[test]
fn strong_series_loss_tracks_the_independent_ltra_convolution() {
    let deck = include_str!("../../../verify/probes/r18b_txl_ltra.cir")
        .replace(".param probe=4", ".param probe=200");
    let result = simulate(&deck);
    let tr = &result.analyses[0];
    for (txl, ltra) in [("v(a)", "v(b)"), ("v(x)", "v(y)")] {
        for (wave, control) in tr.real(txl).unwrap().iter().zip(tr.real(ltra).unwrap()) {
            assert!((wave - control).abs() < 2e-6);
        }
    }
}

#[test]
fn asymmetric_loss_propagates_the_loaded_ac_phasors() {
    let r = simulate(
        "coupled harmonic
Vin src 0 dc 0 ac 1 sin(0 1 100meg)
Ra src a 50
Rb b 0 50
Rx x 0 75
Ry y 0 80
P1 a b 0 x y 0 pair
.model pair cpl length=1 r=12 0 60 g=.3m 0 .3m l=300n 65n 300n c=115p -13p 115p
.ac lin 1 100meg 100meg
.tran .125n 100n
.save v(a) v(b) v(x) v(y)
.end",
    );
    let (ac, tr) = (&r.analyses[0], &r.analyses[1]);
    for name in ["v(a)", "v(b)", "v(x)", "v(y)"] {
        let (re, im) = ac.complex(name).unwrap()[0];
        for (&t, &v) in tr.scale_values.iter().zip(tr.real(name).unwrap()) {
            if t > 80e-9 {
                let (sin, cos) = (std::f64::consts::TAU * 1e8 * t).sin_cos();
                assert!((v - re * sin - im * cos).abs() < 2.5e-4, "{name} at {t}");
            }
        }
    }
}
