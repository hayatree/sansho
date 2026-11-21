//! Native HICUM/L2 bipolar device, SPICE level 8.
//! Provenance: Schröter and Chakravorty, Compact Hierarchical Bipolar
//! Transistor Modeling with HICUM (2010); ngspice 44 manual §7.3.5,
//! Fig. 7.3. In the equation comments, S&C denotes the 2010 monograph.
//! Textbook junction and ionization laws follow Sze & Ng, Physics of
//! Semiconductor Devices (3rd ed.). Open mappings are listed in docs/hicum.md.

mod charge;
mod params;
pub use params::{Model, VERSION};

use crate::{
    BOLTZMANN, CHARGE, DeviceGroup, EvalCtx, Node, NoiseSource, SlotOffsets, Slots, StampBuilder,
    StateRef, Unknowns,
    dual::Dual,
    junction::{limit_junction, safe_exp, vcrit},
    temperature::Temperature,
    volt,
};
use charge::{base_charge, depletion, positive_part, transport};
use sansho_diag::Diagnostic;
use std::sync::Arc;

const C: usize = 0;
const B: usize = 1;
const E: usize = 2;
const S: usize = 3;
const CI: usize = 4;
const BX: usize = 5;
const BI: usize = 6;
const EI: usize = 7;
const SI: usize = 8;
const TJ: usize = 9;
const IT: usize = 10;
const QF: usize = 11;
const VELOCITY: usize = 12;
const N: usize = 13;
type D = Dual<N>;
const ROWS: [&[usize]; 4] = [&[C, CI], &[B, BX, BI], &[E, EI], &[S, SI]];
const CURRENT: [&str; 4] = ["ic", "ib", "ie", "is"];
const K_OVER_Q: f64 = BOLTZMANN / CHARGE;

fn floor(x: D, lo: f64) -> D {
    if x.v < lo { D::cst(lo) } else { x }
}
fn branch(rows: &mut [D; N], p: usize, n: usize, i: D) {
    rows[p] += i;
    rows[n] += -i;
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
    avalanche: D,
    critical: D,
    tau: D,
}

impl Instance {
    fn heated(&self) -> bool {
        self.model.flsh > 0. && self.model.rth > 0.
    }
    fn evaluate(
        &self,
        x: [f64; N],
        gmin: f64,
        small_signal: bool,
        mut noise: impl FnMut(usize, usize, f64, f64),
    ) -> Bias {
        let m = &self.model;
        let scale = self.area * self.copies;
        let s = m.polarity;
        let zero = D::cst(0.);
        let v: [D; N] = std::array::from_fn(|k| D::var(x[k], k));
        let u = |p: usize, n: usize| (v[p] - v[n]) * s;
        let be = u(BI, EI);
        let bp = u(BX, EI);
        let bc = u(BI, CI);
        let bx = u(BX, CI);
        let sc = u(SI, CI);
        let ce = u(CI, EI);
        let t = if self.heated() {
            self.t.kelvin + v[TJ]
        } else {
            D::cst(self.t.kelvin)
        };
        let vt = t * K_OVER_Q;
        let ratio = t / self.t.nominal;
        let dt = t - self.t.nominal;
        // Carrier statistics (Sze & Ng). The T ln(T) gap interpolation and
        // thermal prefactor exponents are the R16 mapping of the card.
        let gap = |g: f64| g + m.f1vg * t * t.ln() + m.f2vg * t;
        let nominal_gap =
            |g: f64| g + m.f1vg * self.t.nominal * self.t.nominal.ln() + m.f2vg * self.t.nominal;
        let sat = |nom: f64, g: f64, z: f64, n: f64| {
            nom * safe_exp((z * ratio.ln() + g * (ratio - 1.) / vt) / n)
        };
        let exp = |x: D| {
            if x.v < 80. {
                x.exp_m1()
            } else {
                safe_exp(x) - 1.
            }
        };
        // Shockley ideal and SRH recombination currents with individual
        // emission coefficients. The effective gap for a heterojunction
        // is an open choice; use the average gap of its adjoining regions.
        let diode = |x: D, i: f64, n: f64, g: f64| sat(i, g, m.zetact, n) * exp(x / (n * vt));
        let egbe = (m.vge + m.vgb) * 0.5;
        let egbc = (m.vgc + m.vgb) * 0.5;
        let egsc = (m.vgs + m.vgc) * 0.5;
        let beta_factor = safe_exp(-m.alb * dt);
        let ibe =
            diode(be, m.ibeis, m.mbei, m.vgb) * beta_factor + diode(be, m.ireis, m.mrei, m.vgb);
        let ibp =
            diode(bp, m.ibeps, m.mbep, m.vgb) * beta_factor + diode(bp, m.ireps, m.mrep, m.vgb);
        let ibc = diode(bc, m.ibcis, m.mbci, egbc);
        let ibx = diode(bx, m.ibcxs, m.mbcx, egbc);
        let isc = diode(sc, m.iscs, m.msc, egsc);
        // Built-in potential from the mass-action law (Sze & Ng). Preserve the
        // nominal doping profile by scaling C0 with (Vd0/Vd(T))^z.
        let junction = |x: D, c: f64, p: f64, z: f64, amax: f64, vpt: f64, eg: f64| {
            let vd = floor(
                p * ratio + gap(eg) - nominal_gap(eg) * ratio - 3. * vt * ratio.ln(),
                0.01,
            );
            let (q, cap) = depletion(x, vd, z, amax, vpt, vt);
            let c0 = c * (p / vd).powf(z);
            (q * c0, cap, vd, c0)
        };
        let (qje, ce_ratio, vde, _) = junction(be, m.cjei0, m.vdei, m.zei, m.ajei, 0., egbe);
        let (qjp, cp_ratio, vdp, _) = junction(bp, m.cjep0, m.vdep, m.zep, m.ajep, 0., egbe);
        let (qjc, cc_ratio, vdc, cc0) = junction(bc, m.cjci0, m.vdci, m.zci, 2.4, m.vptci, egbc);
        let qcx = |x| junction(x, m.cjcx0, m.vdcx, m.zcx, 2.4, m.vptcx, egbc).0;
        let qjs = junction(sc, m.cjs0, m.vds, m.zs, 2.4, m.vpts, egsc).0;
        let qscp = junction(u(S, C), m.cscp0, m.vdsp, m.zsp, 2.4, m.vptsp, egsc).0;
        // S&C, GICCR: normalized diffusion numerators are exponentials. At
        // equilibrium the two directional currents cancel.
        let pref = sat(m.c10 / m.qp0, m.vgb, m.zetact, 1.);
        let jf = pref * safe_exp(be / (m.mcf * vt));
        let jr = pref * safe_exp(bc / vt);
        // Average an exponential hole weight over the swept depletion width.
        // AHJEI=1 and 4 independently give the same fractional displacement.
        let heterojunction = if m.ahjei != 0. {
            let edge = 2. * std::f64::consts::LN_2 * vt;
            let depleted = positive_part(vde - positive_part(be, edge), m.rhjei * edge) / vde;
            let span = m.ahjei * (1. - depleted.powf(m.zei));
            if span.v.abs() < f64::EPSILON.sqrt() {
                1. + span * 0.5
            } else {
                exp(span) / span
            }
        } else {
            D::cst(1.)
        };
        let hje = m.hjei * ratio.powf(m.zetahjei) * heterojunction;
        let equilibrium = 1. + 0.5 * m.zei * (1. - vde / m.vdei);
        let hole = equilibrium + (hje * qje + m.hjci * qjc) / m.qp0;
        // S&C, critical current: velocity saturation bounds the pre-punch-
        // through knee at Vlim/Rci. The two roundings follow the R16 sweeps.
        // VCES is the residual voltage; VPT introduces reach-through slope.
        let head = vt
            + positive_part(
                ce - m.vces * (1. + m.alces * dt) - vt,
                2. * std::f64::consts::LN_2 * vt,
            );
        let velocity = m.vlim * (1. - m.alvs * dt);
        let overdrive = velocity / vt - 1.;
        let vlim = if overdrive.v > 0. {
            velocity + vt * (-overdrive).exp().ln_1p()
        } else {
            vt * (1. + overdrive.exp().ln_1p())
        };
        let crit = if m.rci0 > 0. {
            let field = head / vlim;
            head / (m.rci0 * ratio.powf(m.zetaci) * (1. + field.powf(m.delck)).powf(1. / m.delck))
                * (if m.vpt > 0. {
                    1. + positive_part((head - vlim) / m.vpt, D::cst(m.aick.sqrt()))
                } else {
                    D::cst(1.)
                })
        } else {
            D::cst(1e30)
        };
        let crit = floor(crit, 1e-20);
        // S&C, low-injection transit time: BC depletion width is proportional
        // to inverse junction capacitance. TBVL represents velocity-limited
        // drift and DT0H the low-current base-width correction.
        let tau = floor(
            (m.t0 + m.tbvl * (cc_ratio - 1.) + m.dt0h * (1. / cc_ratio - 1.))
                * (1. + m.alt0 * dt + m.kt0 * dt.sq()),
            0.,
        );
        let mut base = base_charge(hole, equilibrium);
        // The static-charge AC measurements replace the BE contribution to
        // d ln(Qbase) by Vbe dVbe. Retain the BC depletion derivative.
        // Keep the derivative of the DC balance for Newton and transient.
        if small_signal && tau.v == 0. && m.tef0 == 0. && m.thcs == 0. {
            let correction = base.v * be.v + s * base.d[EI];
            for (partial, voltage) in base.d[..TJ].iter_mut().zip(be.d) {
                *partial += correction * voltage;
            }
        }
        let stored = transport(m, base, jf, jr, tau, crit, ratio);
        let enhancement = if m.ich > 0. {
            1. + stored.forward / m.ich
        } else {
            D::cst(1.)
        };
        let forward = stored.forward * enhancement;
        let reverse = stored.reverse;
        // Excess phase and charge delay (Schröter & Chakravorty, NQS).
        // Each relaxation row uses fixed state scaling so a bias-dependent
        // time constant multiplies the state derivative only.
        let mut f = [zero; N];
        let mut q = [zero; N];
        let time = stored.transit_time;
        let qf = if m.t0 > 0. && m.alqf > 0. && time.v > 0. {
            let scale = m.t0.max(1e-12);
            f[QF] = (v[QF] - stored.qf / m.qp0) * (scale / (m.alqf * time));
            q[QF] = v[QF] * scale;
            v[QF] * m.qp0
        } else {
            f[QF] = v[QF] - stored.qf / m.qp0;
            stored.qf
        };
        let it = if m.t0 > 0. && m.alit > 0. && time.v > 0. {
            // Distributed transit: H(s) = 1/(1+sL+(sL)^2/3).
            let lag = m.alit * time;
            let scale = m.t0.max(1e-12);
            f[IT] = -v[VELOCITY] * (scale / lag);
            q[IT] = v[IT] * scale;
            f[VELOCITY] = (v[IT] + v[VELOCITY] - forward) * (3. * scale / lag);
            q[VELOCITY] = v[VELOCITY] * scale;
            v[IT]
        } else {
            if m.t0 > 0. {
                f[IT] = v[IT] - forward;
                f[VELOCITY] = v[VELOCITY];
            }
            forward
        } - reverse;
        // Shockley-Read-Hall loss of excess base charge.
        let high_rec = if m.tbhrec > 0. {
            (1. - m.fthc) * stored.excess / m.tbhrec
        } else {
            zero
        };
        // R16 ABET ratio isolates the field in the Kane exponential.
        // Field is normalized by its zero-bias value using junction width.
        let (tn, tv, cap, pot) = if m.tunode == 1. {
            (BX, bp, cp_ratio, vdp)
        } else {
            (BI, be, ce_ratio, vde)
        };
        let tunnel = if tv.v < 0. && m.ibets > 0. {
            let field = (1. - tv / pot) * cap;
            -m.ibets * (-tv / pot) * field * safe_exp(-m.abet / field)
        } else {
            zero
        };
        // Sze & Ng, impact ionization integrated over the reverse BC field.
        // QAVL/Cj is a characteristic voltage. KAVL feeds the generated
        // carriers back into the primary multiplication (docs/hicum.md).
        let junction_field = vdc - bc;
        let avalanche = if junction_field.v > 0.
            && forward.v > 0.
            && m.favl > 0.
            && cc0.v > 0.
            && m.qavl > 0.
        {
            let onset = m.qavl * safe_exp(m.alqav * dt) / cc0;
            let slope = onset / cc_ratio;
            let field = if junction_field.v <= onset.v {
                junction_field * safe_exp(-slope / junction_field)
            } else {
                safe_exp(-slope / onset) * (onset + (1. + slope / onset) * (junction_field - onset))
            };
            let gain = m.favl * safe_exp(m.alfav * dt) * field;
            let feedback = m.kavl * (1. + m.alkav * dt);
            if feedback.v > 0. {
                forward * gain / positive_part(1. - feedback * gain, D::cst(0.1))
            } else {
                forward * gain
            }
        } else {
            zero
        };
        // Lateral substrate transistor, Ebers-Moll difference of opposing
        // injected populations (manual Fig. 7.3).
        let sub_sat = sat(m.itss, egsc, m.zetact, m.msf) * (m.zetact + egsc / vt) / (m.msf * t);
        let sub_f = sub_sat * exp(bx / (m.msf * vt));
        let sub_r = sub_sat * exp(sc / (m.msf * vt));
        let bei = ibe + high_rec;
        let bci = ibc - avalanche;
        for (p, n, i) in [
            (CI, EI, it),
            (BI, EI, bei),
            (BX, EI, ibp),
            (tn, EI, tunnel),
            (BI, CI, bci),
            (BX, CI, ibx),
            (SI, CI, isc),
            (BX, SI, sub_f - sub_r),
        ] {
            branch(&mut f, p, n, i * s);
        }
        // S&C, modulated sheet resistance inversely follows base hole charge.
        // Current crowding lowers the spreading resistance through a smooth
        // distributed-emitter factor. FDQR0 adds unmodulated base charge.
        let sheet_charge = base_charge(equilibrium + (qje + qjc) / m.qp0, equilibrium)
            + (stored.qf + stored.qr) / m.qp0;
        let sheet =
            m.rbi0 * ratio.powf(m.zetarbi) * (1. + m.fdqr0) / floor(sheet_charge + m.fdqr0, 0.01);
        let crowd = m.fgeo * floor(bei, 0.) * sheet / vt;
        let rbi = m.fqi
            * if crowd.v > 0. {
                sheet * crowd.ln_1p() / crowd
            } else {
                sheet
            };
        for (p, n, r) in [
            (B, BX, m.rbx * ratio.powf(m.zetarbx)),
            (BX, BI, rbi),
            (C, CI, m.rcx * ratio.powf(m.zetarcx)),
            (E, EI, m.re * ratio.powf(m.zetare)),
            (S, SI, D::cst(m.rsu)),
        ] {
            if r.v > 0. {
                let current = (v[p] - v[n]) / r;
                branch(&mut f, p, n, current);
                // Johnson-Nyquist and empirical emitter-contact flicker PSDs.
                noise(p, n, 4. * BOLTZMANN * t.v * scale / r.v, 0.);
                if p == E && m.kfre > 0. {
                    noise(
                        p,
                        n,
                        self.copies * m.kfre * (self.area * current.v.abs()).powf(m.afre),
                        1.,
                    );
                }
            }
        }
        // Schottky shot spectra; independent parallel copies add PSD. No
        // correlation network in the native 44 model (manual §7.3.5).
        for (p, n, i) in [
            (CI, EI, forward - reverse),
            (BI, EI, bei),
            (BX, EI, ibp),
            (tn, EI, tunnel),
            (BI, CI, ibc + gmin * bc / scale),
            (BI, CI, avalanche),
            (BX, CI, ibx + gmin * bx / scale),
            (SI, CI, isc + gmin * sc / scale),
        ] {
            noise(p, n, 2. * CHARGE * i.v.abs() * scale, 0.);
        }
        let flicker_i = bei + ibp;
        if m.kf > 0. {
            noise(
                BX,
                EI,
                self.copies * m.kf * (self.area * flicker_i.v.abs()).powf(m.af),
                1.,
            );
        }

        // Terminal partition follows Fig. 7.3, with the reverse diffusion
        // charge on BC and the substrate diffusion charge on the B*–C' junction.
        // FBCPAR partitions the sum of nominal depletion and overlap capacitance.
        // The peripheral base takes depletion first and the contact takes overlap first.
        let inside_bc = m.fbcpar * (m.cjcx0 + m.cbcpar);
        let inside_depletion = if m.cjcx0 > 0. {
            (inside_bc / m.cjcx0).min(1.)
        } else {
            0.
        };
        let inside_overlap = (inside_bc - m.cjcx0).max(0.);
        let qbe = qje + qf;
        let qbc = qjc + stored.qr;
        for (p, n, value) in [
            (BI, EI, qbe),
            (BX, EI, qjp),
            (BI, CI, qbc),
            (B, CI, (1. - inside_depletion) * qcx(u(B, CI))),
            (BX, CI, inside_depletion * qcx(bx) + m.tsf * sub_f),
            (B, C, (m.cbcpar - inside_overlap) * u(B, C)),
            (BX, C, inside_overlap * u(BX, C)),
            (B, E, (1. - m.fbepar) * m.cbepar * u(B, E)),
            (BX, E, m.fbepar * m.cbepar * u(BX, E)),
            (SI, CI, qjs),
            (S, C, qscp),
            (S, SI, m.csu * u(S, SI)),
        ] {
            branch(&mut q, p, n, value * s);
        }
        // Manual §7.3.5: FLSH=1 selects the main current dissipation,
        // FLSH=2 collects the power of every dissipative branch, including
        // the lead drops. Each parallel cell has its own Rth and Cth.
        let power = if m.flsh == 1. {
            ce * it + be * bei + bc * bci
        } else {
            (0..TJ).fold(zero, |acc, k| acc + v[k] * f[k])
        };
        if self.heated() {
            let rth = m.rth * ratio.powf(m.zetarth) * floor(1. + m.alrth * dt, 1e-6);
            f[TJ] = v[TJ] / rth - power;
            let heat_capacity = if m.cth > 0. { m.cth } else { 1e-12 };
            q[TJ] = heat_capacity * v[TJ];
        }
        let mut f = f.map(|a| a * scale);
        // Per-instance junction shunts follow the SPICE GMIN convention.
        // The GMIN probe distinguishes unscaled and area-scaled shunts.
        for (p, n) in [(BI, EI), (BX, EI), (BI, CI), (BX, CI), (SI, CI)] {
            branch(&mut f, p, n, gmin * (v[p] - v[n]));
        }
        Bias {
            f,
            q: q.map(|a| a * scale),
            transport: (forward - reverse) * (s * scale),
            base_be: bei * (s * scale),
            base_bc: bci * (s * scale),
            qbe: qbe * (s * scale),
            qbc: qbc * (s * scale),
            power: power * scale,
            avalanche: avalanche * (s * scale),
            critical: crit * scale,
            tau: time,
        }
    }
}
/// Batch of native HICUM/L2 instances in netlist order.
#[derive(Default, Debug)]
pub struct Hicums(Vec<Instance>);

impl Hicums {
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

impl DeviceGroup for Hicums {
    fn type_name(&self) -> &'static str {
        "hicum"
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
                (CI, C, m.rcx, "ci"),
                (BX, B, m.rbx, "bx"),
                (BI, BX, m.rbi0 * m.fqi, "bi"),
                (EI, E, m.re, "ei"),
                (SI, S, m.rsu, "si"),
            ] {
                d.nodes[k] = if r > 0. {
                    Some(u.alloc_internal_node(&d.name, name))
                } else {
                    d.nodes[lead]
                };
            }
            if d.heated() {
                d.nodes[TJ] = d
                    .thermal_pin
                    .unwrap_or_else(|| Some(u.alloc_internal_node(&d.name, "tj")));
            }
            if m.t0 > 0. {
                for (k, name) in [
                    (IT, "transport"),
                    (QF, "charge"),
                    (VELOCITY, "transit_slope"),
                ] {
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
    fn exponential_step_limits(&self, reltol: f64, out: &mut Vec<(Node, Node, f64)>) {
        for d in &self.0 {
            if d.model.c10 > 0. {
                let vt = K_OVER_Q * d.t.kelvin * d.model.polarity;
                let span = 4. * vt * (0.5 * reltol).sqrt().asinh();
                out.push((d.nodes[BI], d.nodes[EI], d.model.mcf * span));
                out.push((d.nodes[BI], d.nodes[CI], span));
            }
        }
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
            let t = d.t.kelvin + act[TJ];
            if !t.is_finite() || t <= 0. {
                out.error = Some(Diagnostic::error(
                    "E-ANALYSIS",
                    format!("{}: invalid HICUM/L2 junction temperature", d.name),
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
                for (k, v, n) in [(0, &mut be, d.model.mcf), (1, &mut bc, 1.)] {
                    let (value, limited) = limit_junction(
                        *v,
                        st.current[2 * i + k],
                        n * vt,
                        vcrit((d.model.c10 / d.model.qp0).max(1e-30), n * vt),
                    );
                    *v = value;
                    out.noncon |= limited;
                }
            }
            st.current[2 * i..2 * i + 2].copy_from_slice(&[be, bc]);
            lin[BI] = lin[EI] + sign * be;
            lin[CI] = lin[BI] - sign * bc;
            let b = d.evaluate(
                lin,
                ctx.gmin,
                matches!(ctx.mode, crate::Mode::Ac | crate::Mode::Noise) || !out.outputs.is_empty(),
                |_, _, _, _| {},
            );
            if let Some(values) = out.outputs.get_mut(i) {
                for (name, rows) in CURRENT.into_iter().zip(ROWS) {
                    values.push((name, rows.iter().map(|k| b.f[*k].v).sum()));
                }
                values.extend([
                    ("vbe", act[B] - act[E]),
                    ("vbc", act[B] - act[C]),
                    ("vce", act[C] - act[E]),
                    ("vsc", act[S] - act[C]),
                    ("vbbp", act[B] - act[BX]),
                    ("vbpbi", lin[BX] - lin[BI]),
                    ("ibei", b.base_be.v),
                    ("ibci", b.base_bc.v + b.avalanche.v),
                    ("p", b.power.v),
                    ("gm", b.transport.d[BI]),
                    ("go", b.transport.d[CI]),
                    ("iavl", b.avalanche.v),
                    ("ick", b.critical.v),
                    ("tf", b.tau.v),
                    ("gmi", b.transport.d[BI]),
                    ("vbiei", sign * be),
                    ("vbici", sign * bc),
                    ("vciei", lin[CI] - lin[EI]),
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
            d.evaluate(
                d.nodes.map(|n| volt(x, n)),
                ctx.gmin,
                true,
                |p, n, power, exp| {
                    if power > 0. {
                        out.push(NoiseSource::branch(
                            &d.name, d.nodes[p], d.nodes[n], power, exp,
                        ));
                    }
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
