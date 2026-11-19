//! Junction diode (SPICE level-1 diode model).
//!
//! Forward current, including sidewall and recombination, rolls off at IKF. Recombination
//! depends on depletion width. At breakdown the reverse branch reaches IBV at BV
//! with exponential slope set by NBV. RS adds an internal anode.
//!
//! Provenance: Massobrio & Antognetti, *Semiconductor Device Modeling with SPICE*,
//! chapter 1, and the ngspice user manual, section 7.2.2, equations 7.1–7.3.
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
    /// Prepared operating temperature in kelvin.
    pub temperature: Option<f64>,
    pub kf: f64,
    pub af: f64,
    pub tnom: Option<f64>,
    pub eg: f64,
    pub xti: f64,

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
    pub nbv: f64,
    pub isr: f64,
    pub nr: f64,
    pub ikf: f64,
    pub jsw: f64,
    pub ns: f64,
    pub cjp: f64,
    pub php: f64,
    pub mjsw: f64,
    pub fcs: f64,
    pub perimeter: f64,
}

impl DiodeModel {
    pub fn at_temperature(&mut self, t: crate::temperature::Temperature) {
        self.temperature = Some(t.kelvin);
        if t.kelvin == t.nominal {
            return;
        }
        self.is *= (t.saturation(self.eg, self.xti) / self.n).exp();
        self.isr *= (t.saturation(self.eg, self.xti) / self.nr).exp();
        self.jsw *= (t.saturation(self.eg, self.xti) / self.ns).exp();
        self.cjp *= t.capacitance(self.php, self.mjsw);
        self.php = t.potential(self.php);
        self.cjo *= t.capacitance(self.vj, self.m);
        self.vj = t.potential(self.vj);
    }
    /// Build from `.model` parameters. Returns the model and the names of ignored parameters.
    pub fn from_params(p: &ModelParams) -> (Self, Vec<String>) {
        let m = Self {
            temperature: None,
            kf: p.get("kf", 0.),
            af: p.get("af", 1.),
            tnom: p.opt(&["tnom"]),
            eg: p.get("eg", 1.11),
            xti: p.get("xti", 3.0),

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
            nbv: p.get("nbv", p.get("n", 1.)),
            isr: p.get("isr", 0.),
            nr: p.get("nr", 2.),
            ikf: p.opt(&["ikf", "ik"]).unwrap_or(0.),
            jsw: p.opt(&["jsw", "isw"]).unwrap_or(0.),
            ns: p.get("ns", 1.),
            cjp: p.opt(&["cjp", "cjsw"]).unwrap_or(0.),
            php: p.opt(&["php", "vjsw"]).unwrap_or(1.),
            mjsw: p.get("mjsw", 0.33),
            fcs: p.get("fcs", 0.5),
            perimeter: 0.,
        };
        p.accept(&["level", "kf", "af"]);
        (m, p.unused())
    }
}

/// Eq. 7.1 continues the Shockley branch below -3 nVt with a cubic
/// in reciprocal voltage. Both current and slope agree at the join.
fn diffusion_current(v: Dual<2>, saturation: f64, nvt: f64) -> Dual<2> {
    if v.v >= -3. * nvt {
        ideal_current(v, saturation, nvt)
    } else {
        let ratio = (3. * nvt / std::f64::consts::E) / v;
        -saturation * (1. + ratio.sq() * ratio)
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
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        slots: &Slots,
        out: &mut Vec<crate::NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for i in 0..self.len() {
            let m = &self.model[i];
            let temp = m.temperature.unwrap_or(ctx.temp);
            let thermal = 4. * crate::BOLTZMANN * temp;
            let mut at = self.at[i];
            let mut emit = |p, n, power, exp| {
                out.push(crate::NoiseSource::branch(&self.names[i], p, n, power, exp))
            };
            let (p, n) = (self.ai[i], self.k[i]);
            if m.rs > 0. {
                emit(self.a[i], p, thermal * self.area[i] / m.rs, 0.);
                at.f += 2;
            }
            let id = (slots.f[at.f] - ctx.gmin * (volt(x, p) - volt(x, n))).abs();
            emit(p, n, 2. * crate::CHARGE * id, 0.);
            emit(p, n, m.kf * id.powf(m.af), 1.);
        }
        Ok(())
    }
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
        for i in 0..self.len() {
            let m = &self.model[i];
            let vt = thermal_voltage(m.temperature.unwrap_or(ctx.temp));
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
                let breakdown_vt = m.nbv * vt;
                if m.bv.is_finite() && v < (-m.bv + 10. * breakdown_vt).min(0.) {
                    let (vr, limited) = limit_junction(
                        -(v + m.bv),
                        -(old + m.bv),
                        breakdown_vt,
                        vcrit(m.ibv * area, breakdown_vt),
                    );
                    v = -(vr + m.bv);
                    lim |= limited;
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
            let diffusion = diffusion_current(v, is, nvt)
                + diffusion_current(v, m.jsw * m.perimeter, m.ns * vt);
            let generation = if m.isr > 0. {
                let width = ((1. - v / m.vj).sq() + 0.005).powf(m.m * 0.5);
                ideal_current(v, m.isr * area, m.nr * vt) * width
            } else {
                Dual::cst(0.)
            };
            let mut id = diffusion + generation;
            if m.bv.is_finite() {
                // Algorithm 7.1: match the reverse exponential to the leakage
                // at nominal BV. NBV sets the avalanche voltage scale.
                let leakage = -is * (-m.bv / nvt).exp_m1()
                    - m.jsw * m.perimeter * (-m.bv / (m.ns * vt)).exp_m1();
                let target = (area * m.ibv).max(leakage);
                let breakdown_vt = m.nbv * vt;
                let knee = if leakage > 0. {
                    -m.bv + breakdown_vt * (target.ln() - leakage.ln())
                } else {
                    0.
                };
                if vd < 0. && vd <= knee {
                    let avalanche = -target * crate::junction::safe_exp(-(v + m.bv) / breakdown_vt);
                    // Generation can exceed the diffusion floor at the knee.
                    // Switch where avalanche overtakes the full reverse current.
                    if avalanche.v < id.v {
                        id = avalanche;
                    }
                }
            }
            if m.ikf > 0. && id.v > 0. {
                // Equation 7.3 acts on the total forward junction current.
                id = id / (1. + (id / (m.ikf * area)).sqrt());
            }
            let q = id * m.tt
                + depletion_charge(v, m.cjo * area, m.vj, m.m, m.fc)
                + depletion_charge(v, m.cjp * m.perimeter, m.php, m.mjsw, m.fcs);
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
    fn breakdown_inherits_emission_coefficient_without_override() {
        let m = DiodeModel::from_params(&ModelParams::new(&[("n".into(), 1.05)])).0;
        assert_eq!(m.nbv, m.n);
    }

    #[test]
    fn series_resistance_adds_internal_node() {
        let (_, l, u) = setup(10.0);
        assert_eq!(u.names(), ["v(a)", "v(d1#a)"]);
        assert_eq!(l.g.len(), 4 + 4);
    }
}
