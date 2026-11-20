//! Analytic checks of the original R12 analog-visible waveforms for offline verification.
//! Provenance: Boolean timing, voltage-divider logic levels and the first-order RC equation.

#![allow(clippy::unwrap_used)]

use sansho_output::AnalysisResult;
use std::path::Path;

#[test]
fn original_circuits_obey_logic_timing_and_rc_laws() {
    for stem in ["bus", "counter", "latch", "mixed", "ring"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../verify/r12-decks/{stem}.cir"));
        let netlist = sansho_netlist::parse_file(&path).unwrap();
        let run = sansho_engine::run(&netlist).unwrap();
        check(stem, &run.results.analyses[0]);
    }
}

fn check(stem: &str, a: &AnalysisResult) {
    let sample = |signal: &str, ns: f64, expected: f64| {
        let t = ns * 1e-9;
        let i = a.scale_values.partition_point(|&s| s < t);
        let col = a.real(signal).unwrap();
        let v = if i == 0 {
            col[0]
        } else {
            let times = &a.scale_values[i - 1..=i];
            col[i - 1] + (col[i] - col[i - 1]) * (t - times[0]) / (times[1] - times[0])
        };
        assert!(
            (v - expected).abs() < 1e-6,
            "{stem}: {signal} at {ns} ns: expected {expected}, found {v}"
        );
    };
    match stem {
        "ring" => {
            sample("v(out)", 0., 1.);
            for i in 0..10 {
                sample("v(out)", 12.5 + 6. * f64::from(i), f64::from(i % 2));
            }
            sample("v(out)", 81., 1.);
        }
        "counter" => {
            for i in 0..6 {
                let t = if i == 0 {
                    0.
                } else {
                    7. + 10. * f64::from(i - 1)
                };
                sample("v(lo)", t, f64::from(i % 2));
                sample("v(hi)", t, f64::from(i / 2 % 2));
            }
        }
        "latch" => {
            for (t, v) in [
                (0., 0.),
                (4., 0.),
                (7., 1.),
                (11., 1.),
                (17., 1.),
                (21., 0.),
                (28., 0.),
            ] {
                sample("v(out)", t, v);
            }
        }
        "bus" => {
            for (t, v) in [(0., 1.), (6., 0.), (12., 0.4), (18., 1.), (25., 1.)] {
                sample("v(out)", t, v);
            }
        }
        "mixed" => {
            for i in 0..6 {
                sample(
                    "v(drive)",
                    if i == 0 {
                        0.
                    } else {
                        9. + 12. * f64::from(i - 1)
                    },
                    f64::from(i % 2),
                );
            }
            check_rc(a);
        }
        _ => unreachable!(),
    }
}

fn check_rc(a: &AnalysisResult) {
    let vin = a.real("v(drive)").unwrap();
    let vout = a.real("v(fb)").unwrap();
    let mut expected = 0.;
    for ((t, u), &v) in a
        .scale_values
        .windows(2)
        .zip(vin.windows(2))
        .zip(vout.iter().skip(1))
    {
        let dt = (t[1] - t[0]) / 5e-9;
        let relax = -(-dt).exp_m1();
        expected += (u[0] - expected) * relax + (u[1] - u[0]) * (1. - relax / dt);
        assert!(
            (expected - v).abs() < 0.003,
            "RC output at {} s: expected {expected}, found {v}",
            t[1]
        );
    }
    let cmp = a.real("v(diff)").unwrap();
    let rising_samples = cmp
        .windows(2)
        .zip(&a.scale_values[1..])
        .filter_map(|(v, &t)| (v[0] < 0. && v[1] >= 0.).then_some(t));
    let midpoints = vin
        .windows(2)
        .zip(a.scale_values.windows(2))
        .filter(|(v, _)| (v[0] < 0.5 && v[1] >= 0.5) || (v[0] > 0.5 && v[1] <= 0.5))
        .map(|(v, t)| t[0] + (t[1] - t[0]) * (0.5 - v[0]) / (v[1] - v[0]))
        .collect::<Vec<_>>();
    let clocks: Vec<_> = rising_samples.collect();
    assert_eq!((clocks.len(), midpoints.len()), (5, 5));
    for (c, e) in clocks.into_iter().zip(midpoints) {
        assert!((e - c - 0.75e-9).abs() < 5e-12);
    }
}
