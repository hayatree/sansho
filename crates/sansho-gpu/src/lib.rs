//! Batched GPU device evaluation for runs of one fixed topology (architecture §8, G1).
//!
//! Each run of a batch keeps its own Newton iteration, time step and convergence control on
//! the CPU. Only device evaluation is shared: a run that needs a wrapped group evaluated
//! submits its unknowns to a [`Coordinator`] and blocks. When every live run is waiting,
//! the last one to arrive launches a single kernel for all pending requests and wakes the
//! others. A kernel thread reads only its own run's inputs, so results never depend on how
//! runs were grouped into launches.
//!
//! The derivative part of a device evaluation may optionally use single precision
//! ([`Jacobian::F32`]) during Newton iterations. Residuals, charges and every evaluation
//! outside a Newton iteration (AC, noise, converged linearizations) stay in double precision,
//! so a converged solution satisfies the same tolerances.
//!
//! Provenance: original implementation. See `docs/PROVENANCE.md`.

mod coordinator;
mod group;

pub use coordinator::{Coordinator, GroupPlan, Stats};
pub use group::Batched;

/// Precision of device Jacobians during Newton iterations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Jacobian {
    /// Double precision everywhere (the default).
    #[default]
    F64,
    /// Single-precision derivatives during limited Newton iterations.
    F32,
}

/// GPU batch configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Device ordinal.
    pub device: usize,
    /// Number of concurrently simulated runs, i.e. the largest kernel batch.
    pub lanes: usize,
    /// Jacobian precision during Newton iterations.
    pub jacobian: Jacobian,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: 0,
            lanes: 256,
            jacobian: Jacobian::F64,
        }
    }
}

/// Device types with a GPU kernel.
pub fn supports(type_name: &str) -> bool {
    type_name == "diode"
}
