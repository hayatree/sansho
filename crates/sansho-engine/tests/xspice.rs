//! End-to-end analog model measurements, model scoping and input diagnostics.
//! Provenance: original circuits and elementary transfer-function and waveform identities.
#![allow(clippy::unwrap_used, clippy::panic)]

use sansho_engine::{RunConfig, run_with};
use sansho_netlist::{parse, parse_file};
use sansho_output::AnalysisResult;
use std::path::Path;

fn deck(name: &str) -> AnalysisResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../verify/r11-decks")
        .join(format!("{name}.cir"));
    let nl = parse_file(&path).unwrap();
    let run = |threads| {
        let r = run_with(&nl, &RunConfig { threads }).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        r.results
    };
    let mut r = run(1);
    assert_eq!(r, run(2));
    r.analyses.remove(0)
}
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() < tol, "{a} vs {b}");
}
#[test]
fn algebra_and_smoothed_corners() {
    let d = deck("arithmetic");
    for (i, &x) in d.scale_values.iter().enumerate() {
        let u = 2. * x - 0.5;
        let sum = 2. * (u - 0.75);
        close(d.real("v(ratio)").unwrap()[i], 2. * u * sum, 1e-12);
    }
    let d = deck("curves");
    for (i, &x) in d.scale_values.iter().enumerate() {
        let a = x.abs();
        let expected = if a <= 0.9 {
            a
        } else if a >= 1.1 {
            1.
        } else {
            1. - (1.1 - a).powi(2) / 0.4
        };
        close(d.real("v(clip)").unwrap()[i], x.signum() * expected, 1e-13);
    }
    // At the PWL corner the two slopes are 0.2 and 1.0.
    close(d.real("v(shape)").unwrap()[30], 0.22, 1e-13);
    let d = deck("aswitch");
    for (i, &x) in d.scale_values.iter().enumerate() {
        let r = 1e4 * 0.01_f64.powf(x);
        close(d.real("v(load)").unwrap()[i], 1.5 * r / (1000. + r), 1e-12);
    }
    close(d.real("v(clipped)").unwrap()[0], 0.8, 1e-12);
    close(*d.real("v(clipped)").unwrap().last().unwrap(), 0.2, 1e-12);
}
#[test]
fn operator_frequency_response_and_current_port_ramp() {
    let d = deck("operators");
    for (i, &f) in d.scale_values.iter().enumerate() {
        let w = std::f64::consts::TAU * f / 1000.;
        let re = 1. / (1. + w * w);
        let im = -w * re;
        for (v, r, j) in [
            ("v(low)", re, im),
            ("v(rate)", -im * w, re * w),
            ("v(area)", re, im),
        ] {
            let a = d.complex(v).unwrap()[i];
            close(a.0, r, 2e-12);
            close(a.1, j, 2e-12);
        }
    }
    let d = deck("ramp");
    for (i, &t) in d.scale_values.iter().enumerate() {
        let s = 1000. * t;
        let area = 0.2 + if s <= 1. { 0.5 * s * s } else { s - 0.5 };
        for v in ["v(area)", "v(load)"] {
            close(d.real(v).unwrap()[i], area, 2e-7);
        }
    }
}
#[test]
fn file_times_and_oscillator_phase() {
    let d = deck("file");
    for (t, r, s) in [
        (0., 0.25, 0.125),
        (2e-4, 0.25, 2.125),
        (5e-4, 0.5, 0.125),
        (1e-3, 0.5, -1.875),
    ] {
        let i = d
            .scale_values
            .iter()
            .position(|v| (t - v).abs() < 1e-16)
            .unwrap();
        close(d.real("v(out)").unwrap()[i], 0.5 * (s - r), 1e-12);
    }
    let d = deck("oscillators");
    let slew = d.real("v(sl)").unwrap();
    for (i, &t) in d.scale_values.iter().enumerate() {
        let cycles = 1000. * t;
        let phase = cycles.fract();
        close(
            d.real("v(sn)").unwrap()[i],
            (phase * std::f64::consts::TAU).sin(),
            1e-8,
        );
        close(
            d.real("v(tr)").unwrap()[i],
            1. - (2. * phase - 1.).abs(),
            1e-8,
        );
        // The first rise starts at 600 us. Later cycles start with the falling edge.
        let pulse = |rise: f64, fall: f64| {
            if cycles < 0.6 {
                0.
            } else if phase < 0.6 {
                (1. - phase / fall).max(0.)
            } else {
                ((phase - 0.6) / rise).min(1.)
            }
        };
        close(d.real("v(sq)").unwrap()[i], pulse(0.01, 0.02), 1e-8);
        close(slew[i], pulse(0.2, 0.25), 1e-8);
        if i > 0 {
            let dt = t - d.scale_values[i - 1];
            let change = slew[i] - slew[i - 1];
            assert!((-4000. * dt - 1e-9..=5000. * dt + 1e-9).contains(&change));
        }
    }
}

#[test]
fn required_parameters_name_the_model_and_missing_field() {
    for (kind, fields) in [
        ("int", &["out_lower_limit=-1", "out_upper_limit=1"][..]),
        ("d_dt", &["out_lower_limit=-1", "out_upper_limit=1"]),
        ("pwl", &["x_array=[0 1]", "y_array=[0 1]"]),
        ("s_xfer", &["num_coeff=[1]", "den_coeff=[1 1]"]),
        ("zener", &["v_breakdown=4.7"]),
        ("multi_input_pwl", &["x=[0 1]", "y=[0 1]"]),
        (
            "core",
            &["h_array=[0 1]", "b_array=[0 1]", "length=1", "area=1"],
        ),
        ("d_genlut", &["table_values=\"10\""]),
        ("pwlts", &["x_array=[0 1]", "y_array=[0 1]"]),
        ("d_lut", &["table_values=\"10\""]),
    ] {
        for (i, field) in fields.iter().enumerate() {
            let params = fields
                .iter()
                .enumerate()
                .filter_map(|(j, &kv)| (j != i).then_some(kv))
                .collect::<Vec<_>>()
                .join(" ");
            let key = field.split_once('=').unwrap().0;
            let errors = parse(&format!(
                "missing field\n.model sample {kind}({params})\n.end"
            ))
            .unwrap_err();
            assert!(
                errors.iter().any(|d| d.code == "E-ELAB-PARAM"
                    && d.span.is_some()
                    && d.message.contains("sample")
                    && d.message.contains(key)),
                "{errors:?}"
            );
        }
    }
}

#[test]
fn dynamic_operator_limits_apply_to_the_output() {
    let net = parse(
        "operator rails
V1 in 0 PWL(0 0 1 2)
Ad in slope dd
Ai in area ii
.model dd d_dt(out_offset=0.25 out_lower_limit=-1 out_upper_limit=1)
.model ii int(out_ic=-0.2 out_lower_limit=-0.1 out_upper_limit=0.3)
.tran 10m 1
.end",
    )
    .unwrap();
    let run = run_with(&net, &RunConfig { threads: 1 }).unwrap();
    let d = &run.results.analyses[0];
    let slope = d.real("v(slope)").unwrap();
    let area = d.real("v(area)").unwrap();
    close(slope[0], 0.25, 1e-10);
    close(area[0], -0.1, 1e-10);
    close(*slope.last().unwrap(), 1., 1e-9);
    close(*area.last().unwrap(), 0.3, 1e-9);
}
#[test]
fn local_model_arrays_roundtrip_and_failures() {
    let src = "local\n.subckt cell a b\nA1 %vd[a 0 b 0] out m\nR1 out 0 1k\n.model m summer(in_gain=[1 2])\n.ends\nV1 x 0 1\nX1 x 0 cell\n.op";
    let nl = parse(src).unwrap();
    let text = sansho_netlist::write(&nl);
    let again = parse(&text).unwrap();
    assert_eq!(nl.elements[0].kind, again.elements[0].kind);
    assert!(text.contains("x1.out"));
    {
        let ds = parse("digital\nA1 [a b] out m\n.model m d_unsupported").unwrap_err();
        assert!(ds.iter().any(|d|d.code=="E-XSPICE-MODEL" && d.message.contains("unsupported code model")));
    }
    for path in ["../waveform.dat", "/outside.dat", "input.dat"] {
        let d = parse(&format!(
            "table\nA1 [out] m\n.model m filesource(file=\"{path}\")"
        ))
        .unwrap_err();
        assert!(d.iter().any(|d| d.code == "E-XSPICE-FILE"));
    }
}

#[test]
fn gain_noise_transfer_sensitivity_and_linear_distortion() {
    let net=parse("gain measurements\nVin in 0 dc 2 ac 1 distof1 0.1\nR1 in mid 1k\nA1 mid out m\nR2 out 0 1k noisy=0\n.model m gain(gain=3)\n.noise v(out) Vin lin 2 10 20\n.sens v(out)\n.disto lin 2 10 20").unwrap();
    let result = run_with(&net, &RunConfig { threads: 1 }).unwrap().results;
    let a = &result.analyses;
    let spectrum = (4. * sansho_devices::BOLTZMANN * 300.15 * 1000.).sqrt();
    close(
        a[0].real("onoise_spectrum").unwrap()[0],
        3. * spectrum,
        1e-17,
    );
    close(a[2].real("m:gain").unwrap()[0], 2., 1e-8);
    for harmonic in &a[3..] {
        assert!(
            harmonic
                .complex("v(out)")
                .unwrap()
                .iter()
                .all(|&v| v == (0., 0.))
        );
    }
}

#[cfg(unix)]
#[test]
fn table_loader_confines_leaf_and_parent_symlinks() {
    use sansho_netlist::{FsLoader, SourceLoader};
    use std::os::unix::fs::symlink;
    let temp = std::env::temp_dir().join(format!("sansho-xspice-paths-{}", std::process::id()));
    let root = temp.join("deck");
    let external = temp.join("external");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("file"), "0 1").unwrap();
    symlink(&external, root.join("sub")).unwrap();
    symlink(external.join("file"), root.join("leaf")).unwrap();
    let loader = FsLoader { base: root };
    for name in ["sub/file", "leaf", "../external/file"] {
        assert!(loader.waveform(Path::new(name), None).is_err());
    }
    std::fs::remove_dir_all(temp).unwrap();
}
