//! Three-terminal dispersive junction FET with package and relaxation states.
//! Provenance: Parker and Skellern, IEEE T-MTT 45 (1997), ngspice 44
//! manual §7.4.5. The interpolations below are clean-room working choices.

use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns,
    dual::Dual,
    junction::{depletion_charge, ideal_current, limit_junction, safe_exp, vcrit},
    temperature::Temperature,
    thermal_voltage, volt,
};
use sansho_diag::Diagnostic;

#[derive(Debug)]
pub struct Jfet2 {
    name: String,
    ext: [Node; 3],
    int: [Node; 6],
    r: [f64; 3],
    span: f64,
    sign: f64,
    off: bool,
    p: f64,
    q: f64,
    beta: f64,
    threshold: f64,
    vst: f64,
    mvst: f64,
    xi: f64,
    z: f64,
    low: [f64; 3],
    high: [f64; 3],
    eta: [f64; 3],
    tau_g: f64,
    tau_d: f64,
    delta: f64,
    caps: [f64; 2],
    acgam: f64,
    xc: f64,
    fc: f64,
    vbi: f64,
    is: f64,
    ibd: f64,
    vbd: f64,
    nvt: f64,
    kelvin: f64,
    copies: f64,
    kf: f64,
    af: f64,
}

impl Jfet2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        ext: [Node; 3],
        model: &ModelParams,
        sign: f64,
        t: Temperature,
        area: f64,
        copies: f64,
        off: bool,
    ) -> Result<Self, Diagnostic> {
        let get = |k, default| model.get(k, default);
        let scale = area * copies;
        let n = get("n", 1.);
        let vbi = get("vbi", 1.);
        // Manual §7.4.1/5. Original geometry convention: junction and channel
        // quantities add in parallel; lead impedances divide by total scale.
        let dev = Self {
            name: name.into(),
            ext,
            int: [ext[0], ext[1], ext[2], None, None, None],
            sign,
            off,
            copies,
            p: get("p", 2.),
            q: get("q", 2.),
            beta: get("beta", 1e-4) * scale,
            threshold: get("vto", -2.),
            vst: get("vst", 0.),
            mvst: get("mvst", 0.),
            xi: get("xi", 1000.),
            z: get("z", 1.),
            low: ["lfgam", "lfg1", "lfg2"].map(|k| get(k, 0.)),
            high: ["hfgam", "hfg1", "hfg2"].map(|k| get(k, 0.)),
            eta: ["hfeta", "hfe1", "hfe2"].map(|k| get(k, 0.)),
            tau_g: get("taug", 0.),
            tau_d: get("taud", 0.),
            delta: get("delta", 0.) / scale,
            r: [get("rd", 0.) / scale, 0., get("rs", 0.) / scale],
            span: vbi - get("vto", -2.),
            acgam: get("acgam", 0.),
            xc: get("xc", 0.),
            caps: [
                get("cgs", 0.) * t.capacitance(vbi, 0.5),
                get("cgd", 0.) * t.capacitance(vbi, 0.5),
            ]
            .map(|c| c * scale),
            fc: get("fc", 0.5),
            vbi: t.potential(vbi),
            is: get("is", 1e-14) * scale * (t.saturation(1.11, 0.) / n).exp(),
            ibd: get("ibd", 0.) * scale,
            vbd: get("vbd", 1.),
            nvt: n * thermal_voltage(t.kelvin),
            kelvin: t.kelvin,
            kf: get("kf", 0.),
            af: get("af", 1.),
        };
        if ![
            area, copies, dev.span, dev.nvt, dev.p, dev.q, dev.xi, dev.z, dev.vbi, dev.vbd,
        ]
        .into_iter()
        .all(|v| v.is_finite() && v > 0.)
            || !dev
                .r
                .iter()
                .chain(&dev.caps)
                .chain([
                    &dev.is, &dev.ibd, &dev.kf, &dev.beta, &dev.tau_g, &dev.tau_d, &dev.delta,
                    &dev.vst,
                ])
                .all(|v| v.is_finite() && *v >= 0.)
            || !dev
                .low
                .iter()
                .chain(&dev.high)
                .chain(&dev.eta)
                .chain([&dev.threshold, &dev.mvst, &dev.af])
                .all(|v| v.is_finite())
            || !dev.acgam.is_finite()
            || !(0. ..=1.).contains(&dev.xc)
            || !(0. ..1.).contains(&dev.fc)
        {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                format!(
                    "{name}: invalid JFET2 geometry, junction, channel or relaxation parameter"
                ),
            ));
        }
        Ok(dev)
    }

    fn channel<const N: usize>(
        &self,
        gs: Dual<N>,
        gd: Dual<N>,
        slow_s: Dual<N>,
        slow_d: Dual<N>,
    ) -> Dual<N> {
        let ds = gs - gd;
        let (v, w, slow, far, d, sign) = if ds.v >= 0. {
            (gs, gd, slow_s, slow_d, ds, 1.)
        } else {
            (gd, gs, slow_d, slow_s, -ds, -1.)
        };
        let u = {
            let low = self.low[0] - self.low[1] * slow + self.low[2] * far;
            let high = self.high[0] - self.high[1] * v + self.high[2] * w;
            let eta = -self.eta[0] + self.eta[1] * w - self.eta[2] * v;
            let u = v - self.threshold - low * far + high * (far - w) + eta * (v - slow);
            let scale = self.vst * (1. + self.mvst * d);
            if scale.v > 0. {
                if u.v > 0. {
                    u + scale * (-u / scale).exp().ln_1p()
                } else {
                    scale * (u / scale).exp().ln_1p()
                }
            } else if u.v > 0. {
                u
            } else {
                Dual::cst(0.)
            }
        };
        if u.v <= f64::MIN_POSITIVE {
            return Dual::cst(0.);
        }
        // Invert the measured knee relation using ratios no larger than one.
        let linear = (self.q / self.p) * u * (u / self.span).powf(self.q - self.p);
        let knee = 1. / (1. + u / (self.xi * self.span));
        let limit = linear * knee;
        if limit.v <= f64::MIN_POSITIVE {
            return Dual::cst(0.);
        }
        let (ratio, scale) = if d.v < limit.v {
            (d / limit, d / linear)
        } else {
            (limit / d, knee)
        };
        let r2 = ratio.sq();
        let radical = ((1. - r2).sq() + (4. * self.z / (1. + self.z)) * r2).sqrt();
        let drop = scale * (2. / (1. + r2 + radical)).sqrt();
        let current = if drop.v < 1. {
            -((-drop).ln_1p() * self.q).exp_m1()
        } else {
            Dual::cst(1.)
        };
        sign * self.beta * u.powf(self.q) * current
    }

    fn steady<const N: usize>(&self, cold: Dual<N>, ds: Dual<N>) -> Dual<N> {
        cold / (1. + self.delta * cold * ds)
    }

    fn junction<const N: usize>(&self, v: Dual<N>, gmin: f64) -> Dual<N> {
        ideal_current(v, self.is, self.nvt) + gmin * v - self.ibd * (safe_exp(-v / self.vbd) - 1.)
    }
}

impl DeviceGroup for Jfet2 {
    fn len(&self) -> usize {
        1
    }
    fn state_len(&self) -> usize {
        4
    }
    fn type_name(&self) -> &'static str {
        "jfet2"
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn instance_ic(&self, _: usize, ic: &[f64], _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        if let [ds, gs] = ic {
            edges.extend([
                (self.int[0], self.int[2], *ds),
                (self.int[1], self.int[2], *gs),
            ]);
        }
    }
    fn output_charges(&self, _: usize, p: &str) -> Vec<(usize, f64)> {
        match p {
            "id" => vec![(0, 1.)],
            "ig" => vec![(1, 1.)],
            "is" => vec![(2, 1.)],
            _ => Vec::new(),
        }
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (k, key) in ["d", "g", "s"].into_iter().enumerate() {
            if self.r[k] > 0. {
                self.int[k] = Some(u.alloc_internal_node(&self.name, key));
            }
        }
        for (k, key) in [(3, "gs"), (4, "gd"), (5, "heat")] {
            if (k < 5 && self.tau_g > 0.) || (k == 5 && self.tau_d > 0. && self.delta > 0.) {
                self.int[k] = Some(u.alloc_internal_node(&self.name, key));
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        b.charge_core(&self.int);
        for k in 0..3 {
            if self.r[k] > 0. {
                b.resistor(self.ext[k], self.int[k]);
            }
        }
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let mut act = self.int.map(|n| volt(x, n));
        if let [ds, gs] = out.initial_bias(0) {
            act[0] = act[2] + ds;
            act[1] = act[2] + gs;
        }
        let mut lin = act;
        for (k, prev) in [2, 0].into_iter().zip(st.current.iter_mut()) {
            let v = (act[1] - act[k]) * self.sign;
            let v = if ctx.init_junctions {
                if self.off { 0. } else { 0.5 * self.threshold }
            } else if ctx.limit {
                let (limited, flag) = limit_junction(v, *prev, self.nvt, vcrit(self.is, self.nvt));
                out.noncon |= flag;
                limited
            } else {
                v
            };
            *prev = v;
            lin[k] = act[1] - self.sign * v;
        }
        let [d, g, s, slow_s, slow_d, heat] = std::array::from_fn(|i| Dual::<6>::var(lin[i], i));
        let gs = (g - s) * self.sign;
        let gd = (g - d) * self.sign;
        let (ls, ld) = if self.tau_g > 0. {
            (slow_s, slow_d)
        } else {
            (gs, gd)
        };
        let cold = self.channel(gs, gd, ls, ld);
        // DELTA sweeps measure feedback proportional to cold channel power.
        let hot = if self.int[5].is_some() {
            cold / (1. + if heat.v > -0.5 { heat } else { Dual::cst(-0.5) })
        } else {
            self.steady(cold, gs - gd)
        };
        let id = self.sign * hot;
        let igs = self.sign * self.junction(gs, ctx.gmin);
        let igd = self.sign * self.junction(gd, ctx.gmin);
        // Integrate the measured gate capacitance along the two rectangular
        // paths from the accepted biases, as for the Statz gate charge.
        let gate = |s: Dual<6>, d: Dual<6>| {
            let ds = s - d;
            let knee = 0.5 * self.span / (1. + 1. / self.xi);
            let width = (ds.sq() + knee * knee).sqrt();
            let modulation = self.acgam * if ds.v < 0. { -ds } else { ds };
            let high = (s + d + width) * 0.5 + modulation;
            let low = (s + d - width) * 0.5 + modulation;
            // As in the Statz charge primitive, a 0.2 V rounded cutoff
            // precedes depletion. The FC sweeps resolve both its value and slope.
            let pinch = (self.xc - 1.) * (high - self.threshold);
            let active = high + 0.5 * (pinch + (pinch.sq() + 0.04).sqrt());
            depletion_charge(active, self.caps[0], self.vbi, 0.5, self.fc) + self.caps[1] * low
        };
        let old_s = Dual::cst(st.accepted[0]);
        let old_d = Dual::cst(st.accepted[1]);
        let change = gate(gs, gd) - gate(old_s, old_d);
        let cross = gate(gs, old_d) - gate(old_s, gd);
        let qgs = st.accepted[2] + (change + cross) * 0.5;
        let qgd = st.accepted[3] + (change - cross) * 0.5;
        st.current[2..4].copy_from_slice(&[qgs.v, qgd.v]);
        let (qgs, qgd) = (qgs * self.sign, qgd * self.sign);
        let currents = [
            id - igd,
            igs + igd,
            -id - igs,
            ls - gs,
            ld - gd,
            heat - self.delta * cold * (gs - gd),
        ];
        let charges = [
            -qgd,
            qgs + qgd,
            -qgs,
            self.tau_g * ls,
            self.tau_g * ld,
            self.tau_d * heat,
        ];
        let mut at = SlotOffsets::default();
        out.core(&mut at, &currents, &charges, &act, &lin);
        if let Some(values) = out.outputs.first_mut() {
            let relaxed = self.sign * self.steady(self.channel(gs, gd, gs, gd), gs - gd);
            values.extend([
                ("id", currents[0].v),
                ("ig", currents[1].v),
                ("is", currents[2].v),
                ("gm", relaxed.d[1]),
                ("gds", relaxed.d[0]),
                ("cgs", -(qgs + qgd).d[2]),
                ("cgd", -(qgs + qgd).d[0]),
                ("qgs", qgs.v),
                ("qgd", qgd.v),
                ("vgs", gs.v),
                ("vgd", gd.v),
            ]);
        }
        for k in 0..3 {
            let a = volt(x, self.ext[k]);
            let b = volt(x, self.int[k]);
            if self.r[k] > 0. {
                out.resistor(&mut at, 1. / self.r[k], a, b);
            }
        }
    }
    fn noise(
        &self,
        _: &EvalCtx,
        x: &[f64],
        _: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        let kt4 = 4. * crate::BOLTZMANN * self.kelvin;
        for k in 0..3 {
            let r = self.r[k];
            if r > 0. {
                out.push(NoiseSource::branch(
                    &self.name,
                    self.ext[k],
                    self.int[k],
                    kt4 / r,
                    0.,
                ));
            }
        }
        let [d, g, s, ..] = self.int.map(|n| volt(x, n));
        let gs = Dual::<1>::var(self.sign * (g - s), 0);
        let gd = Dual::var(self.sign * (g - d), 0);
        let cold = self.channel(gs, gd, gs, gd);
        let id = self.steady(cold, gs - gd);
        // Inherit the Sansho junction FET channel and flicker noise convention.
        for (p, e) in [
            (kt4 * 2. / 3. * id.d[0].abs(), 0.),
            (
                self.kf * self.copies * (id.v.abs() / self.copies).powf(self.af),
                1.,
            ),
        ] {
            out.push(NoiseSource::branch(
                &self.name,
                self.int[0],
                self.int[2],
                p,
                e,
            ));
        }
        Ok(())
    }
}
