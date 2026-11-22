//! Combined four-terminal stamps for devices without internal resistive nodes.
//! Provenance: original algebraic assembly of the BSIM4 4.8.2 terminal equations.

use super::{D, Instance, PreparedModel, Size};
use crate::{EvalCtx, SlotOffsets, Slots, StateRef, dual::Dual, junction::limit_junction};

/// All three contributions are retained separately for charge truncation control.
fn write_charges(out: &mut [f64], parts: &[f64; 12]) {
    let [d, g, s, b, od, og, os, ob, db, dd, sb, ss] = *parts;
    out[..4].copy_from_slice(&[d + od + dd, g + og, s + os + ss, b + ob + db + sb]);
    out[4..].copy_from_slice(parts);
}

#[derive(Clone, Debug)]
struct Stamp {
    act: [f64; 4],
    gmin: f64,
    tied_body: bool,
    f: [f64; 4],
    q: [f64; 4],
    g: [[f64; 4]; 4],
    c: [[f64; 4]; 4],
    parts: [f64; 12],
}

impl Stamp {
    fn write(&self, at: SlotOffsets, act: [f64; 4], out: &mut Slots) {
        out.g[at.g..at.g + 16].copy_from_slice(self.g.as_flattened());
        out.c[at.c..at.c + 16].copy_from_slice(self.c.as_flattened());
        let mut f = self.f;
        if self.act.map(f64::to_bits) != act.map(f64::to_bits) {
            let dv = std::array::from_fn::<_, 4, _>(|k| act[k] - self.act[k]);
            for (i, row) in self.g.iter().enumerate() {
                for (k, &delta) in dv.iter().enumerate() {
                    f[i] += row[k] * delta;
                }
            }
        }
        out.f[at.f..at.f + 4].copy_from_slice(&f);
        self.write_charges(act, &mut out.q[at.q..at.q + 16]);
    }

    /// Terminal charges expanded to `act`, then the twelve component charges.
    fn write_charges(&self, act: [f64; 4], out: &mut [f64]) {
        let mut q = self.q;
        if self.act.map(f64::to_bits) != act.map(f64::to_bits) {
            let dv = std::array::from_fn::<_, 4, _>(|k| act[k] - self.act[k]);
            for (i, row) in self.c.iter().enumerate() {
                for (k, &delta) in dv.iter().enumerate() {
                    q[i] += row[k] * delta;
                }
            }
        }
        out[..4].copy_from_slice(&q);
        out[4..].copy_from_slice(&self.parts);
    }
}

#[derive(Default, Debug)]
pub(super) struct Cache {
    pub(super) profile: Option<super::profile::Profile>,
    last: Vec<Option<Stamp>>,
    shared: Vec<Option<(usize, Stamp)>>,
    charges: Vec<Option<ChargeSample>>,
}
#[derive(Debug)]
struct ChargeSample {
    model: usize,
    bits: [u64; 4],
    parts: [f64; 12],
}

// Low bits of nearly settled node voltages still participate in the key.
fn slot(bits: [u64; 4]) -> usize {
    let hash = bits.into_iter().fold(0u64, |a, b| a.rotate_left(13) ^ b);
    (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 58) as usize
}

impl Size {
    pub(super) fn overlaps<const N: usize>(&self, v: [Dual<N>; 4]) -> [Dual<N>; 4] {
        let [d, g, s, b] = v;
        let gd = self.overlap_charge((g - d) * self.sign, 0) * self.sign * self.mult;
        let gs = self.overlap_charge((g - s) * self.sign, 1) * self.sign * self.mult;
        let gb = (g - b) * (self.overlap[2] * self.mult);
        [-gd, gd + gs + gb, -gs, -gb]
    }

    fn charge_parts(&self, act: [f64; 4]) -> [f64; 12] {
        let v = act.map(Dual::<0>::cst);
        let [d, g, s, b] = v.map(|x| x * self.sign);
        let rev = d.v < s.v;
        let (dr, src) = if rev { (s, d) } else { (d, s) };
        let bias = self.electrostatics(g - src, dr - src, b - src).0;
        let mut q = self.charges(&bias).map(|x| x.v * (self.sign * self.mult));
        if rev {
            q.swap(0, 2);
        }
        let ov = self.overlaps(v).map(|x| x.v);
        let [db, sb] = std::array::from_fn(|i| {
            let bias = (v[3] - v[2 * i]) * self.sign;
            self.junction[i].charge(bias).v * self.sign * self.mult
        });
        [
            q[0], q[1], q[2], q[3], ov[0], ov[1], ov[2], ov[3], db, -db, sb, -sb,
        ]
    }

    // The source column follows from translation invariance. When source and
    // bulk are wired together, their summed column follows from just D and G.
    fn intrinsic<const N: usize>(&self, act: [f64; 4], lin: [f64; 4]) -> ([D; 4], [D; 4]) {
        let x = [
            Dual::<N>::var(lin[0], 0),
            Dual::var(lin[1], 1),
            Dual::cst(lin[2]),
            Dual::var(lin[3], 2),
        ];
        let (f, q, _) = self.terminal_equations(x);
        let expand = |z: Dual<N>| {
            let d = z.d[0];
            let g = z.d[1];
            let b = z.d.get(2).copied().unwrap_or(0.);
            let grad = [d, g, -(d + g + b), b];
            let mut value = z.v;
            if lin != act {
                for k in 0..4 {
                    value += grad[k] * (act[k] - lin[k]);
                }
            }
            D { v: value, d: grad }
        };
        (f.map(expand), q.map(expand))
    }
    fn combined(
        &self,
        act: [f64; 4],
        lin: [f64; 4],
        jbias: [f64; 2],
        gmin: f64,
        tied_body: bool,
    ) -> Stamp {
        let (mut f, q) = if tied_body {
            self.intrinsic::<2>(act, lin)
        } else {
            self.intrinsic::<3>(act, lin)
        };
        let ov = self.overlaps(std::array::from_fn(|i| D::var(act[i], i)));
        let mut parts = [0.; 12];
        parts[..4].copy_from_slice(&q.map(|v| v.v));
        parts[4..8].copy_from_slice(&ov.map(|v| v.v));
        let mut q = std::array::from_fn(|i| q[i] + ov[i]);
        for (i, &bias) in jbias.iter().enumerate() {
            let k = 2 * i;
            let v = Dual::<1>::var(bias, 0);
            let (current, charge) = self.junction[i].eval(v, gmin);
            let branch = |z: Dual<1>| {
                let slope = z.d[0] * self.mult;
                let value =
                    z.v * self.sign * self.mult + slope * ((act[3] - act[k]) - self.sign * bias);
                let mut d = [0.; 4];
                d[3] = slope;
                d[k] = -slope;
                D { v: value, d }
            };
            let i_j = branch(current);
            let q_j = branch(charge);
            f[3] += i_j;
            f[k] += -i_j;
            q[3] += q_j;
            q[k] += -q_j;
            parts[8 + 2 * i] = q_j.v;
            parts[9 + 2 * i] = -q_j.v;
        }
        Stamp {
            act,
            gmin,
            tied_body,
            f: f.map(|z| z.v),
            q: q.map(|z| z.v),
            g: f.map(|z| z.d),
            c: q.map(|z| z.d),
            parts,
        }
    }
}

/// Topology and model bindings are streamed independently of instance names
/// and elaboration metadata. Each lane has the same terminal stamp layout.
#[derive(Debug)]
pub(super) struct Batch {
    nodes: Vec<[usize; 4]>,
    binding: Vec<usize>,
    models: Vec<std::sync::Arc<PreparedModel>>,
    off: Vec<bool>,
    base: SlotOffsets,
}
impl Batch {
    pub(super) fn new(instances: &[Instance]) -> Self {
        let mut models = Vec::new();
        let mut binding = Vec::with_capacity(instances.len());
        for d in instances {
            let index = models
                .iter()
                .position(|m| std::sync::Arc::ptr_eq(m, &d.model))
                .unwrap_or_else(|| {
                    models.push(d.model.clone());
                    models.len() - 1
                });
            binding.push(index);
        }
        Self {
            models,
            binding,
            nodes: instances
                .iter()
                .map(|d| d.inner.map(|n| n.unwrap_or(usize::MAX)))
                .collect(),
            off: instances.iter().map(|d| d.off).collect(),
            base: instances
                .first()
                .map_or_else(SlotOffsets::default, |d| d.at),
        }
    }
    fn offsets(&self, i: usize) -> SlotOffsets {
        SlotOffsets {
            g: self.base.g + 16 * i,
            c: self.base.c + 16 * i,
            q: self.base.q + 16 * i,
            f: self.base.f + 4 * i,
        }
    }
    fn voltages(&self, i: usize, x: &[f64]) -> [f64; 4] {
        self.nodes[i].map(|k| x.get(k).copied().unwrap_or(0.))
    }

    pub(super) fn sample_terminals(
        &self,
        x: &[f64],
        state: &mut StateRef<'_>,
        out: &mut Slots,
        cache: &mut Cache,
        tolerance: f64,
    ) {
        if let Some(p) = &mut cache.profile {
            p.samples += self.nodes.len() as u64;
        }
        cache.charges.resize_with(64, || None);
        for (index, history) in state
            .current
            .as_chunks_mut::<5>()
            .0
            .iter_mut()
            .take(self.nodes.len())
            .enumerate()
        {
            let act = self.voltages(index, x);
            let model = self.binding[index];
            let m = &self.models[model].size;
            let [d, g, s, b] = act;
            *history = [g - s, d - s, b - s, b - d, b - s].map(|v| v * m.sign);
            let offset = self.offsets(index).q;
            // Newton bypass accuracy: a first-order expansion of the terminal
            // charges from the last linearization when every terminal is within
            // `tolerance`. The component charges for LTE are reused as they are.
            if tolerance > 0.
                && let Some(stamp) = cache.last.get(index).and_then(Option::as_ref)
                && stamp
                    .act
                    .iter()
                    .zip(act)
                    .all(|(&a, v)| (a - v).abs() <= tolerance)
            {
                if let Some(p) = &mut cache.profile {
                    p.charge_bypass += 1;
                }
                stamp.write_charges(act, &mut out.q[offset..offset + 16]);
                continue;
            }
            let bits = act.map(f64::to_bits);
            let entry = &mut cache.charges[slot(bits)];
            if !entry
                .as_ref()
                .is_some_and(|e| e.bits == bits && e.model == model)
            {
                *entry = None;
            }
            let s = entry.get_or_insert_with(|| {
                let timer = cache.profile.as_mut().and_then(|p| {
                    p.scalar += 1;
                    (p.scalar % 1024 == 1).then(std::time::Instant::now)
                });
                let parts = m.charge_parts(act);
                if let (Some(t), Some(p)) = (timer, cache.profile.as_mut()) {
                    p.scalar_time += t.elapsed();
                }
                ChargeSample { model, bits, parts }
            });
            write_charges(&mut out.q[offset..offset + 16], &s.parts);
        }
    }

    pub(super) fn eval_terminals(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        cache: &mut Cache,
        tolerance: f64,
    ) {
        if let Some(p) = &mut cache.profile {
            p.visits += self.nodes.len() as u64;
        }
        cache.last.resize_with(self.nodes.len(), || None);
        cache.shared.resize_with(64, || None);
        for (index, (old, last)) in state
            .current
            .as_chunks_mut::<5>()
            .0
            .iter_mut()
            .zip(&mut cache.last)
            .enumerate()
        {
            let model = self.binding[index];
            let m = &self.models[model].size;
            let act = out.mos_voltages(index, self.voltages(index, x));
            let [d, g, s, b] = act;
            let mut biases = [g - s, d - s, b - s, b - d, b - s].map(|v| v * m.sign);
            let (bias, mut limited) =
                m.limit_intrinsic([biases[0], biases[1], biases[2]], old, ctx, self.off[index]);
            biases[..3].copy_from_slice(&bias);
            let intrinsic_limited = limited || ctx.init_junctions;
            for i in 0..2 {
                if ctx.limit {
                    let j = &m.junction[i];
                    let (v, l) = limit_junction(biases[3 + i], old[3 + i], j.nvt, j.vcrit);
                    biases[3 + i] = v;
                    limited |= l;
                }
            }
            *old = biases;
            out.noncon |= limited;
            let valid = !limited && !ctx.init_junctions;
            let bits = act.map(f64::to_bits);
            let close = |s: &Stamp| {
                s.gmin == ctx.gmin
                    && (s.act.map(f64::to_bits) == bits
                        || (ctx.limit
                            && tolerance > 0.
                            && s.act
                                .iter()
                                .zip(act)
                                .all(|(&a, b)| (a - b).abs() <= tolerance)))
            };
            if valid && let Some(stamp) = last.as_ref().filter(|s| close(s)) {
                if let Some(p) = &mut cache.profile
                    && stamp.act.map(f64::to_bits) != bits
                {
                    p.approximate += 1;
                }
                stamp.write(self.offsets(index), act, out);
                continue;
            }
            let lin = if intrinsic_limited {
                [
                    act[2] + m.sign * biases[1],
                    act[2] + m.sign * biases[0],
                    act[2],
                    act[2] + m.sign * biases[2],
                ]
            } else {
                act
            };
            let tied_body = self.nodes[index][2] == self.nodes[index][3] && !intrinsic_limited;
            let mut create = || {
                if let Some(p) = &mut cache.profile {
                    if tied_body {
                        p.evaluation::<2>(m, lin);
                    } else {
                        p.evaluation::<3>(m, lin);
                    }
                }
                m.combined(act, lin, [biases[3], biases[4]], ctx.gmin, tied_body)
            };
            let scratch;
            let stamp = if !valid {
                scratch = create();
                &scratch
            } else {
                let entry = &mut cache.shared[slot(bits)];
                if !entry.as_ref().is_some_and(|(id, s)| {
                    *id == model
                        && s.tied_body == tied_body
                        && s.gmin == ctx.gmin
                        && s.act.map(f64::to_bits) == bits
                }) {
                    *entry = None;
                }
                let shared = entry.is_some();
                let s = last.insert(entry.get_or_insert_with(|| (model, create())).1.clone());
                if shared && let Some(p) = &mut cache.profile {
                    p.shared += 1;
                }
                s
            };
            stamp.write(self.offsets(index), act, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::transistor;

    #[test]
    fn tied_bulk_stamps_match_the_physical_column_sum_and_charge_sample() {
        for sign in [-1., 1.] {
            let model = transistor(&[("vth0", 0.45 * sign)], sign, 27.);
            for vd in [-0.75, 0., 0.75] {
                let act = [0.3 + sign * vd, 0.3 + sign * 0.8, 0.3, 0.3];
                let bias = [sign * (act[3] - act[0]), 0.];
                let full = model.combined(act, act, bias, 1e-12, false);
                let tied = model.combined(act, act, bias, 1e-12, true);
                assert_eq!(tied.f, full.f);
                assert_eq!(tied.parts, model.charge_parts(act));
                for (ordinary, shared, atol) in [(full.g, tied.g, 1e-16), (full.c, tied.c, 1e-26)] {
                    for row in 0..4 {
                        for cols in [&[0][..], &[1], &[2, 3]] {
                            let sum = |m: [[f64; 4]; 4]| {
                                cols.iter().map(|&k: &usize| m[row][k]).sum::<f64>()
                            };
                            assert!((sum(ordinary) - sum(shared)).abs() < atol);
                        }
                    }
                }
            }
        }
    }
}
