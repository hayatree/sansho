//! Native vertical bipolar inter-company transistor, SPICE level 4.
//! Provenance: McAndrew et al., VBIC95 (JSSC, 1996) and MSM 2003,
//! Kull et al. (IEEE TED, 1985) and ngspice 44 manual §§7.3.4, 27.5.2.
//! Independent ngspice 44.2 measurements and their limits are in docs/vbic.md.

mod params;
pub use params::Model;

use crate::{
    DeviceGroup, EvalCtx, Node, NoiseSource, SlotOffsets, Slots, StampBuilder, StateRef, Unknowns,
    dual::Dual,
    junction::{limit_junction, safe_exp, vcrit},
    temperature::Temperature,
    volt,
};
use sansho_diag::Diagnostic;
use std::sync::Arc;

const C: usize = 0;
const B: usize = 1;
const E: usize = 2;
const S: usize = 3;
const CX: usize = 4;
const CI: usize = 5;
const BX: usize = 6;
const BI: usize = 7;
const EI: usize = 8;
const BP: usize = 9;
const SI: usize = 10;
const TJ: usize = 11;
const PH1: usize = 12;
const PH2: usize = 13;
const N: usize = 14;
type D = Dual<N>;

// Historical rounded k/q in V/K. The slope of the 44.2 default-card sweep
// independently gives 8.61734789e-5 after the base drop and shunt currents.
const K_OVER_Q: f64 = 1.380_662e-23 / 1.602_189e-19;

// Summing the electrical KCL rows behind each lead cancels its internal
// contact currents, including when two *external* leads share a net.
const ROWS: [&[usize]; 4] = [&[C, CX, CI, BP], &[B, BX, BI], &[E, EI], &[S, SI]];
const CURRENT: [&str; 4] = ["ic", "ib", "ie", "is"];

fn floor(v: D, min: f64) -> D {
    if v.v < min { D::cst(min) } else { v }
}

// R15 AVC1/2 and reverse-epi sweeps both resolve a 0.1 V transition width.
const FIELD_WIDTH_SQUARED: f64 = 0.1 * 0.1;

fn rounded_voltage(v: D) -> D {
    (v.sq() + FIELD_WIDTH_SQUARED).sqrt()
}

fn junction(v: D, scale: D, slope: D) -> D {
    // Shockley law, using expm1 at small bias and a tangent at extreme bias.
    let x = v / slope;
    if x.v < 80. {
        scale * x.exp_m1()
    } else {
        scale * (safe_exp(x) - 1.)
    }
}

// Integral of the abrupt/graded junction capacitance (Getreu). In normalized
// voltage coordinates P cancels out of the integral. McAndrew's smoothed
// junction transitions to constant capacitance above FC and at reach-through.
fn depletion(v: D, potential: D, grading: f64, fc: f64, aj: f64, vrt: f64, art: f64) -> D {
    let primitive = |u: D| {
        let arg = 1. - u / potential;
        if (1. - grading).abs() < 1e-10 {
            -potential * arg.ln()
        } else {
            potential * (1. - arg.powf(1. - grading)) / (1. - grading)
        }
    };
    let knee = potential * fc;
    let cmax = (1. - fc).powf(-grading);
    let integrated = |u: D| {
        let mut z = u;
        let mut extension = D::cst(0.);
        if vrt > 0. {
            let a = u + vrt;
            z = if aj > 0. && art > 0. {
                (a + (a.sq() + art * art).sqrt()) * 0.5 - vrt
            } else {
                floor(u, -vrt)
            };
            extension = (u - z) * (1. + vrt / potential).powf(-grading);
        }
        let d = z - knee;
        if aj > 0. {
            let lower = knee + (d - (d.sq() + 4. * aj * aj).sqrt()) * 0.5;
            primitive(lower) + (z - lower) * cmax + extension
        } else if d.v <= 0. {
            primitive(z) + extension
        } else {
            primitive(knee) + cmax * d * (1. + 0.5 * grading * d / (potential - knee)) + extension
        }
    };
    integrated(v) - integrated(D::cst(0.))
}

#[derive(Debug)]
struct Instance {
    name: String,
    model: Arc<Model>,
    nodes: [Node; N],
    thermal_pin: Option<Node>,
    t: Temperature,
    area: f64,
    copies: f64,
    off: bool,
    at: SlotOffsets,
}

struct Bias {
    f: [D; N],
    q: [D; N],
    transport: D,
    base_be: D,
    base_bc: D,
    qbe: D,
    qbc: D,
    power: D,
}

fn branch(rows: &mut [D; N], p: usize, n: usize, value: D) {
    rows[p] += value;
    rows[n] += -value;
}

impl Instance {
    fn heated(&self) -> bool {
        self.thermal_pin.is_some() && self.model.selft == 1. && self.model.rth > 0.
    }

    fn evaluate(
        &self,
        x: [f64; N],
        gmin: f64,
        mut noise: impl FnMut(usize, usize, f64, f64),
    ) -> Bias {
        let m = &self.model;
        let scale = self.area * self.copies;
        let v: [D; N] = std::array::from_fn(|k| D::var(x[k], k));
        let zero = D::cst(0.);
        let mut f = [zero; N];
        let mut q = [zero; N];
        let sign = m.polarity;
        let u = |a: usize, b: usize| (v[a] - v[b]) * sign;
        let be = u(BI, EI);
        let bx = u(BX, EI);
        let bc = u(BI, CI);
        let bep = u(BX, BP);
        let bcp = u(SI, BP);
        let heated = self.heated();
        let t = if heated { v[TJ] } else { zero } + self.t.kelvin + m.dtemp;
        let vt = t * K_OVER_Q;
        let ratio = t / self.t.nominal;
        let dt = t - self.t.nominal;

        // McAndrew, MSM 2003: map parameters from their measurement temperature
        // before evaluating any branch. Activation energies are in eV.
        let activation = (ratio - 1.) / vt;
        let sat = |i: f64, x: f64, ea: f64, ideality: f64| {
            i * safe_exp((ratio.ln() * x + activation * ea) / ideality)
        };
        let potential = |p: f64, ea: f64| {
            // Abrupt junction built-in voltage from carrier statistics (Getreu).
            floor(ea + ratio * (p - ea) - 3. * vt * ratio.ln(), 0.01)
        };
        let pe = potential(m.pe, m.eaie);
        let pc = potential(m.pc, m.eaic);
        let ps = potential(m.ps, m.eais);
        let de = |v| depletion(v, pe, m.me, m.fc, m.aje, 0., 0.);
        let dc = |v| depletion(v, pc, m.mc, m.fc, m.ajc, m.vrt, m.art);
        let de_be = de(be);
        let dc_bc = dc(bc);
        let thermal_emission = floor(1. + dt * m.tnf, 0.01);
        let is = sat(m.is_, m.xis, m.ea, m.nf);
        let forward = junction(be, is, vt * m.nf * thermal_emission);
        let reverse = junction(
            bc,
            is * sat(m.isrr, m.xisr, m.dear, m.nr),
            vt * m.nr * thermal_emission,
        );

        // VBIC95 integral base charge: electrostatic width from normalized
        // junction charges, and mobile charge from double injection.
        let inv = |p: f64| if p > 0. { 1. / p } else { 0. };
        let width = floor(1. + de_be * inv(m.ver) + dc_bc * inv(m.vef), 1e-4);
        let injection = forward * inv(m.ikf) / ratio.powf(m.xikf) + reverse * inv(m.ikr);
        let qb = if m.qbm == 1. {
            width * (1. + floor(1. + 4. * injection, 1e-12).powf(m.nkf)) * 0.5
        } else {
            (width + floor(width.powf(1. / m.nkf) + 4. * injection, 1e-12).powf(m.nkf)) * 0.5
        };
        let itf = forward / qb;
        let itr = reverse / qb;
        // McAndrew excess phase, second-order low-pass with low-frequency
        // delay TD. Two first-order DAE rows also supply the AC transfer.
        let delayed = if m.td > 0. {
            f[PH1] = v[PH1] - itf + v[PH2];
            q[PH1] = v[PH2] * (m.td / 3.);
            f[PH2] = -v[PH2];
            q[PH2] = v[PH1] * m.td;
            v[PH1]
        } else {
            itf
        };
        let transport = delayed - itr;

        // Independent ideal and recombination junctions (VBIC95).
        let pair = |v, ii, ei, ni, inon, en, nn| {
            junction(v, sat(ii, m.xii, ei, ni), vt * ni)
                + junction(v, sat(inon, m.xin, en, nn), vt * nn)
        };
        let base = |v| {
            let base = pair(v, m.ibei, m.eaie, m.nei, m.iben, m.eane, m.nen);
            if m.vbbe <= 0. {
                return base;
            }
            // Explicit EBBE replaces the calculated zero-bias exponential.
            let bv = m.vbbe * (1. + dt * (m.tvbbe1 + dt * m.tvbbe2));
            let slope = vt * m.nbbe * floor(1. + dt * m.tnbbe, 0.01);
            base - m.ibbe
                * (safe_exp((-bv - v) / slope)
                    - m.ebbe.map_or_else(|| safe_exp(-bv / slope), D::cst))
        };
        let bei = base(be) * m.wbe;
        let bex = base(bx) * (1. - m.wbe);
        let bci = pair(bc, m.ibci, m.eaic, m.nci, m.ibcn, m.eanc, m.ncn);
        // Local-field weak avalanche integral (McAndrew et al., 1996).
        let reverse_bias = pc - bc;
        let field = if reverse_bias.v < 0. {
            // Conjugate form preserves the small positive field in forward bias.
            0.5 * FIELD_WIDTH_SQUARED / (rounded_voltage(reverse_bias) - reverse_bias)
        } else {
            (reverse_bias + rounded_voltage(reverse_bias)) * 0.5
        };
        let avalanche = (itf - itr - bci)
            * m.avc1
            * field
            * safe_exp(-m.avc2 * (1. + m.tavc * dt) * field.powf(m.mc - 1.));
        let base_bc = bci - avalanche;

        // Opposite-polarity substrate transistor. Its emitter is the extrinsic
        // base, its collector the substrate, and its base the collector pocket.
        let isp = sat(m.isp, m.xis, m.eap, m.nfp);
        let fp =
            m.wsp * junction(bep, isp, vt * m.nfp) + (1. - m.wsp) * junction(bc, isp, vt * m.nfp);
        let rp = junction(bcp, isp, vt * m.nfp);
        let qp = (1. + floor(1. + 4. * fp * inv(m.ikp), 1e-12).sqrt()) * 0.5;
        let parasitic = (fp - rp) / qp;
        let ibep = pair(bep, m.ibeip, m.eaic, m.nci, m.ibenp, m.eanc, m.ncn);
        let ibcp = pair(bcp, m.ibcip, m.eais, m.ncip, m.ibcnp, m.eans, m.ncnp);

        // Static terminal currents.
        for (p, n, i) in [
            (CI, EI, transport),
            (BI, EI, bei),
            (BX, EI, bex),
            (BI, CI, base_bc),
            (BX, SI, parasitic),
            (BX, BP, ibep),
            (SI, BP, ibcp),
        ] {
            branch(&mut f, p, n, i * sign);
        }
        // The collector noise sweep resolves forward transport shot noise,
        // while the avalanche sweep excludes the net B-C branch as a source.
        for (p, n, i) in [
            (CI, EI, itf),
            (BI, EI, bei),
            (BX, EI, bex),
            (BX, SI, parasitic),
            (BX, BP, ibep),
            (SI, BP, ibcp),
        ] {
            noise(p, n, 2. * crate::CHARGE * i.v.abs() * scale, 0.);
        }
        // Each parallel copy generates flicker from its area-scaled current.
        for (p, i) in [(BI, bei), (BX, bex)] {
            noise(
                p,
                EI,
                m.kfn * (i.v.abs() * self.area).powf(m.afn) * self.copies,
                m.bfn,
            );
        }

        // Kull et al. conductivity integral in the epitaxial collector. The
        // rounded drop and HRCF determine the bias-dependent limiting voltage.
        let carriers = |bias| {
            (1. + m.gamm * safe_exp(activation * m.ea) * ratio.powf(m.xis) * safe_exp(bias / vt))
                .sqrt()
        };
        let ki = carriers(bc);
        let kx = carriers(u(BI, CX));
        for (p, n, r, xr, charge) in [
            (C, CX, m.rcx, m.xrcx, D::cst(1.)),
            (CX, CI, m.rci, m.xrci, D::cst(1.)),
            (B, BX, m.rbx, m.xrbx, D::cst(1.)),
            (BX, BI, m.rbi, m.xrbi, qb),
            (E, EI, m.re, m.xre, D::cst(1.)),
            (S, SI, m.rs, m.xrs, D::cst(1.)),
            (CX, BP, m.rbp, m.xrbp, qp),
        ] {
            if r <= 0. {
                continue;
            }
            let g = charge / (r * ratio.powf(xr));
            let drop = u(p, n);
            let mut i = drop * g;
            if n == CI {
                let density = ki - kx;
                let ohmic = drop + vt * (density - (density / (1. + kx)).ln_1p());
                let vo = m.vo * ratio.powf(m.xvo);
                let limit = if m.vo > 0. {
                    let mag = rounded_voltage(drop);
                    (1. + (ohmic / (vo + mag * (0.5 * inv(m.hrcf)))).sq()).sqrt()
                } else {
                    D::cst(1.)
                };
                i = ohmic * g / limit;
            }
            branch(&mut f, p, n, i * sign);
            // Johnson–Nyquist current spectrum of the local dissipative lead.
            noise(p, n, 4. * crate::BOLTZMANN * t.v * g.v * scale, 0.);
        }

        // VBIC transit-time base-width and bias modulation, plus depletion,
        // overlap and Kull excess-majority-carrier storage.
        let fraction = if forward.v <= 0. {
            zero
        } else if m.itf > 0. {
            forward / (forward + m.itf)
        } else {
            D::cst(1.)
        };
        let tf = m.tf
            * (1. + m.qtf * width)
            * (1. + m.xtf * fraction.sq() * safe_exp(bc * (inv(m.vtf) / 1.44)));
        let cje = m.cje * (m.pe / pe).powf(m.me);
        let cjc = (m.pc / pc).powf(m.mc);
        let qbe = m.wbe * cje * de_be + tf * itf;
        let qbc = m.cjc * cjc * dc_bc + m.tr * reverse + m.qco * (ki - 1.);
        for (p, n, value) in [
            (BI, EI, qbe),
            (BX, EI, (1. - m.wbe) * cje * de(bx)),
            (BI, CI, qbc),
            (BI, CX, m.qco * (kx - 1.)),
            (
                BX,
                BP,
                m.cjep * cjc * depletion(bep, pc, m.mc, m.fc, m.ajc, 0., 0.) + m.tr * fp,
            ),
            (
                SI,
                BP,
                m.cjcp * (m.ps / ps).powf(m.ms) * depletion(bcp, ps, m.ms, m.fc, m.ajs, 0., 0.),
            ),
            (B, E, m.cbeo * u(B, E)),
            (B, C, m.cbco * u(B, C)),
            (C, S, m.ccso * u(C, S)),
        ] {
            branch(&mut q, p, n, value * sign);
        }
        // Manual §7.3.4: static electrical branch power drives a parallel
        // RTH/CTH to ambient. The phase states are control variables.
        let power = (0..TJ).fold(zero, |sum, k| sum + v[k] * f[k]);
        if heated {
            f[TJ] = v[TJ] / m.rth - power;
            q[TJ] = m.cth * v[TJ];
        }
        let mut f = f.map(|a| a * scale);
        // GMIN belongs to each instance stamp. The scaled PNP sweep resolves
        // the same absolute shunt current as a unit transistor.
        for (p, n) in [(BI, EI), (BX, EI), (BI, CI), (BX, BP), (SI, BP)] {
            branch(&mut f, p, n, (v[p] - v[n]) * gmin);
        }
        Bias {
            f,
            q: q.map(|a| a * scale),
            power: power * scale,
            transport: (itf - itr) * (scale * sign),
            base_be: bei * (scale * sign),
            base_bc: base_bc * (scale * sign),
            qbe: qbe * (scale * sign),
            qbc: qbc * (scale * sign),
        }
    }
}

/// Batch of native VBIC instances in netlist order.
#[derive(Default, Debug)]
pub struct Vbics(Vec<Instance>);

impl Vbics {
    /// Bind C/B/E/S and an optional (possibly grounded) junction-rise pin.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        name: &str,
        pins: [Node; 4],
        thermal_pin: Option<Node>,
        model: Arc<Model>,
        t: Temperature,
        area: f64,
        copies: f64,
        off: bool,
    ) {
        let mut nodes = [None; N];
        nodes[..4].copy_from_slice(&pins);
        self.0.push(Instance {
            name: name.into(),
            model,
            nodes,
            thermal_pin,
            t,
            area,
            copies,
            off,
            at: SlotOffsets::default(),
        });
    }
}

impl DeviceGroup for Vbics {
    fn type_name(&self) -> &'static str {
        "vbic"
    }
    fn instance_name(&self, i: usize) -> Option<&str> {
        self.0.get(i).map(|d| d.name.as_str())
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_linear(&self) -> bool {
        false
    }
    fn state_len(&self) -> usize {
        self.len() * 2
    }
    fn alloc_unknowns(&mut self, u: &mut Unknowns) {
        for d in &mut self.0 {
            let m = &d.model;
            for (k, lead, r, name) in [
                (CX, C, m.rcx, "cx"),
                (CI, CX, m.rci, "ci"),
                (BX, B, m.rbx, "bx"),
                (BI, BX, m.rbi, "bi"),
                (EI, E, m.re, "ei"),
                (BP, CX, m.rbp, "bp"),
                (SI, S, m.rs, "si"),
            ] {
                d.nodes[k] = if r > 0. {
                    Some(u.alloc_internal_node(&d.name, name))
                } else {
                    d.nodes[lead]
                };
            }
            if d.heated() {
                d.nodes[TJ] = d.thermal_pin.flatten();
            }
            if m.td > 0. {
                for (k, name) in [(PH1, "phase"), (PH2, "phase_rate")] {
                    d.nodes[k] = Some(u.alloc_internal_node(&d.name, name));
                }
            }
        }
    }
    fn setup(&mut self, b: &mut StampBuilder<'_>) -> Result<(), Diagnostic> {
        for d in &mut self.0 {
            d.at = b.counts();
            b.charge_core(&d.nodes);
        }
        Ok(())
    }
    fn instance_ic(&self, i: usize, ic: &[f64], _: &mut [f64], edges: &mut Vec<(Node, Node, f64)>) {
        let n = self.0[i].nodes;
        if let [be, ce] = ic {
            edges.extend([(n[BI], n[EI], *be), (n[CI], n[EI], *ce)]);
        }
    }
    fn output_charges(&self, i: usize, p: &str) -> Vec<(usize, f64)> {
        CURRENT
            .iter()
            .position(|name| *name == p)
            .into_iter()
            .flat_map(|k| ROWS[k].iter().map(|row| (self.0[i].at.q + row, 1.)))
            .collect()
    }
    fn eval(&self, ctx: &EvalCtx, x: &[f64], st: StateRef<'_>, out: &mut Slots) {
        for (i, d) in self.0.iter().enumerate() {
            let act = d.nodes.map(|n| volt(x, n));
            let mut lin = act;
            let t = d.t.kelvin + d.model.dtemp + act[TJ];
            if !t.is_finite() || t <= 0. {
                out.error = Some(Diagnostic::error(
                    "E-ANALYSIS",
                    format!("{}: invalid VBIC junction temperature", d.name),
                ));
                return;
            }
            let sign = d.model.polarity;
            let vt = t * K_OVER_Q;
            let [mut be, mut bc] = [(act[BI] - act[EI]) * sign, (act[BI] - act[CI]) * sign];
            if let [vbe, vce] = out.initial_bias(i) {
                be = sign * vbe;
                bc = sign * (vbe - vce);
            } else if ctx.init_junctions && d.off {
                be = 0.;
                bc = 0.;
            } else if ctx.limit {
                for (k, v, n) in [(0, &mut be, d.model.nf), (1, &mut bc, d.model.nr)] {
                    let (value, limited) = limit_junction(
                        *v,
                        st.current[2 * i + k],
                        n * vt,
                        vcrit(d.model.is_.max(1e-30), n * vt),
                    );
                    *v = value;
                    out.noncon |= limited;
                }
            }
            st.current[2 * i..2 * i + 2].copy_from_slice(&[be, bc]);
            lin[BI] = lin[EI] + sign * be;
            lin[CI] = lin[BI] - sign * bc;
            let b = d.evaluate(lin, ctx.gmin, |_, _, _, _| {});
            if let Some(values) = out.outputs.get_mut(i) {
                for (name, rows) in CURRENT.into_iter().zip(ROWS) {
                    values.push((name, rows.iter().map(|k| b.f[*k].v).sum()));
                }
                values.extend([
                    ("vbe", sign * be),
                    ("vbc", sign * bc),
                    ("p", b.power.v),
                    ("gm", b.transport.d[BI]),
                    ("go", b.transport.d[CI]),
                    ("gpi", -b.base_be.d[EI]),
                    ("gmu", -b.base_bc.d[CI]),
                    ("cpi", -b.qbe.d[EI]),
                    ("cmu", -b.qbc.d[CI]),
                ]);
            }
            let mut at = d.at;
            out.core(&mut at, &b.f, &b.q, &act, &lin);
        }
    }
    fn noise(
        &self,
        ctx: &EvalCtx,
        x: &[f64],
        _: &Slots,
        out: &mut Vec<NoiseSource>,
    ) -> Result<(), Diagnostic> {
        for d in &self.0 {
            d.evaluate(d.nodes.map(|n| volt(x, n)), ctx.gmin, |p, n, power, exp| {
                if power > 0. {
                    out.push(NoiseSource::branch(
                        &d.name, d.nodes[p], d.nodes[n], power, exp,
                    ));
                }
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelParams;

    #[test]
    fn smoothed_junction_charge_has_a_zero_origin_and_continuous_slope() {
        for aj in [-0.5, 0.05] {
            for m in [0.33, 1.] {
                let charge = |v| depletion(D::var(v, 0), D::cst(0.75), m, 0.8, aj, 1., 0.1);
                assert_eq!(charge(0.).v, 0.);
                for v in [-2., -1., -0.4, 0., 0.6, 1.] {
                    // At the hard reverse join capacitance is continuous with a
                    // corner, so the centered difference is first-order there.
                    let slope = (charge(v + 1e-7).v - charge(v - 1e-7).v) / 2e-7;
                    assert!(slope > 0. && (slope - charge(v).d[0]).abs() < 2e-8);
                }
            }
        }
    }

    #[test]
    fn all_coupled_conductance_and_charge_columns_match_bias_perturbations() {
        let fields = [
            ("rci", 7.),
            ("gamm", 1e-9),
            ("vo", 0.4),
            ("hrcf", 2.),
            ("rcx", 1.),
            ("re", 1.),
            ("rs", 3.),
            ("rbi", 30.),
            ("rbx", 4.),
            ("rbp", 11.),
            ("qco", 1e-12),
            ("is", 3e-16),
            ("isp", 1e-15),
            ("ibcip", 1e-17),
            ("ibeip", 2e-16),
            ("wsp", 0.6),
            ("vef", 20.),
            ("ikf", 0.01),
            ("qtf", 0.3),
            ("avc1", 0.1),
            ("avc2", 2.),
            ("tf", 1e-9),
            ("tr", 1e-9),
            ("td", 1e-10),
            ("cje", 2e-12),
            ("cjc", 1e-12),
            ("cjcp", 1e-12),
            ("cjep", 1e-12),
            ("rth", 300.),
            ("cth", 1e-8),
            ("selft", 1.),
        ];
        let params = ModelParams::new(&fields.map(|(k, v)| (k.into(), v)));
        let model = Arc::new(Model::from_params(&params, 1.).unwrap());
        let mut group = Vbics::default();
        group.push(
            "q",
            [None; 4],
            Some(Some(0)),
            model,
            Temperature {
                kelvin: 310.,
                nominal: 300.,
            },
            1.5,
            1.,
            false,
        );
        let d = &group.0[0];
        let x = [
            0.21, 0.73, 0., -0.2, 0.20, 0.18, 0.72, 0.71, 0.01, 0.19, -0.19, 4., 0.001, 0.0001,
        ];
        let eval = |v| d.evaluate(v, 1e-12, |_, _, _, _| {});
        let bias = eval(x);
        for rows in [&bias.f, &bias.q] {
            let sum: f64 = rows[..TJ].iter().map(|r| r.v).sum();
            assert!(sum.abs() < 1e-15 * rows[..TJ].iter().map(|r| r.v.abs()).sum::<f64>());
        }
        for col in 0..N {
            let mut plus = x;
            let mut minus = x;
            let h = if col == TJ { 1e-4 } else { 1e-6 };
            plus[col] += h;
            minus[col] -= h;
            let (a, b) = (eval(plus), eval(minus));
            for (ref_rows, hi, lo, abs) in
                [(&bias.f, &a.f, &b.f, 1e-10), (&bias.q, &a.q, &b.q, 1e-20)]
            {
                for row in 0..N {
                    let measured = (hi[row].v - lo[row].v) / (2. * h);
                    let exact = ref_rows[row].d[col];
                    assert!(
                        (measured - exact).abs() < abs + 1e-6 * exact.abs(),
                        "row {row}, column {col}: {exact} vs {measured}"
                    );
                }
            }
        }
    }
}
