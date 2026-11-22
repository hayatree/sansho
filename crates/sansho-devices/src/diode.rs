//! Junction diode (SPICE level-1 diode model).
//!
//! Forward current, including sidewall and recombination, rolls off at IKF. Recombination
//! depends on depletion width. The avalanche knee is matched to bottom diffusion current.
//! Bottom and sidewall currents share the NBV exponential. RS adds an internal anode.
//!
//! Provenance: Massobrio & Antognetti, *Semiconductor Device Modeling with SPICE*,
//! chapter 1, and the ngspice user manual, section 7.2.2, equations 7.1–7.3.
//! Breakdown matching also uses the ngspice 44.2 black-box measurements in `verify/probes`.

use sansho_diag::Diagnostic;

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
    pub trs: [f64; 2],
    pub ttt: [f64; 2],
    pub tm: [f64; 2],

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
    /// Breakdown voltage decreases by TCV volts per kelvin above TNOM.
    pub tcv: f64,
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
        let delta = t.kelvin - t.nominal;
        let factor = |[a, b]: [f64; 2]| 1. + delta * (a + delta * b);
        self.bv -= self.tcv * delta;
        self.rs *= factor(self.trs);
        self.tt *= factor(self.ttt);
        self.is *= (t.saturation(self.eg, self.xti) / self.n).exp();
        self.isr *= (t.saturation(self.eg, self.xti) / self.nr).exp();
        self.jsw *= (t.saturation(self.eg, self.xti) / self.ns).exp();
        self.cjp *= t.capacitance(self.php, self.mjsw);
        self.php = t.potential(self.php);
        self.cjo *= t.capacitance(self.vj, self.m);
        self.vj = t.potential(self.vj);
        self.m *= factor(self.tm);
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
            trs: [p.opt(&["trs", "trs1"]).unwrap_or(0.), p.get("trs2", 0.)],
            ttt: [p.get("ttt1", 0.), p.get("ttt2", 0.)],
            tm: [p.get("tm1", 0.), p.get("tm2", 0.)],

            is: p.get("is", 1e-14),
            n: p.get("n", 1.0),
            rs: p.get("rs", 0.0),
            cjo: p.opt(&["cjo", "cj0", "cj"]).unwrap_or(0.0),
            vj: p.opt(&["vj", "pb"]).unwrap_or(1.0),
            m: p.get("m", 0.5),
            fc: p.get("fc", 0.5),
            tt: p.get("tt", 0.0),
            bv: p.get("bv", f64::INFINITY),
            tcv: p.get("tcv", 0.),
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

/// Avalanche knee (signed junction voltage) and reverse current at BV(T).
#[derive(Debug, Clone, Copy)]
struct Breakdown {
    knee: f64,
    at_bv: f64,
}

impl DiodeModel {
    fn breakdown(&self, area: f64, vt: f64) -> Breakdown {
        let bottom = self.is * area;
        let total = bottom + self.jsw * self.perimeter;
        let slope = self.nbv * vt;
        if bottom <= 0. {
            // The measured matching law requires a positive bottom current.
            // Preserve the area-scaled target for a sidewall-only junction.
            let at_bv = (self.ibv * area).max(total);
            let knee = if total > 0. {
                -self.bv + slope * (at_bv.ln() - total.ln())
            } else {
                0.
            };
            return Breakdown { knee, at_bv };
        }

        // Measured 44.2 matching condition for positive knee magnitude B:
        // IBV / Ibottom = exp((BV - B)/slope) - 1 + B/(N Vt).
        let nvt = self.n * vt;
        let minimum = bottom * (self.bv / nvt);
        if self.ibv <= minimum {
            return Breakdown {
                knee: -self.bv,
                at_bv: total,
            };
        }

        // Solve for the bottom contribution at BV in amperes. Taking the
        // difference of logs keeps even a very small positive IS usable.
        let excess = self.ibv - minimum;
        let log_bottom = bottom.ln();
        let rhs = |current: f64| {
            bottom + excess + bottom * (self.nbv / self.n) * (current.ln() - log_bottom)
        };
        let mut lo = bottom;
        let mut hi = self.ibv.max(bottom * 2.);
        while rhs(hi) > hi {
            hi *= 2.;
        }
        for _ in 0..54 {
            let mid = lo + 0.5 * (hi - lo);
            if rhs(mid) > mid {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let current = lo + 0.5 * (hi - lo);
        Breakdown {
            knee: -self.bv + slope * (current.ln() - log_bottom),
            at_bv: current * (total / bottom),
        }
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
    breakdown: Vec<Option<Breakdown>>,
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
        self.breakdown.push(
            model
                .temperature
                .filter(|_| model.bv.is_finite())
                .map(|t| model.breakdown(area, thermal_voltage(t))),
        );
        self.model.push(model);
    }
}

impl DeviceGroup for Diodes {
    fn distortion(
        &self,
        ctx: &EvalCtx,
        bias: &[f64],
        out: &mut Vec<crate::distortion::Term>,
    ) -> Result<(), Diagnostic> {
        use crate::distortion::{Jet, expand};
        for (i, m) in self.model.iter().enumerate() {
            let vt = thermal_voltage(m.temperature.unwrap_or(ctx.temp));
            let area = self.area[i];
            let avalanche = m.bv.is_finite().then(|| m.breakdown(area, vt));
            expand([self.ai[i], self.k[i]], bias, out, |[a, k]| {
                let v = a - k;
                let diffusion = |is: f64, nvt: f64| {
                    if v.value() >= -3. * nvt {
                        v.diode(is, nvt)
                    } else {
                        let r = Jet::constant(3. * nvt / std::f64::consts::E) / v;
                        (r * r * r + 1.) * (-is)
                    }
                };
                let mut id =
                    diffusion(m.is * area, m.n * vt) + diffusion(m.jsw * m.perimeter, m.ns * vt);
                if m.isr > 0. && v.value() >= -3. * m.n * vt {
                    let u = -v / m.vj + 1.;
                    id = id + v.diode(m.isr * area, m.nr * vt) * (u * u + 0.005).powf(0.5 * m.m);
                }
                if let Some(b) = avalanche.filter(|b| v.value() < 0. && v.value() <= b.knee) {
                    let z = -(v + m.bv) / (m.nbv * vt);
                    id = if z.value() > 80. {
                        (z - 79.) * 80f64.exp()
                    } else {
                        z.exp()
                    } * (-b.at_bv);
                }
                if m.ikf > 0. && id.value() > 0. {
                    id = id / ((id / (m.ikf * area)).powf(0.5) + 1.);
                }
                let q = id * m.tt
                    + v.depletion(m.cjo * area, m.vj, m.m, m.fc)
                    + v.depletion(m.cjp * m.perimeter, m.php, m.mjsw, m.fcs);
                ([id, -id], [q, -q])
            });
        }
        Ok(())
    }
    fn output_charges(&self, i: usize, p: &str) -> Vec<(usize, f64)> {
        if matches!(p, "id" | "capcur") {
            vec![(self.at[i].q, 1.)]
        } else {
            Vec::new()
        }
    }
    fn instance_ic(&self, i: usize, ic: &[f64], _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [v] = ic {
            edges.push((self.ai[i], self.k[i], *v));
        }
    }
    fn noise_components(&self) -> &'static [&'static str] {
        &["rs", "id", "1overf"]
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        _x: &[f64],
        slots: &Slots,
        out: &mut Vec<crate::NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for i in 0..self.len() {
            let m = &self.model[i];
            let temp = m.temperature.unwrap_or(ctx.temp);
            let thermal = 4. * crate::BOLTZMANN * temp;
            let mut at = self.at[i];
            let mut emit = |label, p, n, power, exp| {
                let mut source = crate::NoiseSource::branch(&self.names[i], p, n, power, exp);
                source.component = Some(label);
                out.push(source);
            };
            let (p, n) = (self.ai[i], self.k[i]);
            if m.rs > 0. {
                emit("rs", self.a[i], p, thermal * self.area[i] / m.rs, 0.);
                at.f += 2;
            }
            // ngspice 44.2 evaluates shot and flicker noise from the junction
            // current including its parallel gmin branch. At low bias the
            // measured power ratio is (I + gmin V) / I (R22g probes).
            let id = slots.f[at.f].abs();
            emit("id", p, n, 2. * crate::CHARGE * id, 0.);
            emit("1overf", p, n, m.kf * id.powf(m.af), 1.);
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
                self.ai[i] = Some(u.alloc_private_node(&self.names[i], "a"));
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
            let vk = volt(x, self.k[i]);
            let va = match out.initial_bias(i) {
                [v] => vk + v,
                _ => volt(x, self.ai[i]),
            };
            let nvt = m.n * vt;
            let is = m.is * area;
            let avalanche =
                m.bv.is_finite()
                    .then(|| self.breakdown[i].unwrap_or_else(|| m.breakdown(area, vt)));
            let vc = vcrit(is, nvt);
            let vd_act = va - vk;
            let vd = if ctx.init_junctions {
                if self.off[i] { 0.0 } else { vc }
            } else if ctx.limit {
                let old = st.current[i];
                let (mut v, mut lim) = limit_junction(vd_act, old, nvt, vc);
                let breakdown_vt = m.nbv * vt;
                if let Some(b) = avalanche.filter(|_| v < (-m.bv + 10. * breakdown_vt).min(0.)) {
                    let (vr, limited) = limit_junction(
                        -(v + m.bv),
                        -(old + m.bv),
                        breakdown_vt,
                        vcrit(b.at_bv, breakdown_vt),
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
            let generation = if m.isr > 0. && vd >= -3. * nvt {
                let width = ((1. - v / m.vj).sq() + 0.005).powf(m.m * 0.5);
                ideal_current(v, m.isr * area, m.nr * vt) * width
            } else {
                Dual::cst(0.)
            };
            let mut id = diffusion + generation;
            if let Some(b) = avalanche.filter(|b| vd < 0. && vd <= b.knee) {
                id = -b.at_bv * crate::junction::safe_exp(-(v + m.bv) / (m.nbv * vt));
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
            if let Some(values) = out.outputs.get_mut(i) {
                values.extend([
                    ("id", id.v),
                    ("gd", id.d[0]),
                    ("cd", q.d[0]),
                    ("capcur", 0.),
                    ("charge", q.v),
                ]);
            }
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
    fn tcv_shifts_the_fixed_current_breakdown_voltage() {
        let terminal_voltage = |celsius: f64, tcv: f64| {
            let (mut m, ignored) = DiodeModel::from_params(&ModelParams::new(&[
                ("tcv".into(), tcv),
                ("bv".into(), 6.2),
                ("is".into(), 2e-12),
                ("rs".into(), 3.),
                ("trs1".into(), 0.004),
                ("trs2".into(), 2e-6),
            ]));
            assert!(ignored.is_empty());
            let kelvin = celsius + 273.15;
            m.at_temperature(crate::temperature::Temperature {
                kelvin,
                nominal: 293.15,
            });
            let vt = thermal_voltage(kelvin);
            let current = 1e-3;
            // Invert the avalanche exponential at the probe's 1 mA reverse bias.
            let junction = -m.bv - m.nbv * vt * (current / m.breakdown(1., vt).at_bv).ln();
            junction - current * m.rs
        };
        let shift = |t| terminal_voltage(t, 0.0007) - terminal_voltage(t, 0.);
        // Subtraction cancels the common offset in the two 44.2 .op readings.
        let measured: f64 = -6.148482324 - (-6.204485200);
        assert!((shift(100.) - measured).abs() < 1e-6 * measured.abs());
        assert_eq!(shift(20.), 0.);
        // At -20 °C saturation current is negligible, so the shift is TCV × (-40 K).
        assert!((shift(-20.) + 0.028).abs() < 1e-10);
    }

    #[test]
    fn series_resistance_adds_internal_node() {
        let (_, l, u) = setup(10.0);
        assert_eq!(u.names(), ["v(a)", "v(d1#a)"]);
        assert_eq!(l.g.len(), 4 + 4);
    }
}
