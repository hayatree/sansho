//! Optional DC and Jacobian smoke tests for locally supplied compact models.
//! Provenance: original test harness using the sansho-va public API and central differences.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sansho_va::*;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

struct Includes(PathBuf);
impl VaLoader for Includes {
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
        let parent = from
            .filter(|p| *p != "<input>")
            .and_then(|p| Path::new(p).parent())
            .unwrap_or(&self.0);
        let file = parent.join(path);
        fs::read_to_string(&file)
            .map(|text| (file.display().to_string(), text))
            .map_err(|e| e.to_string())
    }
}

fn smoke(file: &str, defines: &str, expected: Option<&str>) {
    let root = std::env::var_os("SANSHO_VA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../va-models/ihp"));
    let path = root.join(file);
    if !path.try_exists().unwrap() {
        eprintln!("skipping missing {}", path.display());
        return;
    }
    let text = format!("{defines}{}", fs::read_to_string(&path).unwrap());
    let loader = Includes(path.parent().unwrap().to_owned());
    let start = Instant::now();
    let ast = parse_va(&text, &loader).unwrap_or_else(|e| panic!("{file}: {:?}", e.first()));
    let module = compile_module(&ast, expected).unwrap_or_else(|e| {
        panic!(
            "{file}: {:?} in {:?}",
            e.first(),
            e[0].span.and_then(|s| ast.sources.get(s.file as usize))
        )
    });
    let compile_time = start.elapsed();
    let defaults = module.bind(&[]).unwrap();
    let b = if file.contains("psp103") && !file.contains("juncap") {
        module
            .bind_instance(
                &defaults,
                &[("TYPE".into(), 1.), ("L".into(), 1e-6), ("W".into(), 1e-6)],
            )
            .unwrap()
    } else {
        defaults
    };
    let mut x = vec![0.; b.layout().unknown_len()];
    for (i, v) in b
        .layout()
        .representatives()
        .iter()
        .zip(&mut x)
        .filter(|(i, _)| **i < module.ports.len() + module.internal_nodes.len())
    {
        *v = 0.03 + 0.017 * (*i) as f64;
    }
    if file == "psp103/psp103.va" {
        x[b.layout().index(0).unwrap()] = 1.2;
        x[b.layout().index(1).unwrap()] = 0.85;
        x[b.layout().index(2).unwrap()] = 0.;
        x[b.layout().index(3).unwrap()] = 0.;
    }
    let eval = |x: &[f64], scratch: &mut Scratch, initial_step: bool| {
        let mut out = EvalOutput::default();
        let state = vec![0.; module.state_len()];
        let mut current = state.clone();
        let start = Instant::now();
        module.eval_with_scratch(
            &b,
            &EvalInput {
                x,
                temperature: 300.15,
                analysis: Analysis::Dc,
                initial_step,
                final_step: false,
                abstime: 0.,
                gmin: 1e-12,
                limit: false,
                simparams: &|_| None,
                mfactor: 1.,
            },
            StateIo {
                accepted: &state,
                current: &mut current,
            },
            scratch,
            &mut out,
        );
        let elapsed = start.elapsed();
        assert_eq!(out.error, None, "{file}");
        for values in [&out.f, &out.q, &out.df, &out.dq] {
            assert!(values.iter().all(|v| v.is_finite()), "{file}: {values:?}");
        }
        (out, elapsed)
    };
    let mut scratch = Scratch::default();
    let (reference, cold) = eval(&x, &mut scratch, true);
    #[cfg(feature = "jit")]
    if std::env::var_os("SANSHO_VA_JIT").is_none_or(|v| v != "0") {
        let start = Instant::now();
        assert!(module.prepare_jit(&b, 300.15, 1.).is_none());
        eprintln!("{} native compilation {:?}", module.name, start.elapsed());
    }
    let (warm_output, specialize) = eval(&x, &mut scratch, false);
    if file == "psp103/psp103.va" {
        for (cold, warm) in [
            (&reference.f, &warm_output.f),
            (&reference.q, &warm_output.q),
            (&reference.df, &warm_output.df),
            (&reference.dq, &warm_output.dq),
        ] {
            for (&a, &b) in cold.iter().zip(warm) {
                assert_eq!(a.to_bits(), b.to_bits(), "{file}: cold {a:e}, warm {b:e}");
            }
        }
    }
    let mut samples: Vec<_> = (0..11).map(|_| eval(&x, &mut scratch, false).1).collect();
    samples.sort_unstable();
    let warm = samples[samples.len() / 2];
    eprintln!(
        "{}: compile {compile_time:?}, initialized evaluation {cold:?}, second evaluation (includes specialization) {specialize:?}, median warm evaluation {warm:?}, {} bound unknowns",
        module.name,
        x.len()
    );
    #[cfg(feature = "jit")]
    {
        let native = std::env::var_os("SANSHO_VA_JIT").is_none_or(|v| v != "0");
        assert_eq!(native, scratch.used_jit(), "{}", module.name);
        eprintln!("{} native specialization: {native}", module.name);
        equivalence(&module, &b, &x, native);
        // Exercise independent bindings on both sides of a legal parameter value.
        let alternates = module
            .params
            .iter()
            .filter(|p| !p.local && p.ty == "real" && p.name != "m")
            .find_map(|p| {
                let initial = module.param(&b, &p.name)?;
                if initial == 0. {
                    return None;
                }
                let bind = |scale| module.bind(&[(p.name.clone(), initial * scale)]).ok();
                Some([bind(0.99)?, bind(1.01)?])
            })
            .unwrap();
        for alternate in alternates {
            let bias: Vec<_> = alternate
                .layout()
                .representatives()
                .iter()
                .map(|&i| b.layout().index(i).map_or(0., |j| x[j]))
                .collect();
            equivalence(&module, &alternate, &bias, native);
        }
    }
    for j in 0..x.len() {
        let h = 1e-6;
        let mut hi = x.clone();
        let mut lo = x.clone();
        hi[j] += h;
        lo[j] -= h;
        let (hi, _) = eval(&hi, &mut Scratch::default(), true);
        let (lo, _) = eval(&lo, &mut Scratch::default(), true);
        for (a, b, jac, floor) in [
            (&hi.f, &lo.f, &reference.df, 1e-12),
            (&hi.q, &lo.q, &reference.dq, 1e-23),
        ] {
            for i in 0..x.len() {
                let fd = (a[i] - b[i]) / (2. * h);
                let ad = jac[i * x.len() + j];
                let roundoff = 8. * f64::EPSILON * (a[i].abs() + b[i].abs()) / h;
                assert!(
                    (fd - ad).abs() <= floor + 1e-5 * fd.abs().max(ad.abs()) + roundoff,
                    "{file} ({i},{j}): central {fd:e}, AD {ad:e}"
                );
            }
        }
    }
}
macro_rules! model_tests {
    ($($test:ident: $path:literal),+ $(,)?) => {
        $(#[test] fn $test() { smoke($path, "", None); })+
    };
}
model_tests! {
    psp103: "psp103/psp103.va",
    thermal: "psp103/psp103t.va",
    nqs: "psp103/psp103_nqs.va",
    junction: "psp103/juncap200.va",
    bipolar: "vbic_1p3/vbic_1p3.va",
    resistor: "r3_cmc/r3_cmc.va",
    varactor: "mosvar/mosvar.va",
}

#[test]
fn psp_named_variant() {
    smoke("psp103/psp103.va", "`define __XYCE__\n", Some("PSP103_VA"));
}
#[test]
fn vbic_three_terminal_variant() {
    smoke(
        "vbic_1p3/vbic_1p3.va",
        "`define threeTerminal\n`define __XYCE__\n",
        Some("vbic13_va"),
    );
}

#[cfg(feature = "jit")]
#[test]
fn vbic_five_terminal_variant() {
    smoke(
        "vbic_1p3/vbic_1p3.va",
        "`define fiveTerminal\n",
        Some("vbic13_5t"),
    );
}

#[cfg(feature = "jit")]
fn equivalence(module: &CompiledModule, b: &Bound, bias: &[f64], expect_native: bool) {
    let mut work = [Scratch::default(), Scratch::default()];
    work[1].set_jit(false);
    let mut outputs = [EvalOutput::default(), EvalOutput::default()];
    let accepted = vec![0.; module.state_len()];
    let mut current = [accepted.clone(), accepted.clone()];
    let mut used = false;
    for k in 0..6 {
        let x: Vec<_> = bias
            .iter()
            .enumerate()
            .map(|(i, v)| v + (k as f64 - 2.) * 0.002 * (1. + (i % 3) as f64))
            .collect();
        for path in 0..2 {
            let input = EvalInput {
                x: &x,
                temperature: 300.15,
                analysis: if k % 2 == 0 {
                    Analysis::Dc
                } else {
                    Analysis::Tran
                },
                initial_step: k == 0,
                final_step: k == 5,
                abstime: k as f64 * 1e-12,
                gmin: 1e-12,
                limit: k % 3 != 0,
                simparams: &|_| None,
                mfactor: 1.,
            };
            module.eval_with_scratch(
                b,
                &input,
                StateIo {
                    accepted: &accepted,
                    current: &mut current[path],
                },
                &mut work[path],
                &mut outputs[path],
            );
        }
        used |= work[0].used_jit();
        let [a, b] = &outputs;
        assert_eq!(a.error, None, "{} at point {k}", module.name);
        assert_eq!(a.error, b.error);
        assert_eq!(a.noncon, b.noncon);
        for (name, a, b) in [
            ("f", &a.f, &b.f),
            ("q", &a.q, &b.q),
            ("df", &a.df, &b.df),
            ("dq", &a.dq, &b.dq),
            ("state", &current[0], &current[1]),
        ] {
            assert_eq!(a.len(), b.len());
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                assert_eq!(
                    a.to_bits(),
                    b.to_bits(),
                    "{} point {k} {name}[{i}]: {a:e} vs {b:e}",
                    module.name
                );
            }
        }
    }
    if std::env::var_os("SANSHO_VA_JIT").is_none_or(|v| v != "0") {
        assert_eq!(used, expect_native, "backend for {}", module.name);
    }
}

#[cfg(feature = "jit")]
#[test]
fn committed_model_equivalence() {
    for (file, parameter, alternate, native) in [
        ("resistor", "r", 27., true),
        ("collapse", "RS", 4., true),
        ("diode", "rs", 3., true),
        ("mos", "beta", 0.002, false),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/models")
            .join(format!("{file}.va"));
        let ast = parse_va(&fs::read_to_string(path).unwrap(), &NoVaFiles).unwrap();
        let m = compile_module(&ast, None).unwrap();
        for params in [
            vec![],
            vec![(parameter.to_string(), alternate)],
            vec![(parameter.to_string(), alternate * 2.)],
        ] {
            let b = m.bind(&params).unwrap();
            let x: Vec<_> = (0..b.layout().unknown_len())
                .map(|i| 0.06 * i as f64)
                .collect();
            equivalence(&m, &b, &x, native);
        }
    }
}
