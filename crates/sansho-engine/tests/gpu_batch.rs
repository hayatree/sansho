//! GPU Monte Carlo against the CPU batch. Skipped on hosts without CUDA.
//! Provenance: original test of the G1 batch path.

#![cfg(feature = "gpu")]
#![allow(clippy::unwrap_used, clippy::panic)]
use sansho_engine::RunConfig;
use sansho_engine::gpu_batch::{GpuConfig, Jacobian};
use sansho_engine::monte_carlo::MonteCarlo;
use sansho_netlist::NoFiles;

const DECK: &str = include_str!("../../../verify/mc-decks/diode_clamp.cir");

fn gpu(
    plan: &MonteCarlo,
    lanes: usize,
    jacobian: Jacobian,
) -> Option<sansho_engine::monte_carlo::Report> {
    let config = GpuConfig {
        lanes,
        jacobian,
        ..GpuConfig::default()
    };
    match plan.batch_gpu(5, 24, false, &config) {
        Ok((report, stats)) => {
            assert!(stats.launches > 0 && stats.requests >= stats.launches);
            Some(report)
        }
        Err(d) if d[0].code == "E-GPU" => {
            eprintln!("skipping GPU batch test: {}", d[0].message);
            None
        }
        Err(d) => panic!("{d:?}"),
    }
}

#[test]
fn gpu_batch_matches_cpu_within_reltol_and_ignores_lane_count() {
    let plan = MonteCarlo::prepare(DECK, &NoFiles).unwrap();
    let Some(wide) = gpu(&plan, 24, Jacobian::F64) else {
        return;
    };
    let narrow = gpu(&plan, 5, Jacobian::F64).unwrap();
    assert_eq!(wide, narrow);
    let cpu = plan.batch(5, 24, false, &RunConfig { threads: 4 }).unwrap();
    let f32 = gpu(&plan, 24, Jacobian::F32).unwrap();
    for other in [&wide, &f32] {
        for (a, b) in cpu.runs.iter().zip(&other.runs) {
            assert!(a.success && b.success);
            for (x, y) in a.measures.iter().zip(&b.measures) {
                let (x, y) = (x.value.unwrap(), y.value.unwrap());
                assert!((x - y).abs() <= 1e-3 * x.abs(), "{x} vs {y}");
            }
        }
    }
}

#[test]
fn circuits_without_kernels_are_rejected() {
    let deck = include_str!("../../../verify/mc-decks/rc_delay.cir");
    let plan = MonteCarlo::prepare(deck, &NoFiles).unwrap();
    let err = plan
        .batch_gpu(1, 2, false, &GpuConfig::default())
        .unwrap_err();
    assert_eq!(err[0].code, "E-GPU-UNSUPPORTED");
}
