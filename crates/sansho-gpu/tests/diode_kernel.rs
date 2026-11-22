//! The diode kernel against the CPU diode group, evaluation by evaluation.
//! Provenance: original test. Model cards exercise every branch of `diode.rs`.
//!
//! Skipped (with a message) on hosts without a usable CUDA driver.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use sansho_devices::diode::{DiodeModel, Diodes};
use sansho_devices::model::ModelParams;
use sansho_devices::temperature::Temperature;
use sansho_devices::{DeviceGroup, EvalCtx, Mode, Slots, StampBuilder, StateRef, Unknowns};
use sansho_gpu::{Batched, Config, Coordinator, GroupPlan, Jacobian};
use std::sync::Arc;

fn model(pairs: &[(&str, f64)], celsius: f64) -> DiodeModel {
    let p = ModelParams::new(
        &pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect::<Vec<_>>(),
    );
    let (mut m, ignored) = DiodeModel::from_params(&p);
    assert!(ignored.is_empty(), "{ignored:?}");
    m.at_temperature(Temperature {
        kelvin: celsius + 273.15,
        nominal: 300.15,
    });
    m
}

fn group() -> (Diodes, Unknowns, sansho_devices::StampLayout) {
    let cards: Vec<DiodeModel> = vec![
        model(&[], 27.),
        model(&[("rs", 3.), ("cjo", 2e-12), ("tt", 4e-9), ("m", 0.4)], 27.),
        model(
            &[
                ("bv", 5.1),
                ("ibv", 1e-3),
                ("rs", 1.),
                ("n", 1.1),
                ("tcv", 1e-3),
            ],
            85.,
        ),
        model(
            &[("isr", 1e-12), ("nr", 2.), ("ikf", 0.05), ("vj", 0.8)],
            27.,
        ),
        model(
            &[
                ("jsw", 1e-15),
                ("cjp", 1e-13),
                ("ns", 1.3),
                ("fc", 0.9),
                ("m", 1.),
            ],
            -40.,
        ),
        model(
            &[("is", 1e-9), ("bv", 40.), ("ibv", 1e-9), ("nbv", 3.)],
            27.,
        ),
    ];
    let mut d = Diodes::default();
    let mut u = Unknowns::with_nodes(["a", "b", "c", "d", "e"]);
    for (i, mut m) in cards.into_iter().enumerate() {
        m.perimeter = 1e-4 * (i as f64 + 1.);
        let a = Some(i % 5);
        let k = if i % 2 == 0 { None } else { Some((i + 2) % 5) };
        d.push(&format!("d{i}"), a, k, m, 1. + 0.5 * i as f64, i == 3);
    }
    d.alloc_unknowns(&mut u);
    let mut b = StampBuilder::new(&u);
    d.setup(&mut b).unwrap();
    let layout = b.finish();
    (d, u, layout)
}

/// Deterministic pseudo-random voltages in [-8, 2] V.
fn voltages(seed: u64, n: usize) -> Vec<f64> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            -8. + 10. * (s >> 11) as f64 / (1u64 << 53) as f64
        })
        .collect()
}

fn close(a: &[f64], b: &[f64], tol: f64, what: &str) {
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let scale = x.abs().max(y.abs()).max(1e-300);
        assert!(
            x == y || (x - y).abs() <= tol * scale,
            "{what}[{i}]: cpu {x:e} gpu {y:e}"
        );
    }
}

fn open(jacobian: Jacobian) -> Option<(Arc<Diodes>, Unknowns, Slots, Batched)> {
    let (d, u, layout) = group();
    let (topology, params) = d.batch_rows().unwrap();
    let plan = GroupPlan {
        topology,
        slots: [
            layout.g.len(),
            layout.c.len(),
            layout.f.len(),
            layout.q.len(),
        ],
    };
    let config = Config {
        lanes: 1,
        jacobian,
        ..Config::default()
    };
    let coordinator = match Coordinator::new(&config, vec![plan], u.len()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU kernel test: {}", e.message);
            return None;
        }
    };
    coordinator.bind(0, 0, &params).unwrap();
    let d = Arc::new(d);
    let gpu = Batched::new(d.clone(), coordinator, 0, 0);
    Some((d, u, Slots::for_layout(&layout), gpu))
}

#[test]
fn kernel_matches_cpu_evaluation() {
    let Some((cpu, u, slots, gpu)) = open(Jacobian::F64) else {
        return;
    };
    let n = cpu.len();
    for seed in 0..200u64 {
        let x = voltages(seed, u.len());
        let old = voltages(seed + 1000, n);
        for (limit, init) in [(true, false), (false, false), (true, true)] {
            let ctx = EvalCtx {
                limit,
                init_junctions: init,
                gmin: 1e-12,
                mode: Mode::Tran,
                ..EvalCtx::dc()
            };
            let (mut a, mut b) = (slots.clone(), slots.clone());
            let (mut sa, mut sb) = (old.clone(), old.clone());
            cpu.eval(
                &ctx,
                &x,
                StateRef {
                    accepted: &old,
                    current: &mut sa,
                },
                &mut a,
            );
            gpu.eval(
                &ctx,
                &x,
                StateRef {
                    accepted: &old,
                    current: &mut sb,
                },
                &mut b,
            );
            assert!(b.error.is_none(), "{:?}", b.error);
            assert_eq!(a.noncon, b.noncon, "seed {seed}");
            // Only exp/log/pow differ between the CUDA and host math libraries.
            close(&sa, &sb, 0., "state");
            close(&a.g, &b.g, 1e-12, "g");
            close(&a.c, &b.c, 1e-12, "c");
            close(&a.f, &b.f, 1e-12, "f");
            close(&a.q, &b.q, 1e-12, "q");
        }
    }
}

#[test]
fn single_precision_jacobian_keeps_double_residuals() {
    let Some((cpu, u, slots, gpu)) = open(Jacobian::F32) else {
        return;
    };
    let n = cpu.len();
    for seed in 0..50u64 {
        let x = voltages(seed, u.len());
        let old = voltages(seed + 7, n);
        // Unlimited evaluations (converged linearizations) stay in double precision.
        let exact = EvalCtx {
            limit: false,
            mode: Mode::Dc,
            ..EvalCtx::dc()
        };
        let (mut a, mut b) = (slots.clone(), slots.clone());
        let (mut sa, mut sb) = (old.clone(), old.clone());
        cpu.eval(
            &exact,
            &x,
            StateRef {
                accepted: &old,
                current: &mut sa,
            },
            &mut a,
        );
        gpu.eval(
            &exact,
            &x,
            StateRef {
                accepted: &old,
                current: &mut sb,
            },
            &mut b,
        );
        close(&a.g, &b.g, 1e-12, "g");
        close(&a.f, &b.f, 1e-12, "f");
    }
}
