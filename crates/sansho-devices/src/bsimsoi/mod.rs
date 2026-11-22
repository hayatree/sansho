//! Native silicon-on-insulator MOSFETs with an iterated film and thermal balance.
//! Provenance: BSIMSOIv4.4 Users' Manual (UC Berkeley, 2010), chapters 2–8 and Appendix A.

mod channel;
mod leakage;
mod noise;
mod params;
#[cfg(test)]
mod tests;

use crate::{
    BOLTZMANN, CHARGE, DeviceGroup, EvalCtx, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns, bsim4::floor, dual::Dual, junction::limit_junction, volt,
};
pub use params::Model;
use sansho_diag::Diagnostic;

// The geometric ratio of the isdif body sweep gives k/q in Eq. 3.1.
const VT_PER_K: f64 = 8.617087e-5;
const EPS_OX: f64 = 3.9 * 8.8541878128e-12;
const EPS_SI: f64 = 11.7 * 8.8541878128e-12;
// D, intrinsic G, S, film B, back gate E, temperature rise T, gate access/middle gate.
// Junction-body nodes occupy 7/8, external drain/source occupy 9/10, and P occupies 11.
const N: usize = 12;
type D = Dual<N>;
const ZERO: D = Dual::cst(0.);

fn branch(rows: &mut [D; N], p: usize, n: usize, value: D) {
    rows[p] += value;
    rows[n] += -value;
}

struct Point {
    bias: channel::Bias,
    drain: usize,
    i: [D; N],
    q: [D; N],
    qinv: f64,
    film_charge: f64,
    contact_gate: D,
    junction: [leakage::Junction; 2],
    gate: [D; 5],
    induced: [D; 2],
    extension_g: [D; 2],
    impact: D,
    bjt: D,
    gaccess: D,
}

impl Model {
    // Integral of the two quadratic capacitance segments in Eq. 4.17.
    fn box_charge(&self, v: D) -> D {
        let span = self.vsdth - self.vsdfb;
        if span.abs() < 1e-12 {
            return self.cbox * v;
        }
        let x = (v - self.vsdfb) / span;
        let a = self.asd;
        let delta = self.cbox - self.cmin;
        let join = self.cbox * a - delta * a * a / 3.;
        let end = join + self.cmin * (1. - a) + delta * (1. - a).powi(2) / 3.;
        let integral = if x.v <= 0. {
            self.cbox * x
        } else if x.v < a {
            self.cbox * x - delta * x * x.sq() / (3. * a)
        } else if x.v < 1. {
            end - self.cmin * (1. - x) - delta * (1. - x).powf(3.) / (3. * (1. - a))
        } else {
            end + self.cmin * (x - 1.)
        };
        self.cbox * self.vsdfb + span * integral
    }
    fn evaluate(&self, voltage: [f64; N], gmin: f64) -> Point {
        let v: [D; N] =
            std::array::from_fn(|k| D::var(voltage[k], k) * if k == 5 { 1. } else { self.sign });
        let reverse = v[0].v < v[2].v;
        let (drain, src) = if reverse { (2, 0) } else { (0, 2) };
        let temp = if self.rth > 0. {
            floor(self.temp + v[5], 50.)
        } else {
            D::cst(self.temp)
        };
        let bias = self.channel(
            v[1] - v[src],
            v[drain] - v[src],
            v[4] - v[src],
            v[3] - v[src],
            temp,
        );
        let (qch, qinv) = self.charges(&bias);
        let mut q = [ZERO; N];
        let mut i = [ZERO; N];
        for (k, val) in [drain, 1, src, 3].into_iter().zip(qch) {
            q[k] = val
        }
        // Eqs. 3.14 and 4.19. Treat the opposite implant as a local MOS
        // capacitor over the film. The opposite_gate pole moves with patch area
        // behind the body contact resistance. The tunnelling voltage is Vgp.
        let mut contact_gate = ZERO;
        if self.opposite_area > 0. && self.mode != 2 {
            let vgp = v[1] - v[11];
            let gap = crate::temperature::gap(self.nominal);
            let patch = self.channel(v[1] - v[3] - gap, ZERO, ZERO, ZERO, temp);
            let area = self.wcv * self.lcv * self.nf / self.nseg + self.extra_area[0];
            branch(
                &mut q,
                1,
                3,
                self.opposite_area / area * self.charges(&patch).0[1],
            );
            if self.igb {
                let voltage = vgp - self.vfb - gap;
                let smooth = 0.5 * ((voltage.sq() + 0.0001).sqrt() - voltage - 0.01);
                contact_gate = self
                    .opposite_gate
                    .current(-floor(-voltage, 0.), smooth, smooth);
                branch(&mut i, 1, 11, contact_gate);
            }
        }
        // Keep the pre-junction film charge for the DEBUG=-1 readout.
        let mut film_charge = q[3].v;
        let gate = self.tunneling(&bias, v[1] - v[3], v[1] - v[0], v[1] - v[2], reverse);
        branch(&mut i, 1, 0, gate[0] + gate[2]);
        branch(&mut i, 1, 2, gate[1] + gate[3]);
        branch(&mut i, 1, 3, gate[4]);
        // Isolated leads (R17 isbjt=0 and contact_transfer) measure GMIN/1e6.
        let shunt = gmin * 1e-6;
        branch(&mut i, 1, 0, shunt * (v[1] - v[0]));
        if self.mode != 2 {
            branch(&mut i, 3, 2, shunt * (v[3] - v[2]));
        }
        let alpha = (-0.5 * (self.l / self.p.ln).powi(2)).exp();
        let mut junction = [leakage::Junction {
            current: ZERO,
            charge: ZERO,
            scale: ZERO,
            emission: ZERO,
            high: ZERO,
        }; 2];
        let mut bjt = ZERO;
        let mut impact = ZERO;
        let mut induced = [ZERO; 2];
        let mut extension_g = [ZERO; 2];
        let overlap_gate = if self.rg == 3 { 6 } else { 1 };
        for (j, end) in [0, 2].into_iter().enumerate() {
            let local = if self.junction_r[j] > 0. && self.mode != 2 {
                7 + j
            } else {
                3
            };
            let vb = v[local] - v[end];
            if self.mode != 2 {
                let pn = self.junction(j, vb, &bias, alpha);
                let leakage =
                    self.induced(j, v[end] - v[2 - end], v[1] - v[2 - end], v[end] - v[3]);
                induced[j] = leakage;
                branch(&mut i, local, end, pn.current);
                branch(&mut i, end, 3, leakage);
                branch(&mut q, local, end, pn.charge);
                junction[j] = pn;
            }
            if let Some(extension) = self.extension {
                // §2.4, external LDD and sheet resistances in series.
                let [r, min] = extension[j];
                let flat = if self.ngate > 0. {
                    bias.vt * (self.ngate / 1e20).ln()
                } else {
                    ZERO
                };
                let gate = floor(v[1] - v[end] - flat, 0.);
                let factor = floor(
                    1. / floor(1. + self.p.prwg * gate, 0.05) - self.p.prwb * (v[3] - v[end]),
                    0.,
                );
                let res =
                    self.rsd[j] + (min + r * factor) / ((1e6 * self.w).powf(self.p.wr) * self.nf);
                if res.v > 0. {
                    extension_g[j] = 1. / floor(res, 1e-9);
                    branch(&mut i, 9 + j, end, extension_g[j] * (v[9 + j] - v[end]));
                }
            }
            let ov = v[overlap_gate] - v[end];
            let s = &self.side[j];
            branch(
                &mut q,
                overlap_gate,
                end,
                s.overlap * ov + s.ldd * crate::bsim4::ldd_overlap(ov, s.kappa),
            );
            let ve = v[end] - v[4];
            branch(&mut q, end, 4, s.area * self.box_charge(ve) + s.fringe * ve);
        }
        if self.mode != 2 {
            let early = floor(
                1. + (2. * v[3] - v[0] - v[2]) / (self.p.vabjt + self.p.aely * self.l).max(1e-6),
                0.01,
            );
            let second =
                0.5 * (early + (early.sq() + 4. * (junction[0].high + junction[1].high)).sqrt());
            if self.bjt {
                bjt = alpha
                    * junction[1].scale
                    * (junction[src / 2].emission - junction[drain / 2].emission)
                    / second;
            }
            impact = self.impact(&bias, bjt, v[3] - v[drain]);
            branch(&mut i, drain, 3, impact);
            // Eq. 4.14 gives a distinct back-gate row and its opposite film charge.
            let area = self.wcv * (self.lcv - self.p.dlcb + 2. * self.p.dlbg) * self.nf / self.nseg
                + self.extra_area[1];
            let qe = self.p.kb1 * self.p.fbody * area * self.cbox * (v[4] - v[3] - bias.flat_back);
            branch(&mut q, 4, 3, qe);
            film_charge -= qe.v;
        }
        branch(
            &mut q,
            overlap_gate,
            4,
            self.cgeo * (v[overlap_gate] - v[4]),
        );
        branch(&mut i, drain, src, bias.ids + bjt);
        let mut gaccess = ZERO;
        if self.rg >= 2 {
            // Eq. 7.2. The mode-3 overlap currents enter the middle gate.
            gaccess = floor(
                self.p.xrcrg1 * (bias.conductance + self.p.xrcrg2 * bias.beta * bias.vt),
                1e-15,
            );
            if self.rg == 2 {
                gaccess = gaccess / (1. + self.relectrode * gaccess);
            }
            branch(&mut i, 6, 1, gaccess * (v[6] - v[1]));
        }
        let scale = self.sign * self.copies;
        i = i.map(|x| x * scale);
        q = q.map(|x| x * scale);
        if self.rth > 0. {
            // Eqs. 5.2–5.3. The temperature unknown is a rise in kelvin.
            i[5] = self.copies * (v[5] / self.rth - (bias.ids + bjt) * bias.vd);
            q[5] = self.copies * self.cth * v[5];
        }
        Point {
            bias,
            drain,
            i,
            q,
            qinv,
            film_charge: scale * film_charge,
            contact_gate,
            junction,
            induced,
            extension_g,
            gate,
            impact,
            bjt,
            gaccess,
        }
    }
}
#[derive(Debug)]
struct Instance {
    name: String,
    model: Model,
    nodes: [Node; N],
    contact: Option<Node>,
    film: Option<Node>,
    thermal: Option<Node>,
    external: [Node; 3],
    off: bool,
    body_short: Node,
    leads: Vec<(Node, Node, f64)>,
    slots: SlotOffsets,
}

#[derive(Default, Debug)]
pub struct Sois {
    instances: Vec<Instance>,
}

impl Sois {
    /// Optional terminals retain an explicit ground separately from an omitted pin.
    pub fn push(
        &mut self,
        name: &str,
        external: [Node; 4],
        extra: &[Node],
        thermal: bool,
        off: bool,
        model: Model,
    ) -> Result<(), String> {
        let (pins, t) = if thermal {
            let (last, rest) = extra
                .split_last()
                .ok_or("TNODEOUT needs an explicit temperature pin")?;
            (rest, Some(*last))
        } else {
            (extra, None)
        };
        if pins.len() > 2 {
            return Err("a seventh terminal requires TNODEOUT".into());
        }
        let [d, g, s, e] = external;
        self.instances.push(Instance {
            name: name.into(),
            model,
            nodes: [d, g, s, None, e, None, g, None, None, d, s, None],
            contact: pins.first().copied(),
            film: pins.get(1).copied(),
            thermal: t,
            external: [d, g, s],
            off,
            body_short: None,
            leads: Vec::new(),
            slots: SlotOffsets::default(),
        });
        Ok(())
    }
}

impl Instance {
    fn report(
        &self,
        v: &Point,
        volt: [f64; N],
        dc_sweep: bool,
        out: &mut Vec<(&'static str, f64)>,
    ) {
        let m = &self.model;
        let a = &v.bias;
        let n = m.copies;
        out.extend([
            ("id", a.ids.v * n),
            ("gm", a.ids.d[1] * m.sign * n),
            ("gds", a.ids.d[v.drain] * m.sign * n),
            ("gmbs", a.ids.d[3] * m.sign * n),
            ("vth", a.threshold.v),
            ("vdsat", a.sat.v),
            (
                "ig",
                (v.gate.iter().map(|x| x.v).sum::<f64>() + v.contact_gate.v) * n * m.sign,
            ),
            ("is", v.i[2].v + v.i[10].v),
            ("ib", v.i[3].v + v.i[7].v + v.i[8].v),
            ("ie", v.i[4].v),
            (
                "vbs",
                if m.mode == 2 {
                    0.
                } else {
                    m.sign * (volt[3] - volt[2])
                },
            ),
            ("ves", volt[4] - volt[2]),
            ("vgs", volt[1] - volt[2]),
            ("vds", volt[0] - volt[2]),
            ("ids", (a.ids.v + v.bjt.v) * n),
            ("igidl", v.induced[0].v * n),
            ("igisl", v.induced[1].v * n),
            ("ic", v.bjt.v * n),
            ("isub", v.impact.v * n),
        ]);
        for (name, x) in ["igd", "igs", "igcd", "igcs", "igb"]
            .into_iter()
            .zip(v.gate)
        {
            out.push((name, x.v * n))
        }
        for (j, pn) in v.junction.iter().enumerate() {
            out.extend([
                (["ibd", "ibs"][j], pn.current.v * n),
                (
                    ["capbd", "capbs"][j],
                    (pn.charge.d[3] + pn.charge.d[7 + j]) * m.sign * n,
                ),
                (["qbd", "qbs"][j], pn.charge.v * n),
            ]);
        }
        // Appendix A.3 enables stored DC charges with DEBUG=-1.
        for (row, q) in v.q[..4].iter().enumerate() {
            let q = if dc_sweep && !m.dc_charges { ZERO } else { *q };
            let charge = if dc_sweep && m.dc_charges && row == 3 {
                v.film_charge
            } else {
                q.v
            };
            crate::observables::terminal_charge(out, row, charge, [q.d[0], q.d[1], q.d[2], q.d[3]]);
        }
    }
}

impl DeviceGroup for Sois {
    fn type_name(&self) -> &'static str {
        "bsimsoi"
    }
    fn len(&self) -> usize {
        self.instances.len()
    }
    fn instance_name(&self, k: usize) -> Option<&str> {
        self.instances.get(k).map(|d| d.name.as_str())
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        2 * self.len()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for d in &mut self.instances {
            let m = &d.model;
            let mut internal = |key: &str| Some(u.alloc_internal_node(&d.name, key));
            if m.mode != 2 {
                d.nodes[3] = if let Some(b) = d.film {
                    b
                } else if m.rb == 0. && d.contact.is_some() {
                    d.contact.flatten()
                } else {
                    internal("body")
                };
                if let Some(p) = d.contact
                    && p != d.nodes[3]
                    && m.rb > 0.
                {
                    d.leads.push((p, d.nodes[3], m.rb));
                }
            }
            d.nodes[11] = d.contact.unwrap_or(d.nodes[3]);
            if m.rth > 0. {
                d.nodes[5] = d.thermal.unwrap_or_else(|| internal("temp"));
            }
            if m.rg >= 2 || m.rg == 1 && m.relectrode > 0. {
                d.nodes[1] = internal("gate");
                if m.rg == 1 {
                    d.leads.push((d.external[1], d.nodes[1], m.relectrode));
                }
                if m.rg == 3 && m.relectrode > 0. {
                    d.nodes[6] = internal("midgate");
                    d.leads.push((d.external[1], d.nodes[6], m.relectrode));
                }
            }
            for (j, end) in [0, 2].into_iter().enumerate() {
                if m.mode != 2 && m.junction_r[j] > 0. {
                    d.nodes[7 + j] = internal(["dbody", "sbody"][j]);
                    d.leads.push((
                        d.nodes[7 + j],
                        d.nodes[3],
                        1. / (1. / m.junction_r[j] + m.gbmin),
                    ));
                } else {
                    d.nodes[7 + j] = d.nodes[3];
                }
                let variable = m
                    .extension
                    .is_some_and(|ext| ext[j].iter().any(|r| *r > 0.));
                if m.rsd[j] > 0. || variable {
                    d.nodes[end] = internal(["drain", "source"][j]);
                    if m.extension.is_none() {
                        d.leads.push((d.external[end], d.nodes[end], m.rsd[j]));
                    }
                }
            }
            if m.mode != 2
                && m.rb == 0.
                && let (Some(p), Some(b)) = (d.contact, d.film)
                && p != b
            {
                d.body_short = Some(u.alloc_branch(&format!("{}#bodytie", d.name)));
            }
        }
    }
    fn initial_values(&self, _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        for d in &self.instances {
            if let Some(v) = d.model.vbsusr
                && d.model.mode != 2
            {
                edges.push((d.nodes[3], d.nodes[2], v));
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for d in &mut self.instances {
            d.slots = b.counts();
            b.charge_core(&d.nodes);
            if let Some(k) = d.body_short {
                let p = d.contact.flatten();
                let film = d.nodes[3];
                for (r, c) in [(p, Some(k)), (film, Some(k)), (Some(k), p), (Some(k), film)] {
                    b.g(r, c);
                }
                for r in [p, film, Some(k)] {
                    b.f(r);
                }
            }
            for &(a, c, _) in &d.leads {
                b.resistor(a, c);
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        for (k, (d, prev)) in self
            .instances
            .iter()
            .zip(state.current.as_chunks_mut::<2>().0)
            .enumerate()
        {
            let m = &d.model;
            // R17 IC_BODY starts from the same stored film charge for all
            // three IC tuples. Sample the nodal UIC solution, including .ic.
            let act = d.nodes.map(|n| volt(x, n));
            let mut lin = act;
            let mut bs = m.sign * (act[3] - act[2]);
            let mut bd = m.sign * (act[3] - act[0]);
            if ctx.init_junctions && d.off {
                lin[1] = act[2];
            }
            if ctx.limit && m.mode != 2 {
                let vt = VT_PER_K * m.temp;
                let (s, ls) = limit_junction(bs, prev[0], vt, 0.6);
                let (t, ld) = limit_junction(bd, prev[1], vt, 0.6);
                if ls || ld {
                    bs = s;
                    bd = t;
                    lin[3] = act[2] + m.sign * bs;
                    lin[0] = lin[3] - m.sign * bd;
                    out.noncon = true;
                }
            }
            *prev = [bs, bd];
            let v = m.evaluate(lin, ctx.gmin);
            if let Some(list) = out.outputs.get_mut(k) {
                d.report(&v, lin, ctx.dc_sweep, list);
            }
            let mut at = d.slots;
            out.core(&mut at, &v.i, &v.q, &act, &lin);
            if let Some(k) = d.body_short {
                out.g[at.g..at.g + 4].copy_from_slice(&[1., -1., 1., -1.]);
                out.f[at.f..at.f + 3].copy_from_slice(&[
                    x[k],
                    -x[k],
                    volt(x, d.contact.flatten()) - act[3],
                ]);
                at.g += 4;
                at.f += 3;
            }
            for &(p, n, r) in &d.leads {
                out.resistor(&mut at, m.copies / r, volt(x, p), volt(x, n));
            }
        }
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        _: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for d in &self.instances {
            let m = &d.model;
            let v = m.evaluate(d.nodes.map(|n| volt(x, n)), ctx.gmin);
            let [white, flicker, source_voltage] = m.noise.powers(m, &v);
            let mut add = |p, n, power: f64, ef| {
                if power != 0. {
                    out.push(NoiseSource::branch(&d.name, p, n, power, ef))
                }
            };
            add(d.nodes[0], d.nodes[2], white, 0.);
            add(d.nodes[0], d.nodes[2], flicker, m.noise.ef);
            let shot = 2. * CHARGE * m.copies;
            let kt = 4. * BOLTZMANN * v.bias.temp.v * m.copies;
            for (j, k) in [0, 2].into_iter().enumerate() {
                add(
                    d.nodes[7 + j],
                    d.nodes[k],
                    m.noise.noif * shot * v.junction[j].current.v.abs(),
                    0.,
                );
                add(d.nodes[9 + j], d.nodes[k], kt * v.extension_g[j].v, 0.);
                add(
                    d.nodes[1],
                    d.nodes[k],
                    shot * (v.gate[j].v + v.gate[j + 2].v).abs(),
                    0.,
                );
            }
            add(d.nodes[1], d.nodes[3], shot * v.gate[4].v.abs(), 0.);
            add(d.nodes[1], d.nodes[11], shot * v.contact_gate.v.abs(), 0.);
            add(d.nodes[6], d.nodes[1], kt * v.gaccess.v, 0.);
            for &(p, n, r) in &d.leads {
                add(p, n, kt / r, 0.);
            }
            // Eq. 8.8 inserts a series source voltage. Its residual weights
            // are the source column of the intrinsic G+jωC linearization.
            let source = 2 - v.drain;
            if source_voltage > 0. && d.nodes[source] != d.external[source] {
                let mut generator = NoiseSource::branch(&d.name, None, None, source_voltage, 0.);
                for (r, node) in d.nodes.iter().enumerate() {
                    if let Some(n) = node {
                        generator
                            .rows
                            .push((*n, v.i[r].d[source], v.q[r].d[source]));
                    }
                }
                out.push(generator);
            }
            if !white.is_finite() || !flicker.is_finite() {
                return Err(Diagnostic::error(
                    "E-ELAB-MODEL",
                    format!("{}: nonfinite SOI noise power", d.name),
                ));
            }
        }
        Ok(())
    }
}
