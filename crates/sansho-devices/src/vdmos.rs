//! Electrical model of a vertical power MOSFET.
//! Provenance: ngspice user manual, VDMOS section (channel, Miller capacitance
//! and body diode); Shichman and Hodges (1968), square-law channel. Thermal
//! port and drift laws from original R9 black-box sweeps in verify/probes.

use crate::dual::Dual;
use crate::junction::{depletion_charge, ideal_current, limit_junction, safe_exp, vcrit};
use crate::temperature::Temperature;
use crate::{
    DeviceGroup, EvalCtx, ModelParams, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns, thermal_voltage, volt,
};
use sansho_diag::Diagnostic;

#[derive(Debug, Clone, Copy)]
struct TemperatureLaw {
    mu: f64,
    tcvth: f64,
    ideality: f64,
    eg: f64,
    xti: f64,
}

#[derive(Debug, Clone)]
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
    law: TemperatureLaw,
    subshift: f64,
    sub_slope: f64,
    drift_branch: Node,
    rq: f64,
    vq: f64,
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
        let law = TemperatureLaw {
            mu: p.get("mu", -1.5),
            tcvth: p.get("tcvth", 0.),
            ideality,
            eg: p.get("eg", 1.11),
            xti: p.get("xti", 3.),
        };
        let model = Self {
            name: name.into(),
            ext: nodes,
            int: nodes,
            body: nodes[2],
            series: ["rd", "rg", "rs", "rb"].map(|key| p.get(key, 0.) / copies),
            sign,
            temp: t.nominal,
            kp: p.get("kp", 1.) * copies,
            threshold: sign * p.get("vto", 0.),
            law,
            subshift: p.get("subshift", 0.),
            sub_slope: p.get("ksubthres", 0.1),
            drift_branch: None,
            rq: p.get("rq", 0.) / copies,
            vq: p.get("vq", 1.),
            lambda: p.get("lambda", 0.),
            theta: p.get("theta", 0.),
            mtriode: p.get("mtriode", 1.),
            cgs: p.get("cgs", 0.) * copies,
            cmin: p.get("cgdmin", 20e-12) * copies,
            cmax: p.get("cgdmax", 2e-9) * copies,
            a: p.get("a", 1.),
            saturation: p.get("is", 1e-14) * copies,
            nvt: ideality * thermal_voltage(t.nominal),
            cjo: p.get("cjo", 500e-12) * copies,
            vj,
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
        let model = model.at_temperature(t.kelvin);
        if ![model.sub_slope, model.vq]
            .iter()
            .all(|x| x.is_finite() && *x > 0.)
            || !model.subshift.is_finite()
            || !model.rq.is_finite()
            || model.rq < 0.
            || sign < 0. && nchan != 0.
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

    // Rebinding from the operating temperature also supplies the self-heating
    // small-signal temperature column.
    fn at_temperature(&self, kelvin: f64) -> Self {
        let mut hot = self.clone();
        if kelvin == self.temp {
            return hot;
        }
        let t = Temperature {
            nominal: self.temp,
            kelvin,
        };
        hot.temp = kelvin;
        hot.kp *= t.ratio().powf(self.law.mu);
        hot.threshold -= self.law.tcvth * (kelvin - self.temp);
        // The two R9b drain biases give the same drift multiplier at 55 °C.
        hot.rq *= t.ratio().powf(0.3);
        hot.nvt = self.law.ideality * thermal_voltage(kelvin);
        hot.saturation *= (t.saturation(self.law.eg, self.law.xti) / self.law.ideality).exp();
        hot.cjo *= t.capacitance(self.vj, self.grading);
        hot.vj = t.potential(self.vj);
        hot
    }

    fn channel<const N: usize>(&self, gs: Dual<N>, ds: Dual<N>) -> Dual<N> {
        let (drive, drop, sign) = if ds.v >= 0. {
            (gs, ds, 1.)
        } else {
            (gs - ds, -ds, -1.)
        };
        // Smooth turn-on. The default 0.1 V transition is visible in the
        // reference DC sweeps even half a volt above threshold.
        let overdrive = drive - self.threshold - self.subshift;
        let slope = self.sub_slope;
        let u = if overdrive.v >= 0. {
            overdrive + slope * (-overdrive / slope).exp().ln_1p()
        } else {
            slope * (overdrive / slope).exp().ln_1p()
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

    fn drift_resistance<const N: usize>(&self, ds: Dual<N>) -> Dual<N> {
        let magnitude = if ds.v < 0. { -ds } else { ds };
        self.series[0] + self.rq * (magnitude / (self.vq + magnitude))
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
            if self.series[k] > 0. || k == 0 && self.rq > 0. {
                let n = Some(u.alloc_internal_node(&self.name, suffix));
                if k == 3 {
                    self.body = n;
                } else {
                    self.int[k] = n;
                }
            }
        }
        if self.rq > 0. {
            self.drift_branch = Some(u.alloc_branch(&format!("{}#drift", self.name)));
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for (k, (p, n, r)) in self.resistors().into_iter().enumerate() {
            if k == 0 && self.drift_branch.is_some() {
                let branch = self.drift_branch;
                b.g(p, branch);
                b.g(n, branch);
                for col in [p, n, self.ext[2], branch] {
                    b.g(branch, col);
                }
                for row in [p, n, branch] {
                    b.f(row);
                }
            } else if r > 0. {
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
        for (k, (p, n, r)) in self.resistors().into_iter().enumerate() {
            if k == 0 && self.drift_branch.is_some() {
                // Vext - Vint = R(Vds) I remains regular at zero bias with RD=0.
                let [d, drain, source, current] =
                    [p, n, self.ext[2], self.drift_branch].map(|node| volt(x, node));
                let ds = Dual::<4>::var(d, 0) - Dual::var(source, 2);
                let drop = Dual::var(d, 0)
                    - Dual::var(drain, 1)
                    - self.drift_resistance(ds) * Dual::var(current, 3);
                out.g[at.g..at.g + 2].copy_from_slice(&[1., -1.]);
                out.g[at.g + 2..at.g + 6].copy_from_slice(&drop.d);
                out.f[at.f..at.f + 3].copy_from_slice(&[current, -current, drop.v]);
                at.g += 6;
                at.f += 3;
            } else if r > 0. {
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
        for (k, (p, n, r)) in self.resistors().into_iter().enumerate() {
            if k == 0 && self.drift_branch.is_some() {
                let ds = volt(x, p) - volt(x, self.ext[2]);
                let r = self.drift_resistance(Dual::<0>::cst(ds)).v;
                out.push(NoiseSource::branch(
                    &self.name,
                    self.drift_branch,
                    None,
                    thermal * r,
                    0.,
                ));
                offset += 3;
            } else if r > 0. {
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

/// Electrothermal extension of the electrical stamp, with the measured
/// junction and case port currents of the R9 thermal sweeps.
#[derive(Debug)]
pub struct ThermalVdmos {
    electrical: Vdmos,
    pins: [Node; 2],
    rthjc: f64,
    rthca: f64,
    cthj: f64,
    layout: crate::StampLayout,
}
impl ThermalVdmos {
    pub fn new(
        electrical: Vdmos,
        pins: [Node; 2],
        rthjc: f64,
        rthca: f64,
        cthj: f64,
    ) -> Result<Self, Diagnostic> {
        if !([rthjc, rthca].iter().all(|r| r.is_finite() && *r > 0.)
            && cthj.is_finite()
            && cthj >= 0.)
        {
            return Err(Diagnostic::error(
                "E-ELAB-PARAM",
                "thermal VDMOS requires positive RTHJC and RTHCA and nonnegative CTHJ",
            ));
        }
        let copies = electrical.copies;
        Ok(Self {
            electrical,
            pins,
            rthjc: rthjc / copies,
            rthca: rthca / copies,
            cthj: cthj * copies,
            layout: Default::default(),
        })
    }
    fn kelvin(&self, x: &[f64]) -> f64 {
        self.electrical.temp + volt(x, self.pins[0])
    }

    fn electrical_at(&self, kelvin: f64) -> Vdmos {
        let mut hot = self.electrical.at_temperature(kelvin);
        // At clamped Tj the heated-to-isothermal KP ratio follows
        // (Tj/Tamb)^(2 MU). at_temperature already applied one factor.
        let ratio = kelvin / self.electrical.temp;
        hot.kp *= ratio.powf(hot.law.mu);
        // The clamped hot transfer resolves RD separately from RS.
        hot.series[0] *= ratio.powf(1.5);
        hot
    }
}
impl DeviceGroup for ThermalVdmos {
    fn has_crossings(&self) -> bool {
        true
    }
    fn crossing(&self, old: &[f64], new: &[f64], _: &[f64], _: &[f64]) -> Option<f64> {
        // A fast gate ramp can traverse turn-on before stored heat changes
        // appreciably. Resolve the gate intercept on the accepted time grid.
        let dev = &self.electrical;
        let overdrive = |x: &[f64]| {
            let [d, g, s] = dev.int.map(|n| dev.sign * volt(x, n));
            g - s.min(d) - dev.threshold - dev.subshift + dev.law.tcvth * volt(x, self.pins[0])
        };
        let (a, b) = (overdrive(old), overdrive(new));
        (a * b < 0. && (b - a).abs() > dev.sub_slope).then(|| -a / (b - a))
    }
    fn probe_nodes(&self) -> Vec<usize> {
        // Junction temperature is also the control of the electrical stamp.
        self.pins[0].into_iter().collect()
    }
    fn len(&self) -> usize {
        1
    }
    fn type_name(&self) -> &'static str {
        "vdmos"
    }
    fn instance_name(&self, i: usize) -> Option<&str> {
        self.electrical.instance_name(i)
    }
    fn state_len(&self) -> usize {
        self.electrical.state_len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn output_charges(&self, i: usize, p: &str) -> Vec<(usize, f64)> {
        self.electrical.output_charges(i, p)
    }
    fn instance_ic(&self, i: usize, ic: &[f64], x: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        self.electrical.instance_ic(i, ic, x, edges);
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        self.electrical.alloc_unknowns(u);
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        self.electrical.setup(b)?;
        self.layout = b.layout.clone();
        let [tj, tc] = self.pins;
        // dF/dT and dQ/dT, then d(-sum V F)/dV and d(-sum V F)/dT.
        for &row in &self.layout.f {
            b.g(row, tj);
        }
        for &row in &self.layout.q {
            b.c(row, tj);
        }
        for &row in &self.layout.f {
            b.g(tj, row);
        }
        for &(_, col) in &self.layout.g {
            b.g(tj, col);
        }
        if self.electrical.series[2] > 0. {
            b.g(tj, self.electrical.ext[2]);
            b.g(tj, self.electrical.int[2]);
        }
        b.g(tj, tj);
        b.f(tj);
        b.charge_core(&[tj, tc]);
        Ok(())
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], state: StateRef<'_>, out: &mut Slots) {
        let t = self.kelvin(x);
        if !t.is_finite() || t <= 0. {
            out.error = Some(Diagnostic::error(
                "E-ANALYSIS",
                "VDMOS junction reached nonpositive or nonfinite absolute temperature",
            ));
            return;
        }
        // Start the coupled solve at the actual body bias. The tangent at
        // a fictitious forward bias can inject enough apparent heat into
        // an open case to select a different thermal equilibrium.
        let ctx = &EvalCtx {
            init_junctions: false,
            ..*ctx
        };
        let hot = self.electrical_at(t);
        // Differentiate only the temperature dependence with a symmetric
        // stencil. Both samples start with the same limiter and startup state.
        let h = (t * 1e-5).min(t * 0.25);
        let sample = |delta: f64| {
            let mut slots = Slots::for_layout(&self.layout);
            slots.initial_conditions.clone_from(&out.initial_conditions);
            let mut current = state.current.to_vec();
            self.electrical_at(t + delta).eval(
                ctx,
                x,
                StateRef {
                    accepted: state.accepted,
                    current: &mut current,
                },
                &mut slots,
            );
            slots
        };
        let minus = sample(-h);
        let plus = sample(h);
        hot.eval(ctx, x, state, out);
        let mut gi = self.layout.g.len();
        let mut power = 0.;
        let mut dpdt = 0.;
        let power_voltage = |row| {
            if row == self.electrical.drift_branch {
                0.
            } else {
                volt(x, row)
            }
        };
        for (i, &row) in self.layout.f.iter().enumerate() {
            let v = power_voltage(row);
            let df = (plus.f[i] - minus.f[i]) / (2. * h);
            power += v * out.f[i];
            dpdt += v * df;
            out.g[gi] = df;
            gi += 1;
        }
        let mut ci = self.layout.c.len();
        for i in 0..self.layout.q.len() {
            out.c[ci] = (plus.q[i] - minus.q[i]) / (2. * h);
            ci += 1;
        }
        for (&row, &current) in self.layout.f.iter().zip(&out.f) {
            out.g[gi] = if row == self.electrical.drift_branch {
                0.
            } else {
                -current
            };
            gi += 1;
        }
        for (i, &(row, _)) in self.layout.g.iter().enumerate() {
            out.g[gi] = -power_voltage(row) * out.g[i];
            gi += 1;
        }
        // The source access resistor heats outside the junction. Remove its
        // contribution to the static power and to the thermal Jacobian row.
        let rs = hot.series[2];
        if rs > 0. {
            let drop = volt(x, hot.ext[2]) - volt(x, hot.int[2]);
            let current = drop / rs;
            power -= drop * current;
            out.g[gi..gi + 2].copy_from_slice(&[2. * current, -2. * current]);
            gi += 2;
        }
        out.g[gi] = -dpdt;
        out.f[self.layout.f.len()] = -power;
        let mut at = SlotOffsets {
            g: gi + 1,
            c: ci,
            f: self.layout.f.len() + 1,
            q: self.layout.q.len(),
        };
        let actual = self.pins.map(|n| volt(x, n));
        let tj = Dual::<2>::var(actual[0], 0);
        let tc = Dual::var(actual[1], 1);
        let flow = (tj - tc) / self.rthjc;
        // Clamp currents separate the ambient drive into the junction from
        // the case shunt. A case step produces no capacitive feedthrough.
        let ambient_drive = (self.electrical.temp - 273.15) / self.rthca;
        out.core(
            &mut at,
            &[flow - ambient_drive, tc / self.rthca - flow],
            &[tj * (2. * self.cthj), Dual::cst(0.)],
            &actual,
            &actual,
        );
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        slots: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        self.electrical_at(self.kelvin(x)).noise(ctx, x, slots, out)
    }
}
