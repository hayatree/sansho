//! Four-terminal quasi-static BSIM3v3.3 devices for MOS levels 8 and 49.
//! Provenance: original implementation of the UC Berkeley BSIM3v3.3 manual,
//! chapters 2–5, 8–9 and Appendices A, B and D. See docs/bsim3.md.

mod channel;
mod charges;
mod junction;
mod noise;
mod params;
#[cfg(test)]
mod tests;

use crate::{
    BOLTZMANN, DeviceGroup, EvalCtx, MosBinding, Node, NoiseSource, SlotOffsets, Slots,
    StampBuilder, StateRef, Unknowns, dual::Dual, junction::limit_junction, mos1::limit_step, volt,
};
use junction::Junction;
use noise::Noise;
pub use params::Model;
use sansho_diag::Diagnostic;
use std::sync::Arc;

type D = Dual<4>;
const EPS_SI: f64 = 11.7 * 8.8541878128e-12;
const EPS_OX: f64 = 3.9 * 8.8541878128e-12;

fn floor(x: D, min: f64) -> D {
    if x.v < min { D::cst(min) } else { x }
}

// Conjugate of the lower quadratic root (3.6.4), including its zero-bias slope.
fn smooth_min(limit: D, x: D, delta: f64) -> D {
    let diff = limit - x - delta;
    2. * limit * x / (limit + x + delta + (diff.sq() + 4. * delta * limit).sqrt())
}

#[derive(Debug)]
struct Instance {
    model: Arc<Model>,
    name: String,
    external: [Node; 4],
    inner: [Node; 4],
    off: bool,
    slots: SlotOffsets,
}

#[derive(Debug, Default)]
pub struct Bsim3s {
    instances: Vec<Instance>,
}

impl Model {
    /// Prepare a size- and temperature-specific model shared by identical bindings.
    pub fn bind(
        name: &str,
        b: &MosBinding<'_>,
        version: Option<&str>,
    ) -> Result<(Arc<Self>, Vec<Diagnostic>), Diagnostic> {
        if version.is_some_and(|s| s != "3.3.0") || b.model.get("version", 3.3) != 3.3 {
            return Err(Diagnostic::error(
                "E-UNSUPPORTED",
                format!("{name}: BSIM3 requires version 3.3 or 3.3.0"),
            ));
        }
        for k in ["nqsmod", "acnqsmod"] {
            if b.instance.get(k, b.model.get(k, 0.)) != 0. {
                return Err(Diagnostic::error(
                    "E-UNSUPPORTED",
                    format!("{name}: BSIM3 {k} requires a channel charge state"),
                ));
            }
        }
        let model =
            Model::new(b).map_err(|e| Diagnostic::error("E-ELAB-MODEL", format!("{name}: {e}")))?;
        let mut warnings = Vec::new();
        if b.model.get("paramchk", 0.) != 0. {
            for (key, value, low, high) in [
                ("acde", model.p.acde, 0.4, 1.6),
                ("moin", model.p.moin, 5., 25.),
                ("noff", model.p.noff, 0.1, 4.),
                ("voffcv", model.p.voffcv, -0.5, 0.5),
            ] {
                if !(low..=high).contains(&value) {
                    warnings.push(Diagnostic::warning(
                        "W-BSIM3-PARAM",
                        format!("{name}: {key}={value} outside [{low}, {high}]"),
                    ));
                }
            }
        }
        Ok((Arc::new(model), warnings))
    }
}

impl Bsim3s {
    /// Attach the physical terminals to a validated binding.
    pub fn push_prepared(&mut self, name: &str, nodes: [Node; 4], off: bool, model: Arc<Model>) {
        self.instances.push(Instance {
            model,
            name: name.into(),
            external: nodes,
            inner: nodes,
            off,
            slots: SlotOffsets::default(),
        });
    }
}

impl Model {
    fn terminals(&self, lin: [f64; 4], gmin: f64) -> ([D; 4], [D; 4]) {
        let v: [D; 4] = std::array::from_fn(|i| D::var(lin[i], i) * self.sign);
        let [d, g, s, b] = v;
        let reverse = d.v < s.v;
        let (drain, source) = if reverse { (s, d) } else { (d, s) };
        let bias = self.channel(g - source, drain - source, b - source);
        let mut i = [
            bias.ids + bias.substrate,
            D::cst(0.),
            -bias.ids,
            -bias.substrate,
        ];
        let mut q = self.charges(&bias);
        if reverse {
            i.swap(0, 2);
            q.swap(0, 2);
        }
        for (side, node) in [0, 2].into_iter().enumerate() {
            let (ij, qj) = self.junction[side].evaluate(b - v[node], gmin);
            i[3] += ij;
            i[node] += -ij;
            let qo = self.overlap_charge(side, g - v[node]);
            q[1] += qo;
            q[node] += -qo - qj;
            q[3] += qj;
        }
        let gb = self.overlap[2] * (g - b);
        q[1] += gb;
        q[3] += -gb;
        let scale = self.mult * self.sign;
        (i.map(|x| x * scale), q.map(|x| x * scale))
    }
}

impl DeviceGroup for Bsim3s {
    fn instance_ic(&self, i: usize, ic: &[f64], _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [ds, gs, bs] = ic {
            let [d, g, s, b] = self.instances[i].inner;
            edges.extend([(d, s, *ds), (g, s, *gs), (b, s, *bs)]);
        }
    }
    fn type_name(&self) -> &'static str {
        "bsim3"
    }
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.instances.get(index).map(|d| d.name.as_str())
    }
    fn len(&self) -> usize {
        self.instances.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        3 * self.len()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for d in &mut self.instances {
            for (idx, name) in ["drain", "source"].into_iter().enumerate() {
                if d.model.resistance[idx] > 0. {
                    d.inner[2 * idx] = Some(u.alloc_internal_node(&d.name, name));
                }
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for d in &mut self.instances {
            d.slots = b.counts();
            b.charge_core(&d.inner);
            for n in [0, 2] {
                if d.inner[n] != d.external[n] {
                    b.resistor(d.external[n], d.inner[n]);
                }
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        for (index, (d, prev)) in self
            .instances
            .iter()
            .zip(state.current.as_chunks_mut::<3>().0)
            .enumerate()
        {
            let m = &d.model;
            let act = out.mos_voltages(index, d.inner.map(|n| volt(x, n)));
            if let Some(values) = out.outputs.get_mut(index) {
                let [d, g, s, b] = act.map(|v| v * m.sign);
                let reverse = d < s;
                let (src, drn) = if reverse { (d, s) } else { (s, d) };
                let (vg, vd, vb) = (D::var(g - src, 1), D::var(drn - src, 0), D::var(b - src, 3));
                let bias = m.channel(vg, vd, vb);
                let ids = bias.ids;
                let threshold = bias.threshold.v;
                let sat = bias.sat.v;
                let q = m.charges(&bias);
                crate::observables::mos(values, ids, q, m.mult, threshold, sat, reverse);
                values.push(("isub", bias.substrate.v * m.mult));
                for (side, v) in [b - d, b - s].into_iter().enumerate() {
                    let (j, q) = m.junction[side].evaluate(D::var(v, 0), ctx.gmin);
                    values.extend([
                        (["ibd", "ibs"][side], j.v * m.mult),
                        (["capbd", "capbs"][side], q.d[0] * m.mult),
                        (["qbd", "qbs"][side], q.v * m.mult),
                    ]);
                }
            }
            let [drain, gate, s, body] = act;
            let mut lin = act;
            let mut bias = [gate - s, drain - s, body - s].map(|v| m.sign * v);
            if ctx.init_junctions {
                bias = if d.off {
                    [0.; 3]
                } else {
                    [m.threshold_base + m.k1 * m.phi.sqrt() + 0.1, 0.1, 0.]
                };
            } else if ctx.limit {
                let (g, lg) = limit_step(bias[0], prev[0], prev[0] - m.threshold_base);
                let (v, ld) = limit_step(bias[1], prev[1], prev[1]);
                let [jd, js] = &m.junction;
                let (b, lb) = limit_junction(bias[2], prev[2], js.nvt, js.critical);
                let (bd, lbd) = limit_junction(b - v, prev[2] - prev[1], jd.nvt, jd.critical);
                out.noncon |= lg || ld || lb || lbd;
                bias = [g, if lbd { b - bd } else { v }, b];
            }
            *prev = bias;
            if ctx.init_junctions || ctx.limit {
                lin = [
                    s + m.sign * bias[1],
                    s + m.sign * bias[0],
                    s,
                    s + m.sign * bias[2],
                ];
            }
            let (i, q) = m.terminals(lin, ctx.gmin);
            if let Some(values) = out.outputs.get_mut(index) {
                values.extend([("ig", i[1].v), ("is", i[2].v), ("ib", i[3].v)]);
            }
            let mut at = d.slots;
            out.core(&mut at, &i, &q, &act, &lin);
            for (side, n) in [0, 2].into_iter().enumerate() {
                if d.external[n] != d.inner[n] {
                    out.resistor(
                        &mut at,
                        m.mult / m.resistance[side],
                        volt(x, d.external[n]),
                        act[n],
                    );
                }
            }
        }
    }
    fn noise(
        &self,
        _: &EvalCtx,
        x: &[f64],
        _: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for d in &self.instances {
            let m = &d.model;
            let [dr, g, s, b] = d.inner.map(|n| m.sign * volt(x, n));
            let source = dr.min(s);
            let [thermal, flicker] = m.noise.powers(m, g - source, (dr - s).abs(), b - source);
            if !thermal.is_finite() || !flicker.is_finite() {
                return Err(Diagnostic::error(
                    "E-ELAB-MODEL",
                    format!("{}: nonfinite BSIM3 noise power", d.name),
                ));
            }
            for (power, ef) in [(thermal, 0.), (flicker, m.noise.ef)] {
                out.push(NoiseSource::branch(
                    &d.name, d.inner[0], d.inner[2], power, ef,
                ));
            }
            for (side, n) in [0, 2].into_iter().enumerate() {
                if m.resistance[side] > 0. {
                    out.push(NoiseSource::branch(
                        &d.name,
                        d.inner[n],
                        d.external[n],
                        4. * BOLTZMANN * m.temp * m.mult / m.resistance[side],
                        0.,
                    ));
                }
            }
        }
        Ok(())
    }
}
