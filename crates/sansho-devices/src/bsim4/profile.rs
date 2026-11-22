//! Job-local BSIM4 work counters and sampled equation costs.
//! Provenance: original instrumentation of this repository's BSIM4 equations.

use super::{CoreCache, Size};
use crate::dual::Dual;
use std::{
    hint::black_box,
    io::Write,
    time::{Duration, Instant},
};

/// Replays are isolated from simulation state and are opt-in. Use the ordinary
/// aggregate phase timers (without replays) for wall measurements.
#[derive(Debug, Default)]
pub(super) struct Profile {
    pub(super) visits: u64,
    pub(super) full: u64,
    pub(super) samples: u64,
    pub(super) scalar: u64,
    probes: u64,
    pub(super) shared: u64,
    pub(super) approximate: u64,
    pub(super) charge_bypass: u64,
    pub(super) scalar_time: Duration,
    intrinsic: Duration,
    primal: Duration,
    gate: Duration,
    junction_overlap: Duration,
}

impl Profile {
    pub(super) fn evaluation<const N: usize>(&mut self, m: &Size, lin: [f64; 4]) {
        self.full += 1;
        if self.full % 1024 != 1 {
            return;
        }
        self.probes += 1;
        let v = if N == 4 {
            std::array::from_fn(|i| Dual::<N>::var(lin[i], i))
        } else {
            [
                Dual::var(lin[0], 0),
                Dual::var(lin[1], 1),
                Dual::cst(lin[2]),
                Dual::var(lin[3], 2),
            ]
        };
        self.intrinsic += measure(|| m.terminal_equations(black_box(v)));
        self.primal += measure(|| m.terminal_equations(black_box(lin.map(Dual::<0>::cst))));
        let (_, _, bias) = m.terminal_equations(v);
        if m.gate.enabled() {
            self.gate += measure(|| {
                let g = m.tunneling(black_box(&bias));
                let edges = std::array::from_fn::<_, 2, _>(|i| {
                    m.edge_tunneling(black_box((v[1] - v[i * 2]) * m.sign), i)
                });
                (g, edges)
            });
        }
        self.junction_overlap += measure(|| {
            let ov = m.overlaps(black_box(std::array::from_fn(|i| {
                Dual::<4>::var(lin[i], i)
            })));
            let js = std::array::from_fn::<_, 2, _>(|i| {
                m.junction[i].eval(
                    black_box(Dual::<1>::var((lin[3] - lin[i * 2]) * m.sign, 0)),
                    black_box(1e-12),
                )
            });
            (ov, js)
        });
    }
}

// Warm both derivative widths before measuring: the zero-lane full IV kernel
// otherwise executes only once per thousand misses and is instruction-cache cold.
fn measure<T>(mut eval: impl FnMut() -> T) -> Duration {
    black_box(eval());
    let start = Instant::now();
    for _ in 0..8 {
        black_box(eval());
    }
    start.elapsed() / 8
}

impl CoreCache {
    /// Enable per-group counters and occasional timed replays for detailed profiling.
    pub fn enable_profile(&mut self) {
        self.terminal.profile = Some(Profile::default());
    }

    /// Emit pooled counts and mean sampled costs in nanoseconds.
    pub fn report_profile(caches: &[Self], out: &mut impl Write) -> std::io::Result<()> {
        let mut total = Profile::default();
        for p in caches.iter().filter_map(|c| c.terminal.profile.as_ref()) {
            total.visits += p.visits;
            total.full += p.full;
            total.samples += p.samples;
            total.scalar += p.scalar;
            total.probes += p.probes;
            total.approximate += p.approximate;
            total.charge_bypass += p.charge_bypass;
            total.shared += p.shared;
            total.scalar_time += p.scalar_time;
            total.intrinsic += p.intrinsic;
            total.primal += p.primal;
            total.gate += p.gate;
            total.junction_overlap += p.junction_overlap;
        }
        if total.visits == 0 {
            return Ok(());
        }
        writeln!(
            out,
            "sansho-bsim4 visits={} evaluations={} charge_visits={} scalar_evaluations={} probes={} shared={} bypass={} charge_bypass={}",
            total.visits,
            total.full,
            total.samples,
            total.scalar,
            total.probes,
            total.shared,
            total.approximate,
            total.charge_bypass
        )?;
        let ns = |d: Duration| d.as_secs_f64() * 1e9 / (total.probes.max(1) as f64);
        writeln!(
            out,
            "sansho-bsim4-ns intrinsic={:.1} scalar={:.1} ad_overhead={:.1} gate={:.1} overlap_junction={:.1} charge_only={:.1}",
            ns(total.intrinsic),
            ns(total.primal),
            ns(total.intrinsic) - ns(total.primal),
            ns(total.gate),
            ns(total.junction_overlap),
            total.scalar_time.as_secs_f64() * 1e9
                / caches
                    .iter()
                    .filter_map(|c| c.terminal.profile.as_ref())
                    .map(|p| p.scalar.div_ceil(1024))
                    .sum::<u64>()
                    .max(1) as f64
        )
    }
}
