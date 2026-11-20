//! Bipolar junction transistor (Gummel–Poon model, SPICE level 1).
//!
//! Terminals: collector, base, emitter and optional substrate. Positive series resistances
//! add internal nodes. RBM and IRB describe base spreading at operating bias.
//!
//! Static model (normalized to NPN; PNP flips all voltages and currents):
//! - `Ibf = IS(exp(Vbe/(NF·Vt)) − 1)`, `Ibr = IS(exp(Vbc/(NR·Vt)) − 1)`
//! - leakage `Ile = ISE(exp(Vbe/(NE·Vt)) − 1)`, `Ilc = ISC(exp(Vbc/(NC·Vt)) − 1)`
//! - base charge `qb = q1/2 · (1 + √(1 + 4·q2))`, `q1 = 1/(1 − Vbc/VAF − Vbe/VAR)`,
//!   `q2 = Ibf/IKF + Ibr/IKR`
//! - `Ic = (Ibf − Ibr)/qb − Ibr/BR − Ilc`, `Ib = Ibf/BF + Ile + Ibr/BR + Ilc`
//!
//! Charges: `Qbe = TFeff·Ibf/qb + Qdep(CJE)`, `Qbc = TR·Ibr + Qdep(CJC)`, `Qcs = Qdep(CJS)`, with
//! `TFeff = TF·(1 + XTF·(Ibf/(Ibf+ITF))²·exp(Vbc/(1.44·VTF)))`.
//!
//! XCJC partitions collector depletion charge between the internal and external base.
//! Forward transport with excess phase uses the Weil–McNamee (1978) delay approximation
//! in transient analysis and the exponential phase factor in frequency-domain analyses.
//!
//! Provenance: original implementation of the model equations published in H. K. Gummel &
//! H. C. Poon, "An integral charge control model of bipolar transistors", Bell Syst. Tech. J.
//! 49 (1970), and I. Getreu, *Modeling the Bipolar Transistor* (Tektronix, 1976), in the
//! parameterization of Antognetti & Massobrio (1993), ch. 2. The epitaxial
//! collector branch follows Kull et al., IEEE TED 32 (1985). See `docs/PROVENANCE.md`.

use sansho_diag::Diagnostic;

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, vcrit};
use crate::model::ModelParams;
use crate::{
    DeviceGroup, EvalCtx, Node, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns,
    thermal_voltage, volt,
};

/// Gummel–Poon model parameters.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub struct BjtModel {
    /// Prepared operating temperature in kelvin.
    pub temperature: Option<f64>,
    pub kf: f64,
    pub af: f64,
    pub tnom: Option<f64>,
    pub eg: f64,
    pub xti: f64,
    pub xtb: f64,

    /// +1 for NPN, −1 for PNP.
    pub polarity: f64,
    pub is: f64,
    pub bf: f64,
    pub nf: f64,
    pub vaf: f64,
    pub ikf: f64,
    pub nkf: f64,
    pub iss: f64,
    pub subs: f64,
    pub ns: f64,
    pub ise: f64,
    pub ne: f64,
    pub br: f64,
    pub nr: f64,
    pub var: f64,
    pub ikr: f64,
    pub isc: f64,
    pub nc: f64,
    pub rb: f64,
    pub rbm: f64,
    pub irb: f64,
    pub xcjc: f64,
    pub ptf: f64,
    pub re: f64,
    pub rc: f64,
    pub rco: f64,
    pub vo: f64,
    pub gamma: f64,
    pub qco: f64,
    /// Ratio of the base–collector junction area to the emitter area.
    pub bc_area_ratio: f64,
    pub cje: f64,
    pub vje: f64,
    pub mje: f64,
    pub tf: f64,
    pub xtf: f64,
    pub vtf: f64,
    pub itf: f64,
    pub cjc: f64,
    pub vjc: f64,
    pub mjc: f64,
    pub tr: f64,
    pub cjs: f64,
    pub vjs: f64,
    pub mjs: f64,
    pub fc: f64,
}

impl BjtModel {
    /// Vertical devices locate the collector junction under the base.
    /// In the lateral geometry the base is the substrate-facing region.
    pub fn junction_areas(&mut self, emitter: f64, base: f64, collector: f64) {
        let (bc, sub) = if self.subs > 0. {
            (base, collector)
        } else {
            (collector, base)
        };
        self.bc_area_ratio = bc / emitter;
        self.cjc *= self.bc_area_ratio;
        self.isc *= self.bc_area_ratio;
        self.cjs *= sub / emitter;
        self.iss *= sub / emitter;
    }

    pub fn at_temperature(&mut self, t: crate::temperature::Temperature) {
        self.temperature = Some(t.kelvin);
        if t.kelvin == t.nominal {
            return;
        }
        let log_is = t.saturation(self.eg, self.xti);
        let beta = t.ratio().powf(self.xtb);
        self.is *= log_is.exp();
        self.iss *= (log_is / self.ns).exp();
        self.bf *= beta;
        self.br *= beta;
        self.ise *= (log_is / self.ne).exp() / beta;
        self.isc *= (log_is / self.nc).exp() / beta;
        for (v, c, m) in [
            (&mut self.vje, &mut self.cje, self.mje),
            (&mut self.vjc, &mut self.cjc, self.mjc),
            (&mut self.vjs, &mut self.cjs, self.mjs),
        ] {
            *c *= t.capacitance(*v, m);
            *v = t.potential(*v);
        }
    }
    /// Build from `.model` parameters (`polarity` from the model type). Returns the model and
    /// the names of ignored parameters.
    pub fn from_params(p: &ModelParams, polarity: f64) -> (Self, Vec<String>) {
        let inf = f64::INFINITY;
        let nz = |v: f64| if v == 0.0 { inf } else { v };
        let m = Self {
            temperature: None,
            kf: p.get("kf", 0.),
            af: p.get("af", 1.),
            tnom: p.opt(&["tnom"]),
            eg: p.get("eg", 1.11),
            xti: p.get("xti", 3.0),
            xtb: p.get("xtb", 0.0),

            polarity,
            is: p.get("is", 1e-16),
            bf: p.get("bf", 100.0),
            nf: p.get("nf", 1.0),
            vaf: nz(p.opt(&["vaf", "va"]).unwrap_or(inf)),
            iss: p.get("iss", 0.),
            subs: p.get("subs", 1.),
            ns: p.get("ns", 1.),
            nkf: p.get("nkf", 0.5),
            ikf: nz(p.opt(&["ikf", "ik"]).unwrap_or(inf)),
            ise: p.get("ise", p.get("is", 1e-16) * p.get("c2", 0.)),
            ne: p.get("ne", 1.5),
            br: p.get("br", 1.0),
            nr: p.get("nr", 1.0),
            var: nz(p.opt(&["var", "vb"]).unwrap_or(inf)),
            ikr: nz(p.get("ikr", inf)),
            isc: p.get("isc", p.get("is", 1e-16) * p.get("c4", 0.)),
            nc: p.get("nc", 2.0),
            rb: p.get("rb", 0.0),
            rbm: p.get("rbm", p.get("rb", 0.)),
            irb: p.get("irb", 0.),
            xcjc: p.get("xcjc", 1.),
            ptf: p.get("ptf", 0.),
            re: p.get("re", 0.0),
            rc: p.get("rc", 0.0),
            rco: p.get("rco", 0.),
            vo: p.get("vo", 10.),
            gamma: p.get("gamma", 1e-11),
            qco: p.get("qco", 0.),
            bc_area_ratio: 1.,
            cje: p.get("cje", 0.0),
            vje: p.opt(&["vje", "pe"]).unwrap_or(0.75),
            mje: p.opt(&["mje", "me"]).unwrap_or(0.33),
            tf: p.get("tf", 0.0),
            xtf: p.get("xtf", 0.0),
            vtf: nz(p.get("vtf", inf)),
            itf: p.get("itf", 0.0),
            cjc: p.get("cjc", 0.0),
            vjc: p.opt(&["vjc", "pc"]).unwrap_or(0.75),
            mjc: p.opt(&["mjc", "mc"]).unwrap_or(0.33),
            tr: p.get("tr", 0.0),
            cjs: p.opt(&["cjs", "ccs"]).unwrap_or(0.0),
            vjs: p.opt(&["vjs", "ps"]).unwrap_or(0.75),
            mjs: p.opt(&["mjs", "ms"]).unwrap_or(0.0),
            fc: p.get("fc", 0.5),
        };
        p.accept(&["level", "kf", "af"]);
        (m, p.unused())
    }
}

/// All BJT instances.
#[derive(Debug, Default)]
pub struct Bjts {
    names: Vec<String>,
    /// External c, b, e, s.
    ext: Vec<[Node; 4]>,
    /// Internal c', b', e' (equal to external when the resistance is zero), s.
    int: Vec<[Node; 4]>,
    area: Vec<f64>,
    collector_contact: Vec<Node>,
    off: Vec<bool>,
    model: Vec<BjtModel>,
    at: Vec<SlotOffsets>,
    phase_index: Vec<Option<usize>>,
    phase_count: usize,
}

impl Bjts {
    /// Add an instance with terminals `[c, b, e, s]`.
    pub fn push(&mut self, name: &str, nodes: [Node; 4], model: BjtModel, area: f64, off: bool) {
        self.names.push(name.to_string());
        self.ext.push(nodes);
        self.int.push(nodes);
        self.area.push(area);
        self.collector_contact.push(nodes[0]);
        self.off.push(off);
        self.phase_index
            .push((model.ptf > 0. && model.tf > 0.).then_some(self.phase_count));
        self.phase_count += usize::from(model.ptf > 0. && model.tf > 0.);
        self.model.push(model);
    }
}

/// Kull et al., IEEE TED 32 (1985), epitaxial-region conductivity modulation.
/// Integrate the neutral-region conductivity between the two collector
/// potentials. Subtract equilibrium charge at each end of the region.
fn epitaxial<const N: usize>(
    m: &BjtModel,
    area: f64,
    vt: f64,
    cx: Dual<N>,
    ci: Dual<N>,
    base: Dual<N>,
) -> (Dual<N>, Dual<N>, Dual<N>) {
    let carriers = |v| (1. + m.gamma * crate::junction::safe_exp(v / vt)).sqrt();
    let ki = carriers((base - ci) * m.polarity);
    let kx = carriers((base - cx) * m.polarity);
    let density = ki - kx;
    let modulation = density - (density / (1. + kx)).ln_1p();
    let drop = cx - ci;
    // Drift velocity depends on the electric field across the epi layer.
    let magnitude = if drop.v < 0. { -drop } else { drop };
    let current = (drop + m.polarity * vt * modulation) / (1. + magnitude / m.vo);
    (
        current * (area / m.rco),
        m.polarity * m.qco * area * (ki - 1.),
        m.polarity * m.qco * area * (kx - 1.),
    )
}

const SUFFIX: [&str; 3] = ["c", "b", "e"];

// Distributed base contact current crowding (Getreu). Rationalizing the
// angle keeps it accurate at small positive base currents.
fn base_resistance(m: &BjtModel, ib: f64, qb: f64) -> f64 {
    if m.rb == m.rbm {
        return m.rb;
    }
    let fraction = if m.irb > 0. {
        let ratio = ib.max(0.) / m.irb;
        let root = ratio.sqrt();
        let angle = if root > 1. {
            (14.59025 / 2.4317) / (14.59025_f64.sqrt().hypot(1. / root) + 1. / root)
        } else {
            (14.59025 / 2.4317) * root / ((1. + 14.59025 * ratio).sqrt() + 1.)
        };
        if angle < 1e-3 {
            1. - 4. * angle.powi(2) / 15.
        } else {
            let cot = angle.tan().recip();
            (3. * cot * (angle.recip() - cot)).max(f64::EPSILON)
        }
    } else {
        if qb <= 0. { 1. } else { qb.recip() }
    };
    m.rbm + (m.rb - m.rbm) * fraction
}

// Time, last step, filtered current, previous filtered current and three
// derivatives of forward transport current for the exact AC phase factor.
const PHASE_STATE: usize = 7;

impl DeviceGroup for Bjts {
    fn distortion(
        &self,
        ctx: &EvalCtx,
        bias: &[f64],
        out: &mut Vec<crate::distortion::Term>,
    ) -> Result<(), Diagnostic> {
        use crate::distortion::{Jet, expand};
        for (i, m) in self.model.iter().enumerate() {
            if m.rco > 0. || self.phase_index[i].is_some() || (m.rb > 0. && m.rbm != m.rb) {
                return Err(Diagnostic::error(
                    "E-ANALYSIS",
                    format!(
                        "{}: DISTO derivatives unavailable for quasi-saturation, excess phase or variable base resistance",
                        self.names[i]
                    ),
                ));
            }
            let area = self.area[i];
            let vt = thermal_voltage(m.temperature.unwrap_or(ctx.temp));
            let pol = m.polarity;
            expand(self.int[i], bias, out, |[c, b, e, s]| {
                let be = (b - e) * pol;
                let bc = (b - c) * pol;
                let target = usize::from(m.subs < 0.);
                let sc = (s - if target == 0 { c } else { b }) * (pol * m.subs);
                let f = be.diode(m.is * area, m.nf * vt);
                let r = bc.diode(m.is * area * m.bc_area_ratio, m.nr * vt);
                let high = (f / (m.ikf * area) + r / (m.ikr * area)) * 4. + 1.;
                let rolloff = if high.value() > 0. {
                    high.powf(m.nkf)
                } else {
                    Jet::default()
                };
                let iqb = (-bc / m.vaf - be / m.var + 1.) * 2. / (rolloff + 1.);
                let bf = f / m.bf + be.diode(m.ise * area, m.ne * vt);
                let br = r / m.br + bc.diode(m.isc * area, m.nc * vt);
                let ic = ((f - r) * iqb - br) * pol;
                let ib = (bf + br) * pol;
                let mut tf = Jet::constant(m.tf);
                if f.value() > 0. && m.tf > 0. && m.xtf != 0. {
                    let ratio = if m.itf > 0. {
                        f / (f + m.itf * area)
                    } else {
                        Jet::constant(1.)
                    };
                    tf = (ratio * ratio * (bc / (1.44 * m.vtf)).exp() * m.xtf + 1.) * m.tf;
                }
                let qbe = (tf * f * iqb + be.depletion(m.cje * area, m.vje, m.mje, m.fc)) * pol;
                let qbc =
                    (r * m.tr + bc.depletion(m.xcjc * m.cjc * area, m.vjc, m.mjc, m.fc)) * pol;
                let is = sc.diode(m.iss * area, m.ns * vt) * (pol * m.subs);
                let qs = sc.depletion(m.cjs * area, m.vjs, m.mjs, 0.) * (pol * m.subs);
                let mut currents = [ic, ib, -ic - ib, is];
                let mut charges = [-qbc, qbc + qbe, -qbe, qs];
                currents[target] = currents[target] - is;
                charges[target] = charges[target] - qs;
                (currents, charges)
            });
            if m.xcjc != 1. && m.cjc != 0. {
                expand([self.ext[i][1], self.int[i][0]], bias, out, |[b, c]| {
                    let q =
                        ((b - c) * pol).depletion((1. - m.xcjc) * m.cjc * area, m.vjc, m.mjc, m.fc)
                            * pol;
                    ([Jet::default(); 2], [q, -q])
                });
            }
        }
        Ok(())
    }
    fn output_charges(&self, i: usize, p: &str) -> Vec<(usize, f64)> {
        let row = match p {
            "ic" => 0,
            "ib" => 1,
            "ie" => 2,
            "is" => 3,
            _ => return Vec::new(),
        };
        let first = self.at[i].q;
        let mut terms = vec![(first + row, 1.)];
        if row < 2 && self.model[i].xcjc != 1. && self.model[i].cjc != 0. {
            terms.push((first + 4 + usize::from(row == 0), 1.));
        }
        if self.model[i].rco > 0. {
            let split = usize::from(self.model[i].xcjc != 1. && self.model[i].cjc != 0.);
            let epi = first + 4 + 2 * split;
            if row == 0 {
                terms.extend([(epi, 1.), (epi + 1, 1.)]);
            }
            if row == 1 {
                terms.push((epi + 2, 1.));
            }
        }
        terms
    }
    fn instance_ic(&self, i: usize, ic: &[f64], _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [be, ce] = ic {
            let [c, b, e, _] = self.int[i];
            edges.extend([(b, e, *be), (c, e, *ce)]);
        }
    }
    fn transport_delays(&self, state: &[f64], emit: &mut dyn FnMut(f64, Node, Node, f64)) {
        for (i, index) in self.phase_index.iter().enumerate() {
            if let Some(index) = index {
                let tau = self.model[i].tf * self.model[i].ptf.to_radians();
                let start = 2 * self.len() + PHASE_STATE * index + 4;
                for (&col, &grad) in self.int[i].iter().zip(&state[start..start + 3]) {
                    emit(tau, self.int[i][0], col, grad);
                    emit(tau, self.int[i][2], col, -grad);
                }
            }
        }
    }
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
            let [c, b, e, sub] = self.int[i];
            for (k, r) in [m.rc, m.rb, m.re].into_iter().enumerate() {
                if r > 0. {
                    emit(
                        self.ext[i][k],
                        if k == 0 {
                            self.collector_contact[i]
                        } else {
                            self.int[i][k]
                        },
                        thermal * slots.g[at.g],
                        0.,
                    );
                    at.f += 2;
                    at.g += 4;
                }
            }
            let bc = volt(x, b) - volt(x, c);
            let be = volt(x, b) - volt(x, e);
            let substrate = slots.f[at.f + 3];
            if m.iss > 0. {
                emit(
                    sub,
                    if m.subs > 0. { c } else { b },
                    2. * crate::CHARGE * substrate.abs(),
                    0.,
                );
            }
            let mut intrinsic = [
                slots.f[at.f] + ctx.gmin * bc,
                slots.f[at.f + 1] - ctx.gmin * (be + bc),
            ];
            intrinsic[usize::from(m.subs < 0.)] += substrate;
            let [ic, ib] = intrinsic.map(f64::abs);
            emit(c, e, 2. * crate::CHARGE * ic, 0.);
            emit(b, e, 2. * crate::CHARGE * ib, 0.);
            emit(b, e, m.kf * ib.powf(m.af), 1.);
        }
        Ok(())
    }
    fn instance_name(&self, index: usize) -> Option<&str> {
        self.names.get(index).map(String::as_str)
    }
    fn type_name(&self) -> &'static str {
        "bjt"
    }
    fn len(&self) -> usize {
        self.names.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        2 * self.len() + PHASE_STATE * self.phase_count
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for i in 0..self.len() {
            let m = &self.model[i];
            for (k, r) in [m.rc, m.rb, m.re].into_iter().enumerate() {
                if r > 0.0 {
                    self.int[i][k] = Some(u.alloc_internal_node(&self.names[i], SUFFIX[k]));
                }
            }
            self.collector_contact[i] = self.int[i][0];
            if m.rco > 0. {
                self.int[i][0] = Some(u.alloc_internal_node(&self.names[i], "ci"));
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.at.clear();
        for i in 0..self.len() {
            self.at.push(b.counts());
            let m = &self.model[i];
            for (k, r) in [m.rc, m.rb, m.re].into_iter().enumerate() {
                if r > 0.0 {
                    b.resistor(
                        self.ext[i][k],
                        if k == 0 {
                            self.collector_contact[i]
                        } else {
                            self.int[i][k]
                        },
                    );
                }
            }
            b.charge_core(&self.int[i]);
            if m.xcjc != 1. && m.cjc != 0. {
                b.charge_core(&[self.ext[i][1], self.int[i][0]]);
            }
            if m.rco > 0. {
                b.charge_core(&[self.collector_contact[i], self.int[i][0], self.int[i][1]]);
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let m = &self.model[i];
            let vt = thermal_voltage(m.temperature.unwrap_or(ctx.temp));
            let area = self.area[i];
            let mut at = self.at[i];
            let mut act = self.int[i].map(|n| volt(x, n));
            if let [be, ce] = out.initial_bias(i) {
                act[0] = act[2] + ce;
                act[1] = act[2] + be;
            }
            let [vc, vb, ve, vs] = act;
            let sgn = m.polarity;
            let is = m.is * area;
            let is_bc = is * m.bc_area_ratio;
            let (nf_vt, nr_vt) = (m.nf * vt, m.nr * vt);
            let vbe_act = sgn * (vb - ve);
            let vbc_act = sgn * (vb - vc);
            let (vbe, vbc) = if ctx.init_junctions {
                if self.off[i] {
                    (0.0, 0.0)
                } else {
                    (vcrit(is, nf_vt), 0.0)
                }
            } else if ctx.limit {
                let (a, la) = limit_junction(vbe_act, st.current[2 * i], nf_vt, vcrit(is, nf_vt));
                let (b, lb) =
                    limit_junction(vbc_act, st.current[2 * i + 1], nr_vt, vcrit(is_bc, nr_vt));
                if la || lb {
                    out.noncon = true;
                }
                (a, b)
            } else {
                (vbe_act, vbc_act)
            };
            st.current[2 * i] = vbe;
            st.current[2 * i + 1] = vbc;
            // Linearization point in terminal voltages consistent with the limited junctions.
            let lb_ = ve + sgn * vbe;
            let lin = [lb_ - sgn * vbc, lb_, ve, vs];
            let [dc, db, de, ds] = [0, 1, 2, 3].map(|k| Dual::<4>::var(lin[k], k));
            let vbe = (db - de) * sgn;
            let vbc = (db - dc) * sgn;
            let substrate_node = usize::from(m.subs < 0.);
            let sub_sign = sgn * m.subs;
            let vsc = (ds - if substrate_node == 0 { dc } else { db }) * sub_sign;

            let ibf = ideal_current(vbe, is, nf_vt);
            let ibr = ideal_current(vbc, is_bc, nr_vt);
            let ile = ideal_current(vbe, m.ise * area, m.ne * vt);
            let ilc = ideal_current(vbc, m.isc * area, m.nc * vt);
            let q1 = 1.0 / (1.0 - vbc / m.vaf - vbe / m.var);
            let q2 = ibf / (m.ikf * area) + ibr / (m.ikr * area);
            let high = q2 * 4. + 1.;
            let rolloff = if high.v <= 0. {
                Dual::cst(0.)
            } else if m.nkf == 0.5 {
                high.sqrt()
            } else {
                high.powf(m.nkf)
            };
            let qb = q1 * 0.5 * (rolloff + 1.);
            let forward = ibf / qb;
            let delayed = if let Some(index) = self.phase_index[i] {
                let k = 2 * self.len() + PHASE_STATE * index;
                let previous = &st.accepted[k..k + PHASE_STATE];
                let current = &mut st.current[k..k + PHASE_STATE];
                let step = ctx.time - previous[0];
                let tau = m.tf * m.ptf.to_radians();
                let filtered = if ctx.mode == crate::Mode::Tran && step > 0. {
                    // Weil–McNamee second-order approximation to transport delay.
                    // Backward differences use only accepted step history.
                    let dt0 = if previous[1] > 0. { previous[1] } else { step };
                    let first = tau / step;
                    let second = (2. / 3.) * first * tau / (dt0 + step);
                    (forward
                        + previous[2] * (first + second)
                        + second * (step / dt0) * (previous[2] - previous[3]))
                        / (1. + first + second)
                } else if ctx.mode == crate::Mode::Tran && ctx.time > 0. {
                    Dual::cst(previous[2])
                } else {
                    forward
                };
                current[..4].copy_from_slice(&[
                    ctx.time,
                    step,
                    filtered.v,
                    if ctx.time > 0. {
                        previous[2]
                    } else {
                        filtered.v
                    },
                ]);
                for (dst, grad) in current[4..].iter_mut().zip(forward.d) {
                    *dst = sgn * grad;
                }
                filtered
            } else {
                forward
            };
            let ict = delayed - ibr / qb;
            let ic = ict - ibr / m.br - ilc - vbc * ctx.gmin;
            let ib = ibf / m.bf + ile + ibr / m.br + ilc + (vbe + vbc) * ctx.gmin;

            let rb = base_resistance(m, ib.v / area, qb.v);
            for (k, r) in [m.rc, rb, m.re].into_iter().enumerate() {
                if r > 0.0 {
                    out.resistor(
                        &mut at,
                        area / r,
                        volt(x, self.ext[i][k]),
                        volt(
                            x,
                            if k == 0 {
                                self.collector_contact[i]
                            } else {
                                self.int[i][k]
                            },
                        ),
                    );
                }
            }
            let tf_eff = if ibf.v > 0. && m.tf > 0.0 && m.xtf != 0.0 {
                let ratio = if m.itf > 0.0 {
                    ibf / (ibf + m.itf * area)
                } else {
                    Dual::cst(1.0)
                };
                (ratio.sq() * (vbc / (1.44 * m.vtf)).exp() * m.xtf + 1.0) * m.tf
            } else {
                Dual::cst(m.tf)
            };
            let qbe = tf_eff * ibf / qb + depletion_charge(vbe, m.cje * area, m.vje, m.mje, m.fc);
            let qbc = ibr * m.tr + depletion_charge(vbc, m.xcjc * m.cjc * area, m.vjc, m.mjc, m.fc);
            let qcs = depletion_charge(vsc, m.cjs * area, m.vjs, m.mjs, 0.0);

            let substrate = ideal_current(vsc, m.iss * area, m.ns * vt) * sub_sign;
            let sub_charge = qcs * sub_sign;
            let mut i_t = [ic * sgn, ib * sgn, -(ic + ib) * sgn, substrate];
            i_t[substrate_node] = i_t[substrate_node] - substrate;
            if let Some(values) = out.outputs.get_mut(i) {
                values.extend([
                    ("ic", i_t[0].v),
                    ("ib", i_t[1].v),
                    ("ie", -(ic.v + ib.v) * sgn),
                    ("is", substrate.v),
                    ("gm", (ict.d[1]) * sgn),
                    ("go", ict.d[0] * sgn),
                    ("gpi", -ib.d[2] * sgn),
                    ("gmu", -ib.d[0] * sgn),
                    ("cpi", -qbe.d[2] * sgn),
                    ("cmu", -qbc.d[0] * sgn),
                ]);
            }
            let mut q_t = [-qbc * sgn, (qbe + qbc) * sgn, -qbe * sgn, sub_charge];
            q_t[substrate_node] = q_t[substrate_node] - sub_charge;
            out.core(&mut at, &i_t, &q_t, &act, &lin);
            if m.xcjc != 1. && m.cjc != 0. {
                let bias = [volt(x, self.ext[i][1]), vc];
                let v = (Dual::<2>::var(bias[0], 0) - Dual::var(vc, 1)) * sgn;
                let q = depletion_charge(v, (1. - m.xcjc) * m.cjc * area, m.vjc, m.mjc, m.fc) * sgn;
                out.core(&mut at, &[Dual::cst(0.); 2], &[q, -q], &bias, &bias);
            }
            if m.rco > 0. {
                let cx = volt(x, self.collector_contact[i]);
                let epi_lin = [cx, lin[0], lin[1]];
                let [xnode, inode, base] = [0, 1, 2].map(|k| Dual::var(epi_lin[k], k));
                let (current, qi, qx) = epitaxial(m, area, vt, xnode, inode, base);
                out.core(
                    &mut at,
                    &[current, -current, Dual::cst(0.)],
                    &[-qx, -qi, qi + qx],
                    &[cx, vc, vb],
                    &epi_lin,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_spreading_resistance_half_current_and_charge() {
        let mut m = BjtModel::from_params(&ModelParams::default(), 1.).0;
        m.rb = 100.;
        m.rbm = 20.;
        m.irb = 1e-5;
        assert_eq!(base_resistance(&m, 0., 1.), 100.);
        // The published crowding-angle approximation reaches roughly half
        // the variable resistance at IRB. At pi/4 its tangent is exactly one.
        assert!((base_resistance(&m, m.irb, 1.) - 60.).abs() < 0.5);
        let a = std::f64::consts::FRAC_PI_4;
        let z = 2.4317 * a;
        let ratio = (2. * z / (14.59025 - z * z)).powi(2);
        assert!(
            (base_resistance(&m, ratio * m.irb, 1.) - (20. + 80. * 3. * (1. / a - 1.))).abs()
                < 1e-12
        );
        m.irb = 0.;
        assert_eq!(base_resistance(&m, 0., 4.), 40.);
    }

    #[test]
    fn forward_active_current_gain() {
        let (model, ignored) = BjtModel::from_params(&ModelParams::default(), 1.0);
        assert!(ignored.is_empty());
        let mut q = Bjts::default();
        q.push("q1", [Some(0), Some(1), None, None], model, 1.0, false);
        let u = Unknowns::with_nodes(["c", "b"]);
        let mut b = StampBuilder::new(&u);
        q.setup(&mut b).unwrap();
        let l = b.finish();
        let mut s = Slots::for_layout(&l);
        let ctx = EvalCtx {
            limit: false,
            gmin: 0.0,
            ..EvalCtx::dc()
        };
        let mut cur = [0.0; 2];
        q.eval(
            &ctx,
            &[2.0, 0.7],
            StateRef {
                accepted: &[0.0; 2],
                current: &mut cur,
            },
            &mut s,
        );
        let (ic, ib) = (s.f[0], s.f[1]);
        assert!(
            ic > 0.0 && (ic / ib - 100.0).abs() < 1e-6,
            "ic={ic} ib={ib}"
        );
    }
}
