//! Ideal two-port with a finite propagation delay.
//! Provenance: Branin (1967), method of characteristics for lossless lines.

use crate::{DeviceGroup, EvalCtx, Mode, Node, Slots, StampBuilder, StateRef, Unknowns, volt};
use sansho_diag::Diagnostic;

#[derive(Debug)]
pub struct Line {
    name: String,
    ports: [[Node; 2]; 2],
    currents: [Node; 2],
    z0: f64,
    pub delay: f64,
}

impl Line {
    pub fn new(
        name: &str,
        [p, n, q, m]: [Node; 4],
        z0: f64,
        delay: f64,
    ) -> Result<Self, Diagnostic> {
        if ![z0, delay].iter().all(|v| v.is_finite() && *v > 0.) {
            return Err(Diagnostic::error(
                "E-LINE-PARAM",
                "Z0 and TD must be positive and finite",
            ));
        }
        Ok(Self {
            name: name.into(),
            ports: [[p, n], [q, m]],
            currents: [None; 2],
            z0,
            delay,
        })
    }
    fn voltage(&self, x: &[f64], port: usize) -> f64 {
        let [p, n] = self.ports[port];
        volt(x, p) - volt(x, n)
    }
    pub fn initial_waves(&self, [v1, i1, v2, i2]: [f64; 4]) -> [f64; 2] {
        [v1 + self.z0 * i1, v2 + self.z0 * i2]
    }
    pub fn outgoing(&self, x: &[f64]) -> [f64; 2] {
        [0, 1].map(|k| self.voltage(x, k) + self.z0 * volt(x, self.currents[k]))
    }
    /// Entries of G carrying the opposite port's outgoing wave.
    pub fn delayed_entries(&self) -> impl Iterator<Item = (Node, Node, f64)> {
        [0, 1].into_iter().flat_map(|k| {
            let [p, n] = self.ports[1 - k];
            let row = self.currents[k];
            [
                (row, p, -1.),
                (row, n, 1.),
                (row, self.currents[1 - k], -self.z0),
            ]
        })
    }
}

impl DeviceGroup for Line {
    fn instance_ic(&self, _: usize, ic: &[f64], x: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [v1, i1, v2, i2] = ic {
            for (k, (v, cur)) in [(*v1, *i1), (*v2, *i2)].into_iter().enumerate() {
                edges.push((self.ports[k][0], self.ports[k][1], v));
                if let Some(i) = self.currents[k] {
                    x[i] = cur;
                }
            }
        }
    }
    fn type_name(&self) -> &'static str {
        "tline"
    }
    fn line(&self) -> Option<&Line> {
        Some(self)
    }
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        true
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (k, i) in self.currents.iter_mut().enumerate() {
            *i = Some(u.alloc_branch(&format!("{}#{}", self.name, k + 1)));
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for k in [0, 1] {
            let [p, n] = self.ports[k];
            let i = self.currents[k];
            for (r, c) in [(p, i), (n, i), (i, p), (i, n), (i, i)] {
                b.g(r, c);
            }
            for row in [p, n, i] {
                b.f(row);
            }
            b.b(i); // The incident wave is known throughout a transient trial.
        }
        for (row, col, _) in self.delayed_entries() {
            b.g(row, col);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        if let Some(o) = out.outputs.first_mut() {
            o.extend([
                ("i1", volt(x, self.currents[0])),
                ("i2", volt(x, self.currents[1])),
                ("v1", self.voltage(x, 0)),
                ("v2", self.voltage(x, 1)),
            ]);
        }
        let propagating = ctx.mode == Mode::Tran && ctx.time > 0.;
        let waves = if propagating {
            [0.; 2]
        } else {
            out.b.fill(0.);
            self.outgoing(x)
        };
        for k in [0, 1] {
            let current = volt(x, self.currents[k]);
            out.f[3 * k..3 * k + 3].copy_from_slice(&[
                current,
                -current,
                self.voltage(x, k) - self.z0 * current - waves[1 - k],
            ]);
            out.g[5 * k..5 * k + 5].copy_from_slice(&[1., -1., 1., -1., -self.z0]);
        }
        for (slot, (_, _, v)) in out.g[10..].iter_mut().zip(self.delayed_entries()) {
            *slot = if propagating { 0. } else { v };
        }
    }
}
