//! GPU-batched Monte Carlo (architecture §8, G1).
//!
//! Every lane is an OS thread that simulates runs `lane, lane + lanes, ...` with the
//! ordinary CPU engine inside its own one-worker pool. Device groups with a GPU kernel are
//! replaced after binding by [`sansho_gpu::Batched`], which hands their evaluations to a
//! shared coordinator. A run whose rows cannot be expressed for the kernel keeps the CPU
//! group, so coverage never changes results, only where they are computed.
//!
//! Provenance: original implementation. See `docs/PROVENANCE.md`.

use super::monte_carlo::{MonteCarlo, Report, Run};
use sansho_devices::DeviceGroup;
use sansho_devices::diode::Diodes;
use sansho_diag::Diagnostic;
use sansho_gpu::{Batched, Coordinator, GroupPlan};
pub use sansho_gpu::{Config as GpuConfig, Jacobian, Stats as GpuStats};
use std::sync::{Arc, Mutex, PoisonError};

fn diodes(g: &Arc<dyn DeviceGroup>) -> Option<&Diodes> {
    if !sansho_gpu::supports(g.type_name()) {
        return None;
    }
    (g.as_ref() as &dyn std::any::Any).downcast_ref::<Diodes>()
}

struct Leave<'a>(&'a Coordinator);

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

impl MonteCarlo {
    /// Run indices `0..n` with device evaluation batched on a GPU.
    ///
    /// Returns the same report shape as [`MonteCarlo::batch`] together with launch counters.
    pub fn batch_gpu(
        &self,
        seed: u64,
        n: usize,
        save_waves: bool,
        config: &GpuConfig,
    ) -> Result<(Report, GpuStats), Vec<Diagnostic>> {
        if n == 0 {
            return Err(vec![Diagnostic::error(
                "E-MC-COUNT",
                "at least one run is required",
            )]);
        }
        let base = &self.base.circuit;
        let mut plans = Vec::new();
        let mut targets = Vec::new();
        for (i, g) in base.groups.iter().enumerate() {
            let Some((topology, _)) = diodes(g).and_then(Diodes::batch_rows) else {
                continue;
            };
            let l = &base.layouts[i];
            plans.push(GroupPlan {
                topology,
                slots: [l.g.len(), l.c.len(), l.f.len(), l.q.len()],
            });
            targets.push(i);
        }
        if plans.is_empty() {
            return Err(vec![Diagnostic::error(
                "E-GPU-UNSUPPORTED",
                "no device group of this circuit has a GPU kernel",
            )]);
        }
        let lanes = config.lanes.clamp(1, n);
        let config = GpuConfig { lanes, ..*config };
        let coordinator = Coordinator::new(&config, plans, base.dim()).map_err(|d| vec![d])?;
        let rows: Vec<Mutex<Option<Run>>> = (0..n).map(|_| Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            for lane in 0..lanes {
                let (coordinator, targets, rows) = (&coordinator, &targets, &rows);
                let spawned = std::thread::Builder::new()
                    .name(format!("sansho-gpu-lane-{lane}"))
                    .spawn_scoped(scope, move || {
                        let _leave = Leave(coordinator);
                        super::parallel::with_threads(1, || {
                            for index in (lane..n).step_by(lanes) {
                                let index = index as u64;
                                let outcome = self.run_bound(seed, index, |c| {
                                    bind(c, coordinator, lane, targets)
                                });
                                let row = Self::row_from(seed, index, save_waves, outcome);
                                *rows[index as usize]
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner) = Some(row);
                            }
                        });
                    });
                if spawned.is_err() {
                    coordinator.leave();
                }
            }
        });
        let stats = coordinator.stats();
        let runs = rows
            .into_iter()
            .enumerate()
            .map(|(i, cell)| {
                cell.into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .unwrap_or_else(|| Run {
                        seed,
                        index: i as u64,
                        success: false,
                        measures: Vec::new(),
                        diagnostics: vec![Diagnostic::error(
                            "E-GPU",
                            "batch lane terminated before finishing this run",
                        )],
                        waves: None,
                    })
            })
            .collect();
        Ok((self.report(seed, runs), stats))
    }
}

/// Replace supported groups of one bound run by GPU-backed adapters.
fn bind(
    c: &mut super::circuit::Circuit,
    coordinator: &Arc<Coordinator>,
    lane: usize,
    targets: &[usize],
) -> Result<(), Vec<Diagnostic>> {
    for (group, &i) in targets.iter().enumerate() {
        let Some(g) = c.groups.get(i).cloned() else {
            continue;
        };
        let Some((topology, params)) = diodes(&g).and_then(Diodes::batch_rows) else {
            continue;
        };
        if coordinator.plans().get(group).map(|p| &p.topology) != Some(&topology) {
            continue;
        }
        coordinator
            .bind(lane, group, &params)
            .map_err(|d| vec![d])?;
        c.groups[i] = Arc::new(Batched::new(g, coordinator.clone(), lane, group));
    }
    Ok(())
}
