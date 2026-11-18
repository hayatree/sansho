//! Built-in silicon BSIM4 level-54 devices, stamped using four-terminal forward-mode AD.
//! Provenance: original realization of the Berkeley BSIM4 4.8.2 technical manual,
//! chapters 1–3, 5–7, 11–13 and Appendix A. See docs/bsim4.md for selector coverage.

pub mod catalog;
mod channel;
mod charges;
mod gate;
mod junction;
mod layout;
mod params;
#[cfg(test)]
mod tests;
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns,
    dual::Dual,
    junction::{limit_junction, vcrit},
    mos1::{Mos1Geometry, limit_step},
    volt,
};
use charges::ChargeModel;
use junction::Junction;
use layout::Network;
use params::Size;
use spx_diag::Diagnostic;

type D = Dual<4>;
const EPS0: f64 = 8.8541878128e-12;
const EPS_SI: f64 = 11.7 * EPS0;

fn floor(x: D, lower: f64) -> D {
    if x.v < lower { D::cst(lower) } else { x }
}
fn theta(x: D) -> D {
    let e = (-floor(x, 1e-8)).exp();
    e / (1. - e).sq()
}
fn softplus(x: D) -> D {
    if x.v > 35. {
        x
    } else if x.v < -35. {
        x.exp()
    } else {
        (1. + x.exp()).ln()
    }
}
/// Rationalized lower root of the smoothing quadratic (5.33), exact zero at x=0.
fn smooth_min(limit: D, x: D, delta: f64) -> D {
    let diff = limit - x - delta;
    2. * limit * x / (limit + x + delta + (diff.sq() + 4. * delta * limit).sqrt())
}

#[derive(Debug)]
struct Instance {
    name: String,
    size: Size,
    network: Network,
    nodes: [Node; 4],
    inner: [Node; 4],
    body: [Node; 2],
    middle: Node,
    off: bool,
    at: SlotOffsets,
}

/// Chunk of built-in level-54 instances.
#[derive(Debug, Default)]
pub struct Bsim4s {
    instances: Vec<Instance>,
}

/// Elaboration inputs for one transistor. Temperatures in degrees Celsius.
#[derive(Debug)]
pub struct Binding<'a> {
    /// Evaluated model card.
    pub model: &'a ModelParams,
    /// Evaluated instance parameters.
    pub instance: &'a ModelParams,
    /// Geometry after the netlist scale factor.
    pub geometry: Mos1Geometry,
    /// +1 for NMOS, -1 for PMOS.
    pub polarity: f64,
    /// Circuit temperature.
    pub temp: f64,
    /// Geometric scale for additional layout distances.
    pub scale: f64,
    /// Global nominal temperature.
    pub tnom: f64,
}
impl Bsim4s {
    /// Bind an instance and validate its model before registering stamp slots.
    pub fn push(
        &mut self,
        name: &str,
        nodes: [Node; 4],
        off: bool,
        b: Binding<'_>,
    ) -> Result<Vec<Diagnostic>, Diagnostic> {
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
        let z = D::cst(0.);
        size.cv.flat_zero =
            size.channel(z, z, z).2.threshold.v - size.phi - size.k1 * size.phi.sqrt();
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
        self.instances.push(Instance {
            name: name.into(),
            size,
            network,
            nodes,
            inner: nodes,
            body: [nodes[3]; 2],
            middle: nodes[1],
            off,
            at: SlotOffsets::default(),
        });
        Ok(warnings)
    }
}

impl Size {
    fn terminals(&self, lin: [f64; 4]) -> ([D; 4], [D; 4], channel::Bias) {
        let [d, g, s, b] = std::array::from_fn(|k| D::var(lin[k], k) * self.sign);
        let reverse = d.v < s.v;
        let (drain, source) = if reverse { (s, d) } else { (d, s) };
        let (ids, sub, bias) = self.channel(g - source, drain - source, b - source);
        let [igd, igs, igb] = self.tunneling(&bias);
        let mut f = [ids + sub - igd, igd + igs + igb, -ids - igs, -sub - igb];
        let mut q = self.charges(&bias);
        if reverse {
            f.swap(0, 2);
            q.swap(0, 2);
        }
        for (side, (terminal, other, slot)) in [(d, s, 0), (s, d, 2)].into_iter().enumerate() {
            let leak = self.gidl(
                side,
                terminal - other,
                self.gate_voltage(g - other),
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
        if self.network.gate_mode == 3 {
            nodes[1] = self.middle;
        }
        nodes
    }
    fn series(&self, side: usize, v: [f64; 4]) -> [D; 4] {
        let [outer, inner, gate, bulk] = std::array::from_fn(|k| D::var(v[k], k));
        let net = &self.network;
        let m = &self.size;
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
                if d.network.external || d.network.diffusion[side] > 0. {
                    d.inner[2 * side] = node(name);
                }
            }
            if d.network.gate_mode != 0 {
                d.inner[1] = node("gate");
            }
            if d.network.gate_mode == 3 {
                d.middle = node("midgate");
            }
            if d.network.body.is_some() {
                d.inner[3] = node("body");
                d.body = [node("dbody"), node("sbody")];
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for d in &mut self.instances {
            d.at = b.counts();
            b.charge_core(&d.inner);
            b.charge_core(&d.overlap_nodes());
            for side in 0..2 {
                b.charge_core(&[d.body[side], d.inner[2 * side]]);
                if d.inner[2 * side] != d.nodes[2 * side] {
                    b.core(&d.series_nodes(side));
                }
            }
            if d.network.body.is_some() {
                for node in d.body {
                    b.relaxation_node(node);
                }
                for (a, z) in d.body_branches() {
                    b.resistor(a, z);
                }
            }
            if matches!(d.network.gate_mode, 1 | 3) {
                b.resistor(
                    d.nodes[1],
                    if d.network.gate_mode == 1 {
                        d.inner[1]
                    } else {
                        d.middle
                    },
                );
            }
            if d.network.gate_mode >= 2 {
                let [d1, g, s, body] = d.inner;
                b.core(&[d1, g, s, body, d.middle]);
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        for (dev, prev) in self
            .instances
            .iter()
            .zip(state.current.as_chunks_mut::<5>().0)
        {
            let m = &dev.size;
            let act = dev.inner.map(|n| volt(x, n));
            let [d, g, s, b] = act;
            let mut bias = [g - s, d - s, b - s].map(|v| m.sign * v);
            if ctx.init_junctions {
                bias = if dev.off {
                    [0.; 3]
                } else {
                    [m.vth0 + 0.1, 0.1, -0.1]
                };
            } else if ctx.limit {
                let (vg, lg) = limit_step(bias[0], prev[0], prev[0] - m.vth0);
                let (vd, ld) = limit_step(bias[1], prev[1], prev[1]);
                let limit = |v, old, side: usize| {
                    let j = &m.junction[side];
                    limit_junction(v, old, j.nvt, vcrit(j.isat.max(1e-30), j.nvt))
                };
                let (vb, lb) = limit(bias[2], prev[2], 1);
                let (vbd, lbd) = limit(vb - vd, prev[2] - prev[1], 0);
                bias = [vg, if lbd { vb - vbd } else { vd }, vb];
                out.noncon |= lg || ld || lb || lbd;
            }
            prev[..3].copy_from_slice(&bias);
            let lin = [
                s + m.sign * bias[1],
                s + m.sign * bias[0],
                s,
                s + m.sign * bias[2],
            ];
            let (f, q, bias) = m.terminals(lin);
            let mut at = dev.at;
            out.core(&mut at, &f, &q, &act, &lin);
            let ov = dev.overlap_nodes().map(|n| volt(x, n));
            let [od, og, os, ob] = std::array::from_fn(|k| D::var(ov[k], k));
            let gd = m.overlap_charge((og - od) * m.sign, 0) * m.sign * m.mult;
            let gs = m.overlap_charge((og - os) * m.sign, 1) * m.sign * m.mult;
            let gb = (og - ob) * (m.overlap[2] * m.mult);
            out.core(
                &mut at,
                &[D::cst(0.); 4],
                &[-gd, gd + gs + gb, -gs, -gb],
                &ov,
                &ov,
            );
            for side in 0..2 {
                let j = &m.junction[side];
                let local = [volt(x, dev.body[side]), act[2 * side]];
                let raw = m.sign * (local[0] - local[1]);
                let (v, limited) = if ctx.limit {
                    limit_junction(raw, prev[3 + side], j.nvt, vcrit(j.isat.max(1e-30), j.nvt))
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
            if let Some(body) = dev.network.body {
                for ((a, b), g) in dev.body_branches().into_iter().zip(body) {
                    out.resistor(&mut at, g * m.mult, volt(x, a), volt(x, b));
                }
            }
            let mode = dev.network.gate_mode;
            if matches!(mode, 1 | 3) {
                out.resistor(
                    &mut at,
                    m.mult / dev.network.gate.max(1e-6),
                    volt(x, dev.nodes[1]),
                    volt(x, if mode == 1 { dev.inner[1] } else { dev.middle }),
                );
            }
            if mode >= 2 {
                let conductance =
                    dev.network.xrcrg1 * (bias.id_per_volt + dev.network.xrcrg2 * bias.beta * m.vt);
                let g = if mode == 2 {
                    conductance / (1. + dev.network.gate * conductance)
                } else {
                    conductance
                };
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
