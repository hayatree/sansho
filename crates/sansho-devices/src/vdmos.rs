//! Electrical model of a vertical power MOSFET.
//! Provenance: ngspice user manual, VDMOS section (channel, Miller capacitance
//! and body diode); Shichman and Hodges (1968), square-law channel.

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, safe_exp, vcrit};
use crate::temperature::Temperature;
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns, thermal_voltage, volt,
};
use sansho_diag::Diagnostic;

#[derive(Debug)]
pub struct Vdmos {
    name: String,
    ext: [Node; 3],
    int: [Node; 3],
    body: Node,
    series: [f64; 4],
    sign: f64,
    temp: f64,
    kp: f64,
    threshold: f64,
    lambda: f64,
    theta: f64,
    mtriode: f64,
    cgs: f64,
    cmin: f64,
    cmax: f64,
    a: f64,
    saturation: f64,
    nvt: f64,
    cjo: f64,
    vj: f64,
    grading: f64,
    fc: f64,
    tt: f64,
    bv: f64,
    ibv: f64,
    copies: f64,
    kf: f64,
    af: f64,
    off: bool,
}

impl Vdmos {
    pub fn new(
        name: &str,
        nodes: [Node; 3],
        p: &ModelParams,
        inst: &ModelParams,
        t: Temperature,
        off: bool,
    ) -> Result<Self, Diagnostic> {
        let sign = if p.get("pchan", 0.) != 0. { -1. } else { 1. };
        let nchan = p.get("nchan", 0.);
        let copies = inst.opt(&["m", "mult"]).unwrap_or(1.);
        let grading = p.get("m", 0.5);
        let vj = p.get("vj", 0.8);
        let ideality = p.get("n", 1.);
        let model = Self {
            name: name.into(),
            ext: nodes,
            int: nodes,
            body: nodes[2],
            series: ["rd", "rg", "rs", "rb"].map(|key| p.get(key, 0.) / copies),
            sign,
            temp: t.kelvin,
            kp: p.get("kp", 1.) * copies * t.ratio().powf(p.get("mu", -1.5)),
            threshold: sign * p.get("vto", 0.) - p.get("tcvth", 0.) * (t.kelvin - t.nominal),
            lambda: p.get("lambda", 0.),
            theta: p.get("theta", 0.),
            mtriode: p.get("mtriode", 1.),
            cgs: p.get("cgs", 0.) * copies,
            cmin: p.get("cgdmin", 20e-12) * copies,
            cmax: p.get("cgdmax", 2e-9) * copies,
            a: p.get("a", 1.),
            saturation: p.get("is", 1e-14)
                * copies
                * (t.saturation(p.get("eg", 1.11), p.get("xti", 3.)) / ideality).exp(),
            nvt: ideality * thermal_voltage(t.kelvin),
            cjo: p.get("cjo", 500e-12) * copies * t.capacitance(vj, grading),
            vj: t.potential(vj),
            grading,
            fc: p.get("fc", 0.5),
            tt: p.get("tt", 0.),
            bv: p.get("bv", f64::INFINITY),
            ibv: p.get("ibv", 1e-3) * copies,
            copies,
            kf: p.get("kf", 0.),
            af: p.get("af", 1.),
            off,
        };
        if sign < 0. && nchan != 0.
            || ![copies, model.nvt, model.vj, model.mtriode, model.a]
                .iter()
                .all(|x| x.is_finite() && *x > 0.)
            || !model
                .series
                .iter()
                .chain([
                    &model.kp,
                    &model.cgs,
                    &model.cmin,
                    &model.cmax,
                    &model.saturation,
                    &model.cjo,
                    &model.tt,
                    &model.ibv,
                    &model.theta,
                    &model.kf,
                ])
                .all(|x| x.is_finite() && *x >= 0.)
            || model.cmin > model.cmax
            || !(0. ..1.).contains(&model.fc)
            || model.bv <= 0.
            || !model.threshold.is_finite()
        {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                format!(
                    "{name}: invalid VDMOS polarity, channel, junction, or capacitance parameter"
                ),
            ));
        }
        Ok(model)
    }

    fn channel<const N: usize>(&self, gs: Dual<N>, ds: Dual<N>) -> Dual<N> {
        let (drive, drop, sign) = if ds.v >= 0. {
            (gs, ds, 1.)
        } else {
            (gs - ds, -ds, -1.)
        };
        // Smooth turn-on. The default 0.1 V transition is visible in the
        // reference DC sweeps even half a volt above threshold.
        let overdrive = drive - self.threshold;
        let u = if overdrive.v >= 0. {
            overdrive + 0.1 * (-10. * overdrive).exp().ln_1p()
        } else {
            0.1 * (10. * overdrive).exp().ln_1p()
        };
        let pinch = drop * self.mtriode;
        let v = if pinch.v < u.v { pinch } else { u };
        self.kp * sign * v * (u - v * 0.5) * (1. + self.lambda * drop)
            / (1. + self.theta * if drive.v > 0. { drive } else { Dual::cst(0.) })
    }

    /// Integral of the joined atan/tanh Miller-capacitance curve, Q(0)=0.
    fn miller<const N: usize>(&self, v: Dual<N>) -> Dual<N> {
        use std::f64::consts::{FRAC_PI_2, LN_2};
        let x = v.v * self.a;
        let scale = (self.cmax - self.cmin) / (1. + FRAC_PI_2);
        let mid = self.cmax - scale;
        let (primitive, shape) = if x >= 0. {
            (x + (-2. * x).exp().ln_1p() - LN_2, x.tanh())
        } else {
            (x * x.atan() - 0.5 * (x * x).ln_1p(), x.atan())
        };
        Dual {
            v: mid * v.v + scale * primitive / self.a,
            d: v.d.map(|d| d * (mid + scale * shape)),
        }
    }

    fn resistors(&self) -> [(Node, Node, f64); 4] {
        [0, 1, 2, 3].map(|k| {
            if k == 3 {
                (self.ext[2], self.body, self.series[3])
            } else {
                (self.ext[k], self.int[k], self.series[k])
            }
        })
    }
}

impl DeviceGroup for Vdmos {
    fn len(&self) -> usize {
        1
    }
    fn instance_name(&self, _: usize) -> Option<&str> {
        Some(&self.name)
    }
    fn type_name(&self) -> &'static str {
        "vdmos"
    }
    fn state_len(&self) -> usize {
        1
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for (k, suffix) in ["d", "g", "s", "body"].into_iter().enumerate() {
            if self.series[k] > 0. {
                let n = Some(u.alloc_internal_node(&self.name, suffix));
                if k == 3 {
                    self.body = n;
                } else {
                    self.int[k] = n;
                }
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (p, n, r) in self.resistors() {
            if r > 0. {
                b.resistor(p, n);
            }
        }
        b.charge_core(&self.int);
        b.charge_core(&[self.body, self.ext[0]]);
        Ok(())
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
            "id" => vec![(0, 1.), (4, 1.)],
            "ig" => vec![(1, 1.)],
            "is" => vec![(2, 1.), (3, 1.)],
            "ibd" => vec![(3, 1.)],
            _ => Vec::new(),
        }
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        let mut at = SlotOffsets::default();
        for (p, n, r) in self.resistors() {
            if r > 0. {
                out.resistor(&mut at, 1. / r, volt(x, p), volt(x, n));
            }
        }
        let mut actual = self.int.map(|n| volt(x, n));
        let mut diode = [volt(x, self.body), volt(x, self.ext[0])];
        if let [ds, gs] = out.initial_bias(0) {
            actual[0] = actual[2] + ds;
            actual[1] = actual[2] + gs;
            diode[1] = diode[0] + ds;
        }
        let [d, g, s] = [0, 1, 2].map(|k| Dual::var(actual[k], k));
        let pol = self.sign;
        let gs = (g - s) * pol;
        let ds = (d - s) * pol;
        let id = self.channel(gs, ds) * pol;
        let qgs = gs * (pol * self.cgs);
        let qgd = self.miller((g - d) * pol) * pol;
        out.core(
            &mut at,
            &[id, Dual::cst(0.), -id],
            &[-qgd, qgs + qgd, -qgs],
            &actual,
            &actual,
        );
        let bias = (diode[0] - diode[1]) * pol;
        let vc = vcrit(self.saturation, self.nvt);
        let mut v = bias;
        if ctx.init_junctions && self.saturation > 0. {
            v = if self.off { 0. } else { vc };
        } else if ctx.limit {
            let (limited, changed) = if bias < -self.bv + 10. * self.nvt {
                let (v, lim) = limit_junction(
                    -bias - self.bv,
                    -st.current[0] - self.bv,
                    self.nvt,
                    vcrit(self.ibv, self.nvt),
                );
                (-v - self.bv, lim)
            } else {
                limit_junction(v, st.current[0], self.nvt, vc)
            };
            v = limited;
            out.noncon |= changed;
        }
        st.current[0] = v;
        let lin = [diode[1] + v * pol, diode[1]];
        let bias = Dual { v, d: [pol, -pol] };
        let junction = ideal_current(bias, self.saturation, self.nvt)
            - if self.bv.is_finite() {
                self.ibv * safe_exp(-(bias + self.bv) / self.nvt)
            } else {
                Dual::cst(0.)
            };
        let qb = (depletion_charge(bias, self.cjo, self.vj, self.grading, self.fc)
            + self.tt * junction)
            * pol;
        let ib = (junction + ctx.gmin * bias) * pol;
        out.core(&mut at, &[ib, -ib], &[qb, -qb], &diode, &lin);
        if let Some(values) = out.outputs.first_mut() {
            crate::observables::three_terminal_fet(values, id, [id.v - ib.v, 0., ib.v - id.v]);
            values.extend([
                ("ibd", ib.v),
                ("vgs", gs.v),
                ("vds", ds.v),
                ("cgd", -qgd.d[0]),
                ("cgs", self.cgs),
            ]);
        }
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        slots: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        let thermal = 4. * crate::BOLTZMANN * self.temp;
        let mut offset = 0;
        for (p, n, r) in self.resistors() {
            if r > 0. {
                out.push(NoiseSource::branch(&self.name, p, n, thermal / r, 0.));
                offset += 2;
            }
        }
        let [d, g, s] = self.int.map(|n| volt(x, n));
        let id = self.channel(
            Dual::<1>::var(self.sign * (g - s), 0),
            Dual::cst(self.sign * (d - s)),
        );
        for (power, exp) in [
            (thermal * (2. / 3.) * id.d[0].abs(), 0.),
            (
                self.kf * self.copies * (id.v.abs() / self.copies).powf(self.af),
                1.,
            ),
        ] {
            out.push(NoiseSource::branch(
                &self.name,
                self.int[0],
                self.int[2],
                power,
                exp,
            ));
        }
        let ij = slots.f[offset + 3] - ctx.gmin * (volt(x, self.body) - volt(x, self.ext[0]));
        out.push(NoiseSource::branch(
            &self.name,
            self.body,
            self.ext[0],
            2. * crate::CHARGE * ij.abs(),
            0.,
        ));
        Ok(())
    }
}
