//! Junction diode (SPICE level-1 diode model).
//!
//! Current: `Id = IS·(exp(Vd/(N·Vt)) − 1) − IBV·exp(−(Vd + BV)/(N·Vt))` plus `gmin·Vd`.
//! Charge: transit-time charge `TT·Id` plus the depletion charge of [`crate::junction`].
//! A series resistance `RS` adds an internal anode node.
//!
//! Provenance: original implementation of the textbook diode model as described in
//! Antognetti & Massobrio, *Semiconductor Device Modeling with SPICE* (1993), ch. 1, and
//! Nagel (1975). Breakdown is modelled with a single exponential, a deliberate simplification.
//! See `docs/PROVENANCE.md`.

use spx_diag::Diagnostic;

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, vcrit};
use crate::model::ModelParams;
use crate::{
    DeviceGroup, EvalCtx, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns,
    thermal_voltage, volt,
};

/// Diode model parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct DiodeModel {
    /// Saturation current (A).
    pub is: f64,
    /// Emission coefficient.
    pub n: f64,
    /// Series resistance (Ω).
    pub rs: f64,
    /// Zero-bias junction capacitance (F).
    pub cjo: f64,
    /// Junction potential (V).
    pub vj: f64,
    /// Grading coefficient.
    pub m: f64,
    /// Forward-bias depletion capacitance coefficient.
    pub fc: f64,
    /// Transit time (s).
    pub tt: f64,
    /// Reverse breakdown voltage (V); infinite if not given.
    pub bv: f64,
    /// Current at breakdown voltage (A).
    pub ibv: f64,
}

impl DiodeModel {
    /// Build from `.model` parameters. Returns the model and the names of ignored parameters.
    pub fn from_params(p: &ModelParams) -> (Self, Vec<String>) {
        let m = Self {
            is: p.get("is", 1e-14),
            n: p.get("n", 1.0),
            rs: p.get("rs", 0.0),
            cjo: p.opt(&["cjo", "cj0", "cj"]).unwrap_or(0.0),
            vj: p.opt(&["vj", "pb"]).unwrap_or(1.0),
            m: p.get("m", 0.5),
            fc: p.get("fc", 0.5),
            tt: p.get("tt", 0.0),
            bv: p.get("bv", f64::INFINITY),
            ibv: p.get("ibv", 1e-3),
        };
        // Temperature and noise parameters have no effect while TEMP = TNOM and no noise
        // analysis exists.
        p.accept(&["level", "eg", "xti", "kf", "af", "tnom"]);
        (m, p.unused())
    }
}

/// All diode instances.
#[derive(Debug, Default)]
pub struct Diodes {
    names: Vec<String>,
    a: Vec<Node>,
    k: Vec<Node>,
    ai: Vec<Node>,
    area: Vec<f64>,
    off: Vec<bool>,
    model: Vec<DiodeModel>,
    at: Vec<SlotOffsets>,
}

impl Diodes {
    /// Add an instance between anode `a` and cathode `k`.
    pub fn push(&mut self, name: &str, a: Node, k: Node, model: DiodeModel, area: f64, off: bool) {
        self.names.push(name.to_string());
        self.a.push(a);
        self.k.push(k);
        self.ai.push(a);
        self.area.push(area);
        self.off.push(off);
        self.model.push(model);
    }
}

impl DeviceGroup for Diodes {
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(String::as_str)
    }
    fn type_name(&self) -> &'static str {
        "diode"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        self.len()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for i in 0..self.len() {
            if self.model[i].rs > 0.0 {
                self.ai[i] = Some(u.alloc_internal_node(&self.names[i], "a"));
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.at.clear();
        for i in 0..self.len() {
            self.at.push(b.counts());
            if self.model[i].rs > 0.0 {
                b.resistor(self.a[i], self.ai[i]);
            }
            b.charge_core(&[self.ai[i], self.k[i]]);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let vt = thermal_voltage(ctx.temp);
        for i in 0..self.len() {
            let m = &self.model[i];
            let area = self.area[i];
            let mut at = self.at[i];
            if m.rs > 0.0 {
                out.resistor(
                    &mut at,
                    area / m.rs,
                    volt(x, self.a[i]),
                    volt(x, self.ai[i]),
                );
            }
            let (va, vk) = (volt(x, self.ai[i]), volt(x, self.k[i]));
            let nvt = m.n * vt;
            let is = m.is * area;
            let vc = vcrit(is, nvt);
            let vd_act = va - vk;
            let vd = if ctx.init_junctions {
                if self.off[i] { 0.0 } else { vc }
            } else if ctx.limit {
                let old = st.current[i];
                let (mut v, mut lim) = limit_junction(vd_act, old, nvt, vc);
                if m.bv.is_finite() && v < (-m.bv + 10.0 * nvt).min(0.0) {
                    // Limit the breakdown branch as a mirrored junction.
                    let (vr, l2) = limit_junction(-(v + m.bv), -(old + m.bv), nvt, vc);
                    v = -(vr + m.bv);
                    lim |= l2;
                }
                if lim {
                    out.noncon = true;
                }
                v
            } else {
                vd_act
            };
            st.current[i] = vd;
            let v = Dual::<2>::var(vd, 0);
            let mut id = ideal_current(v, is, nvt);
            if m.bv.is_finite() {
                id = id - (crate::junction::safe_exp(-(v + m.bv) / nvt)) * (m.ibv * area);
            }
            let q = ideal_current(v, is, nvt) * m.tt
                + depletion_charge(v, m.cjo * area, m.vj, m.m, m.fc);
            let id = id + v * ctx.gmin;
            // The core variables are terminal voltages (a', k); vd = va' - vk.
            let lift = |d: Dual<2>| Dual::<2> {
                v: d.v,
                d: [d.d[0], -d.d[0]],
            };
            let (id, q) = (lift(id), lift(q));
            out.core(&mut at, &[id, -id], &[q, -q], &[va, vk], &[vk + vd, vk]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StampLayout, Unknowns};

    fn setup(rs: f64) -> (Diodes, StampLayout, Unknowns) {
        let mut d = Diodes::default();
        let model = DiodeModel {
            rs,
            ..DiodeModel::from_params(&ModelParams::default()).0
        };
        d.push("d1", Some(0), None, model, 1.0, false);
        let mut u = Unknowns::with_nodes(["a"]);
        d.alloc_unknowns(&mut u);
        let mut b = StampBuilder::new(&u);
        d.setup(&mut b).unwrap();
        (d, b.finish(), u)
    }

    #[test]
    fn current_and_conductance_at_operating_point() {
        let (d, l, _) = setup(0.0);
        let mut s = Slots::for_layout(&l);
        let ctx = EvalCtx {
            limit: false,
            gmin: 0.0,
            ..EvalCtx::dc()
        };
        let mut cur = [0.0];
        d.eval(
            &ctx,
            &[0.6],
            StateRef {
                accepted: &[0.0],
                current: &mut cur,
            },
            &mut s,
        );
        let vt = thermal_voltage(crate::REFTEMP);
        let expect = 1e-14 * ((0.6 / vt).exp() - 1.0);
        assert!((s.f[0] - expect).abs() < 1e-12 * expect);
        assert!((s.g[0] - 1e-14 * (0.6 / vt).exp() / vt).abs() < 1e-9);
    }

    #[test]
    fn series_resistance_adds_internal_node() {
        let (_, l, u) = setup(10.0);
        assert_eq!(u.names(), ["v(a)", "v(d1#a)"]);
        assert_eq!(l.g.len(), 4 + 4);
    }
}
