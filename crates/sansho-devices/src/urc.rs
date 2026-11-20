//! Uniform distributed RC line with geometrically graded lumped sections.
//! Provenance: ngspice user manual, Uniform Distributed RC Lines (U);
//! Nagel, SPICE2 (1975), lumped networks and junction companion models.

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, vcrit};
use crate::temperature::Temperature;
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns, thermal_voltage, volt,
};
use sansho_diag::Diagnostic;

#[derive(Debug)]
pub struct Urc {
    name: String,
    ends: [Node; 3],
    taps: Vec<Node>,
    junctions: Vec<Node>,
    resistances: Vec<f64>,
    shunts: Vec<f64>,
    capacitance: f64,
    saturation: f64,
    diode_resistance: f64,
    kelvin: f64,
    potential: f64,
}

impl Urc {
    pub fn new(
        name: &str,
        ends: [Node; 3],
        p: &ModelParams,
        inst: &ModelParams,
        temp: Temperature,
    ) -> Result<Self, Diagnostic> {
        let k = p.get("k", 1.5);
        let fmax = p.get("fmax", 1e9);
        let length = inst.get("l", 0.);
        let r = length * p.get("rperl", 1000.);
        let c = length * p.get("cperl", 1e-12);
        let is = length * p.get("isperl", 0.);
        let rs = length * p.get("rsperl", 0.);
        let error = || {
            Diagnostic::error(
                "E-LINE-PARAM",
                format!("{name}: invalid URC length, lump count or model parameter"),
            )
        };
        if ![length, r, c, fmax, k]
            .iter()
            .all(|v| v.is_finite() && *v > 0.)
            || k <= 1.
            || ![is, rs].iter().all(|v| v.is_finite() && *v >= 0.)
        {
            return Err(error());
        }
        let bandwidth = std::f64::consts::TAU * fmax * r * c;
        let estimate = ((bandwidth * (1. - 1. / k).powi(2)).ln() / k.ln())
            .floor()
            .max(3.);
        let count = inst.get("n", estimate);
        if !(1. ..=4096.).contains(&count) || count.fract() != 0. {
            return Err(error());
        }
        let half = count as usize;
        // Normalize by the largest section before summing, avoiding K^N overflow.
        let weights = |n: usize| -> Vec<f64> {
            let mut values: Vec<_> = (0..n)
                .map(|i| k.powi(i.min(n - 1 - i) as i32 - (half - 1) as i32))
                .collect();
            let sum: f64 = values.iter().sum();
            for v in &mut values {
                *v /= sum;
            }
            values
        };
        let resistances: Vec<_> = weights(half * 2).into_iter().map(|w| w * r).collect();
        let shunts = weights(half * 2 - 1);
        if resistances.iter().any(|r| !r.recip().is_finite()) || shunts.contains(&0.) {
            return Err(error());
        }
        // Each shunt diode has relative area w / w_end. The end diode
        // has access resistance L * RSPERL * N.
        let diode_resistance = rs * (count * shunts[0]);
        Ok(Self {
            name: name.into(),
            ends,
            taps: vec![],
            junctions: vec![],
            resistances,
            shunts,
            capacitance: if is > 0. {
                c * temp.capacitance(1., 0.5)
            } else {
                c
            },
            saturation: is * temp.saturation(1.11, 3.).exp(),
            diode_resistance,
            kelvin: temp.kelvin,
            potential: temp.potential(1.),
        })
    }
    fn branches(&self) -> impl Iterator<Item = (Node, Node, f64)> + '_ {
        self.taps
            .windows(2)
            .zip(&self.resistances)
            .map(|(ends, &r)| (ends[0], ends[1], r))
            .chain(
                self.junctions
                    .iter()
                    .enumerate()
                    .filter(|_| self.saturation > 0. && self.diode_resistance > 0.)
                    .map(|(i, &n)| (self.taps[i + 1], n, self.diode_resistance / self.shunts[i])),
            )
    }
}
impl DeviceGroup for Urc {
    fn len(&self) -> usize {
        1
    }
    fn type_name(&self) -> &'static str {
        "urc"
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        self.saturation == 0.
    }
    fn state_len(&self) -> usize {
        if self.saturation > 0. {
            self.shunts.len()
        } else {
            0
        }
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        self.taps = vec![self.ends[0]];
        self.junctions.clear();
        for i in 0..self.shunts.len() {
            let tap = Some(u.alloc_internal_node(&self.name, &format!("tap{i}")));
            self.taps.push(tap);
            self.junctions
                .push(if self.diode_resistance > 0. && self.saturation > 0. {
                    Some(u.alloc_internal_node(&self.name, &format!("j{i}")))
                } else {
                    tap
                });
        }
        self.taps.push(self.ends[1]);
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (p, n, _) in self.branches() {
            b.resistor(p, n);
        }
        for &n in &self.junctions {
            b.charge_core(&[n, self.ends[2]]);
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let mut at = SlotOffsets::default();
        for (p, n, r) in self.branches() {
            out.resistor(&mut at, 1. / r, volt(x, p), volt(x, n));
        }
        let first = out.f[0];
        let second = out.f[2 * self.resistances.len() - 1];
        if let Some(o) = out.outputs.first_mut() {
            o.extend([("i1", first), ("i2", second)]);
        }
        let vt = thermal_voltage(self.kelvin);
        let sub = volt(x, self.ends[2]);
        for (i, (&n, &w)) in self.junctions.iter().zip(&self.shunts).enumerate() {
            let bias = [volt(x, n), sub];
            let mut v = bias[0] - sub;
            let is = self.saturation * w;
            if let Some(old) = st.current.get_mut(i) {
                if ctx.limit {
                    let (lim, changed) = limit_junction(v, *old, vt, vcrit(is, vt));
                    v = lim;
                    out.noncon |= changed;
                }
                *old = v;
            }
            let voltage = Dual { v, d: [1., -1.] };
            let (current, q) = if self.saturation > 0. {
                (
                    ideal_current(voltage, is, vt) + voltage * ctx.gmin,
                    depletion_charge(voltage, self.capacitance * w, self.potential, 0.5, 0.5),
                )
            } else {
                (Dual::cst(0.), voltage * (self.capacitance * w))
            };
            out.core(
                &mut at,
                &[current, -current],
                &[q, -q],
                &bias,
                &[bias[1] + v, bias[1]],
            );
        }
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        slots: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        let mut at = 0;
        for (p, n, r) in self.branches() {
            out.push(NoiseSource::branch(
                &self.name,
                p,
                n,
                4. * crate::BOLTZMANN * self.kelvin / r,
                0.,
            ));
            at += 2;
        }
        if self.saturation > 0. {
            for &j in &self.junctions {
                let p = self.ends[2];
                let i = slots.f[at] - ctx.gmin * (volt(x, j) - volt(x, p));
                out.push(NoiseSource::branch(
                    &self.name,
                    j,
                    p,
                    2. * crate::CHARGE * i.abs(),
                    0.,
                ));
                at += 2;
            }
        }
        Ok(())
    }
}
