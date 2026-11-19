//! Bipolar junction transistor (Gummel–Poon model, SPICE level 1).
//!
//! Terminals: collector, base, emitter, optional substrate (default ground). Constant series
//! resistances `RC`, `RB`, `RE` add internal nodes.
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
//! Not modelled yet (reported as ignored parameters): current-dependent base resistance
//! (`IRB`, `RBM`), excess phase (`PTF`) and split base–collector capacitance (`XCJC` < 1).
//!
//! Provenance: original implementation of the model equations published in H. K. Gummel &
//! H. C. Poon, "An integral charge control model of bipolar transistors", Bell Syst. Tech. J.
//! 49 (1970), and I. Getreu, *Modeling the Bipolar Transistor* (Tektronix, 1976), in the
//! parameterization of Antognetti & Massobrio (1993), ch. 2. See `docs/PROVENANCE.md`.

use spx_diag::Diagnostic;

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
    pub ise: f64,
    pub ne: f64,
    pub br: f64,
    pub nr: f64,
    pub var: f64,
    pub ikr: f64,
    pub isc: f64,
    pub nc: f64,
    pub rb: f64,
    pub re: f64,
    pub rc: f64,
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
    pub fn at_temperature(&mut self, t: crate::temperature::Temperature) {
        self.temperature = Some(t.kelvin);
        if t.kelvin == t.nominal {
            return;
        }
        let log_is = t.saturation(self.eg, self.xti);
        let beta = t.ratio().powf(self.xtb);
        self.is *= log_is.exp();
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
            ikf: nz(p.opt(&["ikf", "ik"]).unwrap_or(inf)),
            ise: p.get("ise", 0.0),
            ne: p.get("ne", 1.5),
            br: p.get("br", 1.0),
            nr: p.get("nr", 1.0),
            var: nz(p.opt(&["var", "vb"]).unwrap_or(inf)),
            ikr: nz(p.get("ikr", inf)),
            isc: p.get("isc", 0.0),
            nc: p.get("nc", 2.0),
            rb: p.get("rb", 0.0),
            re: p.get("re", 0.0),
            rc: p.get("rc", 0.0),
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
        let mut ignored = p.unused();
        if p.opt(&["xcjc"]).is_some_and(|v| v != 1.0) {
            ignored.push("xcjc".into());
        }
        (m, ignored)
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
    off: Vec<bool>,
    model: Vec<BjtModel>,
    at: Vec<SlotOffsets>,
}

impl Bjts {
    /// Add an instance with terminals `[c, b, e, s]`.
    pub fn push(&mut self, name: &str, nodes: [Node; 4], model: BjtModel, area: f64, off: bool) {
        self.names.push(name.to_string());
        self.ext.push(nodes);
        self.int.push(nodes);
        self.area.push(area);
        self.off.push(off);
        self.model.push(model);
    }
}

const SUFFIX: [&str; 3] = ["c", "b", "e"];

impl DeviceGroup for Bjts {
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
            let [c, b, e, _] = self.int[i];
            for (k, r) in [m.rc, m.rb, m.re].into_iter().enumerate() {
                if r > 0. {
                    emit(
                        self.ext[i][k],
                        self.int[i][k],
                        thermal * self.area[i] / r,
                        0.,
                    );
                    at.f += 2;
                }
            }
            let bc = volt(x, b) - volt(x, c);
            let be = volt(x, b) - volt(x, e);
            let ic = (slots.f[at.f] + ctx.gmin * bc).abs();
            let ib = (slots.f[at.f + 1] - ctx.gmin * (be + bc)).abs();
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
        2 * self.len()
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for i in 0..self.len() {
            let m = &self.model[i];
            for (k, r) in [m.rc, m.rb, m.re].into_iter().enumerate() {
                if r > 0.0 {
                    self.int[i][k] = Some(u.alloc_internal_node(&self.names[i], SUFFIX[k]));
                }
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
                    b.resistor(self.ext[i][k], self.int[i][k]);
                }
            }
            b.charge_core(&self.int[i]);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        for i in 0..self.len() {
            let m = &self.model[i];
            let vt = thermal_voltage(m.temperature.unwrap_or(ctx.temp));
            let area = self.area[i];
            let mut at = self.at[i];
            for (k, r) in [m.rc, m.rb, m.re].into_iter().enumerate() {
                if r > 0.0 {
                    out.resistor(
                        &mut at,
                        area / r,
                        volt(x, self.ext[i][k]),
                        volt(x, self.int[i][k]),
                    );
                }
            }
            let act = self.int[i].map(|n| volt(x, n));
            let [vc, vb, ve, vs] = act;
            let sgn = m.polarity;
            let is = m.is * area;
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
                    limit_junction(vbc_act, st.current[2 * i + 1], nr_vt, vcrit(is, nr_vt));
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
            let vsc = (ds - dc) * sgn;

            let ibf = ideal_current(vbe, is, nf_vt);
            let ibr = ideal_current(vbc, is, nr_vt);
            let ile = ideal_current(vbe, m.ise * area, m.ne * vt);
            let ilc = ideal_current(vbc, m.isc * area, m.nc * vt);
            let q1 = 1.0 / (1.0 - vbc / m.vaf - vbe / m.var);
            let q2 = ibf / (m.ikf * area) + ibr / (m.ikr * area);
            let qb = q1 * 0.5 * ((q2 * 4.0 + 1.0).sqrt() + 1.0);
            let ict = (ibf - ibr) / qb;
            let ic = ict - ibr / m.br - ilc - vbc * ctx.gmin;
            let ib = ibf / m.bf + ile + ibr / m.br + ilc + (vbe + vbc) * ctx.gmin;

            let tf_eff = if m.tf > 0.0 && m.xtf > 0.0 {
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
            let qbc = ibr * m.tr + depletion_charge(vbc, m.cjc * area, m.vjc, m.mjc, m.fc);
            let qcs = depletion_charge(vsc, m.cjs * area, m.vjs, m.mjs, 0.0);

            let zero = Dual::cst(0.0);
            let i_t = [ic * sgn, ib * sgn, -(ic + ib) * sgn, zero];
            let q_t = [-(qbc + qcs) * sgn, (qbe + qbc) * sgn, -qbe * sgn, qcs * sgn];
            out.core(&mut at, &i_t, &q_t, &act, &lin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
