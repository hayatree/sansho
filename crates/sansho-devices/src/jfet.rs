//! Level-one JFET and MESFET channel, gate junctions and terminal charges.
//! Provenance: Massobrio & Antognetti, Semiconductor Device Modeling with SPICE,
//! JFET model; H. Statz et al., IEEE TED 34 (1987), 160–169 (MESFET
//! current and gate charge); ngspice user manual, JFET and MESFET sections.

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, vcrit};
use crate::temperature::Temperature;
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns, thermal_voltage, volt,
};
use sansho_diag::Diagnostic;

#[derive(Debug)]
pub struct Jfet {
    name: String,
    ext: [Node; 3],
    int: [Node; 3],
    polarity: f64,
    off: bool,
    kelvin: f64,
    beta: f64,
    vto: f64,
    lambda: f64,
    b: f64,
    doping_slope: f64,
    mes_alpha: Option<f64>,
    rd: f64,
    rs: f64,
    is: f64,
    cgs: f64,
    cgd: f64,
    pb: f64,
    fc: f64,
    kf: f64,
    af: f64,
    copies: f64,
}

impl Jfet {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        nodes: [Node; 3],
        p: &ModelParams,
        polarity: f64,
        t: Temperature,
        area: f64,
        copies: f64,
        off: bool,
    ) -> Result<Self, Diagnostic> {
        Self::build(name, nodes, p, polarity, t, area, copies, off, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mesfet(
        name: &str,
        nodes: [Node; 3],
        p: &ModelParams,
        polarity: f64,
        t: Temperature,
        area: f64,
        copies: f64,
        off: bool,
    ) -> Result<Self, Diagnostic> {
        let alpha = p.get("alpha", 2.);
        if !alpha.is_finite() || alpha <= 0. {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                format!("{name}: MESFET ALPHA must be positive"),
            ));
        }
        Self::build(name, nodes, p, polarity, t, area, copies, off, Some(alpha))
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        name: &str,
        nodes: [Node; 3],
        p: &ModelParams,
        polarity: f64,
        t: Temperature,
        area: f64,
        copies: f64,
        off: bool,
        mes_alpha: Option<f64>,
    ) -> Result<Self, Diagnostic> {
        let scale = area * copies;
        let pb = p.get("pb", 1.);
        let vto = p.get("vto", -2.);
        let mes = mes_alpha.is_some();
        let b = p.get("b", if mes { 0.3 } else { 1. });
        let delta = t.kelvin - t.nominal;
        let capacitance = if mes {
            scale
        } else {
            t.capacitance(pb, 0.5) * scale
        };
        let model = Self {
            name: name.into(),
            ext: nodes,
            int: nodes,
            off,
            polarity,
            kelvin: t.kelvin,
            beta: p.get("beta", if mes { 2.5e-3 } else { 1e-4 })
                * scale
                * if mes {
                    1.
                } else {
                    t.ratio().powf(p.get("bex", 0.))
                },
            vto: if mes {
                vto
            } else {
                vto - p.get("vtotc", 0.) * delta
            },
            lambda: p.get("lambda", 0.),
            b,
            mes_alpha,
            doping_slope: if mes || b == 1. {
                0.
            } else {
                (1. - b) / (pb - vto)
            },
            rd: p.get("rd", 0.) / scale,
            rs: p.get("rs", 0.) / scale,
            is: p.get("is", 1e-14)
                * scale
                * if mes {
                    1.
                } else {
                    t.saturation(1.11, 3.).exp()
                },
            cgs: p.get("cgs", 0.) * capacitance,
            cgd: p.get("cgd", 0.) * capacitance,
            pb: if mes { pb } else { t.potential(pb) },
            fc: p.get("fc", 0.5),
            kf: p.get("kf", 0.),
            af: p.get("af", 1.),
            copies,
        };
        if ![area, copies, model.pb]
            .iter()
            .all(|v| v.is_finite() && *v > 0.)
            || ![
                model.is, model.beta, model.rd, model.rs, model.cgs, model.cgd, model.kf,
            ]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.)
            || ![
                model.vto,
                model.lambda,
                model.b,
                model.doping_slope,
                model.af,
            ]
            .iter()
            .all(|v| v.is_finite())
            || (mes && b < 0.)
            || !(0. ..1.).contains(&model.fc)
        {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                format!("{name}: invalid junction FET geometry, junction or channel parameters"),
            ));
        }
        Ok(model)
    }

    fn channel<const N: usize>(&self, gs: Dual<N>, gd: Dual<N>) -> Dual<N> {
        if let Some(alpha) = self.mes_alpha {
            let (drive, ds, sign) = if gs.v >= gd.v {
                (gs, gs - gd, 1.)
            } else {
                (gd, gd - gs, -1.)
            };
            let u = drive - self.vto;
            if u.v <= 0. {
                return Dual::cst(0.);
            }
            let z = ds * (alpha / 3.);
            let knee = if z.v < 1. {
                z * (3. - z * (3. - z))
            } else {
                Dual::cst(1.)
            };
            return sign * self.beta * u.sq() / (1. + self.b * u) * knee * (1. + self.lambda * ds);
        }
        // Integrate the channel charge between the two ends. Pinch-off
        // clamps the overdrive of either end to zero.
        let pinch = |v: Dual<N>| {
            let u = v - self.vto;
            if u.v <= 0. {
                Dual::cst(0.)
            } else {
                u.sq() * (u * self.doping_slope + self.b)
            }
        };
        let ds = gs - gd;
        let magnitude = if ds.v < 0. { -ds } else { ds };
        (pinch(gs) - pinch(gd)) * self.beta * (magnitude * self.lambda + 1.)
    }
    /// Statz effective gate potential interpolates continuously between the
    /// two ends of the channel. Split the gate charge by averaging the two
    /// rectangular integration paths from the last accepted bias.
    fn charges<const N: usize>(
        &self,
        gs: Dual<N>,
        gd: Dual<N>,
        accepted: &[f64],
    ) -> (Dual<N>, Dual<N>) {
        let dep = |v, c| depletion_charge(v, c, self.pb, 0.5, self.fc);
        let Some(alpha) = self.mes_alpha else {
            return (dep(gs, self.cgs), dep(gd, self.cgd));
        };
        let gate = |s: Dual<N>, d: Dual<N>| {
            let width = ((s - d).sq() + alpha.recip().powi(2)).sqrt();
            let high = (s + d + width) * 0.5;
            let low = (s + d - width) * 0.5;
            let active = (high + self.vto + ((high - self.vto).sq() + 0.04).sqrt()) * 0.5;
            dep(active, self.cgs) + low * self.cgd
        };
        let last_gs = Dual::cst(accepted[0]);
        let last_gd = Dual::cst(accepted[1]);
        let start = gate(last_gs, last_gd);
        let end = gate(gs, gd);
        let across_source = gate(gs, last_gd);
        let across_drain = gate(last_gs, gd);
        let dqs = ((across_source - start) + (end - across_drain)) * 0.5;
        let dqd = ((across_drain - start) + (end - across_source)) * 0.5;
        (dqs + accepted[2], dqd + accepted[3])
    }

    fn series(&self) -> [(usize, f64); 2] {
        [(0, self.rd), (2, self.rs)]
    }
}

impl DeviceGroup for Jfet {
    fn output_charges(&self, _: usize, p: &str) -> Vec<(usize, f64)> {
        match p {
            "id" => vec![(0, 1.)],
            "ig" => vec![(1, 1.)],
            "is" => vec![(2, 1.)],
            _ => Vec::new(),
        }
    }
    fn instance_ic(&self, _: usize, ic: &[f64], _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [ds, gs] = ic {
            let [d, g, s] = self.int;
            edges.extend([(d, s, *ds), (g, s, *gs)]);
        }
    }
    fn type_name(&self) -> &'static str {
        if self.mes_alpha.is_some() {
            "mesfet"
        } else {
            "jfet"
        }
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn len(&self) -> usize {
        1
    }
    fn state_len(&self) -> usize {
        if self.mes_alpha.is_some() { 4 } else { 2 }
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (k, r) in self.series() {
            if r > 0. {
                self.int[k] =
                    Some(u.alloc_internal_node(&self.name, if k == 0 { "d" } else { "s" }));
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (k, r) in self.series() {
            if r > 0. {
                b.resistor(self.ext[k], self.int[k]);
            }
        }
        b.charge_core(&self.int);
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let mut at = SlotOffsets::default();
        for (k, r) in self.series() {
            if r > 0. {
                out.resistor(&mut at, 1. / r, volt(x, self.ext[k]), volt(x, self.int[k]));
            }
        }
        let mut actual = self.int.map(|n| volt(x, n));
        if let [ds, gs] = out.initial_bias(0) {
            actual[0] = actual[2] + ds;
            actual[1] = actual[2] + gs;
        }
        let [d, g, s] = actual;
        let sign = self.polarity;
        let vt = thermal_voltage(self.kelvin);
        let vc = vcrit(self.is, vt);
        let mut junctions = [sign * (g - s), sign * (g - d)];
        for (v, old) in junctions.iter_mut().zip(st.current.iter_mut()) {
            if ctx.init_junctions {
                *v = if self.off { 0. } else { self.vto * 0.5 };
            } else if ctx.limit {
                let (limited, changed) = limit_junction(*v, *old, vt, vc);
                *v = limited;
                out.noncon |= changed;
            }
            *old = *v;
        }
        let lin = [g - sign * junctions[1], g, g - sign * junctions[0]];
        let [d, g, s] = [0, 1, 2].map(|i| Dual::<3>::var(lin[i], i));
        let gs = (g - s) * sign;
        let gd = (g - d) * sign;
        let ids = self.channel(gs, gd) * sign;
        let junction = |v| (ideal_current(v, self.is, vt) + v * ctx.gmin) * sign;
        let (igs, igd) = (junction(gs), junction(gd));
        let (qgs, qgd) = self.charges(gs, gd, st.accepted);
        if self.mes_alpha.is_some() {
            st.current[2..4].copy_from_slice(&[qgs.v, qgd.v]);
        }
        let (qgs, qgd) = (qgs * sign, qgd * sign);
        if let Some(values) = out.outputs.get_mut(0) {
            crate::observables::three_terminal_fet(
                values,
                ids,
                [ids.v - igd.v, igs.v + igd.v, -ids.v - igs.v],
            );
            if self.mes_alpha.is_some() {
                values.extend([("qgs", qgs.v), ("qgd", qgd.v), ("vgs", gs.v), ("vgd", gd.v)]);
            }
        }

        out.core(
            &mut at,
            &[ids - igd, igs + igd, -ids - igs],
            &[-qgd, qgs + qgd, -qgs],
            &actual,
            &lin,
        );
    }
    fn noise(
        &self,
        _: &EvalCtx,
        x: &[f64],
        _: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        let thermal = 4. * crate::BOLTZMANN * self.kelvin;
        for (k, r) in self.series() {
            if r > 0. {
                out.push(NoiseSource::branch(
                    &self.name,
                    self.ext[k],
                    self.int[k],
                    thermal / r,
                    0.,
                ));
            }
        }
        let [d, g, s] = self.int.map(|n| volt(x, n));
        let gs = Dual::<1>::var(self.polarity * (g - s), 0);
        let gd = Dual::<1>::var(self.polarity * (g - d), 0);
        let current = self.channel(gs, gd);
        for (power, exponent) in [
            (thermal * (2. / 3.) * current.d[0].abs(), 0.),
            (
                self.kf * self.copies * (current.v.abs() / self.copies).powf(self.af),
                1.,
            ),
        ] {
            out.push(NoiseSource::branch(
                &self.name,
                self.int[0],
                self.int[2],
                power,
                exponent,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mirrored_channel_and_nonuniform_doping() {
        let p = ModelParams::new(&[("b".into(), 0.7)]);
        let t = Temperature {
            nominal: 300.15,
            kelvin: 300.15,
        };
        let m = Jfet::new("j", [None; 3], &p, 1., t, 2., 1., false).unwrap();
        let at = |v| Dual::<1>::var(v, 0);
        let forward = m.channel(at(-1.), at(-3.));
        assert!((forward.v - 2e-4 * 0.8).abs() < 1e-18);
        assert!((forward.d[0] - 2e-4 * 1.7).abs() < 1e-18);
        assert_eq!(forward, -m.channel(at(-3.), at(-1.)));
        let hot = Jfet::new(
            "j",
            [None; 3],
            &p,
            1.,
            Temperature { kelvin: 340., ..t },
            2.,
            1.,
            false,
        )
        .unwrap();
        assert!(hot.is > m.is && hot.pb < m.pb);
    }
}
