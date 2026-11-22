//! Monte Carlo distribution, isolation and deterministic replay tests.
//! Provenance: original circuits and analytic product moments of independent variables.

#![allow(clippy::unwrap_used)]
use sansho_engine::{RunConfig, monte_carlo::MonteCarlo};
use sansho_netlist::NoFiles;

const RC: &str = include_str!("../../../verify/mc-decks/rc_delay.cir");

#[test]
fn independent_rc_product_moments() {
    let p = MonteCarlo::prepare(RC, &NoFiles).unwrap();
    let report = p.batch(1, 256, false, &RunConfig { threads: 2 }).unwrap();
    let stats = &report.statistics[0];
    let mean = 1e-6 * std::f64::consts::LN_2;
    let std = mean * (0.03f64.powi(2) + 0.04f64.powi(2) + (0.03f64 * 0.04).powi(2)).sqrt();
    assert_eq!(stats.count, 256);
    assert!((stats.mean.unwrap() - mean).abs() < 4. * std / 16.);
    assert!((stats.std.unwrap() / std - 1.).abs() < 0.16);
    assert!(report.runs.iter().all(|r| r.success && r.waves.is_none()));
}

#[test]
fn worker_count_and_random_access_replay() {
    let plan = MonteCarlo::prepare(RC, &NoFiles).unwrap();
    let serial = plan
        .batch(993, 12, true, &RunConfig { threads: 1 })
        .unwrap();
    let parallel = plan
        .batch(993, 12, true, &RunConfig { threads: 4 })
        .unwrap();
    assert_eq!(serial, parallel);
    let replay = MonteCarlo::prepare(RC, &NoFiles)
        .unwrap()
        .run_with(993, 7, &RunConfig { threads: 1 })
        .unwrap();
    assert_eq!(
        replay.results.analyses,
        serial.runs[7].waves.as_ref().unwrap().as_slice()
    );
    assert_eq!(replay.results.measures, serial.runs[7].measures);
    assert!(plan.batch(0, 0, false, &RunConfig { threads: 1 }).is_err());
}

#[test]
fn a_bad_sample_does_not_abort_neighbors() {
    let deck = "failure isolation\nV1 a 0 1\nR1 a 0 {limit(1,1)}\n.dc V1 0 1 1\n.meas dc current find i(v1) at=1\n.end";
    let p = MonteCarlo::prepare(deck, &NoFiles).unwrap();
    let report = p.batch(4, 32, false, &RunConfig { threads: 2 }).unwrap();
    let good = report.runs.iter().filter(|r| r.success).count();
    assert!(good > 0 && good < 32);
    assert_eq!(report.statistics[0].count, good);
    for r in report.runs.iter().filter(|r| !r.success) {
        assert_eq!(p.run(4, r.index).unwrap_err().diagnostics, r.diagnostics);
    }
}
