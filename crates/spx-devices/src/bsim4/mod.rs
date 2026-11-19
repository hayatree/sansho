//! Built-in silicon BSIM4 level-54 devices, stamped using forward-mode AD.
//! Provenance: original realization of the Berkeley BSIM4 4.8.2 technical manual,
//! chapters 1–3, 5–7, 11–13 and Appendix A. See docs/bsim4.md for selector coverage.

pub mod catalog;
mod channel;
mod charges;
mod gate;
mod junction;
mod layout;
mod noise;
mod params;
mod profile;
mod terminal_stamp;
#[cfg(test)]
mod tests;
use crate::{
    DeviceGroup, EvalCtx, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns, dual::Dual,
    junction::limit_junction, mos1::limit_step, volt,
};
use charges::ChargeModel;
use junction::Junction;
use layout::Network;
use params::Size;
use spx_diag::Diagnostic;
use std::sync::Arc;

type D = Dual<4>;
const EPS0: f64 = 8.8541878128e-12;
const EPS_SI: f64 = 11.7 * EPS0;

fn floor<const N: usize>(x: Dual<N>, lower: f64) -> Dual<N> {
    if x.v < lower { Dual::cst(lower) } else { x }
}
fn theta<const N: usize>(x: Dual<N>) -> Dual<N> {
    let e = (-floor(x, 1e-8)).exp();
    e / (1. - e).sq()
}
fn softplus<const N: usize>(x: Dual<N>) -> Dual<N> {
    if x.v > 35. {
        x
    } else if x.v < -35. {
        x.exp()
    } else {
        (1. + x.exp()).ln()
    }
}
/// Rationalized lower root of the smoothing quadratic (5.33), exact zero at x=0.
fn smooth_min<const N: usize>(limit: Dual<N>, x: Dual<N>, delta: f64) -> Dual<N> {
    let diff = limit - x - delta;
    2. * limit * x / (limit + x + delta + (diff.sq() + 4. * delta * limit).sqrt())
}

/// Temperature-, geometry- and layout-specialized model shared by identical instances.
#[derive(Debug)]
pub struct PreparedModel {
    size: Size,
    network: Network,
}

#[derive(Debug)]
struct Instance {
    name: String,
    model: Arc<PreparedModel>,
    nodes: [Node; 4],
    inner: [Node; 4],
    body: [Node; 2],
    middle: Node,
    off: bool,
    at: SlotOffsets,
}

#[derive(Clone, Debug)]
struct CoreStamp {
    lin: [f64; 4],
    f: [D; 4],
    q: [D; 4],
}

/// Per-workspace intrinsic BSIM4 linearizations. One cache belongs to one
/// configured device group for the lifetime of an analysis.
#[derive(Debug, Default)]
pub struct CoreCache {
    instances: Vec<Option<CoreStamp>>,
    terminal: terminal_stamp::Cache,
    shared: Vec<Option<(usize, CoreStamp)>>,
}
impl CoreCache {
    // A small direct-mapped cache per device chunk also catches equal biases
    // on separate instances of the same fully specialized transistor.
    const INDEX_BITS: u32 = 5;

    fn exact(&mut self, model: &Arc<PreparedModel>, lin: [f64; 4]) -> &CoreStamp {
        // The group owns every model for the cache lifetime. Compare identity
        // without updating shared reference counts in the parallel inner loop.
        let identity = Arc::as_ptr(model) as usize;
        let bits = lin.map(f64::to_bits);
        let hash = bits
            .into_iter()
            .fold(0u64, |acc, v| acc.rotate_left(13) ^ v);
        let index = (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - Self::INDEX_BITS)) as usize;
        let entry = &mut self.shared[index];
        if entry
            .as_ref()
            .is_some_and(|(m, s)| *m != identity || s.lin.map(f64::to_bits) != bits)
        {
            *entry = None;
        }
        if entry.is_some()
            && let Some(p) = &mut self.terminal.profile
        {
            p.shared += 1;
        }
        &entry
            .get_or_insert_with(|| {
                if let Some(p) = &mut self.terminal.profile {
                    p.evaluation::<4>(&model.size, lin);
                }
                let (f, q, _) = model.size.terminals(lin);
                (identity, CoreStamp { lin, f, q })
            })
            .1
    }
}

/// Chunk of built-in level-54 instances.
#[derive(Debug, Default)]
pub struct Bsim4s {
    instances: Vec<Instance>,
    compact: Option<terminal_stamp::Batch>,
}

pub use crate::MosBinding as Binding;

impl Bsim4s {
    /// Bind an instance and validate its model before registering stamp slots.
    pub fn push(
        &mut self,
        name: &str,
        nodes: [Node; 4],
        off: bool,
        b: Binding<'_>,
    ) -> Result<Vec<Diagnostic>, Diagnostic> {
        let (model, warnings) = PreparedModel::new(name, b)?;
        self.push_prepared(name, nodes, off, model);
        Ok(warnings)
    }

    /// Register a device with a previously validated parameter binding.
    pub fn push_prepared(
        &mut self,
        name: &str,
        nodes: [Node; 4],
        off: bool,
        model: Arc<PreparedModel>,
    ) {
        self.instances.push(Instance {
            name: name.into(),
            model,
            nodes,
            inner: nodes,
            body: [nodes[3]; 2],
            middle: nodes[1],
            off,
            at: SlotOffsets::default(),
        });
    }
}

impl PreparedModel {
    /// Evaluate and validate an instance binding, including its effective dimensions.
    pub fn new(name: &str, b: Binding<'_>) -> Result<(Arc<Self>, Vec<Diagnostic>), Diagnostic> {
        let p = b.model;
        let error = |msg: String| Diagnostic::error("E-ELAB-MODEL", format!("{name}: {msg}"));
        for key in ["acnqsmod", "trnqsmod", "mtrlmod"] {
            if b.instance.get(key, p.get(key, 0.)) != 0. {
                return Err(Diagnostic::error(
                    "E-UNSUPPORTED",
                    format!("{name}: nonzero {key}"),
                ));
            }
        }
        if p.get("igbmod", 0.) == 2. {
            return Err(Diagnostic::error(
                "E-UNSUPPORTED",
                format!("{name}: igbMod=2 has no equation in section 4.1"),
            ));
        }
        if p.get("wpemod", 0.) == 1.
            && b.instance.get("sc", 0.) > 0.
            && b.instance.opt(&["sca", "scb", "scc"]).is_none()
        {
            return Err(Diagnostic::error(
                "E-UNSUPPORTED",
                format!("{name}: WPE from SC alone requires an external mask distribution"),
            ));
        }
        for (key, default, max) in [
            ("capmod", 2., 2.),
            ("cvchargemod", 0., 1.),
            ("diomod", 1., 2.),
            ("permod", 1., 1.),
            ("binunit", 1., 2.),
            ("paramchk", 1., 1.),
            ("wpemod", 0., 1.),
        ] {
            let value = p.get(key, default);
            if value.fract() != 0. || !(0. ..=max).contains(&value) {
                return Err(error(format!("invalid {key}={value}")));
            }
        }
        let mut size =
            Size::new(p, b.instance, b.geometry, b.polarity, b.temp, b.tnom).map_err(&error)?;
        layout::stress(&mut size, &b, b.scale).map_err(&error)?;
        let network = Network::new(&b, &mut size).map_err(&error)?;
        size.basic_transport = size.mobmod == 0
            && size.p.a1 == 0.
            && size.p.a2 == 1.
            && size.p.dwg == 0.
            && size.p.dwb == 0.
            && size.p.ud == 0.
            && size.overshoot == 0.
            && size.ballistic_speed == 0.;
        size.zero_body = Some(size.body_bias(Dual::<1>::var(0., 0)));
        let z = D::cst(0.);
        size.cv.flat_zero =
            size.channel(z, z, z).2.cv.threshold.v - size.phi - size.k1 * size.phi.sqrt();
        if !size.cv.flat_zero.is_finite() {
            return Err(error("non-finite zero-bias threshold".into()));
        }
        // A disabled submodel has no effect, including when its coefficients are supplied.
        if !layout::stress_active(b.instance, size.nf) {
            p.accept(&[
                "saref", "sbref", "wlod", "ku0", "kvsat", "kvth0", "tku0", "llodku0", "wlodku0",
                "llodvth", "wlodvth", "lku0", "wku0", "pku0", "lkvth0", "wkvth0", "pkvth0", "stk2",
                "lodk2", "steta0", "lodeta0",
            ]);
        }
        if p.get("wpemod", 0.) == 0. {
            p.accept(&["kvth0we", "k2we", "ku0we", "scref"]);
        }
        if network.gate_mode == 0 {
            p.accept(&["rshg", "xgw", "xgl"]);
        }
        if !network.external {
            p.accept(&["rsw", "rdw", "rdwmin", "rswmin"]);
        }
        p.accept(&[
            "level", "binunit", "version", "lmin", "lmax", "wmin", "wmax",
        ]);
        let mut warnings = Vec::new();
        if p.get("paramchk", 1.) != 0. {
            for (key, value, lower) in [
                ("Leff", size.l, 1e-9),
                ("Weff", size.w, 1e-9),
                ("LeffCV", size.cv.dimensions[0], 1e-9),
                ("WeffCV", size.cv.dimensions[1], 1e-9),
                ("TOXE", size.toxe, 1e-10),
                ("TOXP", size.toxp, 1e-10),
                ("TOXM", p.get("toxm", size.toxe), 1e-10),
            ] {
                if value < lower {
                    warnings.push(Diagnostic::warning(
                        "W-BSIM4-PARAM",
                        format!("{name}: {key}={value:e} m is below {lower:e} m"),
                    ));
                }
            }
        }
        Ok((Arc::new(Self { size, network }), warnings))
    }
}

impl Size {
    fn limit_intrinsic(
        &self,
        mut bias: [f64; 3],
        prev: &[f64; 5],
        ctx: &EvalCtx,
        off: bool,
    ) -> ([f64; 3], bool) {
        if ctx.init_junctions {
            bias = if off {
                [0.; 3]
            } else {
                [self.vth0 + 0.1, 0.1, -0.1]
            };
        } else if ctx.limit {
            let (vg, lg) = limit_step(bias[0], prev[0], prev[0] - self.vth0);
            let (vd, ld) = limit_step(bias[1], prev[1], prev[1]);
            let limit = |v, old, side: usize| {
                let j = &self.junction[side];
                limit_junction(v, old, j.nvt, j.vcrit)
            };
            let (vb, lb) = limit(bias[2], prev[2], 1);
            let (vbd, lbd) = limit(vb - vd, prev[2] - prev[1], 0);
            bias = [vg, if lbd { vb - vbd } else { vd }, vb];
            return (bias, lg || ld || lb || lbd);
        }
        (bias, false)
    }
    fn terminals(&self, lin: [f64; 4]) -> ([D; 4], [D; 4], channel::Bias<4>) {
        self.terminal_equations(std::array::from_fn(|k| D::var(lin[k], k)))
    }

    fn terminal_equations<const N: usize>(
        &self,
        v: [Dual<N>; 4],
    ) -> ([Dual<N>; 4], [Dual<N>; 4], channel::Bias<N>) {
        let [d, g, s, b] = v.map(|x| x * self.sign);
        let reverse = d.v < s.v;
        let (drain, source) = if reverse { (s, d) } else { (d, s) };
        let (ids, sub, bias) = self.channel(g - source, drain - source, b - source);
        let [igd, igs, igb] = self.tunneling(&bias);
        let mut f = [ids + sub - igd, igd + igs + igb, -ids - igs, -sub - igb];
        let mut q = self.charges(&bias.cv);
        if reverse {
            f.swap(0, 2);
            q.swap(0, 2);
        }
        for (side, (terminal, other, slot)) in [(d, s, 0), (s, d, 2)].into_iter().enumerate() {
            let leak = self.gidl(
                side,
                terminal - other,
                if (!reverse && side == 0) || (reverse && side == 1) {
                    bias.cv.gate
                } else {
                    self.gate_voltage(g - other)
                },
                terminal - b,
            );
            let ig = self.edge_tunneling(g - terminal, side);
            f[slot] += leak - ig;
            f[3] += -leak;
            f[1] += ig;
        }

        let scale = self.sign * self.mult;
        (f.map(|v| v * scale), q.map(|v| v * scale), bias)
    }
}
impl Instance {
    fn gate_conductance<const N: usize>(&self, bias: &channel::Bias<N>) -> Dual<N> {
        let net = &self.model.network;
        let gc = net.xrcrg1 * (bias.id_per_volt + net.xrcrg2 * bias.beta * self.model.size.vt);
        if net.gate_mode == 2 {
            gc / (1. + net.gate * gc)
        } else {
            gc
        }
    }
    fn body_branches(&self) -> [(Node, Node); 5] {
        let ext = self.nodes[3];
        let body = self.inner[3];
        let [db, sb] = self.body;
        [(ext, body), (db, body), (sb, body), (ext, db), (ext, sb)]
    }
    fn series_nodes(&self, side: usize) -> [Node; 4] {
        let slot = 2 * side;
        [
            self.nodes[slot],
            self.inner[slot],
            self.inner[1],
            self.inner[3],
        ]
    }
    fn overlap_nodes(&self) -> [Node; 4] {
        let mut nodes = self.inner;
        if self.model.network.gate_mode == 3 {
            nodes[1] = self.middle;
        }
        nodes
    }
    fn series(&self, side: usize, v: [f64; 4]) -> [D; 4] {
        let [outer, inner, gate, bulk] = std::array::from_fn(|k| D::var(v[k], k));
        let net = &self.model.network;
        let m = &self.model.size;
        let resistance = if net.external {
            let [min, r] = net.extension[side];
            let gate_drive = floor(m.sign * (gate - outer) - net.vflat, 0.);
            let modulation =
                1. / floor(1. + m.p.prwg * gate_drive, 0.05) - m.p.prwb * m.sign * (bulk - outer);
            net.diffusion[side] + min + r * floor(modulation, 0.)
        } else {
            D::cst(net.diffusion[side])
        };
        let current = (outer - inner) * m.mult / floor(resistance, 1e-6);
        [current, -current, D::cst(0.), D::cst(0.)]
    }
}
impl DeviceGroup for Bsim4s {
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        _: &Slots,
        out: &mut Vec<crate::NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for d in &self.instances {
            d.noise(x, ctx.gmin, out)?;
        }
        Ok(())
    }
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.instances.get(index).map(|d| d.name.as_str())
    }
    fn type_name(&self) -> &'static str {
        "bsim4"
    }
    fn len(&self) -> usize {
        self.instances.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        5 * self.len()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for d in &mut self.instances {
            let mut node = |n: &str| Some(u.alloc_internal_node(&d.name, n));
            for (side, name) in ["drain", "source"].into_iter().enumerate() {
                if d.model.network.external || d.model.network.diffusion[side] > 0. {
                    d.inner[2 * side] = node(name);
                }
            }
            if d.model.network.gate_mode != 0 {
                d.inner[1] = node("gate");
            }
            if d.model.network.gate_mode == 3 {
                d.middle = node("midgate");
            }
            if d.model.network.body.is_some() {
                d.inner[3] = node("body");
                d.body = [node("dbody"), node("sbody")];
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        let compact = self.instances.iter().all(|d| d.nodes == d.inner);
        for d in &mut self.instances {
            d.at = b.counts();
            if compact {
                b.core(&d.inner);
                // Preserve each component of the LTE test before nodal cancellation.
                for _ in 0..12 {
                    b.tracked_q(None);
                }
                continue;
            }
            b.charge_core(&d.inner);
            let overlap = d.overlap_nodes();
            for r in overlap {
                b.tracked_q(r);
                for c in overlap {
                    b.c(r, c);
                }
            }
            for side in 0..2 {
                b.charge_core(&[d.body[side], d.inner[2 * side]]);
                if d.inner[2 * side] != d.nodes[2 * side] {
                    b.core(&d.series_nodes(side));
                }
            }
            if d.model.network.body.is_some() {
                for node in d.body {
                    b.relaxation_node(node);
                }
                for (a, z) in d.body_branches() {
                    b.resistor(a, z);
                }
            }
            if matches!(d.model.network.gate_mode, 1 | 3) {
                b.resistor(
                    d.nodes[1],
                    if d.model.network.gate_mode == 1 {
                        d.inner[1]
                    } else {
                        d.middle
                    },
                );
            }
            if d.model.network.gate_mode >= 2 {
                let [d1, g, s, body] = d.inner;
                b.core(&[d1, g, s, body, d.middle]);
            }
        }
        self.compact = compact.then(|| terminal_stamp::Batch::new(&self.instances));
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        if self.compact.is_some() {
            self.eval_cached(ctx, x, state, out, &mut CoreCache::default(), 0.);
        } else {
            self.evaluate(ctx, x, state, out, None, 0.);
        }
    }

    fn eval_cached(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        cache: &mut CoreCache,
        tolerance: f64,
    ) {
        if let Some(batch) = &self.compact {
            batch.eval_terminals(ctx, x, state, out, &mut cache.terminal, tolerance);
            return;
        }
        if let Some(p) = &mut cache.terminal.profile {
            p.visits += self.instances.len() as u64;
        }
        cache.instances.resize_with(self.len(), || None);
        cache
            .shared
            .resize_with(1 << CoreCache::INDEX_BITS, || None);
        self.evaluate(ctx, x, state, out, Some(cache), tolerance);
    }
    fn sample_charges(
        &self,
        x: &[f64],
        state: &mut StateRef<'_>,
        out: &mut Slots,
        cache: &mut CoreCache,
    ) -> bool {
        if let Some(batch) = &self.compact {
            batch.sample_terminals(x, state, out, &mut cache.terminal);
        }
        self.compact.is_some()
    }
}

impl Bsim4s {
    fn evaluate(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        state: StateRef<'_>,
        out: &mut Slots,
        mut cache: Option<&mut CoreCache>,
        tolerance: f64,
    ) {
        for (index, (dev, prev)) in self
            .instances
            .iter()
            .zip(state.current.as_chunks_mut::<5>().0)
            .enumerate()
        {
            let m = &dev.model.size;
            let act = dev.inner.map(|n| volt(x, n));
            let [d, g, s, b] = act;
            let (bias, limited) = m.limit_intrinsic(
                [g - s, d - s, b - s].map(|v| m.sign * v),
                prev,
                ctx,
                dev.off,
            );
            out.noncon |= limited;
            prev[..3].copy_from_slice(&bias);
            let lin = [
                s + m.sign * bias[1],
                s + m.sign * bias[0],
                s,
                s + m.sign * bias[2],
            ];
            let mut at = dev.at;
            let bias = if let Some(cache) = cache
                .as_deref_mut()
                .filter(|_| dev.model.network.gate_mode < 2)
            {
                let reusable = cache.instances[index].as_ref().filter(|stamp| {
                    stamp.lin.map(f64::to_bits) == lin.map(f64::to_bits)
                        || (tolerance > 0.
                            && ctx.limit
                            && !ctx.init_junctions
                            && stamp
                                .lin
                                .iter()
                                .zip(lin)
                                .all(|(old, v)| (old - v).abs() <= tolerance))
                });
                if let Some(stamp) = reusable {
                    if stamp.lin.map(f64::to_bits) != lin.map(f64::to_bits)
                        && let Some(p) = &mut cache.terminal.profile
                    {
                        p.approximate += 1;
                    }
                    out.core(&mut at, &stamp.f, &stamp.q, &act, &stamp.lin);
                } else {
                    let stamp = cache.exact(&dev.model, lin).clone();
                    out.core(&mut at, &stamp.f, &stamp.q, &act, &lin);
                    cache.instances[index] = Some(stamp);
                }
                None
            } else {
                if let Some(p) = cache
                    .as_deref_mut()
                    .and_then(|c| c.terminal.profile.as_mut())
                {
                    p.evaluation::<4>(m, lin);
                }
                let (f, q, bias) = m.terminals(lin);
                out.core(&mut at, &f, &q, &act, &lin);
                Some(bias)
            };
            let ov = dev.overlap_nodes().map(|n| volt(x, n));
            let [od, og, os, ob] = std::array::from_fn(|k| D::var(ov[k], k));
            let gd = m.overlap_charge((og - od) * m.sign, 0) * m.sign * m.mult;
            let gs = m.overlap_charge((og - os) * m.sign, 1) * m.sign * m.mult;
            let gb = (og - ob) * (m.overlap[2] * m.mult);
            for q in [-gd, gd + gs + gb, -gs, -gb] {
                out.q[at.q] = q.v;
                out.c[at.c..at.c + 4].copy_from_slice(&q.d);
                at.q += 1;
                at.c += 4;
            }
            for side in 0..2 {
                let j = &m.junction[side];
                let local = [volt(x, dev.body[side]), act[2 * side]];
                let raw = m.sign * (local[0] - local[1]);
                let (v, limited) = if ctx.limit {
                    limit_junction(raw, prev[3 + side], j.nvt, j.vcrit)
                } else {
                    (raw, false)
                };
                out.noncon |= limited;
                prev[3 + side] = v;
                let jl = [local[1] + m.sign * v, local[1]];
                let jv = (Dual::<2>::var(jl[0], 0) - Dual::var(jl[1], 1)) * m.sign;
                let (current, charge) = j.eval(jv, ctx.gmin);
                let current = current * m.sign * m.mult;
                let charge = charge * m.sign * m.mult;
                out.core(
                    &mut at,
                    &[current, -current],
                    &[charge, -charge],
                    &local,
                    &jl,
                );
                if dev.nodes[2 * side] != dev.inner[2 * side] {
                    let v = dev.series_nodes(side).map(|n| volt(x, n));
                    out.core(&mut at, &dev.series(side, v), &[D::cst(0.); 4], &v, &v);
                }
            }
            if let Some(body) = dev.model.network.body {
                for ((a, b), g) in dev.body_branches().into_iter().zip(body) {
                    out.resistor(&mut at, g * m.mult, volt(x, a), volt(x, b));
                }
            }
            let mode = dev.model.network.gate_mode;
            if matches!(mode, 1 | 3) {
                out.resistor(
                    &mut at,
                    m.mult / dev.model.network.gate.max(1e-6),
                    volt(x, dev.nodes[1]),
                    volt(x, if mode == 1 { dev.inner[1] } else { dev.middle }),
                );
            }
            if mode >= 2
                && let Some(bias) = bias
            {
                let g = dev.gate_conductance(&bias);
                let g = Dual::<5> {
                    v: g.v,
                    d: [g.d[0], g.d[1], g.d[2], g.d[3], 0.],
                };
                let middle = volt(x, dev.middle);
                let current = m.mult * g * (Dual::var(middle, 4) - Dual::var(lin[1], 1));
                let z = Dual::cst(0.);
                out.core(
                    &mut at,
                    &[z, -current, z, z, current],
                    &[z; 5],
                    &[act[0], act[1], act[2], act[3], middle],
                    &[lin[0], lin[1], lin[2], lin[3], middle],
                );
            }
        }
    }
}
