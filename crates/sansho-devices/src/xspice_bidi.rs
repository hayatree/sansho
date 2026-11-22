//! Electrical load line of a bidirectional mixed-signal bridge.
//! Provenance: ngspice 44 user manual §8.3.3 and Kirchhoff's current law.
use super::{invalid, positive};
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, Slots, StampBuilder, StateRef, Unknowns, dual::Dual,
    volt,
};
use sansho_diag::Diagnostic;

#[derive(Debug)]
pub struct Bidi {
    name: String,
    pair: [Node; 2],
    controls: [Node; 4],
    rails: [f64; 2],
    limit: [f64; 2],
    resistance: [f64; 4],
    smooth: f64,
}
impl Bidi {
    pub fn new(name: String, pair: [Node; 2], p: &ModelParams) -> Result<Self, Diagnostic> {
        let rails = [p.get("out_low", 0.), p.get("out_high", 3.3)];
        let limit = [p.get("drive_low", 0.02), p.get("drive_high", 0.02)];
        let resistance = [
            p.get("r_stl", 20.),
            p.get("r_sth", 20.),
            p.get("r_low", 1e4),
            p.get("r_high", 1e4),
        ];
        let smooth = p.get("smooth", 0.);
        if rails[0] >= rails[1]
            || resistance.iter().any(|v| *v < 1e-6)
            || limit.iter().any(|v| *v <= 0.)
            || ![0., 1., 2.].contains(&smooth)
        {
            return Err(invalid("invalid bidi_bridge drive characteristic"));
        }
        Ok(Self {
            name,
            pair,
            controls: [None; 4],
            rails,
            limit,
            resistance,
            smooth,
        })
    }
}
impl DeviceGroup for Bidi {
    fn type_name(&self) -> &'static str {
        "bidirectional bridge"
    }
    fn len(&self) -> usize {
        1
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (i, c) in self.controls.iter_mut().enumerate() {
            *c = Some(u.alloc_branch(&format!("{}:drive{i}", self.name)));
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for r in self.pair {
            b.f(r);
            for n in self.pair.into_iter().chain(self.controls) {
                b.g(r, n);
            }
        }
        for c in self.controls {
            b.f(c);
            b.g(c, c);
        }
        Ok(())
    }
    fn eval(&self, _: &EvalCtx, x: &[f64], _: StateRef<'_>, out: &mut Slots) {
        let v = Dual::<8>::var(volt(x, self.pair[0]) - volt(x, self.pair[1]), 0);
        let mut i = Dual::cst(0.);
        for j in 0..4 {
            let rail = j % 2;
            let sign = if rail == 0 { 1. } else { -1. };
            let raw = (v - self.rails[rail]) / self.resistance[j];
            let law = if j < 2 {
                let mag = sign * raw;
                let limit = self.limit[rail];
                let width = 0.01 * self.smooth * limit;
                sign * if mag.v < 0.5 * limit {
                    positive(mag, width)
                } else {
                    limit - positive(limit - mag, width)
                }
            } else {
                raw
            };
            let c = volt(x, self.controls[j]);
            i += Dual::var(c, 1 + j) * law;
            out.f[2 + j] = c;
            out.g[12 + j] = 1.;
        }
        for (row, sign) in [1., -1.].into_iter().enumerate() {
            out.f[row] = sign * i.v;
            let g = &mut out.g[row * 6..row * 6 + 6];
            g[0] = sign * i.d[0];
            g[1] = -g[0];
            for j in 0..4 {
                g[2 + j] = sign * i.d[1 + j];
            }
        }
    }
}
